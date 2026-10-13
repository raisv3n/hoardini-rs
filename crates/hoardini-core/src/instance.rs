//! Named-mutex helpers (the Windows stand-in for the bundle-ID lookups the macOS host did).

use std::ptr;

use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE},
    System::Threading::{CreateMutexW, OpenMutexW},
};

use crate::paths::APP_MUTEX;

const SYNCHRONIZE: u32 = 0x0010_0000;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Held for the app's lifetime; the OS releases it if the process dies.
pub struct SingleInstance(HANDLE);

// The handle is only closed on drop; it is never used concurrently.
unsafe impl Send for SingleInstance {}
unsafe impl Sync for SingleInstance {}

impl SingleInstance {
    /// `None` when another instance already owns the mutex.
    pub fn acquire() -> Option<Self> {
        let name = wide(APP_MUTEX);
        let h = unsafe { CreateMutexW(ptr::null(), 0, name.as_ptr()) };
        if h.is_null() {
            return None;
        }
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe { CloseHandle(h) };
            return None;
        }
        Some(Self(h))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

pub fn app_running() -> bool {
    let name = wide(APP_MUTEX);
    let h = unsafe { OpenMutexW(SYNCHRONIZE, 0, name.as_ptr()) };
    if h.is_null() {
        return false;
    }
    unsafe { CloseHandle(h) };
    true
}
