use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::Write,
    ops::Range,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Mutex,
};

use bytes::Bytes;
use reqwest::{header, Client};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    downloader::{backoff, cancellable},
    fmp4,
    job::url_extension,
    net::{self, with_suffix, FILE_ATTRIBUTE_HIDDEN},
    probe, remux, DownloadProgress, Error, Options, ProgressFn, Result,
};

/// A segment URL, or a byte range within it (EXT-X-BYTERANGE: many segments share one file).
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub url: Url,
    pub range: Option<Range<u64>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaPlaylist {
    pub segments: Vec<Segment>,
    /// EXT-X-MAP (fragmented MP4).
    pub init_segment: Option<Segment>,
    pub encrypted: bool,
    pub is_live: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Variant {
    pub bandwidth: u64,
    pub url: Url,
    /// GROUP-ID of the EXT-X-MEDIA audio renditions this variant plays with.
    pub audio_group: Option<String>,
}

/// An EXT-X-MEDIA:TYPE=AUDIO entry. No URL means the audio is muxed into the variant itself.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioRendition {
    pub group_id: String,
    pub url: Option<Url>,
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Master { variants: Vec<Variant>, audio: Vec<AudioRendition> },
    Media(MediaPlaylist),
}

pub fn parse(text: &str, base: &Url) -> Parsed {
    let lines: Vec<&str> = text.split(['\n', '\r']).map(str::trim).collect();
    let resolve = |s: &str| base.join(s).ok();

    if lines.iter().any(|l| l.starts_with("#EXT-X-STREAM-INF")) {
        let mut variants = Vec::new();
        let mut audio = Vec::new();
        let mut pending: Option<(u64, Option<String>)> = None;
        for l in &lines {
            if l.starts_with("#EXT-X-STREAM-INF") {
                let bw = attribute("BANDWIDTH", l).and_then(|b| b.parse().ok()).unwrap_or(0);
                pending = Some((bw, attribute("AUDIO", l)));
            } else if l.starts_with("#EXT-X-MEDIA:") && attribute("TYPE", l).as_deref() == Some("AUDIO") {
                if let Some(group_id) = attribute("GROUP-ID", l) {
                    audio.push(AudioRendition {
                        group_id,
                        url: attribute("URI", l).and_then(|u| resolve(&u)),
                        is_default: attribute("DEFAULT", l).as_deref() == Some("YES"),
                    });
                }
            } else if !l.is_empty() && !l.starts_with('#') {
                if let (Some((bandwidth, audio_group)), Some(url)) = (pending.take(), resolve(l)) {
                    variants.push(Variant { bandwidth, url, audio_group });
                }
            }
        }
        return Parsed::Master { variants, audio };
    }

    // A range without "@offset" continues where the last one in that file ended.
    let mut range_end: HashMap<Url, u64> = HashMap::new();
    let mut segment = |u: Url, br: Option<(u64, Option<u64>)>| -> Segment {
        let Some((len, offset)) = br else { return Segment { url: u, range: None } };
        let start = offset.or_else(|| range_end.get(&u).copied()).unwrap_or(0);
        range_end.insert(u.clone(), start + len);
        Segment { url: u, range: Some(start..start + len) }
    };
    let mut segments = Vec::new();
    let mut init_segment = None;
    let mut encrypted = false;
    let mut ended = false;
    let mut pending_range = None;
    for l in &lines {
        if l.starts_with("#EXT-X-KEY") || l.starts_with("#EXT-X-SESSION-KEY") {
            if attribute("METHOD", l).is_some_and(|m| m != "NONE") {
                encrypted = true;
            }
        } else if l.starts_with("#EXT-X-MAP") {
            if let Some(u) = attribute("URI", l).and_then(|u| resolve(&u)) {
                let br = attribute("BYTERANGE", l).and_then(|b| byte_range(&b)).map(|(n, o)| (n, Some(o.unwrap_or(0))));
                init_segment = Some(segment(u, br));
            }
        } else if let Some(spec) = l.strip_prefix("#EXT-X-BYTERANGE:") {
            pending_range = byte_range(spec);
        } else if l.starts_with("#EXT-X-ENDLIST") {
            ended = true;
        } else if !l.is_empty() && !l.starts_with('#') {
            if let Some(u) = resolve(l) {
                segments.push(segment(u, pending_range.take()));
            }
        }
    }
    Parsed::Media(MediaPlaylist { segments, init_segment, encrypted, is_live: !ended })
}

