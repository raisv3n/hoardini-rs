//! Windows 11 (Fluent Design) look: Segoe UI + Segoe Fluent Icons, the user's accent color,
//! light/dark following the system, and Mica behind the window where available.
//! Color values are the WinUI 3 theme resources (e.g. TextFillColorSecondary, CardBackgroundFillColorDefault).

use std::{
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, OnceLock,
    },
};

use eframe::egui::{
    self,
    epaint::{CornerRadius, Shadow},
    Color32, Context, FontData, FontDefinitions, FontFamily, FontId, Margin, Stroke, Theme, ThemePreference, Vec2,
    Visuals,
};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_BINARY};

use crate::model::ThemeMode;

/// Segoe Fluent Icons (Windows 11) / Segoe MDL2 Assets (Windows 10) code points.
pub mod icon {
    pub const ADD: char = '\u{E710}';
    pub const DOWNLOAD: char = '\u{E896}';
    pub const SETTINGS: char = '\u{E713}';
    pub const VIDEO: char = '\u{E714}';
    pub const AUDIO: char = '\u{E8D6}';
    pub const CONVERT: char = '\u{E8AB}';
    pub const BROOM: char = '\u{EA99}';
    pub const PAUSE: char = '\u{E769}';
    pub const PLAY: char = '\u{E768}';
    pub const RETRY: char = '\u{E72C}';
    pub const CANCEL: char = '\u{E711}';
    pub const OPEN: char = '\u{E8A7}';
    pub const FOLDER: char = '\u{E838}';
    pub const COMPLETED: char = '\u{EC61}';
    pub const ERROR: char = '\u{EA39}';
    pub const CLOCK: char = '\u{E823}';
    pub const DELETE: char = '\u{E74D}';
    pub const LINK: char = '\u{E71B}';
    pub const WARNING: char = '\u{E7BA}';
    pub const CHECK: char = '\u{E73E}';
    pub const GLOBE: char = '\u{E774}';
    pub const COLOR: char = '\u{E790}';
    pub const INFO: char = '\u{E946}';
    pub const CHEVRON_DOWN: char = '\u{E70D}';
}

pub const RADIUS: u8 = 4; // ControlCornerRadius
pub const OVERLAY_RADIUS: u8 = 8; // OverlayCornerRadius

static MICA: AtomicBool = AtomicBool::new(false);

/// Set once Mica was applied to the window: surfaces then stay translucent so it shows through.
pub fn set_mica(on: bool) {
    MICA.store(on, Ordering::Relaxed);
}

pub fn mica() -> bool {
    MICA.load(Ordering::Relaxed)
}

#[derive(Clone, Copy)]
struct Accent {
    light3: Color32,
    light2: Color32,
    dark1: Color32,
    dark2: Color32,
}

/// The accent shades Windows derives from the user's color (Settings › Personalization › Colors).
fn accent() -> Accent {
    static ACCENT: OnceLock<Accent> = OnceLock::new();
    *ACCENT.get_or_init(|| {
        let default = Accent {
            light3: Color32::from_rgb(0x99, 0xEB, 0xFF),
            light2: Color32::from_rgb(0x4C, 0xC2, 0xFF),
            dark1: Color32::from_rgb(0x00, 0x67, 0xC0),
            dark2: Color32::from_rgb(0x00, 0x3E, 0x92),
        };
        let key: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Accent\0".encode_utf16().collect();
        let value: Vec<u16> = "AccentPalette\0".encode_utf16().collect();
        let mut buf = [0u8; 32];
        let mut len = buf.len() as u32;
        let rc = unsafe {
            RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_BINARY, ptr::null_mut(), buf.as_mut_ptr().cast(), &mut len)
        };
        if rc != 0 || len < 32 {
            return default;
        }
        // Eight RGBA entries: Light3, Light2, Light1, Accent, Dark1, Dark2, Dark3, (unused).
        let c = |i: usize| Color32::from_rgb(buf[i * 4], buf[i * 4 + 1], buf[i * 4 + 2]);
        Accent { light3: c(0), light2: c(1), dark1: c(4), dark2: c(5) }
    })
}

