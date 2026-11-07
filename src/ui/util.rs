use std::io::Write;
use windows_sys::Win32::Foundation::{HINSTANCE, HWND};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextLengthW, GetWindowTextW};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub fn loword(x: usize) -> u32 {
    (x & 0xFFFF) as u32
}

pub fn hiword(x: usize) -> u32 {
    ((x >> 16) & 0xFFFF) as u32
}

pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

pub fn hinst() -> HINSTANCE {
    unsafe { GetModuleHandleW(std::ptr::null()) }
}

pub fn window_text(hwnd: HWND) -> String {
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        let mut buf = vec![0u16; len as usize + 1];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

/// Append a line to `%APPDATA%\Glint\glint.log` (kept under ~256 KB).
pub fn log(msg: &str) {
    let path = crate::config::config_dir().join("glint.log");
    if std::fs::metadata(&path)
        .map(|m| m.len() > 256 * 1024)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(&path);
    }
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "[{secs}] {msg}");
    }
}