fn byte_range(spec: &str) -> Option<(u64, Option<u64>)> {
    let mut parts = spec.splitn(2, '@');
    let n: u64 = parts.next()?.trim().parse().ok()?;
    (n > 0).then(|| (n, parts.next().and_then(|o| o.trim().parse().ok())))
}

/// The requested variant if the master lists it, else the highest bandwidth one.
pub fn pick<'a>(variants: &'a [Variant], preferred: Option<&Url>) -> Option<&'a Variant> {
    if let Some(v) = preferred.and_then(|p| variants.iter().find(|v| &v.url == p)) {
        return Some(v);
    }
    variants.iter().fold(None, |best: Option<&Variant>, v| match best {
        Some(b) if b.bandwidth >= v.bandwidth => Some(b),
        _ => Some(v),
    })
}

/// The separate audio playlist a variant needs, or `None` when its audio is muxed in (or it has none).
pub fn audio_url(variant: &Variant, renditions: &[AudioRendition]) -> Option<Url> {
    let group = variant.audio_group.as_deref()?;
    let members: Vec<_> = renditions.iter().filter(|r| r.group_id == group).collect();
    members.iter().find(|r| r.is_default).or(members.first()).and_then(|r| r.url.clone())
}

/// Reads `NAME=value` or `NAME="value"` out of an HLS tag line.
pub(crate) fn attribute(name: &str, line: &str) -> Option<String> {
    let pat = format!("{name}=");
    let mut s = line;
    loop {
        let idx = s.find(&pat)?;
        // Must start an attribute, not be the tail of another name (e.g. AVERAGE-BANDWIDTH).
        if idx > 0 && !matches!(s[..idx].chars().next_back(), Some(':' | ',')) {
            s = &s[idx + pat.len()..];
            continue;
        }
        let rest = &s[idx + pat.len()..];
        if let Some(body) = rest.strip_prefix('"') {
            return body.find('"').map(|e| body[..e].to_owned());
        }
        return Some(rest.split(',').next().unwrap_or("").to_owned());
    }
}

const PARALLEL: usize = 6;