#[derive(Clone, Copy)]
pub struct Palette {
    pub dark: bool,
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_pressed: Color32,
    pub accent_disabled: Color32,
    pub on_accent: Color32,
    pub accent_text: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub text3: Color32,
    pub disabled: Color32,
    /// Window background behind the navigation pane: transparent over Mica.
    pub base: Color32,
    /// Navigation item fills (they sit directly on Mica).
    pub nav_hover: Color32,
    pub nav_pressed: Color32,
    /// The content layer: LayerFillColorDefault, translucent over Mica.
    pub layer: Color32,
    pub card: Color32,
    pub card_hover: Color32,
    pub card_stroke: Color32,
    pub control: Color32,
    pub control_hover: Color32,
    pub control_pressed: Color32,
    pub control_stroke: Color32,
    pub strong_stroke: Color32,
    pub subtle_hover: Color32,
    pub subtle_pressed: Color32,
    pub divider: Color32,
    pub flyout: Color32,
    pub surface_stroke: Color32,
    pub dialog: Color32,
    pub dialog_footer: Color32,
    pub input_focused: Color32,
    pub success: Color32,
    pub caution: Color32,
    pub critical: Color32,
    /// InfoBar backgrounds (SystemFillColor*Background).
    pub success_bg: Color32,
    pub caution_bg: Color32,
    pub critical_bg: Color32,
    pub smoke: Color32,
}

/// White at `a` opacity (not `Color32::from_white_alpha`, which also works in linear space).
fn w(a: f32) -> Color32 {
    rgba(255, 255, 255, a)
}

fn b(a: f32) -> Color32 {
    Color32::from_black_alpha((a * 255.0).round() as u8)
}

/// sRGB color at `a` opacity, premultiplied in gamma space (how WinUI's resources are defined).
/// egui's own `from_rgba_unmultiplied` premultiplies in linear space, which makes translucent
/// whites several times brighter than intended.
fn rgba(r: u8, g: u8, bl: u8, a: f32) -> Color32 {
    let m = |c: u8| (c as f32 * a).round() as u8;
    Color32::from_rgba_premultiplied(m(r), m(g), m(bl), (a * 255.0).round() as u8)
}

fn with_alpha(c: Color32, a: f32) -> Color32 {
    rgba(c.r(), c.g(), c.b(), a)
}

