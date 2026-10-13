//! Fluent (WinUI 3) controls drawn with egui: Button (accent/standard/subtle), icon AppBarButton,
//! ToggleSwitch, ProgressBar, TextBox and the card surface used for list rows and settings.

use eframe::egui::{
    self,
    epaint::{CornerRadius, RectShape},
    pos2, vec2, Align2, Color32, FontId, Frame, Margin, Rect, Response, Sense, Shape, Stroke, StrokeKind, TextEdit,
    Ui, UiBuilder, Vec2, WidgetText,
};

use crate::theme::{self, Palette, RADIUS};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Accent,
    Standard,
    Subtle,
}

fn fills(p: &Palette, kind: Kind, resp: &Response, enabled: bool) -> (Color32, Color32, Option<Color32>) {
    let pressed = resp.is_pointer_button_down_on();
    let hovered = resp.hovered();
    match kind {
        Kind::Accent if !enabled => (p.accent_disabled, if p.dark { p.disabled } else { Color32::WHITE }, None),
        Kind::Accent => {
            let fill = if pressed { p.accent_pressed } else if hovered { p.accent_hover } else { p.accent };
            (fill, p.on_accent, None)
        }
        Kind::Standard => {
            let fill = if !enabled { p.control } else if pressed { p.control_pressed } else if hovered { p.control_hover } else { p.control };
            (fill, if enabled { if pressed { p.text2 } else { p.text } } else { p.disabled }, Some(p.control_stroke))
        }
        Kind::Subtle => {
            let fill = if !enabled { Color32::TRANSPARENT } else if pressed { p.subtle_pressed } else if hovered { p.subtle_hover } else { Color32::TRANSPARENT };
            (fill, if enabled { if pressed { p.text2 } else { p.text } } else { p.disabled }, None)
        }
    }
}

/// A 32px-high Fluent button with an optional leading icon.
pub fn button(ui: &mut Ui, kind: Kind, glyph: Option<char>, label: &str, enabled: bool) -> Response {
    sized_button(ui, kind, glyph, label, enabled, if kind == Kind::Accent { 96.0 } else { 0.0 })
}

/// A button of a fixed width (dialog footers split the width between their buttons).
pub fn button_wide(ui: &mut Ui, kind: Kind, label: &str, enabled: bool, width: f32) -> Response {
    sized_button(ui, kind, None, label, enabled, width)
}

fn sized_button(ui: &mut Ui, kind: Kind, glyph: Option<char>, label: &str, enabled: bool, min_width: f32) -> Response {
    let p = Palette::current(ui.ctx());
    let galley = ui.painter().layout_no_wrap(label.to_owned(), theme::body(), Color32::PLACEHOLDER);
    let icon_w = if glyph.is_some() { 16.0 + 8.0 } else { 0.0 };
    let size = vec2((12.0 + icon_w + galley.size().x + 12.0).max(min_width), 32.0);
    let (rect, resp) = ui.allocate_exact_size(size, if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let (fill, fg, stroke) = fills(&p, kind, &resp, enabled);
        let painter = ui.painter();
        painter.rect(rect, RADIUS, fill, stroke.map_or(Stroke::NONE, |c| Stroke::new(1.0_f32, c)), StrokeKind::Inside);
        let content_w = icon_w + galley.size().x;
        let mut x = rect.center().x - content_w / 2.0;
        if let Some(g) = glyph {
            painter.text(pos2(x + 8.0, rect.center().y), Align2::CENTER_CENTER, g, theme::icons(16.0), fg);
            x += icon_w;
        }
        painter.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, fg);
    }
    resp
}

/// Square subtle icon button (AppBarButton without label) with a tooltip.
pub fn icon_button(ui: &mut Ui, glyph: char, tooltip: &str) -> Response {
    let p = Palette::current(ui.ctx());
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(32.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let (fill, fg, _) = fills(&p, Kind::Subtle, &resp, true);
        ui.painter().rect_filled(rect, RADIUS, fill);
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, theme::icons(16.0), fg);
    }
    resp.on_hover_text(tooltip)
}

