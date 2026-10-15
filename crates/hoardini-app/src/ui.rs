//! Windows 11 interface: navigation rail (Downloads / Settings) on Mica, content on a rounded layer,
//! downloads as cards, a ContentDialog for adding URLs.

use std::{sync::Arc, time::Duration};

use eframe::egui::{
    self, epaint::CornerRadius, include_image, pos2, vec2, Align, Align2, Color32, Frame, Id, Key, KeyboardShortcut,
    Layout, Margin, Modal, Modifiers, Rect, ScrollArea, Sense, Stroke, Ui, ViewportCommand,
};
use hoardini_core::{paths, JobRequest};

use crate::{
    manager::{winget_path, Ffmpeg, Manager, FFMPEG_DOWNLOAD_PAGE},
    model::{self, DownloadItem, Status, ThemeMode},
    theme::{self, icon, Palette},
    tray, widgets,
    widgets::{Kind, ProgressState, Severity},
    win,
};

/// Decimal units, like Explorer's "size" column.
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if n < 1000 {
        return format!("{n} bytes");
    }
    let mut v = n as f64 / 1000.0;
    let mut u = 0;
    while v >= 1000.0 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    match u {
        0 => format!("{v:.0} {}", UNITS[u]),
        1 => format!("{v:.1} {}", UNITS[u]),
        _ => format!("{v:.2} {}", UNITS[u]),
    }
}

