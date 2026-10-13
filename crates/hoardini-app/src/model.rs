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

/// Containers worth converting without being asked (an auto-convert of a .zip would only fail).
const AUTO_CONVERT: &[&str] = &["ts", "m2ts", "mts", "mkv", "webm", "flv"];

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
            && self.destination.as_deref().is_some_and(|d| !["mp4", "m4v", "mov"].contains(&extension(d).as_str()))
    }

    /// Offered (and done automatically) only for video containers; files picked via
    /// "Convert Files…" are converted whatever their extension, as in the original.
    pub fn should_auto_convert(&self) -> bool {
        self.can_convert() && self.destination.as_deref().is_some_and(|d| AUTO_CONVERT.contains(&extension(d).as_str()))
    }

    /// HLS downloads cannot resume, so they are not pausable.
    pub fn can_pause(&self) -> bool {
        !self.request.is_hls() && self.status == Status::Downloading
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
