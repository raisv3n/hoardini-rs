//! A tiny local HTTP server for the integration tests.
#![allow(dead_code)] // each test binary uses a different subset

use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};

use hoardini_core::{DownloadProgress, ProgressFn};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use url::Url;

pub struct Server {
    pub addr: SocketAddr,
    /// Bytes served from range requests (excludes the 1-byte probes).
    pub ranged_bytes: Arc<AtomicUsize>,
    /// Every path requested, in order.
    pub requests: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Server {
    pub fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://{}{path}", self.addr)).unwrap()
    }
}

/// Serves `files` by path. With `ranges`, honours `Range: bytes=a-b` with 206 + Content-Range.
pub async fn serve<K: Into<String>>(files: impl IntoIterator<Item = (K, Vec<u8>)>, ranges: bool) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let files: Arc<HashMap<String, Vec<u8>>> = Arc::new(files.into_iter().map(|(k, v)| (k.into(), v)).collect());
    let ranged_bytes = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (counter, log) = (ranged_bytes.clone(), requests.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let (files, counter, log) = (files.clone(), counter.clone(), log.clone());
            tokio::spawn(async move {
                let mut req = Vec::new();
                let mut buf = [0u8; 4096];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let text = String::from_utf8_lossy(&req).into_owned();
                let path = text.split_whitespace().nth(1).unwrap_or("/").to_owned();
                log.lock().unwrap().push(path.clone());
                let Some(data) = files.get(&path) else {
                    let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                };
                let range = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_owned));
                let (status, body, extra) = match (ranges, range) {
                    (true, Some(r)) => {
                        let (a, b) = r.trim().split_once('-').unwrap();
                        let a: usize = a.parse().unwrap();
                        let b: usize = b.parse().unwrap_or(data.len() - 1).min(data.len() - 1);
                        if b > a {
                            counter.fetch_add(b + 1 - a, Ordering::SeqCst);
                        }
                        ("206 Partial Content", &data[a..=b], format!("Content-Range: bytes {a}-{b}/{}\r\n", data.len()))
                    }
                    _ => ("200 OK", &data[..], String::new()),
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Disposition: attachment; filename=\"test file.bin\"\r\nETag: \"abc\"\r\nConnection: close\r\n{extra}\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Server { addr, ranged_bytes, requests }
}

pub fn payload(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i * 31 % 251) as u8).collect()
}

pub fn no_progress() -> ProgressFn {
    Arc::new(|_: DownloadProgress| {})
}
