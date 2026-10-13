//! Chrome/Edge native-messaging host. Protocol: 4-byte native-endian length + UTF-8 JSON, on stdin/stdout.
//! The host is short-lived (the browser kills it when the port closes), so it only validates the
//! request and passes it on:
//!   1. hoardini-app.exe installed next to it -> write the job to its inbox (launching the app if needed).
//!   2. otherwise                             -> start a detached `hoardini.exe` process.
//!
//! Request:  {"url", "referrer", "cookies": "a=b; c=d", "userAgent", "kind": "file"|"hls", "filename", "variant"}
//! Response: {"ok": true} or {"ok": false, "error": "..."}

use std::{
    fs,
    io::{self, Read, Write},
    os::windows::process::CommandExt,
    process::{Command, Stdio},
};

use hoardini_core::{instance, paths, JobRequest};
use serde_json::{json, Value};

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

fn read_message(stdin: &mut impl Read) -> Option<Vec<u8>> {
    let mut len = [0u8; 4];
    stdin.read_exact(&mut len).ok()?;
    let len = u32::from_ne_bytes(len) as usize;
    if len == 0 || len >= 1_048_576 {
        return None;
    }
    let mut body = vec![0u8; len];
    stdin.read_exact(&mut body).ok()?;
    Some(body)
}

fn send(stdout: &mut impl Write, v: &Value) -> io::Result<()> {
    let data = serde_json::to_vec(v)?;
    stdout.write_all(&(data.len() as u32).to_ne_bytes())?;
    stdout.write_all(&data)?;
    stdout.flush()
}

/// Chrome runs hosts inside a job object that is closed with the port; children must break away to outlive it.
fn spawn_detached(cmd: &mut Command) -> io::Result<std::process::Child> {
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB)
        .spawn()
        .or_else(|_| cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP).spawn())
}

fn deliver_to_app(job: &JobRequest, app: &std::path::Path) -> Result<(), String> {
    let inbox = paths::inbox();
    let queue = || -> io::Result<()> {
        paths::ensure_dir(&inbox)?;
        let name = format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
        let tmp = inbox.join(format!("{name}.tmp"));
        fs::write(&tmp, serde_json::to_vec(job)?)?;
        fs::rename(&tmp, inbox.join(format!("{name}.json")))
    };
    queue().map_err(|e| format!("could not queue job: {e}"))?;

    if !instance::app_running() {
        // --background: open without stealing focus from the browser.
        spawn_detached(Command::new(app).arg("--background").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()))
            .map_err(|e| format!("could not launch Hoardini: {e}"))?;
    }
    Ok(())
}

fn run_cli(job: &JobRequest, url: &str) -> Result<(), String> {
    let exe = paths::sibling_exe(paths::CLI_EXE).filter(|p| p.is_file()).ok_or("hoardini.exe not found next to host")?;
    // Headers (cookies included) go through a pipe: command-line arguments are visible to other processes.
    let mut cmd = Command::new(exe);
    cmd.arg(url).arg("-o").arg(paths::downloads()).arg("--headers-stdin");
    if job.is_hls() {
        cmd.arg("--hls");
        if let Some(v) = job.valid_variant() {
            cmd.arg("--variant").arg(v.as_str());
        }
    }
    if let Some(n) = job.clean_filename() {
        cmd.arg("--name").arg(n);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = spawn_detached(&mut cmd).map_err(|e| e.to_string())?;
    let header_text: String = job.headers().iter().map(|(k, v)| format!("{k}: {v}\n")).collect();
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(header_text.as_bytes());
    }
    Ok(())
}

fn handle(data: &[u8]) -> Value {
    let Some((job, url)) = serde_json::from_slice::<JobRequest>(data).ok().and_then(|j| j.valid_url().map(|u| (j, u))) else {
        return json!({"ok": false, "error": "invalid url"});
    };
    let result = match paths::sibling_exe(paths::APP_EXE).filter(|p| p.is_file()) {
        Some(app) => deliver_to_app(&job, &app),
        None => run_cli(&job, url.as_str()),
    };
    match result {
        Ok(()) => json!({"ok": true}),
        Err(e) => json!({"ok": false, "error": e}),
    }
}

/// The browser's pipes are this process's std handles. Children (the app can run for hours) must not
/// inherit them, or the browser's end of the port stays open after the host exits.
fn stop_std_handle_inheritance() {
    use windows_sys::Win32::{
        Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE},
        System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE},
    };
    for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let h = unsafe { GetStdHandle(id) };
        if !h.is_null() && h != INVALID_HANDLE_VALUE {
            unsafe { SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0) };
        }
    }
}

fn main() {
    stop_std_handle_inheritance();
    let mut stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    while let Some(msg) = read_message(&mut stdin) {
        if send(&mut stdout, &handle(&msg)).is_err() {
            break;
        }
    }
}