impl Palette {
    pub fn of(theme: Theme) -> Self {
        let acc = accent();
        let mica = mica();
        match theme {
            Theme::Dark => Self {
                dark: true,
                accent: acc.light2,
                accent_hover: with_alpha(acc.light2, 0.9),
                accent_pressed: with_alpha(acc.light2, 0.8),
                accent_disabled: w(0.158),
                on_accent: Color32::BLACK,
                accent_text: acc.light3,
                text: Color32::WHITE,
                text2: w(0.786),
                text3: w(0.544),
                disabled: w(0.363),
                base: if mica { Color32::TRANSPARENT } else { Color32::from_rgb(32, 32, 32) },
                nav_hover: if mica { w(0.061) } else { Color32::from_rgb(45, 45, 45) },
                nav_pressed: if mica { w(0.042) } else { Color32::from_rgb(40, 40, 40) },
                layer: if mica { rgba(58, 58, 58, 0.30) } else { Color32::from_rgb(39, 39, 39) },
                card: w(0.0512),
                card_hover: w(0.0837),
                card_stroke: b(0.10),
                control: w(0.061),
                control_hover: w(0.084),
                control_pressed: w(0.033),
                control_stroke: w(0.07),
                strong_stroke: w(0.544),
                subtle_hover: w(0.061),
                subtle_pressed: w(0.042),
                divider: w(0.0837),
                flyout: Color32::from_rgb(44, 44, 44),
                surface_stroke: b(0.20),
                dialog: Color32::from_rgb(43, 43, 43),
                dialog_footer: Color32::from_rgb(32, 32, 32),
                input_focused: Color32::from_rgb(30, 30, 30),
                success: Color32::from_rgb(108, 203, 95),
                caution: Color32::from_rgb(252, 225, 0),
                critical: Color32::from_rgb(255, 153, 164),
                success_bg: Color32::from_rgb(0x39, 0x3D, 0x1B),
                caution_bg: Color32::from_rgb(0x43, 0x35, 0x19),
                critical_bg: Color32::from_rgb(0x44, 0x27, 0x26),
                smoke: b(0.30),
            },
            Theme::Light => Self {
                dark: false,
                accent: acc.dark1,
                accent_hover: with_alpha(acc.dark1, 0.9),
                accent_pressed: with_alpha(acc.dark1, 0.8),
                accent_disabled: b(0.217),
                on_accent: Color32::WHITE,
                accent_text: acc.dark2,
                text: b(0.896),
                text2: b(0.606),
                text3: b(0.446),
                disabled: b(0.361),
                base: if mica { Color32::TRANSPARENT } else { Color32::from_rgb(243, 243, 243) },
                nav_hover: if mica { b(0.037) } else { Color32::from_rgb(234, 234, 234) },
                nav_pressed: if mica { b(0.024) } else { Color32::from_rgb(237, 237, 237) },
                layer: if mica { w(0.5) } else { Color32::from_rgb(249, 249, 249) },
                card: w(0.70),
                card_hover: rgba(246, 246, 246, 0.9),
                card_stroke: b(0.0578),
                control: w(0.70),
                control_hover: rgba(249, 249, 249, 0.50),
                control_pressed: rgba(249, 249, 249, 0.30),
                control_stroke: b(0.0578),
                strong_stroke: b(0.446),
                subtle_hover: b(0.037),
                subtle_pressed: b(0.024),
                divider: b(0.0803),
                flyout: Color32::from_rgb(249, 249, 249),
                surface_stroke: b(0.0578),
                dialog: Color32::WHITE,
                dialog_footer: Color32::from_rgb(243, 243, 243),
                input_focused: Color32::WHITE,
                success: Color32::from_rgb(15, 123, 15),
                caution: Color32::from_rgb(157, 93, 0),
                critical: Color32::from_rgb(196, 43, 28),
                success_bg: Color32::from_rgb(0xDF, 0xF6, 0xDD),
                caution_bg: Color32::from_rgb(0xFF, 0xF4, 0xCE),
                critical_bg: Color32::from_rgb(0xFD, 0xE7, 0xE9),
                smoke: b(0.30),
            },
        }
    }

    pub fn current(ctx: &Context) -> Self {
        Self::of(ctx.theme())
    }
}

// ---------- type ramp ----------

pub fn body() -> FontId {
    FontId::new(14.0, FontFamily::Proportional)
}

pub fn body_strong() -> FontId {
    FontId::new(14.0, FontFamily::Name("semibold".into()))
}

pub fn caption() -> FontId {
    FontId::new(12.0, FontFamily::Proportional)
}

pub fn subtitle() -> FontId {
    FontId::new(20.0, FontFamily::Name("semibold".into()))
}

pub fn title() -> FontId {
    FontId::new(28.0, FontFamily::Name("semibold".into()))
}

pub fn icons(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

/// Loads Segoe UI and the icon font from the Windows fonts folder (they are not redistributed).
pub fn install_fonts(ctx: &Context) {
    let dir = std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into())).join("Fonts");
    let load = |names: &[&str]| names.iter().find_map(|n| std::fs::read(dir.join(n)).ok());
    let mut fonts = FontDefinitions::default();
    let defaults = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();

    let mut add = |key: &str, bytes: Option<Vec<u8>>| -> Option<String> {
        let bytes = bytes?;
        fonts.font_data.insert(key.to_owned(), Arc::new(FontData::from_owned(bytes)));
        Some(key.to_owned())
    };
    let regular = add("segoe", load(&["segoeui.ttf"]));
    let semibold = add("segoe-semibold", load(&["seguisb.ttf", "segoeuib.ttf"]));
    let icon_font = add("icons", load(&["SegoeIcons.ttf", "segmdl2.ttf"]));

    let chain = |first: &[&Option<String>]| -> Vec<String> {
        first.iter().filter_map(|f| (*f).clone()).chain(defaults.iter().cloned()).collect()
    };
    fonts.families.insert(FontFamily::Proportional, chain(&[&regular]));
    fonts.families.insert(FontFamily::Name("semibold".into()), chain(&[&semibold, &regular]));
    fonts.families.insert(FontFamily::Name("icons".into()), chain(&[&icon_font]));
    ctx.set_fonts(fonts);
}