fn duration(s: f64) -> String {
    let t = s as u64;
    if t >= 3600 {
        format!("{}:{:02}:{:02}", t / 3600, t % 3600 / 60, t % 60)
    } else {
        format!("{}:{:02}", t / 60, t % 60)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Downloads,
    Settings,
}

pub struct HoardiniApp {
    manager: Arc<Manager>,
    hwnd: Option<isize>,
    page: Page,
    adding: Option<String>,
    focus_add: bool,
    dark_applied: Option<bool>,
    /// Started by the browser host: cloaked until eframe's first-frame show has happened, then
    /// re-shown without taking focus from the browser.
    background_start: bool,
    /// The window that had focus when we were launched (the browser).
    previous_foreground: isize,
}

impl HoardiniApp {
    pub fn new(cc: &eframe::CreationContext<'_>, manager: Arc<Manager>, background: bool, previous_foreground: isize) -> Self {
        manager.set_context(cc.egui_ctx.clone());
        let hwnd = win::hwnd(cc);
        theme::install_fonts(&cc.egui_ctx);
        theme::set_mica(hwnd.is_some_and(win::set_mica));
        theme::apply(&cc.egui_ctx, manager.settings().theme);
        tray::install(manager.clone(), hwnd, cc.egui_ctx.clone());
        let background_start = background && hwnd.is_some();
        if let (true, Some(h)) = (background_start, hwnd) {
            win::cloak(h, true);
        }
        Self {
            manager,
            hwnd,
            page: Page::Downloads,
            adding: None,
            focus_add: false,
            dark_applied: None,
            background_start,
            previous_foreground,
        }
    }

    fn open_add_dialog(&mut self) {
        self.adding = Some(String::new());
        self.focus_add = true;
    }

    fn pick_files_to_convert(&self) {
        let picker = rfd::FileDialog::new()
            .set_title("Choose MPEG-TS videos to convert to MP4")
            .add_filter("MPEG-TS video", model::CONVERTIBLE);
        if let Some(files) = picker.pick_files() {
            self.manager.convert_files(files);
        }
    }

    // ---------- navigation rail ----------

    fn nav(&mut self, ui: &mut Ui, p: &Palette, active: usize) {
        let item = |ui: &mut Ui, this: &mut Self, page: Page, glyph: char, label: &str, badge: usize| {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
            let selected = this.page == page;
            let fill = if resp.is_pointer_button_down_on() {
                p.nav_pressed
            } else if selected || resp.hovered() {
                p.nav_hover
            } else {
                Color32::TRANSPARENT
            };
            let painter = ui.painter();
            painter.rect_filled(rect, theme::RADIUS, fill);
            if selected {
                let pill = Rect::from_center_size(pos2(rect.left() + 1.5, rect.center().y), vec2(3.0, 16.0));
                painter.rect_filled(pill, 1.5, p.accent);
            }
            painter.text(rect.center(), Align2::CENTER_CENTER, glyph, theme::icons(16.0), p.text);
            if badge > 0 {
                // InfoBadge
                let c = pos2(rect.center().x + 10.0, rect.top() + 9.0);
                let label = badge.to_string();
                let w = (8.0 + 6.0 * label.len() as f32).max(16.0);
                painter.rect_filled(Rect::from_center_size(c, vec2(w, 16.0)), 8.0, p.accent);
                painter.text(c, Align2::CENTER_CENTER, label, egui::FontId::new(11.0, theme::body_strong().family), p.on_accent);
            }
            if resp.on_hover_text(label).clicked() {
                this.page = page;
            }
        };
        ui.add_space(4.0);
        item(ui, self, Page::Downloads, icon::DOWNLOAD, "Downloads", active);
        ui.with_layout(Layout::bottom_up(Align::Center), |ui| {
            item(ui, self, Page::Settings, icon::SETTINGS, "Settings", 0);
        });
    }

    // ---------- Downloads page ----------

    fn downloads_page(&mut self, ui: &mut Ui, p: &Palette, items: &[DownloadItem]) {
        let (active, speed) = self.manager.activity();
        let finished = self.manager.finished_count();

        widgets::text(ui, "Downloads", theme::title(), p.text, false);
        let summary = match (active, items.iter().filter(|i| i.status == Status::Queued).count()) {
            (0, 0) if items.is_empty() => "Nothing here yet".to_owned(),
            (0, 0) => "No active downloads".to_owned(),
            (0, q) => format!("{q} waiting"),
            (a, 0) => format!("{a} downloading · {}/s", bytes(speed as u64)),
            (a, q) => format!("{a} downloading · {}/s · {q} waiting", bytes(speed as u64)),
        };
        widgets::text(ui, summary, theme::body(), p.text2, false);
        ui.add_space(16.0);
        self.ffmpeg_bar(ui);

        ui.horizontal(|ui| {
            if widgets::button(ui, Kind::Accent, Some(icon::ADD), "Add URL", true).on_hover_text("Ctrl+N").clicked() {
                self.open_add_dialog();
            }
            if widgets::button(ui, Kind::Standard, Some(icon::CONVERT), "Convert files…", true)
                .on_hover_text("Convert existing .ts files to MP4 · Ctrl+O")
                .clicked()
            {
                self.pick_files_to_convert();
            }
            let label = if finished > 0 { format!("Clear finished ({finished})") } else { "Clear finished".to_owned() };
            if widgets::button(ui, Kind::Subtle, Some(icon::BROOM), &label, finished > 0)
                .on_hover_text("Remove finished downloads from this list (files stay in your Downloads folder) · Ctrl+Shift+Del")
                .clicked()
            {
                self.manager.clear_finished();
            }
        });
        ui.add_space(16.0);

        if items.is_empty() {
            self.empty_state(ui, p);
            return;
        }
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for item in items {
                self.row(ui, p, item);
                ui.add_space(4.0);
            }
            ui.add_space(20.0);
        });
    }

    fn empty_state(&mut self, ui: &mut Ui, p: &Palette) {
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() / 2.0 - 130.0).max(16.0));
            ui.add(egui::Image::new(include_image!("../../../assets/icon-256.png")).fit_to_exact_size(vec2(72.0, 72.0)));
            ui.add_space(12.0);
            widgets::text(ui, "No downloads yet", theme::subtitle(), p.text, false);
            widgets::text(ui, "Downloads sent from the browser extension appear here.", theme::body(), p.text2, false);
            ui.add_space(16.0);
            if widgets::button(ui, Kind::Accent, Some(icon::ADD), "Add URL", true).clicked() {
                self.open_add_dialog();
            }
        });
    }

    /// InfoBar about ffmpeg: offer to install it, show the install's progress, then its outcome.
    fn ffmpeg_bar(&mut self, ui: &mut Ui) {
        let m = self.manager.clone();
        let install_label = if winget_path().is_some() { "Install" } else { "Get ffmpeg" };
        let shown = match m.ffmpeg() {
            Ffmpeg::Found(_) if m.ffmpeg_just_installed() => {
                if widgets::info_bar(ui, Severity::Success, "ffmpeg installed.", "MP4 conversion is ready.", true, |_| {}) {
                    m.dismiss_ffmpeg_installed();
                }
                true
            }
            Ffmpeg::Found(_) => false,
            Ffmpeg::Missing if m.settings().hide_ffmpeg_notice && !self.any_waiting_for_ffmpeg() => false,
            Ffmpeg::Missing => {
                let closed = widgets::info_bar(
                    ui,
                    Severity::Warning,
                    "ffmpeg isn't installed.",
                    "MP4 conversion and some HLS videos need it.",
                    true,
                    |ui| {
                        if widgets::button(ui, Kind::Accent, None, install_label, true).clicked() {
                            m.install_ffmpeg();
                        }
                    },
                );
                if closed {
                    m.dismiss_ffmpeg_notice();
                }
                true
            }
            Ffmpeg::Installing => {
                widgets::info_bar(ui, Severity::Informational, "Installing ffmpeg…", "Waiting conversions start by themselves when it's done.", false, |_| {});
                ui.add_space(2.0);
                widgets::progress(ui, None, ProgressState::Running);
                true
            }
            Ffmpeg::InstallFailed(reason) => {
                let closed = widgets::info_bar(ui, Severity::Error, "Couldn't install ffmpeg.", &reason, true, |ui| {
                    if widgets::button(ui, Kind::Standard, None, "Download page", true).clicked() {
                        win::open_url(FFMPEG_DOWNLOAD_PAGE);
                    }
                    if widgets::button(ui, Kind::Accent, None, "Try again", true).clicked() {
                        m.install_ffmpeg();
                    }
                });
                if closed {
                    m.dismiss_ffmpeg_notice();
                }
                true
            }
        };
        if shown {
            ui.add_space(12.0);
        }
    }

    fn any_waiting_for_ffmpeg(&self) -> bool {
        self.manager.items().iter().any(|i| i.needs_ffmpeg)
    }

    fn row(&self, ui: &mut Ui, p: &Palette, item: &DownloadItem) {
        let (glyph, color) = match item.status {
            _ if item.is_converting() => (icon::CONVERT, p.accent),
            Status::Failed if item.needs_ffmpeg => (icon::WARNING, p.caution),
            Status::Queued => (icon::CLOCK, p.text3),
            Status::Downloading => (icon::DOWNLOAD, p.accent),
            Status::Paused => (icon::PAUSE, p.caution),
            Status::Completed => (icon::COMPLETED, p.success),
            Status::Failed => (icon::ERROR, p.critical),
        };
        let (resp, _) = widgets::card(ui, Sense::click(), Margin::symmetric(16, 12), |ui| {
            ui.horizontal(|ui| {
                ui.add_sized([24.0, 40.0], egui::Label::new(egui::RichText::new(glyph.to_string()).font(theme::icons(20.0)).color(color)));
                ui.add_space(8.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.actions(ui, item);
                    ui.add_space(12.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 3.0;
                        widgets::text(ui, item.display_name(), theme::body_strong(), p.text, true);
                        if item.is_converting() {
                            widgets::progress(ui, None, ProgressState::Running);
                        } else if matches!(item.status, Status::Downloading | Status::Paused) {
                            let state = if item.status == Status::Paused { ProgressState::Paused } else { ProgressState::Running };
                            widgets::progress(ui, item.fraction.map(|f| f as f32), state);
                        }
                        let failed = (item.status == Status::Failed && !item.needs_ffmpeg)
                            || (item.status == Status::Completed && item.convert_error.is_some() && !item.is_converting());
                        widgets::text(ui, detail(item), theme::caption(), if failed { p.critical } else { p.text2 }, true);
                    });
                });
            });
        });
        if resp.double_clicked() && item.status == Status::Completed {
            if let Some(path) = self.manager.playable(item.id) {
                win::open(&path);
            }
        }
        resp.context_menu(|ui| self.row_menu(ui, item));
    }

    /// Row buttons, laid out right-to-left (the rightmost comes first).
    fn actions(&self, ui: &mut Ui, item: &DownloadItem) {
        let m = &self.manager;
        let remove_help = if item.status == Status::Completed { "Remove from list" } else { "Cancel and delete partial file" };
        if widgets::icon_button(ui, icon::CANCEL, remove_help).clicked() {
            m.cancel_and_remove(item.id);
        }
        if item.needs_ffmpeg {
            match m.ffmpeg() {
                Ffmpeg::Installing => {}
                _ => {
                    let label = if winget_path().is_some() { "Install ffmpeg" } else { "Get ffmpeg" };
                    if widgets::button(ui, Kind::Standard, None, label, true).clicked() {
                        m.install_ffmpeg();
                    }
                }
            }
            if item.status == Status::Failed {
                return; // retrying can't succeed until ffmpeg is there; it resumes by itself then
            }
        }
        match item.status {
            Status::Downloading if item.can_pause() => {
                if widgets::icon_button(ui, icon::PAUSE, "Pause").clicked() {
                    m.pause(item.id);
                }
            }
            Status::Paused => {
                if widgets::icon_button(ui, icon::PLAY, "Resume").clicked() {
                    m.resume(item.id);
                }
            }
            Status::Failed => {
                if widgets::icon_button(ui, icon::RETRY, "Retry").clicked() {
                    m.resume(item.id);
                }
            }
            Status::Completed => {
                if widgets::icon_button(ui, icon::FOLDER, "Show in Explorer").clicked() {
                    if let Some(p) = m.playable(item.id) {
                        win::reveal(&p);
                    }
                }
                let (glyph, help) = open_button(item);
                if widgets::icon_button(ui, glyph, help).clicked() {
                    if let Some(p) = m.playable(item.id) {
                        win::open(&p);
                    }
                }
                if item.can_convert() && !item.needs_ffmpeg && widgets::icon_button(ui, icon::CONVERT, "Convert to MP4").clicked() {
                    m.convert(item.id);
                }
            }
            _ => {}
        }
    }

    fn row_menu(&self, ui: &mut Ui, item: &DownloadItem) {
        #[derive(Clone, Copy)]
        enum Action {
            Open,
            Reveal,
            Convert,
            Pause,
            Resume,
            CopyAddress,
            Remove,
            Cancel,
            Separator,
        }
        let mut entries = Vec::new();
        match item.status {
            Status::Completed => {
                let (glyph, label) = open_button(item);
                entries.push((glyph, label, Action::Open));
                entries.push((icon::FOLDER, "Show in Explorer", Action::Reveal));
                if item.can_convert() && !item.needs_ffmpeg {
                    entries.push((icon::CONVERT, "Convert to MP4", Action::Convert));
                }
            }
            Status::Downloading if item.can_pause() => entries.push((icon::PAUSE, "Pause", Action::Pause)),
            Status::Paused => entries.push((icon::PLAY, "Resume", Action::Resume)),
            Status::Failed => entries.push((icon::RETRY, "Retry", Action::Resume)),
            _ => {}
        }
        entries.push((icon::LINK, "Copy address", Action::CopyAddress));
        entries.push((' ', "", Action::Separator));
        if item.status != Status::Completed {
            entries.push((icon::CANCEL, "Cancel and delete partial file", Action::Cancel));
        } else if !item.is_converting() {
            entries.push((icon::DELETE, "Remove from list", Action::Remove));
        }

        let m = &self.manager;
        for (glyph, label, action) in entries {
            if let Action::Separator = action {
                ui.separator();
                continue;
            }
            if !widgets::menu_item(ui, glyph, label).clicked() {
                continue;
            }
            match action {
                Action::Open => m.playable(item.id).iter().for_each(|p| win::open(p)),
                Action::Reveal => m.playable(item.id).iter().for_each(|p| win::reveal(p)),
                Action::Convert => m.convert(item.id),
                Action::Pause => m.pause(item.id),
                Action::Resume => m.resume(item.id),
                Action::CopyAddress => ui.ctx().copy_text(item.request.url.clone()),
                Action::Remove => m.remove_from_list(item.id),
                Action::Cancel => m.cancel_and_remove(item.id),
                Action::Separator => {}
            }
            ui.close_menu();
        }
    }

    // ---------- Settings page ----------

    fn settings_page(&mut self, ui: &mut Ui, p: &Palette) {
        widgets::text(ui, "Settings", theme::title(), p.text, false);
        ui.add_space(20.0);
        let ffmpeg = self.manager.ffmpeg();
        let mut recheck = false;
        let mut install = false;
        ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let mut s = self.manager.settings();
            let mut changed = false;

            section(ui, p, "Appearance");
            setting(ui, p, icon::COLOR, "App theme", "Choose whether Hoardini is light, dark, or follows Windows", |ui| {
                let labels = ThemeMode::ALL.map(ThemeMode::label);
                let current = ThemeMode::ALL.iter().position(|&m| m == s.theme).unwrap_or(0);
                if let Some(i) = widgets::combo(ui, "theme", &labels, current) {
                    s.theme = ThemeMode::ALL[i];
                    theme::set_mode(ui.ctx(), s.theme);
                    changed = true;
                }
            });
            ui.add_space(20.0);

            section(ui, p, "Conversion");
            setting(ui, p, icon::CONVERT, "Convert to MP4 automatically", "Remux finished MPEG-TS downloads (.ts, .mts, .m2ts) to MP4 without re-encoding", |ui| {
                changed |= widgets::toggle(ui, &mut s.auto_convert).changed();
            });
            setting(ui, p, icon::DELETE, "Delete the .ts after converting", "Keep only the MP4 once the conversion has finished", |ui| {
                changed |= widgets::toggle(ui, &mut s.delete_original).changed();
            });
            if changed {
                self.manager.set_settings(s);
            }
            let (glyph, desc) = match &ffmpeg {
                Ffmpeg::Found(path) => (icon::CHECK, format!("Found at {}", path.display())),
                Ffmpeg::Missing => (icon::WARNING, "Not installed. MP4 conversion and HLS videos with separate audio need it.".to_owned()),
                Ffmpeg::Installing => (icon::DOWNLOAD, "Installing with winget…".to_owned()),
                Ffmpeg::InstallFailed(reason) => (icon::WARNING, format!("Install failed: {reason}")),
            };
            setting(ui, p, glyph, "ffmpeg", &desc, |ui| match &ffmpeg {
                Ffmpeg::Found(_) => recheck = widgets::button(ui, Kind::Standard, None, "Check again", true).clicked(),
                Ffmpeg::Installing => {
                    widgets::button(ui, Kind::Standard, None, "Installing…", false);
                }
                Ffmpeg::Missing | Ffmpeg::InstallFailed(_) => {
                    recheck = widgets::button(ui, Kind::Standard, None, "Check again", true).clicked();
                    let label = if winget_path().is_some() { "Install" } else { "Get ffmpeg" };
                    install = widgets::button(ui, Kind::Accent, None, label, true).clicked();
                }
            });

            ui.add_space(20.0);
            section(ui, p, "Downloads");
            let dir = paths::downloads();
            setting(ui, p, icon::FOLDER, "Download location", &dir.display().to_string(), |ui| {
                if widgets::button(ui, Kind::Standard, None, "Open folder", true).clicked() {
                    win::open(&dir);
                }
            });
            setting(
                ui,
                p,
                icon::GLOBE,
                "Browser extension",
                "Downloads from Chrome, Edge and Brave arrive here through the Hoardini extension. Turn \"Capture browser downloads\" on or off in its popup.",
                |_| {},
            );

            ui.add_space(20.0);
            section(ui, p, "About");
            widgets::card(ui, Sense::hover(), Margin::symmetric(16, 14), |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(include_image!("../../../assets/icon-256.png")).fit_to_exact_size(vec2(32.0, 32.0)));
                    ui.add_space(8.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        widgets::text(ui, "Hoardini", theme::body(), p.text, false);
                        widgets::text(
                            ui,
                            format!("Version {} · Windows port of Hoardini for macOS · MIT license", env!("CARGO_PKG_VERSION")),
                            theme::caption(),
                            p.text2,
                            false,
                        );
                    });
                });
            });
            ui.add_space(24.0);
        });
        if recheck {
            self.manager.refresh_ffmpeg();
        }
        if install {
            self.manager.install_ffmpeg();
        }
    }

    // ---------- Add URL dialog ----------

    fn add_dialog(&mut self, ctx: &egui::Context, p: &Palette) {
        let Some(text) = self.adding.as_mut() else { return };
        let request = JobRequest::new(text.trim());
        let valid = request.valid_url().is_some();
        let (mut submit, mut cancel) = (false, false);
        let focus = std::mem::take(&mut self.focus_add);
        let frame = Frame::new()
            .fill(p.dialog)
            .stroke(Stroke::new(1.0_f32, p.surface_stroke))
            .corner_radius(theme::OVERLAY_RADIUS)
            .shadow(ctx.style().visuals.window_shadow);
        Modal::new(Id::new("add-url")).frame(frame).backdrop_color(p.smoke).show(ctx, |ui| {
            ui.set_width(448.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            Frame::new().inner_margin(Margin::same(24)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                widgets::text(ui, "Add download", theme::subtitle(), p.text, false);
                ui.add_space(12.0);
                widgets::text(ui, "Paste a link to a file or to an .m3u8 video stream.", theme::body(), p.text, false);
                ui.add_space(12.0);
                let edit = widgets::text_box(ui, text, "https://…", ui.available_width());
                if focus {
                    edit.request_focus();
                }
                if edit.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    submit = valid;
                }
                if !text.trim().is_empty() && !valid {
                    ui.add_space(6.0);
                    widgets::text(ui, "Enter a link that starts with http:// or https://", theme::caption(), p.critical, false);
                }
            });
            let footer = Frame::new()
                .fill(p.dialog_footer)
                .corner_radius(CornerRadius { nw: 0, ne: 0, sw: theme::OVERLAY_RADIUS, se: theme::OVERLAY_RADIUS })
                .inner_margin(Margin::same(24))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.x = 8.0;
                    let half = (ui.available_width() - 8.0) / 2.0;
                    ui.horizontal(|ui| {
                        submit |= widgets::button_wide(ui, Kind::Accent, "Download", valid, half).clicked();
                        cancel |= widgets::button_wide(ui, Kind::Standard, "Cancel", true, half).clicked();
                    });
                });
            let r = footer.response.rect;
            ui.painter().line_segment([r.left_top(), r.right_top()], Stroke::new(1.0_f32, p.divider));
        });
        cancel |= ctx.input(|i| i.key_pressed(Key::Escape));
        if submit {
            self.manager.add(request);
        }
        if submit || cancel {
            self.adding = None;
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.adding.is_some() {
            return;
        }
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)));
        if pressed(Modifiers::COMMAND, Key::N) {
            self.page = Page::Downloads;
            self.open_add_dialog();
        }
        if pressed(Modifiers::COMMAND, Key::O) {
            self.pick_files_to_convert();
        }
        if pressed(Modifiers::COMMAND | Modifiers::SHIFT, Key::Delete) {
            self.manager.clear_finished();
        }
    }
}