/// Fluent ToggleSwitch (40×20), with the "On"/"Off" label Windows shows to its left.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let p = Palette::current(ui.ctx());
    let label = if *on { "On" } else { "Off" };
    ui.label(egui::RichText::new(label).font(theme::body()).color(p.text));
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    if ui.is_rect_visible(rect) {
        let hovered = resp.hovered();
        let painter = ui.painter();
        let radius = CornerRadius::same(10);
        if *on {
            painter.rect_filled(rect, radius, if hovered { p.accent_hover } else { p.accent });
        } else {
            painter.rect(rect, radius, if hovered { p.subtle_hover } else { Color32::TRANSPARENT }, Stroke::new(1.0_f32, p.strong_stroke), StrokeKind::Inside);
        }
        let knob_r = if hovered { 7.0 } else { 6.0 };
        let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
        painter.circle_filled(pos2(x, rect.center().y), knob_r, if *on { p.on_accent } else { p.text2 });
    }
    resp
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    Running,
    Paused,
}

/// Fluent ProgressBar: 1px rail, 3px rounded indicator. `None` = indeterminate.
pub fn progress(ui: &mut Ui, fraction: Option<f32>, state: ProgressState) {
    let p = Palette::current(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
    let painter = ui.painter();
    let color = if state == ProgressState::Paused { p.caution } else { p.accent };
    let y = rect.center().y;
    painter.line_segment([pos2(rect.left(), y), pos2(rect.right(), y)], Stroke::new(1.0_f32, p.strong_stroke));
    let bar = |x0: f32, x1: f32| Rect::from_min_max(pos2(x0, y - 1.5), pos2(x1, y + 1.5));
    match fraction {
        Some(f) => {
            let x1 = rect.left() + rect.width() * f.clamp(0.0, 1.0);
            if x1 > rect.left() {
                painter.rect_filled(bar(rect.left(), x1.max(rect.left() + 3.0)), 2.0, color);
            }
        }
        None => {
            let t = (ui.input(|i| i.time) * 0.6).fract() as f32;
            let w = rect.width() * 0.4;
            let x0 = rect.left() - w + (rect.width() + w) * t;
            let r = bar(x0.max(rect.left()), (x0 + w).min(rect.right()));
            if r.width() > 0.0 {
                painter.rect_filled(r, 2.0, color);
            }
            ui.ctx().request_repaint();
        }
    }
}

/// Fluent TextBox: filled field, accent underline while focused. Returns the inner TextEdit response.
pub fn text_box(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> Response {
    let p = Palette::current(ui.ctx());
    let (rect, _) = ui.allocate_exact_size(vec2(width, 32.0), Sense::hover());
    let bg = ui.painter().add(Shape::Noop);
    let inner = rect.shrink2(vec2(11.0, 6.0));
    let edit = ui.put(
        inner,
        TextEdit::singleline(text)
            .frame(false)
            .hint_text(WidgetText::from(egui::RichText::new(hint).color(p.text3)))
            .font(theme::body())
            .text_color(p.text)
            .vertical_align(egui::Align::Center),
    );
    let focused = edit.has_focus();
    let fill = if focused { p.input_focused } else if edit.hovered() { p.control_hover } else { p.control };
    ui.painter().set(bg, RectShape::new(rect, RADIUS, fill, Stroke::new(1.0_f32, p.control_stroke), StrokeKind::Inside));
    let (h, c) = if focused { (2.0, p.accent) } else { (1.0, p.strong_stroke) };
    let underline = Rect::from_min_max(pos2(rect.left() + 1.0, rect.bottom() - h), pos2(rect.right() - 1.0, rect.bottom()));
    ui.painter().rect_filled(underline, CornerRadius { nw: 0, ne: 0, sw: RADIUS, se: RADIUS }, c);
    edit
}

/// A rounded card surface; the whole card senses `sense` (behind its child widgets).
/// Returns (card response, content result).
pub fn card<R>(ui: &mut Ui, sense: Sense, margin: Margin, content: impl FnOnce(&mut Ui) -> R) -> (Response, R) {
    let p = Palette::current(ui.ctx());
    let bg = ui.painter().add(Shape::Noop);
    let scoped = ui.scope_builder(UiBuilder::new().sense(sense), |ui| {
        Frame::new()
            .inner_margin(margin)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                content(ui)
            })
            .inner
    });
    let resp = scoped.response;
    let interactive = sense != Sense::hover();
    let fill = if interactive && resp.contains_pointer() { p.card_hover } else { p.card };
    ui.painter().set(bg, RectShape::new(resp.rect, RADIUS, fill, Stroke::new(1.0_f32, p.card_stroke), StrokeKind::Inside));
    (resp, scoped.inner)
}

/// Text in a given font/color, optionally truncated with "…".
pub fn text(ui: &mut Ui, s: impl Into<String>, font: FontId, color: Color32, truncate: bool) -> Response {
    let label = egui::Label::new(egui::RichText::new(s.into()).font(font).color(color));
    ui.add(if truncate { label.truncate() } else { label.wrap() })
}