// ---------- egui style ----------

fn visuals(p: &Palette) -> Visuals {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    let r = CornerRadius::same(RADIUS);
    v.panel_fill = p.base;
    v.window_fill = p.flyout;
    v.window_stroke = Stroke::new(1.0_f32, p.surface_stroke);
    v.window_corner_radius = CornerRadius::same(OVERLAY_RADIUS);
    v.menu_corner_radius = CornerRadius::same(OVERLAY_RADIUS);
    v.window_shadow = Shadow { offset: [0, 8], blur: 32, spread: 0, color: b(if p.dark { 0.37 } else { 0.14 }) };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 16, spread: 0, color: b(if p.dark { 0.26 } else { 0.14 }) };
    v.extreme_bg_color = p.input_focused;
    v.faint_bg_color = p.subtle_hover;
    v.code_bg_color = p.control;
    v.hyperlink_color = p.accent_text;
    v.warn_fg_color = p.caution;
    v.error_fg_color = p.critical;
    v.selection.bg_fill = with_alpha(p.accent, 0.45);
    v.selection.stroke = Stroke::new(1.0_f32, p.text);
    v.text_cursor.stroke = Stroke::new(1.5_f32, p.text);

    let ws = &mut v.widgets;
    ws.noninteractive.bg_fill = p.card;
    ws.noninteractive.weak_bg_fill = p.card;
    ws.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.divider);
    ws.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
    ws.noninteractive.corner_radius = r;
    for (state, fill) in [
        (&mut ws.inactive, p.control),
        (&mut ws.hovered, p.control_hover),
        (&mut ws.active, p.control_pressed),
        (&mut ws.open, p.control_hover),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::new(1.0_f32, p.control_stroke);
        state.fg_stroke = Stroke::new(1.0_f32, p.text);
        state.corner_radius = r;
        state.expansion = 0.0;
    }
    ws.active.fg_stroke = Stroke::new(1.0_f32, p.text2);
    v
}

/// Applies the Settings theme choice. HOARDINI_THEME=light|dark overrides it (handy for checking both looks).
pub fn set_mode(ctx: &Context, mode: ThemeMode) {
    let mode = match std::env::var("HOARDINI_THEME").as_deref() {
        Ok("light") => ThemeMode::Light,
        Ok("dark") => ThemeMode::Dark,
        _ => mode,
    };
    let preference = match mode {
        ThemeMode::System => ThemePreference::System,
        ThemeMode::Light => ThemePreference::Light,
        ThemeMode::Dark => ThemePreference::Dark,
    };
    ctx.options_mut(|o| o.theme_preference = preference);
}

pub fn apply(ctx: &Context, mode: ThemeMode) {
    set_mode(ctx, mode);
    for theme in [Theme::Dark, Theme::Light] {
        let p = Palette::of(theme);
        ctx.style_mut_of(theme, |s| {
            s.visuals = visuals(&p);
            s.text_styles.insert(egui::TextStyle::Body, body());
            s.text_styles.insert(egui::TextStyle::Button, body());
            s.text_styles.insert(egui::TextStyle::Small, caption());
            s.text_styles.insert(egui::TextStyle::Heading, subtitle());
            s.spacing.item_spacing = Vec2::new(8.0, 8.0);
            s.spacing.button_padding = Vec2::new(11.0, 5.0);
            s.spacing.interact_size = Vec2::new(32.0, 32.0);
            s.spacing.menu_margin = Margin::same(4);
            s.spacing.window_margin = Margin::same(12);
            s.interaction.tooltip_delay = 0.6;
        });
    }
}
