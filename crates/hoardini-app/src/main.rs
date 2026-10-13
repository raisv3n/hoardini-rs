#![windows_subsystem = "windows"]

mod manager;
mod model;
mod theme;
mod tray;
mod ui;
mod widgets;
mod win;

use eframe::egui;
use hoardini_core::instance::SingleInstance;

fn main() -> eframe::Result<()> {
    // --background: started by the browser host; open without stealing focus from the browser.
    let background = std::env::args().any(|a| a == "--background");
    let previous_foreground = if background { win::foreground() } else { 0 };
    let Some(_instance) = SingleInstance::acquire() else {
        if !background {
            win::show_existing();
        }
        return Ok(());
    };

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let manager = manager::Manager::new(rt.handle().clone());

    let mut viewport = egui::ViewportBuilder::default()
        .with_title(win::TITLE)
        .with_inner_size([820.0, 560.0])
        .with_min_inner_size([600.0, 400.0])
        // Transparent client area so Mica shows through (solid colors are used where Mica is unavailable).
        .with_transparent(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(include_bytes!("../../../assets/icon-256.png")) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };

    let m = manager.clone();
    let result = eframe::run_native(
        win::TITLE,
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx); // for the app icon PNG
            Ok(Box::new(ui::HoardiniApp::new(cc, m, background, previous_foreground)))
        }),
    );
    manager.shutdown();
    result
}
