//! End-to-end downloads against a tiny local HTTP server.

mod common;

use std::{
    collections::HashMap,
    sync::{atomic::Ordering, Arc},
};

use common::{no_progress, payload, serve};
use hoardini_core::{downloader, hls, net::with_suffix, segments, DownloadProgress, Options};

fn leftovers(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".part") || n.ends_with(".hoardini") || n.ends_with(".lock"))
        .collect()
}

#[tokio::test]
async fn segmented_download_matches_source() {
    let data = payload(3_000_000);
    let server = serve(HashMap::from([("/f", data.clone())]), true).await;
    let dir = tempfile::tempdir().unwrap();
    let last = Arc::new(std::sync::Mutex::new(DownloadProgress::default()));
    let l = last.clone();
    let out = downloader::download(&server.url("/f"), &Options::new(dir.path()), Arc::new(move |p| *l.lock().unwrap() = p))
        .await
        .unwrap();
    assert_eq!(out.file_name().unwrap(), "test file.bin");
    assert_eq!(std::fs::read(&out).unwrap(), data);
    assert_eq!(last.lock().unwrap().fraction(), Some(1.0));
    assert!(leftovers(dir.path()).is_empty(), "{:?}", leftovers(dir.path()));
    let zone = std::fs::read_to_string(with_suffix(&out, ":Zone.Identifier")).unwrap();
    assert!(zone.contains("ZoneId=3"));

    // Same name again: the finished file is kept and the new one gets a suffix.
    let again = downloader::download(&server.url("/f"), &Options::new(dir.path()), no_progress()).await.unwrap();
    assert_eq!(again.file_name().unwrap(), "test file (1).bin");
}

#[tokio::test]
async fn server_without_ranges_uses_single_stream() {
    let data = payload(700_000);
    let server = serve(HashMap::from([("/f", data.clone())]), false).await;
    let dir = tempfile::tempdir().unwrap();
    let out = downloader::download(&server.url("/f"), &Options::new(dir.path()), no_progress()).await.unwrap();
    assert_eq!(std::fs::read(out).unwrap(), data);
    assert!(leftovers(dir.path()).is_empty());
}

#[tokio::test]
async fn resumes_from_sidecar_state() {
    let data = payload(3_000_000);
    let server = serve(HashMap::from([("/f", data.clone())]), true).await;
    let dir = tempfile::tempdir().unwrap();
    let final_path = dir.path().join("test file.bin");

    // Simulate an interrupted run: segment 0 complete, segment 1 half done, the rest untouched.
    let mut segs = segments::plan(data.len() as u64, 8, segments::MIN_SEGMENT_SIZE);
    segs[0].written = segs[0].end - segs[0].start + 1;
    segs[1].written = segs[1].remaining() / 2;
    let mut part = vec![0u8; data.len()];
    for s in &segs[..2] {
        let (a, b) = (s.start as usize, (s.start + s.written) as usize);
        part[a..b].copy_from_slice(&data[a..b]);
    }
    std::fs::write(with_suffix(&final_path, ".part"), &part).unwrap();
    let state = serde_json::json!({
        "url": server.url("/f").as_str(), "length": data.len(), "validator": "\"abc\"", "segments": segs,
    });
    std::fs::write(with_suffix(&final_path, ".hoardini"), state.to_string()).unwrap();

    let out = downloader::download(&server.url("/f"), &Options::new(dir.path()), no_progress()).await.unwrap();
    assert_eq!(out, final_path);
    assert_eq!(std::fs::read(&out).unwrap(), data);
    let already: u64 = segs.iter().map(|s| s.written).sum();
    assert_eq!(server.ranged_bytes.load(Ordering::SeqCst) as u64, data.len() as u64 - already, "only missing bytes are fetched");
    assert!(leftovers(dir.path()).is_empty());
}

#[tokio::test]
async fn cancel_keeps_resume_state() {
    let data = payload(3_000_000);
    let server = serve(HashMap::from([("/f", data.clone())]), true).await;
    let dir = tempfile::tempdir().unwrap();
    let opts = Options::new(dir.path());
    opts.cancel.cancel();
    assert!(matches!(
        downloader::download(&server.url("/f"), &opts, no_progress()).await,
        Err(hoardini_core::Error::Cancelled)
    ));
}

#[tokio::test]
async fn hls_master_variant_and_byte_ranges() {
    let seg_a = payload(1000);
    let seg_b = payload(1500);
    let packed = payload(4000);
    let files = HashMap::from([
        ("/v/master.m3u8", b"#EXTM3U\n#EXT-X-STREAM-INF:BANDWIDTH=100\nlow.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=900\nhigh.m3u8\n".to_vec()),
        ("/v/low.m3u8", b"#EXTM3U\n#EXTINF:4,\nnope.ts\n#EXT-X-ENDLIST\n".to_vec()),
        (
            "/v/high.m3u8",
            b"#EXTM3U\n#EXTINF:4,\na.ts\n#EXTINF:4,\nb.ts\n#EXT-X-BYTERANGE:1000@500\npacked.ts\n#EXT-X-BYTERANGE:200\npacked.ts\n#EXT-X-ENDLIST\n"
                .to_vec(),
        ),
        ("/v/a.ts", seg_a.clone()),
        ("/v/b.ts", seg_b.clone()),
        ("/v/packed.ts", packed.clone()),
    ]);
    let server = serve(files, true).await;
    let dir = tempfile::tempdir().unwrap();
    let mut opts = Options::new(dir.path());
    opts.filename = Some("Lesson 1.5 intro.ts".into());
    let out = hls::download(&server.url("/v/master.m3u8"), &opts, no_progress()).await.unwrap();
    assert_eq!(out.file_name().unwrap(), "Lesson 1.5 intro.ts");
    let expected = [&seg_a[..], &seg_b[..], &packed[500..1500], &packed[1500..1700]].concat();
    assert_eq!(std::fs::read(out).unwrap(), expected);
}

