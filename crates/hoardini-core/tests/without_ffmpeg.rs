//! Behaviour on a PC without ffmpeg. A separate test binary, because it sets HOARDINI_FFMPEG
//! (process-wide) to a file that doesn't exist.

mod common;

use common::{no_progress, payload, serve};
use hoardini_core::{hls, remux, Error, Options};

#[tokio::test]
async fn separate_audio_hls_stops_before_downloading() {
    std::env::set_var("HOARDINI_FFMPEG", r"C:\nonexistent\ffmpeg.exe");
    assert!(remux::ffmpeg_path().is_none());

    let files = [
        (
            "/s/master.m3u8",
            b"#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID=\"a\",NAME=\"en\",DEFAULT=YES,URI=\"audio.m3u8\"\n\
              #EXT-X-STREAM-INF:BANDWIDTH=1000,AUDIO=\"a\"\nvideo.m3u8\n"
                .to_vec(),
        ),
        ("/s/video.m3u8", b"#EXTM3U\n#EXTINF:4,\nv0.ts\n#EXT-X-ENDLIST\n".to_vec()),
        ("/s/audio.m3u8", b"#EXTM3U\n#EXTINF:4,\na0.ts\n#EXT-X-ENDLIST\n".to_vec()),
        ("/s/v0.ts", payload(1000)),
        ("/s/a0.ts", payload(1000)),
    ];
    let server = serve(files, true).await;
    let dir = tempfile::tempdir().unwrap();
    let r = hls::download(&server.url("/s/master.m3u8"), &Options::new(dir.path()), no_progress()).await;

    assert!(matches!(r, Err(Error::FfmpegMissing)), "{r:?}");
    let fetched = server.requests.lock().unwrap().clone();
    assert!(fetched.iter().all(|p| p.ends_with(".m3u8")), "no media segments fetched: {fetched:?}");
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0, "nothing written");

    // Muxed-audio streams don't need ffmpeg at all.
    let files = [
        ("/m/index.m3u8", b"#EXTM3U\n#EXTINF:4,\nv0.ts\n#EXT-X-ENDLIST\n".to_vec()),
        ("/m/v0.ts", payload(1000)),
    ];
    let server = serve(files, true).await;
    let out = hls::download(&server.url("/m/index.m3u8"), &Options::new(dir.path()), no_progress()).await.unwrap();
    assert_eq!(std::fs::read(out).unwrap(), payload(1000));
}
