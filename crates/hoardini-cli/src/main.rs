//! usage: hoardini <url> [-o dir] [-n connections] [-H "Name: value"]... [--headers-stdin] [--name file] [--hls] [--variant url] [--audio url]
//!        hoardini convert <file.ts>...    lossless remux to MP4 next to each file
//! --audio: <url> is a video-only file; also download this audio-only file and merge both into one MP4.
//! --headers-stdin reads more "Name: value" lines from stdin, so cookies never appear in the process list.

use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hoardini_core::{downloader, hls, job::http_url, pair, remux, CancellationToken, DownloadProgress, Error, Options};

const USAGE: &str = "usage: hoardini <http(s) url> [-o dir] [-n connections] [-H \"Name: value\"]... [--headers-stdin] [--name file] [--hls] [--variant url] [--audio url]\n       hoardini convert <file>...";

fn take(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let i = args.iter().position(|a| a == flag)?;
    if i + 1 >= args.len() {
        return None;
    }
    let v = args.remove(i + 1);
    args.remove(i);
    Some(v)
}

fn take_switch(args: &mut Vec<String>, flag: &str) -> bool {
    let found = args.iter().position(|a| a == flag);
    if let Some(i) = found {
        args.remove(i);
    }
    found.is_some()
}

fn add_header(headers: &mut Vec<(String, String)>, line: &str) {
    if let Some((k, v)) = line.split_once(':') {
        let k = k.trim().to_owned();
        headers.retain(|(name, _)| !name.eq_ignore_ascii_case(&k));
        headers.push((k, v.trim().to_owned()));
    }
}

/// Ctrl+C pauses: segment progress is saved, and running the same command again resumes.
fn cancel_on_ctrl_c() -> CancellationToken {
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            t.cancel();
        }
    });
    token
}

#[tokio::main]
async fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().map(String::as_str) == Some("convert") {
        let cancel = cancel_on_ctrl_c();
        let mut failed = false;
        for path in &args[1..] {
            match remux::to_mp4(Path::new(path), &cancel).await {
                Ok(p) => println!("{}", p.display()),
                Err(e) => {
                    eprintln!("{path}: {e}");
                    failed = true;
                }
            }
        }
        return if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS };
    }

    let dir = take(&mut args, "-o").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let conns = take(&mut args, "-n").and_then(|n| n.parse().ok()).unwrap_or(8);
    let mut headers = Vec::new();
    while let Some(h) = take(&mut args, "-H") {
        add_header(&mut headers, &h);
    }
    if take_switch(&mut args, "--headers-stdin") {
        let mut input = String::new();
        let _ = std::io::stdin().read_to_string(&mut input);
        input.lines().for_each(|l| add_header(&mut headers, l));
    }
    let name = take(&mut args, "--name");
    let variant = take(&mut args, "--variant").and_then(|v| http_url(&v));
    let audio = take(&mut args, "--audio").and_then(|a| http_url(&a));
    let force_hls = take_switch(&mut args, "--hls");

    let Some(url) = args.first().and_then(|a| http_url(a)) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };

    let start = Instant::now();
    let last = Mutex::new(Instant::now() - Duration::from_secs(1));
    let on_progress = Arc::new(move |p: DownloadProgress| {
        let finished = p.fraction() == Some(1.0);
        {
            let mut l = last.lock().unwrap();
            if !finished && l.elapsed() < Duration::from_millis(250) {
                return;
            }
            *l = Instant::now();
        }
        let secs = start.elapsed().as_secs_f64().max(0.001);
        let mb = p.done as f64 / 1_048_576.0;
        let pct = p.fraction().map(|f| format!("{:5.1}%", f * 100.0)).unwrap_or_else(|| "  ?  ".into());
        eprint!("\r{pct}  {mb:.1} MB  {:.1} MB/s ", mb / secs);
        let _ = std::io::stderr().flush();
    });

    let mut opts = Options::new(dir);
    opts.connections = conns;
    opts.headers = headers;
    opts.filename = name;
    opts.variant = variant;
    opts.cancel = cancel_on_ctrl_c();

    let is_hls = force_hls || hoardini_core::job::url_extension(&url) == "m3u8";
    let result = if is_hls {
        hls::download(&url, &opts, on_progress).await
    } else if let Some(audio) = &audio {
        pair::download(&url, audio, &opts, on_progress).await
    } else {
        downloader::download(&url, &opts, on_progress).await
    };
    eprintln!();
    match result {
        Ok(out) => {
            println!("{}", out.display());
            ExitCode::SUCCESS
        }
        Err(Error::Cancelled) => {
            eprintln!("paused; run the same command again to resume");
            ExitCode::from(130)
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