fn section(ui: &mut Ui, p: &Palette, title: &str) {
    widgets::text(ui, title, theme::body_strong(), p.text, false);
    ui.add_space(4.0);
}

/// The Open button's icon and wording by file type. Clicking always opens the file in its default app.
fn open_button(item: &DownloadItem) -> (char, &'static str) {
    match item.kind() {
        model::Kind::Video => (icon::VIDEO, "Play"),
        model::Kind::Audio => (icon::AUDIO, "Play"),
        model::Kind::Other => (icon::OPEN, "Open"),
    }
}

/// Windows 11 Settings-style card: icon, title + description, control on the right.
fn setting(ui: &mut Ui, p: &Palette, glyph: char, title: &str, desc: &str, trailing: impl FnOnce(&mut Ui)) {
    widgets::card(ui, Sense::hover(), Margin::symmetric(16, 14), |ui| {
        ui.horizontal(|ui| {
            ui.add_sized([24.0, 36.0], egui::Label::new(egui::RichText::new(glyph.to_string()).font(theme::icons(20.0)).color(p.text)));
            ui.add_space(12.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                trailing(ui);
                ui.add_space(16.0);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    widgets::text(ui, title, theme::body(), p.text, false);
                    widgets::text(ui, desc, theme::caption(), p.text2, false);
                });
            });
        });
    });
    ui.add_space(4.0);
}