/// Downloads an HLS stream into `opts.directory`, returning the final file path.
pub async fn download(url: &Url, opts: &Options, progress: ProgressFn) -> Result<PathBuf> {
    let f = Fetcher { client: opts.client.clone(), headers: opts.headers.clone(), origin: url.clone(), cancel: opts.cancel.clone() };
    let mut playlist_url = url.clone();
    let mut media = None;
    let mut audio_url_found = None;
    for _ in 0..3 {
        // master -> variant -> (rarely) nested
        match parse(&f.text(&playlist_url).await?, &playlist_url) {
            Parsed::Master { variants, audio } => {
                let chosen = pick(&variants, opts.variant.as_ref()).ok_or(Error::BadPlaylist)?;
                audio_url_found = audio_url(chosen, &audio).or(audio_url_found);
                playlist_url = chosen.url.clone();
            }
            Parsed::Media(m) => {
                media = Some(m);
                break;
            }
        }
    }
    // Sites like Vimeo serve video-only variants with the audio as a separate rendition.
    let mut audio = match audio_url_found {
        Some(u) => match parse(&f.text(&u).await?, &u) {
            Parsed::Media(a) => Some(a),
            Parsed::Master { .. } => return Err(Error::BadPlaylist),
        },
        None => None,
    };
    let media = media.filter(|m| !m.segments.is_empty()).ok_or(Error::BadPlaylist)?;
    for m in std::iter::once(&media).chain(audio.as_ref()) {
        if m.encrypted {
            return Err(Error::EncryptedStream);
        }
        if m.is_live {
            return Err(Error::LiveStream);
        }
    }
    if audio.as_ref().is_some_and(|a| a.segments.is_empty()) {
        audio = None;
    }
    // Separate tracks can only be merged with ffmpeg: fail now, not after downloading everything.
    if audio.is_some() && remux::ffmpeg_path().is_none() {
        return Err(Error::FfmpegMissing);
    }

    let video_ext = if media.init_segment.is_some() { "mp4" } else { "ts" };
    let ext = if audio.is_some() { "mp4" } else { video_ext };
    let mut stem = opts.filename.clone().unwrap_or_else(|| default_name(url));
    // Only strip an extension we would re-add; "Lesson 1.5 intro" must stay intact.
    for known in [".ts", ".mp4", ".m4v"] {
        let cut = stem.len().saturating_sub(known.len());
        if stem.is_char_boundary(cut) && stem[cut..].eq_ignore_ascii_case(known) {
            stem.truncate(cut);
        }
    }
    let name = format!("{}.{ext}", probe::sanitize(&stem));
    let (final_path, _lock) = net::claim(&opts.directory, &name)?;
    if let Some(cb) = &opts.on_destination {
        cb(&final_path);
    }

    let part = with_suffix(&final_path, ".part");
    // Each track of a split stream goes to its own hidden temp file, then both are muxed into the .part file.
    let tracks: Vec<(&MediaPlaylist, PathBuf)> = match &audio {
        Some(a) => vec![
            (&media, net::track_file(&final_path, "video", video_ext)),
            (a, net::track_file(&final_path, "audio", audio_extension(a))),
        ],
        None => vec![(&media, part.clone())],
    };

    // Pick up where a paused or interrupted download stopped, if it was the same stream.
    let counts: Vec<usize> = tracks.iter().map(|(m, _)| m.segments.len()).collect();
    let mut store = ResumeStore {
        path: with_suffix(&final_path, ".hoardini"),
        state: HlsState::fresh(url, opts.variant.as_ref(), &counts),
    };
    if let Some(saved) = HlsState::load(&store.path, url, opts.variant.as_ref(), &counts) {
        store.state = saved;
    }
    store.save();
    let counter = SegmentCounter::new(counts.iter().sum(), progress, &store.state.tracks);
    for (k, (m, path)) in tracks.iter().enumerate() {
        write(&f, m, path, k, &mut store, &counter, audio.is_some()).await?;
    }
    if audio.is_some() {
        let _ = fs::remove_file(&part);
        // Cancelling (pause) kills ffmpeg; the track files stay, so a resume goes straight to merging.
        remux::merge(&tracks[0].1, &tracks[1].1, &part, &opts.cancel).await?;
        for (_, path) in &tracks {
            let _ = fs::remove_file(path);
        }
    }
    net::mark_of_the_web(&part, url, opts.referrer());
    fs::rename(&part, &final_path)?;
    let _ = fs::remove_file(&store.path);
    Ok(final_path)
}

/// How far one track (video, audio, or the only one) of an HLS download got.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct TrackState {
    /// Playlist length, to tell whether the stream changed.
    segments: usize,
    /// Segments fully written, in playlist order.
    done: usize,
    /// Track file length after those segments (and the init segment).
    bytes: u64,
    /// Next fMP4 fragment number.
    sequence: u32,
}

impl TrackState {
    fn new(segments: usize) -> Self {
        Self { segments, done: 0, bytes: 0, sequence: 1 }
    }
}

/// Persisted as `<final name>.hoardini` so a paused or interrupted HLS download can continue.
/// Same JSON as the macOS app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct HlsState {
    source: Url,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    variant: Option<Url>,
    tracks: Vec<TrackState>,
}

impl HlsState {
    fn fresh(source: &Url, variant: Option<&Url>, counts: &[usize]) -> Self {
        Self { source: source.clone(), variant: variant.cloned(), tracks: counts.iter().map(|&n| TrackState::new(n)).collect() }
    }

