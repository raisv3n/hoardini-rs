//! Lossless container conversion. The macOS app used AVFoundation's passthrough export;
//! on Windows the equivalent is ffmpeg with `-c copy` (no re-encoding).

use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use tokio_util::sync::CancellationToken;

use crate::{
    net::{self, with_suffix},
    Error, Result,
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `HOARDINI_FFMPEG` if set (exactly that file, nothing else), else `ffmpeg.exe` next to Hoardini,
/// then PATH, then winget's links folder (apps started from the Start menu may carry a PATH from
/// before ffmpeg was installed).
pub fn ffmpeg_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("HOARDINI_FFMPEG") {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let candidates = crate::paths::sibling_exe("ffmpeg.exe")
        .into_iter()
        .chain(std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).map(|d| d.join("ffmpeg.exe")))
        .chain(dirs::data_local_dir().map(|d| d.join("Microsoft").join("WinGet").join("Links").join("ffmpeg.exe")));
    candidates.into_iter().find(|p| p.is_file())
}

async fn ffmpeg(args: Vec<OsString>, cancel: &CancellationToken) -> Result<()> {
    let exe = ffmpeg_path().ok_or(Error::FfmpegMissing)?;
    let mut cmd = tokio::process::Command::new(exe);
    cmd.args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y"])
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .creation_flags(CREATE_NO_WINDOW);
    let child = cmd.spawn()?;
    let out = tokio::select! {
        _ = cancel.cancelled() => return Err(Error::Cancelled),
        o = child.wait_with_output() => o?,
    };
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    let msg = stderr.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("ffmpeg failed").to_owned();
    if msg.contains("does not contain any stream") || msg.contains("Invalid data found") {
        return Err(Error::Unreadable);
    }
    Err(Error::Remux(msg))
}

fn os(args: &[&dyn AsRef<std::ffi::OsStr>]) -> Vec<OsString> {
    args.iter().map(|a| a.as_ref().to_owned()).collect()
}

/// Copies the audio/video streams of `source` into an MP4 next to it, without re-encoding.
pub async fn to_mp4(source: &Path, cancel: &CancellationToken) -> Result<PathBuf> {
    if !source.is_file() {
        return Err(Error::Unreadable);
    }
    let dir = source.parent().unwrap_or(Path::new("."));
    let base = source.file_stem().unwrap_or_default().to_string_lossy();
    let (final_path, _lock) = net::claim(dir, &format!("{base}.mp4"))?;
    let tmp = with_suffix(&final_path, ".part");
    let _ = fs::remove_file(&tmp);
    let args = os(&[
        &"-i", &source, &"-map", &"0:v?", &"-map", &"0:a?", &"-c", &"copy",
        // Index at the front: instant seeking.
        &"-movflags", &"+faststart", &"-f", &"mp4", &tmp,
    ]);
    if let Err(e) = ffmpeg(args, cancel).await {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, &final_path)?;
    Ok(final_path)
}

/// Combines a video-only and an audio-only file into one MP4, without re-encoding.
pub async fn merge(video: &Path, audio: &Path, output: &Path, cancel: &CancellationToken) -> Result<()> {
    let args = os(&[
        &"-i", &video, &"-i", &audio, &"-map", &"0:v:0", &"-map", &"1:a:0", &"-c", &"copy",
        &"-movflags", &"+faststart", &"-f", &"mp4", &output,
    ]);
    let r = ffmpeg(args, cancel).await;
    if r.is_err() {
        let _ = fs::remove_file(output);
    }
    r
}
