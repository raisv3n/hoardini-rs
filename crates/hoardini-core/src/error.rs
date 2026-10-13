#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Server returned HTTP {0}")]
    BadResponse(u16),
    #[error("Server ignored the Range header")]
    RangeIgnored,
    #[error("Expected {expected} bytes, got {got}")]
    SizeMismatch { expected: u64, got: u64 },
    #[error("Cannot create a destination file in {0}")]
    CannotWrite(String),
    #[error("Could not read the HLS playlist")]
    BadPlaylist,
    #[error("This stream is encrypted; Hoardini does not download protected streams")]
    EncryptedStream,
    #[error("Live streams are not supported")]
    LiveStream,
    #[error("This file has no readable video or audio")]
    Unreadable,
    #[error("Conversion failed: {0}")]
    Remux(String),
    #[error("ffmpeg was not found. Install it with `winget install Gyan.FFmpeg`, or put ffmpeg.exe next to Hoardini")]
    FfmpegMissing,
    #[error("Cancelled")]
    Cancelled,
    #[error("{}", http_message(.0))]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// reqwest's own message ("error sending request for url ...") hides the cause; show the innermost one.
fn http_message(e: &reqwest::Error) -> String {
    let summary = if e.is_timeout() {
        "The connection timed out"
    } else if e.is_connect() {
        "Could not connect to the server"
    } else if e.is_body() || e.is_decode() {
        "The connection was interrupted"
    } else {
        "Network error"
    };
    let mut cause: Option<&dyn std::error::Error> = std::error::Error::source(e);
    while let Some(next) = cause.and_then(|c| c.source()) {
        cause = Some(next);
    }
    match cause {
        Some(c) => format!("{summary}: {c}"),
        None => summary.to_owned(),
    }
}