    /// Saved progress for this exact stream, or `None` when there is none, it is unreadable (anything
    /// can edit a file in Downloads), or the playlists changed. Segment URLs are not compared: signed
    /// URLs (Tella's CloudFront) are re-signed between runs.
    fn load(path: &Path, source: &Url, variant: Option<&Url>, counts: &[usize]) -> Option<Self> {
        let mut s: HlsState = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
        let same = &s.source == source
            && s.variant.as_ref() == variant
            && s.tracks.iter().map(|t| t.segments).eq(counts.iter().copied())
            && s.tracks.iter().all(|t| t.done <= t.segments && t.sequence > 0);
        if !same {
            return None;
        }
        // Nothing is kept for a track with no finished segment: it starts with a fresh file.
        for t in s.tracks.iter_mut().filter(|t| t.done == 0) {
            *t = TrackState::new(t.segments);
        }
        Some(s)
    }
}

struct ResumeStore {
    path: PathBuf,
    state: HlsState,
}

impl ResumeStore {
    fn save(&self) {
        if let Ok(data) = serde_json::to_vec(&self.state) {
            let tmp = with_suffix(&self.path, ".tmp");
            if fs::write(&tmp, data).is_ok() {
                let _ = fs::rename(&tmp, &self.path);
            }
        }
    }
}

/// Packed audio keeps its own extension (.aac, .mp3) so the muxer recognises it.
fn audio_extension(m: &MediaPlaylist) -> &'static str {
    if m.init_segment.is_some() {
        return "m4a";
    }
    match m.segments.first().map(|s| url_extension(&s.url)).as_deref() {
        Some("aac") => "aac",
        Some("mp3") => "mp3",
        Some("ac3") => "ac3",
        Some("ec3") => "ec3",
        _ => "ts",
    }
}

fn default_name(url: &Url) -> String {
    let last = url.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
    let stem = probe::split_ext(last).0.to_lowercase();
    if !["", "/", "index", "playlist", "master", "main", "manifest", "stream"].contains(&stem.as_str()) {
        return stem;
    }
    format!("video-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"))
}

/// Appends the playlist's segments in order to `path`, continuing after the ones the store says are
/// done, and saves the store after each window. The file is kept on failure or pause so the download
/// can resume.
async fn write(
    f: &Fetcher,
    m: &MediaPlaylist,
    path: &Path,
    k: usize,
    store: &mut ResumeStore,
    counter: &SegmentCounter,
    hidden: bool,
) -> Result<()> {
    let mut t = store.state.tracks[k].clone();
    let on_disk = fs::metadata(path).map(|md| md.len()).ok();
    if t.done > 0 && on_disk.is_none_or(|n| n < t.bytes) {
        // The track file is gone or short: this track starts over.
        counter.sub(t.bytes, t.done);
        t = TrackState::new(t.segments);
    }
    let mut opts = OpenOptions::new();
    opts.write(true).create(true);
    if hidden {
        // Also needed to reopen an existing hidden file: Windows refuses otherwise.
        opts.attributes(FILE_ATTRIBUTE_HIDDEN);
    }
    // Bytes past the last saved window are from one that was cut off; drop them.
    opts.open(path)?.set_len(t.bytes)?;
    // Appended one window at a time and closed in between: NTFS only refreshes the size that
    // Explorer shows when the handle closes, so a file held open looks stuck at 0 KB.
    let append = |datas: &[Bytes], t: &mut TrackState| -> Result<usize> {
        let mut file = OpenOptions::new().append(true).open(path)?;
        let mut bytes = 0;
        for d in datas {
            if m.init_segment.is_some() {
                // fMP4 segments get fresh fragment numbers so players read past the first one.
                let mut d = d.to_vec();
                fmp4::renumber_fragments(&mut d, &mut t.sequence);
                file.write_all(&d)?;
            } else {
                file.write_all(d)?;
            }
            bytes += d.len();
        }
        t.bytes += bytes as u64;
        Ok(bytes)
    };

    if t.done == 0 {
        if let Some(init) = &m.init_segment {
            let d = f.segment(init).await?;
            let mut file = OpenOptions::new().append(true).open(path)?;
            file.write_all(&d)?;
            t.bytes = d.len() as u64;
            counter.add(d.len() as u64, 0);
        }
    }
    // Download in windows of `PARALLEL` segments, writing each window in order.
    for window in m.segments[t.done..].chunks(PARALLEL) {
        let datas = futures_util::future::try_join_all(window.iter().map(|s| f.segment(s))).await?;
        let bytes = append(&datas, &mut t)?;
        t.done += window.len();
        store.state.tracks[k] = t.clone();
        store.save();
        counter.add(bytes as u64, window.len());
    }
    Ok(())
}

