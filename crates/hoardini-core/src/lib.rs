//! Download engine shared by the app, the command-line tool and the browser host.
//! Behaviour follows SPEC.md of the macOS original (range probe, segmenting, resume rules).

pub mod downloader;
pub mod error;
pub mod hls;
pub mod instance;
pub mod job;
pub mod net;
pub mod paths;
pub mod probe;
pub mod remux;
pub mod segments;

pub use downloader::{DestinationFn, DownloadProgress, Options, ProgressFn};
pub use error::{Error, Result};
pub use job::JobRequest;
pub use tokio_util::sync::CancellationToken;
