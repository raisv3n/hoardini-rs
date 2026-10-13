use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    time::{Duration, Instant},
};

use hoardini_core::{
    downloader, hls, paths, remux, CancellationToken, DownloadProgress, Error, JobRequest, Options, ProgressFn,
};
use tokio::runtime::Handle;
use uuid::Uuid;

use crate::model::{DownloadItem, Settings, Status};

const MAX_CONCURRENT: usize = 3;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const FFMPEG_DOWNLOAD_PAGE: &str = "https://ffmpeg.org/download.html";

/// Whether MP4 conversion (and merging separate HLS audio) is possible right now.
#[derive(Debug, Clone, PartialEq)]
pub enum Ffmpeg {
    Found(PathBuf),
    Missing,
    /// `winget install Gyan.FFmpeg` is running.
    Installing,
    InstallFailed(String),
}

/// winget ships with Windows 10 (2004+) and 11 as an App Installer alias; it may still be absent.
pub fn winget_path() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .map(|d| d.join("winget.exe"));
    let alias = dirs::data_local_dir().map(|d| d.join("Microsoft").join("WindowsApps").join("winget.exe"));
    // App execution aliases are reparse points: `symlink_metadata` sees them where `is_file` may not.
    on_path.chain(alias).find(|p| fs::symlink_metadata(p).is_ok())
}

pub struct Manager {
    inner: Mutex<Inner>,
    rt: Handle,
    ctx: OnceLock<eframe::egui::Context>,
    downloads_dir: PathBuf,
}

struct Inner {
    items: Vec<DownloadItem>,
    settings: Settings,
    tasks: HashMap<Uuid, CancellationToken>,
    last_sample: HashMap<Uuid, (Instant, u64)>,
    ffmpeg: Ffmpeg,
    /// Show "ffmpeg installed" until dismissed.
    ffmpeg_just_installed: bool,
}

fn queue_file() -> PathBuf {
    paths::support().join("queue.json")
}

fn settings_file() -> PathBuf {
    paths::support().join("settings.json")
}

/// Tolerates a UTF-8 byte-order mark, which Windows tools (Notepad, PowerShell 5) often add.
fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let data = fs::read(path).ok()?;
    serde_json::from_slice(data.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&data)).ok()
}

fn write_json<T: serde::Serialize>(path: &Path, value: &T) {
    if let Ok(data) = serde_json::to_vec_pretty(value) {
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, data).is_ok() {
            let _ = fs::rename(&tmp, path);
        }
    }
}

impl Manager {
    pub fn new(rt: Handle) -> Arc<Self> {
        let _ = paths::ensure_dir(&paths::inbox());
        let settings = read_json(&settings_file()).unwrap_or_default();
        let mut items: Vec<DownloadItem> = read_json(&queue_file()).unwrap_or_default();
        for item in &mut items {
            item.speed = 0.0;
            item.converting = None; // a conversion cannot survive a quit
            // Anything mid-flight when the app quit is resumable; HLS cannot resume, so it restarts.
            if item.status == Status::Downloading {
                item.status = if item.request.is_hls() { Status::Queued } else { Status::Paused };
            }
        }
        let ffmpeg = remux::ffmpeg_path().map_or(Ffmpeg::Missing, Ffmpeg::Found);
        let m = Arc::new(Self {
            inner: Mutex::new(Inner {
                items,
                settings,
                tasks: HashMap::new(),
                last_sample: HashMap::new(),
                ffmpeg,
                ffmpeg_just_installed: false,
            }),
            rt,
            ctx: OnceLock::new(),
            downloads_dir: paths::downloads(),
        });
        m.pump(&mut m.lock());
        m.watch_inbox();
        m
    }

