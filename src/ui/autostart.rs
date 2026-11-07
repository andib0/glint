//! "Start with Windows" via HKCU\...\Run.

use super::util::wide;
use std::ptr::null_mut;
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const NAME: &str = "Glint";

pub fn is_enabled() -> bool {
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

pub fn set(enabled: bool) -> Result<(), String> {
    let rc = unsafe {
        if enabled {
            let exe = std::env::current_exe().map_err(|e| e.to_string())?;
            let val = wide(&format!("\"{}\"", exe.display()));
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                wide(RUN).as_ptr(),
                wide(NAME).as_ptr(),
                REG_SZ,
                val.as_ptr() as *const _,
                (val.len() * 2) as u32,
            )
        } else {
            RegDeleteKeyValueW(HKEY_CURRENT_USER, wide(RUN).as_ptr(), wide(NAME).as_ptr())
        }
    };
    if rc == 0 || (!enabled && rc == 2) {
        Ok(())
    } else {
        Err(format!("registry error {rc}"))
    }
}
