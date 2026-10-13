use std::{
    ffi::OsString,
    fs::{File, OpenOptions},
    io,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use reqwest::{Client, RequestBuilder};
use url::Url;

use crate::{probe::split_ext, Error, Result};

/// reqwest's default redirect policy already drops Cookie/Authorization when a redirect
/// changes host, which is what the macOS `RedirectGuard` did by hand.
/// The timeouts mirror URLSession's defaults: a stalled server fails (and is retried) instead of hanging.
pub fn client() -> Client {
    Client::builder()
        .user_agent(concat!("Hoardini/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(60))
        .build()
        .expect("HTTP client")
}

/// Headers safe to send to `url`: credentials only go to the host the caller supplied them for.
pub fn apply_headers(mut req: RequestBuilder, headers: &[(String, String)], url: &Url, origin: &Url) -> RequestBuilder {
    let same_host = url.host_str().map(str::to_ascii_lowercase) == origin.host_str().map(str::to_ascii_lowercase);
    for (k, v) in headers {
        let credential = k.eq_ignore_ascii_case("cookie") || k.eq_ignore_ascii_case("authorization");
        if same_host || !credential {
            req = req.header(k.as_str(), v.as_str());
        }
    }
    req
}

pub fn header_value<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// `x.mp4` + `.part` -> `x.mp4.part`.
pub fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

fn without_credentials(u: &Url) -> String {
    let mut u = u.clone();
    let _ = u.set_username("");
    let _ = u.set_password(None);
    u.to_string()
}

/// Tags a downloaded file the way browsers do (Mark of the Web), so SmartScreen and
/// Office Protected View check it before it is opened. The Windows counterpart of the
/// macOS quarantine attribute; silently skipped on file systems without streams.
pub fn mark_of_the_web(file: &Path, source: &Url, referrer: Option<&str>) {
    let mut zone = String::from("[ZoneTransfer]\r\nZoneId=3\r\n");
    if let Some(r) = referrer.and_then(|r| Url::parse(r).ok()) {
        zone += &format!("ReferrerUrl={}\r\n", without_credentials(&r));
    }
    zone += &format!("HostUrl={}\r\n", without_credentials(source));
    let _ = std::fs::write(with_suffix(file, ":Zone.Identifier"), zone);
}

/// An exclusive, delete-on-close `<name>.lock` file: the lock disappears even if the process crashes.
pub struct DestinationLock {
    _file: File,
}

const GENERIC_READ: u32 = 0x8000_0000;
const GENERIC_WRITE: u32 = 0x4000_0000;
const DELETE: u32 = 0x0001_0000;
const FILE_FLAG_DELETE_ON_CLOSE: u32 = 0x0400_0000;
pub(crate) const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

fn open_lock(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
        .share_mode(0)
        .custom_flags(FILE_FLAG_DELETE_ON_CLOSE)
        .attributes(FILE_ATTRIBUTE_HIDDEN)
        .open(path)
}

/// Picks a destination name nobody else is using and holds a lock on it.
/// A name is usable if we get its lock and it is either a resumable download or not yet taken.
pub fn claim(directory: &Path, filename: &str) -> Result<(PathBuf, DestinationLock)> {
    let (base, ext) = split_ext(filename);
    for n in 0..1000 {
        let name = match (n, ext) {
            (0, _) => filename.to_owned(),
            (_, Some(e)) => format!("{base} ({n}).{e}"),
            (_, None) => format!("{base} ({n})"),
        };
        let final_path = directory.join(&name);
        match open_lock(&with_suffix(&final_path, ".lock")) {
            Ok(file) => {
                let resumable = with_suffix(&final_path, ".hoardini").exists();
                if resumable || !final_path.exists() {
                    return Ok((final_path, DestinationLock { _file: file }));
                }
            }
            // 32 = sharing violation (another download holds it), 5 = lock pending deletion.
            Err(e) if matches!(e.raw_os_error(), Some(32) | Some(5)) => continue,
            Err(_) => break,
        }
    }
    Err(Error::CannotWrite(directory.display().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_skips_taken_and_locked_names() {
        let dir = tempfile::tempdir().unwrap();
        let (a, lock_a) = claim(dir.path(), "x.bin").unwrap();
        assert_eq!(a.file_name().unwrap(), "x.bin");
        // Still locked by `lock_a`.
        let (b, _lock_b) = claim(dir.path(), "x.bin").unwrap();
        assert_eq!(b.file_name().unwrap(), "x (1).bin");
        drop(lock_a);
        assert!(!with_suffix(&a, ".lock").exists(), "lock file is removed on close");
        std::fs::write(&a, b"done").unwrap();
        let (c, _lock_c) = claim(dir.path(), "x.bin").unwrap();
        assert_eq!(c.file_name().unwrap(), "x (2).bin");
    }

    #[test]
    fn mark_of_the_web_writes_zone_identifier() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x.exe");
        std::fs::write(&f, b"x").unwrap();
        mark_of_the_web(&f, &Url::parse("https://u:p@example.com/x").unwrap(), Some("https://example.com/"));
        let zone = std::fs::read_to_string(with_suffix(&f, ":Zone.Identifier")).unwrap();
        assert!(zone.contains("ZoneId=3"));
        assert!(zone.contains("HostUrl=https://example.com/x"));
        assert!(zone.contains("ReferrerUrl=https://example.com/"));
    }

    #[test]
    fn credentials_stay_on_origin_host() {
        let c = Client::new();
        let h = vec![("Cookie".to_owned(), "a=b".to_owned()), ("Referer".to_owned(), "r".to_owned())];
        let origin = Url::parse("https://site.example/a").unwrap();
        let cdn = Url::parse("https://cdn.example/a").unwrap();
        let req = apply_headers(c.get(cdn.clone()), &h, &cdn, &origin).build().unwrap();
        assert!(req.headers().get("cookie").is_none());
        assert!(req.headers().get("referer").is_some());
        let req = apply_headers(c.get(origin.clone()), &h, &origin, &origin).build().unwrap();
        assert!(req.headers().get("cookie").is_some());
    }
}
