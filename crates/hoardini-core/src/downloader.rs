use std::{
    fs::{self, File, OpenOptions},
    future::Future,
    io::{self, Write},
    os::windows::fs::FileExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    net::{self, with_suffix},
    probe::{self, ProbeResult},
    segments::{self, Segment, MIN_SEGMENT_SIZE},
    Error, Result,
};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DownloadProgress {
    pub done: u64,
    pub total: Option<u64>,
    /// Set when progress is not byte-based (HLS counts segments).
    pub fraction_override: Option<f64>,
}

impl DownloadProgress {
    pub fn bytes(done: u64, total: Option<u64>) -> Self {
        Self { done, total, fraction_override: None }
    }

    pub fn fraction(&self) -> Option<f64> {
        self.fraction_override
            .or_else(|| self.total.filter(|&t| t > 0).map(|t| (self.done as f64 / t as f64).min(1.0)))
    }
}

pub type ProgressFn = Arc<dyn Fn(DownloadProgress) + Send + Sync>;
/// Called once the destination file name is settled (before bytes are written).
pub type DestinationFn = Arc<dyn Fn(&Path) + Send + Sync>;

#[derive(Clone)]
pub struct Options {
    pub directory: PathBuf,
    pub connections: usize,
    /// Extra request headers (e.g. Cookie, Referer, User-Agent) sent on every request.
    pub headers: Vec<(String, String)>,
    pub filename: Option<String>,
    /// HLS only: a variant playlist chosen in the browser; the URL is then its master.
    pub variant: Option<Url>,
    pub cancel: CancellationToken,
    pub client: Client,
    pub on_destination: Option<DestinationFn>,
}

impl Options {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            connections: 8,
            headers: Vec::new(),
            filename: None,
            variant: None,
            cancel: CancellationToken::new(),
            client: net::client(),
            on_destination: None,
        }
    }

    pub(crate) fn referrer(&self) -> Option<&str> {
        net::header_value(&self.headers, "Referer")
    }
}

const MAX_RETRIES: u32 = 3;
const BUFFER_SIZE: usize = 128 * 1024;

/// Runs `fut` unless `token` fires first.
pub(crate) async fn cancellable<T, E: Into<Error>>(
    token: &CancellationToken,
    fut: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T> {
    tokio::select! {
        biased;
        _ = token.cancelled() => Err(Error::Cancelled),
        r = fut => r.map_err(Into::into),
    }
}

pub(crate) async fn backoff(token: &CancellationToken, attempt: u32) -> Result<()> {
    tokio::select! {
        _ = token.cancelled() => Err(Error::Cancelled),
        _ = tokio::time::sleep(Duration::from_secs(attempt as u64)) => Ok(()),
    }
}

/// Persisted next to the partial file so an interrupted download can continue.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DownloadState {
    url: Url,
    length: u64,
    validator: Option<String>,
    segments: Vec<Segment>,
}

struct SegmentStore {
    inner: Mutex<(DownloadState, Option<Instant>)>,
    path: PathBuf,
}

impl SegmentStore {
    fn segment(&self, i: usize) -> Segment {
        self.inner.lock().unwrap().0.segments[i]
    }

    fn done(&self) -> u64 {
        self.inner.lock().unwrap().0.segments.iter().map(|s| s.written).sum()
    }

    /// Records `n` more bytes on disk for segment `i`; returns the new overall total.
    fn add(&self, n: u64, i: usize) -> u64 {
        let mut g = self.inner.lock().unwrap();
        g.0.segments[i].written += n;
        if g.1.is_none_or(|t| t.elapsed() > Duration::from_secs(1)) {
            Self::write(&mut g, &self.path);
        }
        g.0.segments.iter().map(|s| s.written).sum()
    }

    fn save(&self) {
        Self::write(&mut self.inner.lock().unwrap(), &self.path);
    }

    fn write(g: &mut (DownloadState, Option<Instant>), path: &Path) {
        g.1 = Some(Instant::now());
        if let Ok(data) = serde_json::to_vec(&g.0) {
            let tmp = with_suffix(path, ".tmp");
            if fs::write(&tmp, data).is_ok() {
                let _ = fs::rename(&tmp, path);
            }
        }
    }
}

