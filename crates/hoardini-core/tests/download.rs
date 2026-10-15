//! End-to-end downloads against a tiny local HTTP server.

mod common;

use std::{
    collections::HashMap,
    sync::{atomic::Ordering, Arc},
};

use common::{no_progress, payload, serve};
use hoardini_core::{downloader, hls, net::with_suffix, pair, segments, DownloadProgress, Options};

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

/// A 10-segment MPEG-TS playlist; returns (files to serve, the expected joined output).
fn ts_stream() -> (Vec<(String, Vec<u8>)>, Vec<u8>) {
    let segs: Vec<Vec<u8>> = (0..10).map(|i| payload(1000 + i * 37)).collect();
    let mut playlist = String::from("#EXTM3U\n");
    for i in 0..segs.len() {
        playlist += &format!("#EXTINF:4,\ns{i}.ts\n");
    }
    playlist += "#EXT-X-ENDLIST\n";
    let mut files: Vec<(String, Vec<u8>)> = segs.iter().enumerate().map(|(i, s)| (format!("/h/s{i}.ts"), s.clone())).collect();
    files.push(("/h/index.m3u8".into(), playlist.into_bytes()));
    (files, segs.concat())
}

fn segment_requests(server: &common::Server) -> Vec<String> {
    server.requests.lock().unwrap().iter().filter(|p| p.ends_with(".ts")).cloned().collect()
}

#[tokio::test]
async fn hls_resumes_after_the_last_saved_window() {
    let (files, expected) = ts_stream();
    let server = serve(files, true).await;
    let dir = tempfile::tempdir().unwrap();
    let final_path = dir.path().join("talk.ts");
    let url = server.url("/h/index.m3u8");

    // Interrupted after the first window of 6 segments, with part of the next window already written.
    let saved: usize = (0..6).map(|i| 1000 + i * 37).sum();
    let mut part = expected[..saved].to_vec();
    part.extend_from_slice(b"half-written window");
    std::fs::write(with_suffix(&final_path, ".part"), part).unwrap();
    let state = serde_json::json!({
        "source": url.as_str(), "tracks": [{"segments": 10, "done": 6, "bytes": saved, "sequence": 1}],
    });
    std::fs::write(with_suffix(&final_path, ".hoardini"), state.to_string()).unwrap();

    let first = Arc::new(std::sync::Mutex::new(None));
    let f = first.clone();
    let mut opts = Options::new(dir.path());
    opts.filename = Some("talk".into());
    let progress = Arc::new(move |p: DownloadProgress| {
        f.lock().unwrap().get_or_insert(p);
    });
    let out = hls::download(&url, &opts, progress).await.unwrap();
    assert_eq!(out, final_path);
    assert_eq!(std::fs::read(&out).unwrap(), expected);
    assert_eq!(segment_requests(&server), ["/h/s6.ts", "/h/s7.ts", "/h/s8.ts", "/h/s9.ts"]);
    let first = first.lock().unwrap().unwrap();
    assert_eq!((first.done, first.fraction()), (saved as u64, Some(0.6)), "progress starts from what is on disk");
    assert!(leftovers(dir.path()).is_empty(), "{:?}", leftovers(dir.path()));
}

#[tokio::test]
async fn hls_starts_over_when_the_stream_changed_or_state_is_corrupt() {
    for state in [
        // A different number of segments.
        serde_json::json!({"source": "SOURCE", "tracks": [{"segments": 11, "done": 6, "bytes": 6000, "sequence": 1}]}),
        // Negative byte count (edited file).
        serde_json::json!({"source": "SOURCE", "tracks": [{"segments": 10, "done": 6, "bytes": -5, "sequence": 1}]}),
        // done past the end, and a zero sequence.
        serde_json::json!({"source": "SOURCE", "tracks": [{"segments": 10, "done": 11, "bytes": 6000, "sequence": 1}]}),
        serde_json::json!({"source": "SOURCE", "tracks": [{"segments": 10, "done": 6, "bytes": 6000, "sequence": 0}]}),
    ] {
        let (files, expected) = ts_stream();
        let server = serve(files, true).await;
        let dir = tempfile::tempdir().unwrap();
        let url = server.url("/h/index.m3u8");
        let final_path = dir.path().join("talk.ts");
        std::fs::write(with_suffix(&final_path, ".part"), payload(9000)).unwrap();
        let state = state.to_string().replace("SOURCE", url.as_str());
        std::fs::write(with_suffix(&final_path, ".hoardini"), state).unwrap();
        let mut opts = Options::new(dir.path());
        opts.filename = Some("talk".into());
        let out = hls::download(&url, &opts, no_progress()).await.unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), expected);
        assert_eq!(segment_requests(&server).len(), 10);
    }
}

#[tokio::test]
async fn hls_failure_keeps_progress_for_retry() {
    let (mut files, expected) = ts_stream();
    files.retain(|(p, _)| p != "/h/s8.ts"); // 404 in the second window
    let server = serve(files.clone(), true).await;
    let dir = tempfile::tempdir().unwrap();
    let mut opts = Options::new(dir.path());
    opts.filename = Some("talk".into());
    assert!(hls::download(&server.url("/h/index.m3u8"), &opts, no_progress()).await.is_err());
    let part = dir.path().join("talk.ts.part");
    let saved: usize = (0..6).map(|i| 1000 + i * 37).sum();
    assert_eq!(std::fs::metadata(&part).unwrap().len(), saved as u64);

    // The same playlist served again with the segment back: only the rest is fetched.
    let (files, _) = ts_stream();
    let server2 = serve(files, true).await;
    let state = std::fs::read_to_string(dir.path().join("talk.ts.hoardini")).unwrap();
    let url2 = server2.url("/h/index.m3u8");
    let old_url = server.url("/h/index.m3u8");
    std::fs::write(dir.path().join("talk.ts.hoardini"), state.replace(old_url.as_str(), url2.as_str())).unwrap();
    let out = hls::download(&url2, &opts, no_progress()).await.unwrap();
    assert_eq!(std::fs::read(out).unwrap(), expected);
    assert_eq!(segment_requests(&server2).len(), 4);
}