struct Fetcher {
    client: Client,
    headers: Vec<(String, String)>,
    origin: Url,
    cancel: CancellationToken,
}

impl Fetcher {
    async fn text(&self, u: &Url) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.fetch(u, None).await?).into_owned())
    }

    async fn segment(&self, s: &Segment) -> Result<Bytes> {
        self.fetch(&s.url, s.range.as_ref()).await
    }

    async fn fetch(&self, u: &Url, range: Option<&Range<u64>>) -> Result<Bytes> {
        let mut attempt = 0;
        loop {
            match self.try_fetch(u, range).await {
                Ok(d) => return Ok(d),
                Err(Error::Cancelled) => return Err(Error::Cancelled),
                Err(e) => {
                    attempt += 1;
                    if attempt > 3 {
                        return Err(e);
                    }
                    backoff(&self.cancel, attempt).await?;
                }
            }
        }
    }

    async fn try_fetch(&self, u: &Url, range: Option<&Range<u64>>) -> Result<Bytes> {
        let mut req = net::apply_headers(self.client.get(u.clone()), &self.headers, u, &self.origin);
        if let Some(r) = range {
            req = req.header(header::RANGE, format!("bytes={}-{}", r.start, r.end - 1));
        }
        let resp = cancellable(&self.cancel, req.send()).await?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(Error::BadResponse(status));
        }
        let data = cancellable(&self.cancel, resp.bytes()).await?;
        let Some(r) = range else { return Ok(data) };
        if status == 206 {
            let want = r.end - r.start;
            if data.len() as u64 != want {
                return Err(Error::SizeMismatch { expected: want, got: data.len() as u64 });
            }
            return Ok(data);
        }
        // Server ignored Range and sent the whole file: cut the segment out of it.
        if (data.len() as u64) < r.end {
            return Err(Error::RangeIgnored);
        }
        Ok(data.slice(r.start as usize..r.end as usize))
    }
}

/// Progress across every track of an HLS download, counted in segments.
struct SegmentCounter {
    state: Mutex<(u64, usize)>,
    total: usize,
    progress: ProgressFn,
}

impl SegmentCounter {
    /// Starts from what is already on disk, and reports it right away.
    fn new(total: usize, progress: ProgressFn, saved: &[TrackState]) -> Self {
        let start = (saved.iter().map(|t| t.bytes).sum(), saved.iter().map(|t| t.done).sum());
        let c = Self { state: Mutex::new(start), total: total.max(1), progress };
        c.add(0, 0);
        c
    }

    fn add(&self, bytes: u64, segments: usize) {
        self.update(|s| {
            s.0 += bytes;
            s.1 += segments;
        });
    }

    fn sub(&self, bytes: u64, segments: usize) {
        self.update(|s| {
            s.0 = s.0.saturating_sub(bytes);
            s.1 = s.1.saturating_sub(segments);
        });
    }