/// A caller-supplied name (e.g. a page title) keeps the real file's extension unless it already has it.
pub fn final_name(override_name: Option<&str>, probed: &str) -> String {
    let Some(o) = override_name.map(probe::sanitize) else { return probed.to_owned() };
    match probe::split_ext(probed).1 {
        Some(ext) if !o.to_lowercase().ends_with(&format!(".{}", ext.to_lowercase())) => format!("{o}.{ext}"),
        _ => o,
    }
}

/// Downloads `url` into `opts.directory`, returning the final file path.
pub async fn download(url: &Url, opts: &Options, progress: ProgressFn) -> Result<PathBuf> {
    let probe = cancellable(&opts.cancel, probe::run(&opts.client, url, &opts.headers)).await?;
    let name = final_name(opts.filename.as_deref(), &probe.filename);
    let (final_path, _lock) = net::claim(&opts.directory, &name)?;
    if let Some(f) = &opts.on_destination {
        f(&final_path);
    }
    let part = with_suffix(&final_path, ".part");
    let state_path = with_suffix(&final_path, ".hoardini");

    let length = match (probe.accepts_ranges, probe.length) {
        (true, Some(len)) if len > 0 => len,
        // No range support: single stream, no resume.
        _ => {
            single_stream(url, &probe, opts, &part, &final_path, &progress).await?;
            return Ok(final_path);
        }
    };

    let mut state = load_state(&state_path, &probe, length, part.exists()).unwrap_or_else(|| DownloadState {
        url: probe.final_url.clone(),
        length,
        validator: probe.validator.clone(),
        segments: segments::plan(length, opts.connections.max(1), MIN_SEGMENT_SIZE),
    });
    if state.segments.iter().all(|s| s.written == 0) {
        File::create(&part)?.set_len(length)?;
    }
    state.url = probe.final_url.clone();
    let pending: Vec<usize> = (0..state.segments.len()).filter(|&i| !state.segments[i].is_done()).collect();

    let store = Arc::new(SegmentStore { inner: Mutex::new((state, None)), path: state_path.clone() });
    store.save();
    progress(DownloadProgress::bytes(store.done(), Some(length)));

    // One failing segment stops its siblings (they save their progress first).
    let group = opts.cancel.child_token();
    let ctx = SegmentCtx {
        store: store.clone(),
        part: part.clone(),
        url: probe.final_url.clone(),
        origin: url.clone(),
        client: opts.client.clone(),
        headers: Arc::new(opts.headers.clone()),
        cancel: group.clone(),
        progress,
        total: length,
    };
    let mut set = tokio::task::JoinSet::new();
    for i in pending {
        let ctx = ctx.clone();
        set.spawn(async move {
            let r = download_segment(&ctx, i).await;
            if r.is_err() {
                ctx.store.save(); // keep resume state accurate on pause/failure
            }
            r
        });
    }
    let mut first_err = None;
    while let Some(joined) = set.join_next().await {
        let r = joined.unwrap_or_else(|e| Err(Error::Io(io::Error::other(e))));
        if let Err(e) = r {
            if first_err.is_none() {
                group.cancel();
                first_err = Some(e);
            }
        }
    }
    if let Some(e) = first_err {
        return Err(if opts.cancel.is_cancelled() { Error::Cancelled } else { e });
    }

    let got = store.done();
    if got != length {
        return Err(Error::SizeMismatch { expected: length, got });
    }
    net::mark_of_the_web(&part, &probe.final_url, opts.referrer());
    fs::rename(&part, &final_path)?;
    let _ = fs::remove_file(&state_path);
    Ok(final_path)
}

fn load_state(path: &Path, probe: &ProbeResult, length: u64, part_exists: bool) -> Option<DownloadState> {
    if !part_exists {
        return None;
    }
    let s: DownloadState = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let sane = s.length == length
        && s.validator == probe.validator
        && !s.segments.is_empty()
        && s.segments.iter().all(|seg| seg.end < length && seg.next_offset() <= seg.end + 1);
    sane.then_some(s)
}

#[derive(Clone)]
struct SegmentCtx {
    store: Arc<SegmentStore>,
    part: PathBuf,
    url: Url,
    origin: Url,
    client: Client,
    headers: Arc<Vec<(String, String)>>,
    cancel: CancellationToken,
    progress: ProgressFn,
    total: u64,
}

/// Retries a failed segment with backoff, continuing from its written offset.
async fn download_segment(ctx: &SegmentCtx, i: usize) -> Result<()> {
    let mut attempt = 0;
    loop {
        match stream_segment(ctx, i).await {
            Ok(()) => return Ok(()),
            Err(Error::Cancelled) => return Err(Error::Cancelled),
            Err(e) => {
                attempt += 1;
                if attempt > MAX_RETRIES {
                    return Err(e);
                }
                backoff(&ctx.cancel, attempt).await?;
            }
        }
    }
}