fn detail(item: &DownloadItem) -> String {
    match item.status {
        Status::Queued => "Waiting…".into(),
        Status::Failed if item.needs_ffmpeg => {
            "Needs ffmpeg to merge this video's separate audio track · resumes once it's installed".into()
        }
        Status::Failed => item.error.clone().unwrap_or_else(|| "Failed".into()),
        Status::Completed => {
            if item.is_converting() {
                return "Converting to MP4…".into();
            }
            if item.needs_ffmpeg {
                return format!("{} · Converts to MP4 once ffmpeg is installed", bytes(item.total.unwrap_or(item.done)));
            }
            if let Some(e) = &item.convert_error {
                return format!("Conversion failed: {e}");
            }
            let size = bytes(item.total.unwrap_or(item.done));
            match item.mp4.as_deref().and_then(|p| p.file_name()) {
                Some(n) => format!("{size} · MP4 ready: {}", n.to_string_lossy()),
                None => size,
            }
        }
        Status::Paused | Status::Downloading => {
            let mut parts = Vec::new();
            match item.total {
                Some(t) => parts.push(format!("{} of {}", bytes(item.done), bytes(t))),
                None if item.done > 0 => parts.push(bytes(item.done)),
                None => {}
            }
            if item.status == Status::Paused {
                parts.push("Paused".into());
            }
            if item.speed > 0.0 {
                parts.push(format!("{}/s", bytes(item.speed as u64)));
            }
            if let Some(eta) = item.eta_seconds() {
                parts.push(format!("{} left", duration(eta)));
            }
            if parts.is_empty() {
                parts.push("Starting…".into());
            }
            parts.join(" · ")
        }
    }
}

