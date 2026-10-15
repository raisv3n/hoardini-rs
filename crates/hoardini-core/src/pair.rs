//! A video-only file and an audio-only file (DASH representations, e.g. Facebook's 720p) downloaded
//! side by side and merged into one MP4. Each half is an ordinary segmented, resumable file download
//! into a hidden track file next to the final name.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    downloader::{self, cancellable},
    net::{self, with_suffix},
    probe, remux, DownloadProgress, Error, Options, ProgressFn, Result,
};

/// Downloads `video` and `audio` into `opts.directory` and merges them, returning the final MP4's path.
pub async fn download(video: &Url, audio: &Url, opts: &Options, progress: ProgressFn) -> Result<PathBuf> {
    // The two halves can only be merged with ffmpeg: fail now, not after downloading everything.
    if remux::ffmpeg_path().is_none() {
        return Err(Error::FfmpegMissing);
    }
    let mut stem = opts.filename.clone().unwrap_or_else(|| {
        let last = video.path_segments().and_then(|mut s| s.next_back()).unwrap_or("");
        probe::split_ext(last).0.to_owned()
    });
    for known in [".mp4", ".m4v", ".mov"] {
        let cut = stem.len().saturating_sub(known.len());
        if stem.is_char_boundary(cut) && stem[cut..].eq_ignore_ascii_case(known) {
            stem.truncate(cut);
        }
    }
    let (final_path, _lock) = net::claim(&opts.directory, &format!("{}.mp4", probe::sanitize(&stem)))?;
    if let Some(cb) = &opts.on_destination {
        cb(&final_path);
    }

    // The browser attached cookies for the video's host; they must not travel to a different audio host.
    let audio_headers = net::headers_for(&opts.headers, audio, video);
    // Both sizes up front, so progress runs over the two files from the start.
    let (pv, pa) = tokio::try_join!(
        cancellable(&opts.cancel, probe::run(&opts.client, video, &opts.headers)),
        cancellable(&opts.cancel, probe::run(&opts.client, audio, &audio_headers)),
    )?;
    let sum = Arc::new(PairProgress { done: Mutex::new([0, 0]), total: pv.length.zip(pa.length).map(|(v, a)| v + a), progress });

    // One failing half stops the other (it saves its resume state first).
    let group = opts.cancel.child_token();
    let files = ["video", "audio"].map(|t| net::track_file(&final_path, t, "mp4"));
    let (v, a) = tokio::join!(
        track(video, &files[0], opts.headers.clone(), 0, opts, &group, &sum),
        track(audio, &files[1], audio_headers, 1, opts, &group, &sum),
    );
    let (video_file, audio_file) = match (v, a) {
        (Ok(v), Ok(a)) => (v, a),
        _ if opts.cancel.is_cancelled() => return Err(Error::Cancelled),
        (Err(e), _) | (_, Err(e)) if !matches!(e, Error::Cancelled) => return Err(e),
        _ => return Err(Error::Cancelled),
    };

    let part = with_suffix(&final_path, ".part");
    let _ = fs::remove_file(&part);
    // Cancelling (pause) kills ffmpeg; both track files stay, so a resume goes straight to merging.
    remux::merge(&video_file, &audio_file, &part, &opts.cancel).await?;
    net::mark_of_the_web(&part, video, opts.referrer());
    fs::rename(&part, &final_path)?;
    for f in [video_file, audio_file] {
        let _ = fs::remove_file(f);
    }
    Ok(final_path)
}

/// Downloads one half to its hidden track file, or reuses it when an earlier run finished it
/// (a finished track has no .hoardini resume state next to it).
async fn track(
    url: &Url,
    file: &Path,
    headers: Vec<(String, String)>,
    k: usize,
    opts: &Options,
    group: &CancellationToken,
    sum: &Arc<PairProgress>,
) -> Result<PathBuf> {
    if file.is_file() && !with_suffix(file, ".hoardini").exists() {
        sum.set(k, fs::metadata(file).map(|m| m.len()).unwrap_or(0));
        return Ok(file.to_path_buf());
    }
    let mut o = opts.clone();
    o.headers = headers;
    o.filename = None;
    o.on_destination = None;
    o.cancel = group.clone();
    o.track_name = file.file_name().map(|n| n.to_string_lossy().into_owned());
    let s = sum.clone();
    let r = downloader::download(url, &o, Arc::new(move |p| s.set(k, p.done))).await;
    if r.is_err() {
        group.cancel();
    }
    r
}

struct PairProgress {
    done: Mutex<[u64; 2]>,
    total: Option<u64>,
    progress: ProgressFn,
}

impl PairProgress {
    fn set(&self, k: usize, n: u64) {
        let done = {
            let mut d = self.done.lock().unwrap();
            d[k] = n;
            d[0] + d[1]
        };
        (self.progress)(DownloadProgress::bytes(done, self.total));
    }
}
