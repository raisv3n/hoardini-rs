//! System tray icon: the Windows counterpart of the macOS menu bar extra.
//!
//! Tray and menu events are delivered on the UI thread (from the tray's window procedure),
//! even while the main window is hidden and egui runs no frames. The menu items are not
//! `Send`, so they live in a thread-local that only that thread touches.

use std::{cell::RefCell, sync::Arc};

use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use crate::{manager::Manager, ui::bytes, win};

struct TrayState {
    icon: TrayIcon,
    status: MenuItem,
    clear: MenuItem,
    last: (usize, u64, usize),
}

thread_local! {
    static TRAY: RefCell<Option<TrayState>> = const { RefCell::new(None) };
}

fn load_icon() -> Option<Icon> {
    let img = image::load_from_memory(include_bytes!("../../../assets/icon-256.png")).ok()?;
    let rgba = image::imageops::resize(&img.to_rgba8(), 32, 32, image::imageops::FilterType::Lanczos3);
    Icon::from_rgba(rgba.into_raw(), 32, 32).ok()
}

pub fn install(manager: Arc<Manager>, hwnd: Option<isize>, ctx: eframe::egui::Context) {
    let status = MenuItem::new("No active downloads", false, None);
    let clear = MenuItem::new("Clear Finished", false, None);
    let show = MenuItem::new("Show Hoardini", true, None);
    let quit = MenuItem::new("Quit Hoardini", true, None);
    let menu = Menu::new();
    if menu.append_items(&[&status, &PredefinedMenuItem::separator(), &clear, &show, &quit]).is_err() {
        return;
    }
    let Some(image) = load_icon() else { return };
    let Ok(icon) = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("Hoardini")
        .with_icon(image)
        .build()
    else {
        return;
    };

    let (clear_id, show_id, quit_id) = (clear.id().clone(), show.id().clone(), quit.id().clone());
    TRAY.with(|t| *t.borrow_mut() = Some(TrayState { icon, status, clear, last: (usize::MAX, 0, 0) }));

    let show_window = {
        let ctx = ctx.clone();
        move || {
            if let Some(h) = hwnd {
                win::show(h);
            }
            ctx.request_repaint();
        }
    };
    let m = manager.clone();
    let show_from_menu = show_window.clone();
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == show_id {
            show_from_menu();
        } else if e.id == clear_id {
            m.clear_finished();
            refresh(&m);
        } else if e.id == quit_id {
            m.shutdown();
            std::process::exit(0);
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| match e {
        TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } => show_window(),
        // Refresh before the context menu opens (it opens on button release).
        TrayIconEvent::Click { button: MouseButton::Right, button_state: MouseButtonState::Down, .. }
        | TrayIconEvent::Enter { .. } => refresh(&manager),
        _ => {}
    }));
}

/// Updates the status line, the Clear Finished item and the tooltip. UI thread only.
pub fn refresh(manager: &Manager) {
    let (active, speed) = manager.activity();
    let finished = manager.finished_count();
    TRAY.with(|t| {
        let mut t = t.borrow_mut();
        let Some(t) = t.as_mut() else { return };
        let key = (active, speed as u64 / 1024, finished);
        if t.last == key {
            return;
        }
        t.last = key;
        let line = if active == 0 {
            "No active downloads".to_owned()
        } else {
            format!("{active} downloading · {}/s", bytes(speed as u64))
        };
        t.status.set_text(&line);
        t.clear.set_text(if finished > 0 { format!("Clear Finished ({finished})") } else { "Clear Finished".into() });
        t.clear.set_enabled(finished > 0);
        let _ = t.icon.set_tooltip(Some(if active == 0 { "Hoardini".to_owned() } else { format!("Hoardini — {line}") }));
    });
}
