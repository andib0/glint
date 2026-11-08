//! "Start with Windows" via HKCU\...\Run, plus the matching
//! Explorer\StartupApproved entry. Recent Windows 11 builds skip Run entries
//! that have no "enabled" approval record, so both are written together.

use super::util::wide;
use std::ptr::null_mut;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_BINARY, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ, RegDeleteKeyValueW,
    RegGetValueW, RegSetKeyValueW,
};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const NAME: &str = "Glint";

/// Approval record: first byte even = enabled (02), odd = disabled by the user (03).
const ENABLED: [u8; 12] = [2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

fn run_entry_exists() -> bool {
    let mut size = 0u32;
    unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(RUN).as_ptr(),
            wide(NAME).as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            null_mut(),
            &mut size,
        ) == 0
    }
}

/// None = no approval record; Some(true/false) = enabled/disabled in Settings > Startup apps.
fn approval() -> Option<bool> {
    let mut buf = [0u8; 12];
    let mut size = buf.len() as u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(APPROVED).as_ptr(),
            wide(NAME).as_ptr(),
            RRF_RT_REG_BINARY,
            null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut size,
        )
    };
    (rc == 0 && size > 0).then(|| buf[0] % 2 == 0)
}

/// True only if Windows will actually launch Glint at sign-in.
pub fn is_enabled() -> bool {
    run_entry_exists() && approval() == Some(true)
}

pub fn set(enabled: bool) -> Result<(), String> {
    let (run, appr) = unsafe {
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let val = wide(&format!("\"{}\"", exe.display()));
            let run = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN).as_ptr(),
                wide(NAME).as_ptr(),
                REG_SZ,
                val.as_ptr() as *const _,
                (val.len() * 2) as u32,
            );
            let appr = RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(APPROVED).as_ptr(),
                wide(NAME).as_ptr(),
                REG_BINARY,
                ENABLED.as_ptr() as *const _,
                ENABLED.len() as u32,
            );
            (run, appr)
        } else {
            let run =
                RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN).as_ptr(), wide(NAME).as_ptr());
            let appr = RegDeleteKeyValueW(
                HKEY_CURRENT_USER,
                wide(APPROVED).as_ptr(),
                wide(NAME).as_ptr(),
            );
            (run, appr)
        }
    };
    // 2 = ERROR_FILE_NOT_FOUND, fine when removing something that isn't there.
    let ok = |rc: u32| rc == 0 || (!enabled && rc == 2);
    if ok(run) && ok(appr) {
        Ok(())
    } else {
        Err(format!("registry error {run}/{appr}"))
    }
}