    pub fn set_context(&self, ctx: eframe::egui::Context) {
        let _ = self.ctx.set(ctx);
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    fn repaint(&self) {
        if let Some(c) = self.ctx.get() {
            c.request_repaint();
        }
    }

    // MARK: - Reading

    pub fn items(&self) -> Vec<DownloadItem> {
        self.lock().items.clone()
    }

    pub fn settings(&self) -> Settings {
        self.lock().settings.clone()
    }

    pub fn set_settings(&self, s: Settings) {
        write_json(&settings_file(), &s);
        self.lock().settings = s;
    }

    /// Finished downloads (not ones still converting).
    pub fn finished_count(&self) -> usize {
        self.lock().items.iter().filter(|i| i.status == Status::Completed && !i.is_converting()).count()
    }

    /// (active downloads, combined bytes/s)
    pub fn activity(&self) -> (usize, f64) {
        let g = self.lock();
        let active: Vec<_> = g.items.iter().filter(|i| i.status == Status::Downloading).collect();
        (active.len(), active.iter().map(|i| i.speed).sum())
    }

    // MARK: - ffmpeg

    pub fn ffmpeg(&self) -> Ffmpeg {
        self.lock().ffmpeg.clone()
    }

    pub fn ffmpeg_just_installed(&self) -> bool {
        self.lock().ffmpeg_just_installed
    }

    pub fn dismiss_ffmpeg_installed(&self) {
        self.lock().ffmpeg_just_installed = false;
    }

    /// Hides the "ffmpeg isn't installed" notice for good (Settings still offers the install).
    pub fn dismiss_ffmpeg_notice(&self) {
        let mut s = self.settings();
        s.hide_ffmpeg_notice = true;
        self.set_settings(s);
        let mut g = self.lock();
        if matches!(g.ffmpeg, Ffmpeg::InstallFailed(_)) {
            g.ffmpeg = Ffmpeg::Missing;
        }
    }

    /// Looks for ffmpeg again; when it has appeared, finishes everything that was waiting for it.
    pub fn refresh_ffmpeg(self: &Arc<Self>) {
        let found = remux::ffmpeg_path();
        let mut g = self.lock();
        if g.ffmpeg == Ffmpeg::Installing {
            return;
        }
        match found {
            Some(path) => {
                let was_missing = !matches!(g.ffmpeg, Ffmpeg::Found(_));
                g.ffmpeg = Ffmpeg::Found(path);
                if was_missing {
                    let convert = self.release_waiting(&mut g);
                    drop(g);
                    convert.into_iter().for_each(|id| self.convert(id));
                    self.repaint();
                }
            }
            None if matches!(g.ffmpeg, Ffmpeg::Found(_)) => {
                g.ffmpeg = Ffmpeg::Missing;
                drop(g);
                self.repaint();
            }
            None => {}
        }
    }

    fn mark_ffmpeg_missing(g: &mut Inner) {
        if matches!(g.ffmpeg, Ffmpeg::Found(_)) {
            g.ffmpeg = Ffmpeg::Missing;
        }
    }

    /// Re-queues downloads and returns the conversions that were waiting for ffmpeg.
    fn release_waiting(self: &Arc<Self>, g: &mut Inner) -> Vec<Uuid> {
        let mut convert = Vec::new();
        for item in g.items.iter_mut().filter(|i| i.needs_ffmpeg) {
            item.needs_ffmpeg = false;
            match item.status {
                Status::Completed => convert.push(item.id),
                Status::Failed => {
                    item.status = Status::Queued;
                    item.error = None;
                }
                _ => {}
            }
        }
        Self::save(g);
        self.pump(g);
        convert
    }

    /// Installs ffmpeg with winget in the background, or opens the download page when there is no winget.
    pub fn install_ffmpeg(self: &Arc<Self>) {
        let Some(winget) = winget_path() else {
            crate::win::open_url(FFMPEG_DOWNLOAD_PAGE);
            return;
        };
        {
            let mut g = self.lock();
            if g.ffmpeg == Ffmpeg::Installing {
                return;
            }
            g.ffmpeg = Ffmpeg::Installing;
        }
        self.repaint();
        let m = self.clone();
        self.rt.spawn(async move {
            let out = tokio::process::Command::new(winget)
                .args(["install", "--id", "Gyan.FFmpeg", "--exact", "--silent", "--disable-interactivity"])
                .args(["--accept-package-agreements", "--accept-source-agreements"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .await;
            // Success is judged by finding ffmpeg afterwards ("already installed" is fine too).
            let found = remux::ffmpeg_path();
            let mut g = m.lock();
            match found {
                Some(path) => {
                    g.ffmpeg = Ffmpeg::Found(path);
                    g.ffmpeg_just_installed = true;
                    let convert = m.release_waiting(&mut g);
                    drop(g);
                    convert.into_iter().for_each(|id| m.convert(id));
                }
                None => {
                    let reason = match out {
                        Err(e) => format!("winget could not start: {e}"),
                        Ok(o) => {
                            let text = String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr);
                            let last = text.lines().map(str::trim).rfind(|l| l.len() > 3 && !l.chars().all(|c| "-\\|/ █▒".contains(c)));
                            match (o.status.success(), last) {
                                (true, _) => "winget finished, but ffmpeg still wasn't found".to_owned(),
                                (false, Some(l)) => l.to_owned(),
                                (false, None) => format!("winget exited with code {}", o.status.code().unwrap_or(-1)),
                            }
                        }
                    };
                    g.ffmpeg = Ffmpeg::InstallFailed(reason);
                    drop(g);
                }
            }
            m.repaint();
        });
    }

    // MARK: - Adding

    pub fn add(self: &Arc<Self>, request: JobRequest) {
        if request.valid_url().is_none() {
            return;
        }
        let mut g = self.lock();
        g.items.insert(0, DownloadItem::new(request));
        Self::save(&g);
        self.pump(&mut g);
        drop(g);
        self.repaint();
    }

    /// Jobs dropped into the inbox directory by the browser host.
    fn watch_inbox(self: &Arc<Self>) {
        let m = Arc::downgrade(self);
        self.rt.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(500));
            for n in 0u64.. {
                tick.tick().await;
                let Some(m) = m.upgrade() else { return };
                // Every 10 s, notice an ffmpeg installed outside the app (Hoardini's own install reports itself).
                if n % 20 == 0 && matches!(m.ffmpeg(), Ffmpeg::Missing | Ffmpeg::InstallFailed(_)) {
                    m.refresh_ffmpeg();
                }
                let Ok(entries) = fs::read_dir(paths::inbox()) else { continue };
                for path in entries.flatten().map(|e| e.path()) {
                    if path.extension().is_some_and(|e| e == "json") {
                        let job = read_json::<JobRequest>(&path);
                        let _ = fs::remove_file(&path);
                        if let Some(job) = job {
                            m.add(job);
                        }
                    }
                }
            }
        });
    }

    // MARK: - Controls

    pub fn pause(self: &Arc<Self>, id: Uuid) {
        let mut g = self.lock();
        let Some(i) = index(&g, id) else { return };
        if !g.items[i].can_pause() {
            return;
        }
        g.items[i].status = Status::Paused;
        g.items[i].speed = 0.0;
        if let Some(t) = g.tasks.remove(&id) {
            t.cancel();
        }
        Self::save(&g);
        self.pump(&mut g);
    }

    pub fn resume(self: &Arc<Self>, id: Uuid) {
        let mut g = self.lock();
        let Some(i) = index(&g, id) else { return };
        if !matches!(g.items[i].status, Status::Paused | Status::Failed) {
            return;
        }
        g.items[i].status = Status::Queued;
        g.items[i].error = None;
        g.items[i].needs_ffmpeg = false;
        Self::save(&g);
        self.pump(&mut g);
    }

    /// Stops the download and deletes any partial data.
    pub fn cancel_and_remove(self: &Arc<Self>, id: Uuid) {
        let mut g = self.lock();
        if let Some(t) = g.tasks.remove(&id) {
            t.cancel();
        }
        if let Some(i) = index(&g, id) {
            let item = g.items.remove(i);
            if item.status != Status::Completed {
                if let Some(d) = &item.destination {
                    // The task may still be closing its file handles; retry briefly off the UI thread.
                    let leftovers = [".part", ".hoardini"].map(|s| hoardini_core::net::with_suffix(d, s));
                    self.rt.spawn(async move {
                        for _ in 0..10 {
                            leftovers.iter().for_each(|p| drop(fs::remove_file(p)));
                            if leftovers.iter().all(|p| !p.exists()) {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                    });
                }
            }
        }
        Self::save(&g);
        self.pump(&mut g);
    }

    /// Removes list entries only; files stay on disk.
    pub fn clear_finished(&self) {
        let mut g = self.lock();
        g.items.retain(|i| !(i.status == Status::Completed && !i.is_converting()));
        Self::save(&g);
        drop(g);
        self.repaint();
    }

    /// Removes one finished entry from the list without touching its files.
    pub fn remove_from_list(&self, id: Uuid) {
        let mut g = self.lock();
        if let Some(i) = index(&g, id) {
            if g.items[i].status == Status::Completed && !g.items[i].is_converting() {
                g.items.remove(i);
                Self::save(&g);
            }
        }
    }

    pub fn playable(&self, id: Uuid) -> Option<PathBuf> {
        let g = self.lock();
        index(&g, id).and_then(|i| g.items[i].playable().map(Path::to_path_buf)).filter(|p| p.exists())
    }

    // MARK: - MP4 conversion

    /// Adds existing files (e.g. earlier downloads) to the list and converts them.
    pub fn convert_files(self: &Arc<Self>, files: Vec<PathBuf>) {
        let mut ids = Vec::new();
        {
            let mut g = self.lock();
            for path in files {
                let mut item = DownloadItem::new(JobRequest::new(path.display().to_string()));
                item.status = Status::Completed;
                item.fraction = Some(1.0);
                item.total = fs::metadata(&path).ok().map(|m| m.len());
                item.destination = Some(path);
                ids.push(item.id);
                g.items.insert(0, item);
            }
            Self::save(&g);
        }
        for id in ids {
            self.convert(id);
        }
    }

    pub fn convert(self: &Arc<Self>, id: Uuid) {
        let mut g = self.lock();
        let Some(i) = index(&g, id) else { return };
        if !g.items[i].can_convert() {
            return;
        }
        let Some(src) = g.items[i].destination.clone() else { return };
        g.items[i].converting = Some(true);
        g.items[i].convert_error = None;
        let delete_original = g.settings.delete_original;
        drop(g);
        let m = self.clone();
        self.rt.spawn(async move {
            let result = remux::to_mp4(&src, &CancellationToken::new()).await;
            let mut g = m.lock();
            if let Some(i) = index(&g, id) {
                g.items[i].converting = Some(false);
                match result {
                    Ok(mp4) => {
                        if delete_original && fs::remove_file(&src).is_ok() {
                            g.items[i].destination = Some(mp4.clone());
                        }
                        g.items[i].mp4 = Some(mp4);
                    }
                    // Not a failure: it converts by itself once ffmpeg is installed.
                    Err(Error::FfmpegMissing) => {
                        g.items[i].needs_ffmpeg = true;
                        Self::mark_ffmpeg_missing(&mut g);
                    }
                    Err(e) => g.items[i].convert_error = Some(e.to_string()),
                }
                Self::save(&g);
            }
            drop(g);
            m.repaint();
        });
    }

    /// Called on quit: running downloads are paused so they resume on next launch.
    pub fn shutdown(&self) {
        let mut g = self.lock();
        let had_tasks = !g.tasks.is_empty();
        for (_, t) in g.tasks.drain() {
            t.cancel();
        }
        for item in g.items.iter_mut().filter(|i| i.status == Status::Downloading) {
            item.status = Status::Paused;
        }
        Self::save(&g);
        drop(g);
        if had_tasks {
            // Let segment tasks flush their resume state.
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    // MARK: - Scheduling

    fn pump(self: &Arc<Self>, g: &mut Inner) {
        let running = g.items.iter().filter(|i| i.status == Status::Downloading).count();
        // Oldest first: items are stored newest-first.
        let queued: Vec<Uuid> = g.items.iter().rev().filter(|i| i.status == Status::Queued).map(|i| i.id).collect();
        for id in queued.into_iter().take(MAX_CONCURRENT.saturating_sub(running)) {
            self.start(g, id);
        }
    }

    fn start(self: &Arc<Self>, g: &mut Inner, id: Uuid) {
        let Some(i) = index(g, id) else { return };
        let item = &mut g.items[i];
        item.status = Status::Downloading;
        item.speed = 0.0;
        let (done, req) = (item.done, item.request.clone());
        // On resume keep the name the downloader chose the first time so its state file is found.
        let name = item
            .destination
            .as_deref()
            .and_then(Path::file_name)
            .map(|n| n.to_string_lossy().into_owned())
            .or_else(|| req.clean_filename());
        g.last_sample.insert(id, (Instant::now(), done));
        let Some(url) = req.valid_url() else {
            self.fail_locked(g, id, "Invalid URL".into());
            return;
        };

        let token = CancellationToken::new();
        g.tasks.insert(id, token.clone());
        let mut opts = Options::new(&self.downloads_dir);
        opts.headers = req.headers();
        opts.filename = name;
        opts.variant = req.valid_variant();
        opts.cancel = token.clone();
        let m = Arc::downgrade(self);
        opts.on_destination = Some(Arc::new(move |p: &Path| {
            if let Some(m) = m.upgrade() {
                let mut g = m.lock();
                if let Some(i) = index(&g, id) {
                    g.items[i].destination = Some(p.to_path_buf());
                }
            }
        }));
        let m = Arc::downgrade(self);
        let progress: ProgressFn = Arc::new(move |p| {
            if let Some(m) = m.upgrade() {
                m.progress(id, p);
            }
        });
        let m = self.clone();
        self.rt.spawn(async move {
            let result = if req.is_hls() {
                hls::download(&url, &opts, progress).await
            } else {
                downloader::download(&url, &opts, progress).await
            };
            match result {
                Ok(path) => m.finish(id, path),
                Err(_) if token.is_cancelled() => {} // pause / cancel already updated state
                // An HLS video with separate audio: stopped before downloading; resumes once ffmpeg is installed.
                Err(Error::FfmpegMissing) => {
                    let mut g = m.lock();
                    Self::mark_ffmpeg_missing(&mut g);
                    if let Some(i) = index(&g, id) {
                        g.items[i].needs_ffmpeg = true;
                    }
                    m.fail_locked(&mut g, id, "Needs ffmpeg to merge this video's separate audio track".into());
                }
                Err(e) => {
                    let mut g = m.lock();
                    m.fail_locked(&mut g, id, e.to_string());
                }
            }
            m.repaint();
        });
    }

    fn progress(&self, id: Uuid, p: DownloadProgress) {
        let mut g = self.lock();
        let Some(i) = index(&g, id) else { return };
        if g.items[i].status != Status::Downloading {
            return;
        }
        let now = Instant::now();
        let sample = g.last_sample.get(&id).copied();
        let item = &mut g.items[i];
        item.done = p.done;
        item.total = p.total;
        item.fraction = p.fraction();
        if let Some((t, bytes)) = sample {
            let dt = now.duration_since(t).as_secs_f64();
            if dt >= 0.5 {
                let inst = p.done.saturating_sub(bytes) as f64 / dt;
                item.speed = if item.speed == 0.0 { inst } else { item.speed * 0.7 + inst * 0.3 };
                g.last_sample.insert(id, (now, p.done));
            }
        }
    }

    fn finish(self: &Arc<Self>, id: Uuid, path: PathBuf) {
        let mut g = self.lock();
        g.tasks.remove(&id);
        let Some(i) = index(&g, id) else { return };
        let item = &mut g.items[i];
        item.status = Status::Completed;
        item.destination = Some(path);
        item.fraction = Some(1.0);
        item.speed = 0.0;
        item.request.cookies = None; // session cookies are not kept once they are no longer needed
        let mut auto = g.settings.auto_convert && g.items[i].should_auto_convert();
        if auto && !matches!(g.ffmpeg, Ffmpeg::Found(_)) {
            // Not an error: the notice offers ffmpeg, and this converts once it's installed.
            g.items[i].needs_ffmpeg = true;
            auto = false;
        }
        Self::save(&g);
        self.pump(&mut g);
        drop(g);
        if auto {
            self.convert(id);
        }
    }

    fn fail_locked(self: &Arc<Self>, g: &mut Inner, id: Uuid, message: String) {
        g.tasks.remove(&id);
        if let Some(i) = index(g, id) {
            g.items[i].status = Status::Failed;
            g.items[i].error = Some(message);
            g.items[i].speed = 0.0;
        }
        Self::save(g);
        self.pump(g);
    }

    fn save(g: &Inner) {
        write_json(&queue_file(), &g.items);
    }
}

fn index(g: &Inner, id: Uuid) -> Option<usize> {
    g.items.iter().position(|i| i.id == id)
}