/// Fluent ComboBox: a field showing the current choice and a chevron; the list drops down below it,
/// with the selected entry marked by the accent pill. Returns the newly picked index.
pub fn combo(ui: &mut Ui, id: &str, options: &[&str], selected: usize) -> Option<usize> {
    let p = Palette::current(ui.ctx());
    let popup_id = ui.make_persistent_id(id);
    let open = ui.memory(|m| m.is_popup_open(popup_id));
    let (rect, resp) = ui.allocate_exact_size(vec2(200.0, 32.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if resp.is_pointer_button_down_on() || open {
            p.control_pressed
        } else if resp.hovered() {
            p.control_hover
        } else {
            p.control
        };
        let painter = ui.painter();
        painter.rect(rect, RADIUS, fill, Stroke::new(1.0_f32, p.control_stroke), StrokeKind::Inside);
        let label = options.get(selected).copied().unwrap_or_default();
        painter.text(pos2(rect.left() + 11.0, rect.center().y), Align2::LEFT_CENTER, label, theme::body(), p.text);
        painter.text(pos2(rect.right() - 18.0, rect.center().y), Align2::CENTER_CENTER, theme::icon::CHEVRON_DOWN, theme::icons(12.0), p.text2);
    }
    if resp.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    let mut picked = None;
    egui::popup_below_widget(ui, popup_id, &resp, egui::PopupCloseBehavior::CloseOnClick, |ui| {
        ui.set_min_width(rect.width() - 8.0);
        for (i, option) in options.iter().enumerate() {
            let (r, item) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
            let fill = if item.is_pointer_button_down_on() {
                p.subtle_pressed
            } else if item.hovered() || i == selected {
                p.subtle_hover
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(r, RADIUS, fill);
            if i == selected {
                let pill = Rect::from_center_size(pos2(r.left() + 1.5, r.center().y), vec2(3.0, 16.0));
                ui.painter().rect_filled(pill, 1.5, p.accent);
            }
            ui.painter().text(pos2(r.left() + 12.0, r.center().y), Align2::LEFT_CENTER, *option, theme::body(), p.text);
            if item.clicked() && i != selected {
                picked = Some(i);
            }
        }
    });
    picked
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Informational,
    Success,
    Warning,
    Error,
}

/// Fluent InfoBar: severity icon, bold title, message, optional actions, close button.
/// Returns true when the close button (shown when `closable`) was clicked.
pub fn info_bar(ui: &mut Ui, severity: Severity, title: &str, message: &str, closable: bool, actions: impl FnOnce(&mut Ui)) -> bool {
    let p = Palette::current(ui.ctx());
    let (bg, icon_color, glyph) = match severity {
        Severity::Informational => (p.card, p.accent, theme::icon::INFO),
        Severity::Success => (p.success_bg, p.success, theme::icon::COMPLETED),
        Severity::Warning => (p.caution_bg, p.caution, theme::icon::WARNING),
        Severity::Error => (p.critical_bg, p.critical, theme::icon::ERROR),
    };
    let mut closed = false;
    Frame::new()
        .fill(bg)
        .stroke(Stroke::new(1.0_f32, p.card_stroke))
        .corner_radius(RADIUS)
        .inner_margin(Margin { left: 16, right: 8, top: 8, bottom: 8 })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add_sized([20.0, 32.0], egui::Label::new(egui::RichText::new(glyph.to_string()).font(theme::icons(16.0)).color(icon_color)));
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    closed = closable && icon_button(ui, theme::icon::CANCEL, "Close").clicked();
                    actions(ui);
                    ui.add_space(8.0);
                    // Back to left-to-right (with wrapping) for the text, inside the right-to-left row.
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true), |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        text(ui, title, theme::body_strong(), p.text, false);
                        text(ui, message, theme::body(), p.text, false);
                    });
                });
            });
        });
    closed
}

/// Menu entry with a leading icon, Fluent MenuFlyoutItem style.
pub fn menu_item(ui: &mut Ui, glyph: char, label: &str) -> Response {
    let p = Palette::current(ui.ctx());
    let width = ui.available_width().max(180.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    let (fill, fg, _) = fills(&p, Kind::Subtle, &resp, true);
    ui.painter().rect_filled(rect, RADIUS, fill);
    ui.painter().text(pos2(rect.left() + 20.0, rect.center().y), Align2::CENTER_CENTER, glyph, theme::icons(16.0), fg);
    ui.painter().text(pos2(rect.left() + 40.0, rect.center().y), Align2::LEFT_CENTER, label, theme::body(), fg);
    resp
}