    fn update(&self, f: impl FnOnce(&mut (u64, usize))) {
        let p = {
            let mut s = self.state.lock().unwrap();
            f(&mut s);
            DownloadProgress { done: s.0, total: None, fraction_override: Some(s.1 as f64 / self.total as f64) }
        };
        (self.progress)(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn master_with_separate_audio() {
        let text = "#EXTM3U\n\
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud-lo\",NAME=\"low\",DEFAULT=NO,URI=\"audio/lo.m3u8\"\n\
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud-hi\",NAME=\"high\",DEFAULT=YES,URI=\"audio/hi.m3u8\"\n\
            #EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=900,BANDWIDTH=1000,AUDIO=\"aud-lo\"\n\
            v/360.m3u8\n\
            #EXT-X-STREAM-INF:BANDWIDTH=5000,AUDIO=\"aud-hi\"\n\
            v/1080.m3u8\n";
        let base = Url::parse("https://cdn.example/x/master.m3u8").unwrap();
        let Parsed::Master { variants, audio } = parse(text, &base) else { panic!("not a master") };
        assert_eq!(variants.iter().map(|v| v.bandwidth).collect::<Vec<_>>(), vec![1000, 5000]);
        assert_eq!(audio.len(), 2);

        let best = pick(&variants, None).unwrap();
        assert_eq!(best.url.as_str(), "https://cdn.example/x/v/1080.m3u8");
        assert_eq!(audio_url(best, &audio).unwrap().as_str(), "https://cdn.example/x/audio/hi.m3u8");

        let low = pick(&variants, Some(&Url::parse("https://cdn.example/x/v/360.m3u8").unwrap())).unwrap();
        assert_eq!(audio_url(low, &audio).unwrap().as_str(), "https://cdn.example/x/audio/lo.m3u8");
    }

    #[test]
    fn master_with_muxed_audio_needs_no_separate_track() {
        let text = "#EXTM3U\r\n\
            #EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"main\",DEFAULT=YES\r\n\
            #EXT-X-STREAM-INF:BANDWIDTH=1000,AUDIO=\"a\"\r\n\
            v.m3u8\r\n\
            #EXT-X-STREAM-INF:BANDWIDTH=2000\r\n\
            w.m3u8\r\n";
        let Parsed::Master { variants, audio } = parse(text, &Url::parse("https://e.x/m.m3u8").unwrap()) else {
            panic!("not a master")
        };
        for v in &variants {
            assert!(audio_url(v, &audio).is_none());
        }
    }

    #[test]
    fn media_playlist_byte_ranges_and_flags() {
        let text = "#EXTM3U\n#EXT-X-MAP:URI=\"main.mp4\",BYTERANGE=\"720@0\"\n\
            #EXTINF:4,\n#EXT-X-BYTERANGE:1000@720\nmain.mp4\n\
            #EXTINF:4,\n#EXT-X-BYTERANGE:500\nmain.mp4\n\
            #EXTINF:4,\nother.mp4\n#EXT-X-ENDLIST\n";
        let Parsed::Media(m) = parse(text, &Url::parse("https://e.x/p/index.m3u8").unwrap()) else { panic!() };
        assert_eq!(m.init_segment.unwrap().range, Some(0..720));
        assert_eq!(m.segments[0].range, Some(720..1720));
        assert_eq!(m.segments[1].range, Some(1720..2220));
        assert_eq!(m.segments[2].range, None);
        assert!(!m.is_live && !m.encrypted);

        let Parsed::Media(live) = parse("#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k\"\na.ts\n", &Url::parse("https://e.x/").unwrap())
        else {
            panic!()
        };
        assert!(live.is_live && live.encrypted);
    }

    #[test]
    fn attribute_ignores_name_suffixes() {
        assert_eq!(attribute("BANDWIDTH", "#EXT-X-STREAM-INF:AVERAGE-BANDWIDTH=900,BANDWIDTH=1000").as_deref(), Some("1000"));
        assert_eq!(attribute("URI", "#EXT-X-MAP:URI=\"a,b.mp4\"").as_deref(), Some("a,b.mp4"));
        assert_eq!(attribute("AUDIO", "#EXT-X-STREAM-INF:BANDWIDTH=1"), None);
    }
}
