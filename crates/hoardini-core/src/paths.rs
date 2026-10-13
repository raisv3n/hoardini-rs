use std::{io, path::PathBuf};

/// Named mutex held by the running app: single-instance guard, and how the host knows the app is up.
pub const APP_MUTEX: &str = "Local\\Hoardini.App";
pub const APP_EXE: &str = "hoardini-app.exe";
pub const CLI_EXE: &str = "hoardini.exe";

/// `%APPDATA%\Hoardini` (per-user, already private to the account).
pub fn support() -> PathBuf {
    dirs::data_dir().unwrap_or_else(std::env::temp_dir).join("Hoardini")
}

/// Jobs dropped here by the browser host are picked up by the running app.
pub fn inbox() -> PathBuf {
    support().join("inbox")
}

pub fn downloads() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn ensure_dir(p: &std::path::Path) -> io::Result<()> {
    std::fs::create_dir_all(p)
}

/// A sibling of the running executable, e.g. `hoardini.exe` next to `hoardini-host.exe`.
pub fn sibling_exe(name: &str) -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.join(name))
}