fn write_all_at(file: &File, mut buf: &[u8], mut offset: u64) -> io::Result<()> {
    while !buf.is_empty() {
        let n = file.seek_write(buf, offset)?;
        if n == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

async fn stream_segment(ctx: &SegmentCtx, i: usize) -> Result<()> {
    let seg = ctx.store.segment(i);
    if seg.is_done() {
        return Ok(());
    }
    let req = net::apply_headers(ctx.client.get(ctx.url.clone()), &ctx.headers, &ctx.url, &ctx.origin)
        .header(header::RANGE, format!("bytes={}-{}", seg.next_offset(), seg.end));
    let resp = cancellable(&ctx.cancel, req.send()).await?;
    match resp.status().as_u16() {
        206 => {}
        200 => return Err(Error::RangeIgnored),
        code => return Err(Error::BadResponse(code)),
    }

    let file = OpenOptions::new().write(true).open(&ctx.part)?;
    let mut offset = seg.next_offset();
    let mut remaining = seg.remaining();
    let mut buf = Vec::with_capacity(BUFFER_SIZE);
    let flush = |buf: &mut Vec<u8>, offset: &mut u64| -> Result<()> {
        if buf.is_empty() {
            return Ok(());
        }
        write_all_at(&file, buf, *offset)?;
        *offset += buf.len() as u64;
        let done = ctx.store.add(buf.len() as u64, i);
        (ctx.progress)(DownloadProgress::bytes(done, Some(ctx.total)));
        buf.clear();
        Ok(())
    };

    let mut body = resp.bytes_stream();
    while remaining > 0 {
        let next = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => {
                flush(&mut buf, &mut offset)?;
                return Err(Error::Cancelled);
            }
            chunk = body.next() => chunk,
        };
        let Some(chunk) = next else { break };
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                flush(&mut buf, &mut offset)?; // bytes already received are good; the retry resumes after them
                return Err(e.into());
            }
        };
        // Never write past the segment, even if the server sends more than asked for.
        let take = (chunk.len() as u64).min(remaining) as usize;
        buf.extend_from_slice(&chunk[..take]);
        remaining -= take as u64;
        if buf.len() >= BUFFER_SIZE {
            flush(&mut buf, &mut offset)?;
        }
    }
    flush(&mut buf, &mut offset)?;
    ctx.store.save();
    if remaining > 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "connection closed early").into());
    }
    Ok(())
}

async fn single_stream(
    origin: &Url,
    probe: &ProbeResult,
    opts: &Options,
    part: &Path,
    final_path: &Path,
    progress: &ProgressFn,
) -> Result<()> {
    let req = net::apply_headers(opts.client.get(probe.final_url.clone()), &opts.headers, &probe.final_url, origin);
    let resp = cancellable(&opts.cancel, req.send()).await?;
    if !resp.status().is_success() {
        return Err(Error::BadResponse(resp.status().as_u16()));
    }
    let mut file = File::create(part)?;
    let mut done = 0u64;
    let mut buf = Vec::with_capacity(BUFFER_SIZE);
    let mut body = resp.bytes_stream();
    while let Some(chunk) = cancellable(&opts.cancel, async { Ok::<_, Error>(body.next().await) }).await? {
        buf.extend_from_slice(&chunk?);
        if buf.len() >= BUFFER_SIZE {
            file.write_all(&buf)?;
            done += buf.len() as u64;
            buf.clear();
            progress(DownloadProgress::bytes(done, probe.length));
        }
    }
    file.write_all(&buf)?;
    done += buf.len() as u64;
    drop(file);
    progress(DownloadProgress::bytes(done, probe.length));
    net::mark_of_the_web(part, &probe.final_url, opts.referrer());
    fs::rename(part, final_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_name_keeps_real_extension() {
        assert_eq!(final_name(Some("My Video"), "abc123.mp4"), "My Video.mp4");
        assert_eq!(final_name(Some("Lecture.MP4"), "abc123.mp4"), "Lecture.MP4");
        assert_eq!(final_name(None, "abc123.mp4"), "abc123.mp4");
        assert_eq!(final_name(Some("notes"), "README"), "notes");
    }
}