impl eframe::App for HoardiniApp {
    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if theme::mica() {
            [0.0; 4]
        } else {
            visuals.panel_fill.to_normalized_gamma_f32()
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.background_start {
            if ctx.cumulative_pass_nr() == 0 {
                ctx.request_repaint(); // the second frame comes after eframe has shown the window
            } else if let Some(h) = self.hwnd {
                // eframe's first-frame show may have activated the window; hand focus back, then
                // show it again without activation, behind the browser.
                win::hide(h);
                win::show_inactive(h, self.previous_foreground);
                win::cloak(h, false);
                self.background_start = false;
            }
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            if let Some(h) = self.hwnd {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                win::hide(h);
            }
        }
        let p = Palette::current(ctx);
        if self.dark_applied != Some(p.dark) {
            if let Some(h) = self.hwnd {
                win::set_dark(h, p.dark);
            }
            self.dark_applied = Some(p.dark);
        }
        tray::refresh(&self.manager);
        self.shortcuts(ctx);

        let items = self.manager.items();
        let active = items.iter().filter(|i| i.status == Status::Downloading).count();
        egui::SidePanel::left("nav")
            .resizable(false)
            .exact_width(56.0)
            .show_separator_line(false)
            .frame(Frame::new().fill(p.base).inner_margin(Margin { left: 4, right: 4, top: 4, bottom: 8 }))
            .show(ctx, |ui| self.nav(ui, &p, active));
        egui::CentralPanel::default()
            .frame(
                Frame::new()
                    .fill(p.layer)
                    .stroke(Stroke::new(1.0_f32, p.card_stroke))
                    .corner_radius(CornerRadius { nw: theme::OVERLAY_RADIUS, ne: 0, sw: 0, se: 0 })
                    .inner_margin(Margin { left: 32, right: 32, top: 24, bottom: 0 }),
            )
            .show(ctx, |ui| match self.page {
                Page::Downloads => self.downloads_page(ui, &p, &items),
                Page::Settings => self.settings_page(ui, &p),
            });
        self.add_dialog(ctx, &p);

        if items.iter().any(|i| matches!(i.status, Status::Downloading | Status::Queued) || i.is_converting()) {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_the_original() {
        assert_eq!(bytes(512), "512 bytes");
        assert_eq!(bytes(12_345), "12 KB");
        assert_eq!(bytes(12_345_678), "12.3 MB");
        assert_eq!(bytes(1_500_000_000), "1.50 GB");
        assert_eq!(duration(75.0), "1:15");
        assert_eq!(duration(3_725.0), "1:02:05");
    }
}
