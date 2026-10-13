//! Win32 window plumbing (the NSApp/NSWorkspace calls of the macOS version).

use std::{os::windows::process::CommandExt, path::Path, process::Command, ptr};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Graphics::Dwm::{
        DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMWA_CLOAK, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
    },
    UI::Controls::MARGINS,
    UI::WindowsAndMessaging::{FindWindowW, GetForegroundWindow, IsIconic, SetForegroundWindow, ShowWindow, SW_HIDE, SW_RESTORE, SW_SHOW, SW_SHOWNOACTIVATE},
};

pub const TITLE: &str = "Hoardini";

pub fn hwnd(cc: &eframe::CreationContext<'_>) -> Option<isize> {
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// Shown directly through Win32: a hidden eframe window gets no frames, so it cannot show itself.
pub fn show(hwnd: isize) {
    let h = hwnd as _;
    unsafe {
        ShowWindow(h, if IsIconic(h) != 0 { SW_RESTORE } else { SW_SHOW });
        SetForegroundWindow(h);
    }
}

/// Shows the window without activating it, then gives focus back to `previous` (the browser),
/// like macOS `open -g`.
pub fn show_inactive(hwnd: isize, previous: isize) {
    unsafe {
        ShowWindow(hwnd as _, SW_SHOWNOACTIVATE);
        if previous != 0 && GetForegroundWindow() as isize == hwnd {
            SetForegroundWindow(previous as _);
        }
    }
}

pub fn foreground() -> isize {
    unsafe { GetForegroundWindow() as isize }
}

/// Closing the window keeps downloading; the tray icon stays.
pub fn hide(hwnd: isize) {
    unsafe { ShowWindow(hwnd as _, SW_HIDE) };
}

/// Puts the Mica material behind the (transparent) client area. False on systems without it
/// (Windows 10, Windows 11 before 22H2), where the app keeps solid backgrounds.
pub fn set_mica(hwnd: isize) -> bool {
    let backdrop: i32 = DWMSBT_MAINWINDOW;
    let margins = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
    unsafe {
        let ok = DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_SYSTEMBACKDROP_TYPE as u32,
            (&backdrop as *const i32).cast(),
            size_of::<i32>() as u32,
        ) == 0;
        ok && DwmExtendFrameIntoClientArea(hwnd as _, &margins) == 0
    }
}

/// Dark title bar and Mica tint to match the app theme.
pub fn set_dark(hwnd: isize, dark: bool) {
    let value: i32 = dark.into();
    unsafe {
        DwmSetWindowAttribute(
            hwnd as _,
            DWMWA_USE_IMMERSIVE_DARK_MODE as u32,
            (&value as *const i32).cast(),
            size_of::<i32>() as u32,
        );
    }
}

/// A cloaked window is composed invisibly even while "shown". Used for a tray-only start, since
/// eframe always shows the window after its first frame.
pub fn cloak(hwnd: isize, on: bool) {
    let value: i32 = on.into();
    unsafe {
        DwmSetWindowAttribute(hwnd as _, DWMWA_CLOAK as u32, (&value as *const i32).cast(), size_of::<i32>() as u32);
    }
}

/// Brings the already-running instance's window forward.
pub fn show_existing() {
    let title: Vec<u16> = TITLE.encode_utf16().chain(Some(0)).collect();
    let h = unsafe { FindWindowW(ptr::null(), title.as_ptr()) };
    if !h.is_null() {
        show(h as isize);
    }
}

pub fn open(path: &Path) {
    let _ = Command::new("explorer.exe").arg(path).spawn();
}

/// Opens a web page in the default browser.
pub fn open_url(url: &str) {
    let _ = Command::new("explorer.exe").arg(url).spawn();
}

/// "Show in Explorer" with the file selected.
pub fn reveal(path: &Path) {
    let _ = Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", path.display())).spawn();
}