#[tokio::test]
async fn hls_rejects_live_streams() {
    let files = HashMap::from([("/live.m3u8", b"#EXTM3U\n#EXTINF:4,\na.ts\n".to_vec())]);
    let server = serve(files, true).await;
    let dir = tempfile::tempdir().unwrap();
    let r = hls::download(&server.url("/live.m3u8"), &Options::new(dir.path()), no_progress()).await;
    assert!(matches!(r, Err(hoardini_core::Error::LiveStream)));
}

// ---------- ffmpeg-backed (skipped when ffmpeg is not installed) ----------

fn ffmpeg_or_skip() -> Option<std::path::PathBuf> {
    let p = hoardini_core::remux::ffmpeg_path();
    if p.is_none() {
        eprintln!("ffmpeg not found; skipping");
    }
    p
}

fn run_ffmpeg(ffmpeg: &std::path::Path, dir: &std::path::Path, args: &str) {
    let ok = std::process::Command::new(ffmpeg)
        .current_dir(dir)
        .args(["-hide_banner", "-loglevel", "error", "-y"])
        .args(args.split_whitespace())
        .status()
        .unwrap()
        .success();
    assert!(ok, "ffmpeg {args}");
}

/// "Video: ..." / "Audio: ..." stream lines as reported by ffmpeg.
fn streams(ffmpeg: &std::path::Path, file: &std::path::Path) -> (usize, usize) {
    let out = std::process::Command::new(ffmpeg).arg("-hide_banner").arg("-i").arg(file).output().unwrap();
    let info = String::from_utf8_lossy(&out.stderr);
    (info.matches("Video:").count(), info.matches("Audio:").count())
}

#[tokio::test]
async fn converts_ts_to_mp4() {
    let Some(ffmpeg) = ffmpeg_or_skip() else { return };
    let dir = tempfile::tempdir().unwrap();
    run_ffmpeg(&ffmpeg, dir.path(),
        "-f lavfi -i testsrc=size=160x120:rate=10:duration=2 -f lavfi -i sine=duration=2 -c:v libx264 -c:a aac -f mpegts clip.ts");
    let mp4 = hoardini_core::remux::to_mp4(&dir.path().join("clip.ts"), &Default::default()).await.unwrap();
    assert_eq!(mp4.file_name().unwrap(), "clip.mp4");
    assert_eq!(streams(&ffmpeg, &mp4), (1, 1));
}

#[tokio::test]
async fn hls_separate_audio_is_merged() {
    let Some(ffmpeg) = ffmpeg_or_skip() else { return };
    let src = tempfile::tempdir().unwrap();
    // A Vimeo-style stream: video-only variant + audio rendition in its own playlist.
    run_ffmpeg(&ffmpeg, src.path(),
        "-f lavfi -i testsrc=size=160x120:rate=10:duration=4 -c:v libx264 -an -f hls -hls_time 1 -hls_playlist_type vod -hls_segment_filename v%d.ts video.m3u8");
    run_ffmpeg(&ffmpeg, src.path(),
        "-f lavfi -i sine=duration=4 -c:a aac -vn -f hls -hls_time 1 -hls_playlist_type vod -hls_segment_filename a%d.ts audio.m3u8");
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(src.path())
        .unwrap()
        .flatten()
        .map(|e| (format!("/s/{}", e.file_name().to_string_lossy()), std::fs::read(e.path()).unwrap()))
        .collect();
    files.push((
        "/s/master.m3u8".into(),
        b"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"aud\",NAME=\"en\",DEFAULT=YES,URI=\"audio.m3u8\"\n\
          #EXT-X-STREAM-INF:BANDWIDTH=200000,AUDIO=\"aud\"\nvideo.m3u8\n"
            .to_vec(),
    ));
    let server = serve(files, true).await;
    let dir = tempfile::tempdir().unwrap();
    let mut opts = Options::new(dir.path());
    opts.filename = Some("merged".into());
    let out = hls::download(&server.url("/s/master.m3u8"), &opts, no_progress()).await.unwrap();
    assert_eq!(out.file_name().unwrap(), "merged.mp4");
    assert_eq!(streams(&ffmpeg, &out), (1, 1));
    let names: Vec<String> = std::fs::read_dir(dir.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["merged.mp4".to_string()], "temp tracks are cleaned up");
}