fn mp4_box(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    b.extend_from_slice(kind);
    b.extend_from_slice(body);
    b
}

/// moof(mfhd) + mdat, numbered `seq`.
fn fragment(seq: u32, media: &[u8]) -> Vec<u8> {
    let mfhd = mp4_box(b"mfhd", &[&[0u8, 0, 0, 0][..], &seq.to_be_bytes()].concat());
    [mp4_box(b"moof", &mfhd), mp4_box(b"mdat", media)].concat()
}

#[tokio::test]
async fn hls_fmp4_fragments_are_renumbered() {
    // Tella: every segment says fragment #1.
    let init = mp4_box(b"ftyp", b"cmfc");
    let mut playlist = String::from("#EXTM3U\n#EXT-X-MAP:URI=\"init.mp4\"\n");
    let mut files = vec![("/t/init.mp4".to_string(), init.clone())];
    let mut expected = init.clone();
    for i in 0..8u32 {
        let media = payload(500 + i as usize);
        files.push((format!("/t/{i}.cmfv"), fragment(1, &media)));
        expected.extend(fragment(i + 1, &media));
        playlist += &format!("#EXTINF:6,\n{i}.cmfv\n");
    }
    playlist += "#EXT-X-ENDLIST\n";
    files.push(("/t/index.m3u8".into(), playlist.into_bytes()));
    let server = serve(files, true).await;
    let dir = tempfile::tempdir().unwrap();
    let out = hls::download(&server.url("/t/index.m3u8"), &Options::new(dir.path()), no_progress()).await.unwrap();
    assert_eq!(out.extension().unwrap(), "mp4");
    assert_eq!(std::fs::read(out).unwrap(), expected);
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

fn names(dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()
}

#[tokio::test]
async fn pair_downloads_video_and_audio_and_merges_them() {
    let Some(ffmpeg) = ffmpeg_or_skip() else { return };
    let src = tempfile::tempdir().unwrap();
    // Facebook-style DASH halves: one video-only and one audio-only MP4.
    run_ffmpeg(&ffmpeg, src.path(), "-f lavfi -i testsrc=size=160x120:rate=10:duration=3 -c:v libx264 -an v.mp4");
    run_ffmpeg(&ffmpeg, src.path(), "-f lavfi -i sine=duration=3 -c:a aac -vn a.mp4");
    let video = std::fs::read(src.path().join("v.mp4")).unwrap();
    let audio = std::fs::read(src.path().join("a.mp4")).unwrap();
    let server = serve([("/v.mp4", video.clone()), ("/a.mp4", audio.clone())], true).await;
    let dir = tempfile::tempdir().unwrap();
    let mut opts = Options::new(dir.path());
    opts.filename = Some("Live talk.mp4".into());
    let last = Arc::new(std::sync::Mutex::new(DownloadProgress::default()));
    let l = last.clone();
    let out = pair::download(&server.url("/v.mp4"), &server.url("/a.mp4"), &opts, Arc::new(move |p| *l.lock().unwrap() = p))
        .await
        .unwrap();
    assert_eq!(out.file_name().unwrap(), "Live talk.mp4");
    assert_eq!(streams(&ffmpeg, &out), (1, 1));
    assert_eq!(last.lock().unwrap().total, Some((video.len() + audio.len()) as u64));
    assert_eq!(names(dir.path()), ["Live talk.mp4"], "track files are cleaned up");
    assert!(std::fs::read_to_string(with_suffix(&out, ":Zone.Identifier")).unwrap().contains("ZoneId=3"));

    // A track an earlier run finished (no .hoardini next to it) is reused, not fetched again.
    let video_track = dir.path().join(".again.mp4.video.mp4");
    std::fs::write(&video_track, &video).unwrap();
    opts.filename = Some("again".into());
    server.requests.lock().unwrap().clear();
    let out = pair::download(&server.url("/v.mp4"), &server.url("/a.mp4"), &opts, no_progress()).await.unwrap();
    assert_eq!(streams(&ffmpeg, &out), (1, 1));
    let video_gets = server.requests.lock().unwrap().iter().filter(|p| *p == "/v.mp4").count();
    assert_eq!(video_gets, 1, "only the size probe");
}

#[tokio::test]
async fn pair_cancel_cleanup_removes_every_leftover() {
    let dir = tempfile::tempdir().unwrap();
    let server = serve([("/v.mp4", payload(3_000_000)), ("/a.mp4", payload(1_000_000))], true).await;
    let opts = Options::new(dir.path());
    opts.cancel.cancel();
    let r = pair::download(&server.url("/v.mp4"), &server.url("/a.mp4"), &opts, no_progress()).await;
    if hoardini_core::remux::ffmpeg_path().is_some() {
        assert!(matches!(r, Err(hoardini_core::Error::Cancelled)));
    }
    // What a paused pair leaves behind, then the app's cancel cleanup.
    let final_path = dir.path().join("v.mp4");
    for f in [".v.mp4.video.mp4.part", ".v.mp4.video.mp4.hoardini", ".v.mp4.audio.mp4", "v.mp4.part"] {
        std::fs::write(dir.path().join(f), b"x").unwrap();
    }
    hoardini_core::net::remove_partial(&final_path);
    assert!(names(dir.path()).is_empty(), "{:?}", names(dir.path()));
}
