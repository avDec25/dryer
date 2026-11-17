//! Start Dryer when the user logs in.
//!
//! This is the per-user `Run` registry key, so it needs no administrator rights and
//! only affects this account.

use std::ffi::c_void;

use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};
use windows::core::w;

/// Is Dryer set to start at login?
pub fn is_enabled() -> bool {
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
            w!("Dryer"),
            RRF_RT_REG_SZ,
            None,
            None,
            None,
        ) == ERROR_SUCCESS
    }
}

/// Turn start-at-login on or off. Returns whether the registry accepted the change.
pub fn set_enabled(on: bool) -> bool {
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return false };
            // Quoted, so a path with spaces still works.
            let command = format!("\"{}\"", exe.display());
            let wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
                w!("Dryer"),
                REG_SZ.0,
                Some(wide.as_ptr() as *const c_void),
                (wide.len() * 2) as u32,
            ) == ERROR_SUCCESS
        } else {
            // Deleting a value that is not there is fine too.
            let _ = RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
                w!("Dryer"),
            );
            !is_enabled()
        }
    }
}

/// If start-at-login is on, point it at the executable that is running now, so
/// moving or rebuilding Dryer does not leave a stale path behind.
pub fn refresh() {
    if is_enabled() {
        set_enabled(true);
    }
}
