use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use hoardini_core::JobRequest;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Queued,
    Downloading,
    Paused,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadItem {
    pub id: Uuid,
    pub request: JobRequest,
    pub status: Status,
    /// Final file path, known once the downloader picks a name.
    #[serde(default)]
    pub destination: Option<PathBuf>,
    #[serde(default)]
    pub done: u64,
    #[serde(default)]
    pub total: Option<u64>,
    #[serde(default)]
    pub fraction: Option<f64>,
    #[serde(default)]
    pub error: Option<String>,
    pub added: DateTime<Utc>,
    /// Bytes/s, smoothed; not meaningful across launches.
    #[serde(default)]
    pub speed: f64,
    /// Converted copy, when one exists.
    #[serde(default)]
    pub mp4: Option<PathBuf>,
    #[serde(default)]
    pub converting: Option<bool>,
    #[serde(default)]
    pub convert_error: Option<String>,
    /// Waiting for ffmpeg: a skipped auto-conversion, or an HLS video whose separate audio needs
    /// merging. Picked up automatically once ffmpeg is available.
    #[serde(default)]
    pub needs_ffmpeg: bool,
}

fn extension(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

/// The only files "Convert to MP4" is offered for (and auto-convert applies to): MPEG-TS, which is
/// what older HLS downloads produce and what ffmpeg remuxes losslessly into MP4.
pub const CONVERTIBLE: &[&str] = &["ts", "mts", "m2ts"];

/// What a finished file is, for the Open button's icon and wording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
    Other,
}

impl DownloadItem {
    pub fn new(request: JobRequest) -> Self {
        Self {
            id: Uuid::new_v4(),
            request,
            status: Status::Queued,
            destination: None,
            done: 0,
            total: None,
            fraction: None,
            error: None,
            added: Utc::now(),
            speed: 0.0,
            mp4: None,
            converting: None,
            convert_error: None,
            needs_ffmpeg: false,
        }
    }

    pub fn display_name(&self) -> String {
        if let Some(n) = self.destination.as_deref().and_then(Path::file_name) {
            return n.to_string_lossy().into_owned();
        }
        if let Some(f) = self.request.clean_filename() {
            return f;
        }
        self.request
            .valid_url()
            .and_then(|u| u.path_segments().and_then(|mut s| s.next_back()).filter(|s| !s.is_empty()).map(str::to_owned))
            .unwrap_or_else(|| self.request.url.clone())
    }

    /// What "Open" and "Show in Explorer" act on: the MP4 when there is one.
    pub fn playable(&self) -> Option<&Path> {
        self.mp4.as_deref().or(self.destination.as_deref())
    }

    pub fn is_converting(&self) -> bool {
        self.converting == Some(true)
    }

    pub fn can_convert(&self) -> bool {
        self.status == Status::Completed
            && !self.is_converting()
            && self.mp4.is_none()
            && self.destination.as_deref().is_some_and(|d| CONVERTIBLE.contains(&extension(d).as_str()))
    }

    /// Judged on the converted MP4 when there is one. A file without an extension is a video if it
    /// came from a stream.
    pub fn kind(&self) -> Kind {
        let ext = self.playable().map(extension).unwrap_or_default();
        if ext.is_empty() {
            return if self.request.is_hls() { Kind::Video } else { Kind::Other };
        }
        if CONVERTIBLE.contains(&ext.as_str()) {
            return Kind::Video;
        }
        crate::win::perceived_kind(&ext)
    }

    /// Every download can pause, streams included: they resume from the next unfinished segment.
    pub fn can_pause(&self) -> bool {
        self.status == Status::Downloading
    }

    pub fn eta_seconds(&self) -> Option<f64> {
        let t = self.total?;
        (self.status == Status::Downloading && self.speed > 1.0).then(|| t.saturating_sub(self.done) as f64 / self.speed)
    }
}

/// App theme: follow Windows, or force light/dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::Light, ThemeMode::Dark, ThemeMode::System];

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::System => "Use system setting",
            ThemeMode::Light => "Light",
            ThemeMode::Dark => "Dark",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub auto_convert: bool,
    pub delete_original: bool,
    pub theme: ThemeMode,
    /// The "ffmpeg isn't installed" notice was dismissed (Settings still offers the install).
    pub hide_ffmpeg_notice: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { auto_convert: true, delete_original: false, theme: ThemeMode::System, hide_ffmpeg_notice: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finished(name: &str, hls: bool) -> DownloadItem {
        let mut req = JobRequest::new("https://e.x/a");
        if hls {
            req.kind = Some("hls".into());
        }
        let mut item = DownloadItem::new(req);
        item.status = Status::Completed;
        item.destination = Some(PathBuf::from(format!(r"C:\Downloads\{name}")));
        item
    }

    #[test]
    fn only_mpeg_ts_is_convertible() {
        for name in ["a.ts", "b.MTS", "c.m2ts"] {
            assert!(finished(name, false).can_convert(), "{name}");
        }
        for name in ["a.zip", "b.pdf", "c.mkv", "d.mp4", "e"] {
            assert!(!finished(name, false).can_convert(), "{name}");
        }
    }

    #[test]
    fn kind_picks_the_open_button() {
        assert_eq!(finished("a.ts", false).kind(), Kind::Video);
        assert_eq!(finished("a.mp4", false).kind(), Kind::Video);
        assert_eq!(finished("a.mp3", false).kind(), Kind::Audio);
        assert_eq!(finished("a.zip", false).kind(), Kind::Other);
        assert_eq!(finished("noext", true).kind(), Kind::Video);
        assert_eq!(finished("noext", false).kind(), Kind::Other);
        // The converted MP4 decides once there is one.
        let mut converted = finished("a.ts", false);
        converted.mp4 = Some(PathBuf::from(r"C:\Downloads\a.mp4"));
        assert_eq!(converted.kind(), Kind::Video);
        assert!(!converted.can_convert());
    }

    #[test]
    fn every_download_can_pause() {
        let mut item = finished("a.ts", true);
        item.status = Status::Downloading;
        assert!(item.can_pause());
    }
}
