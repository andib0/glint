//! Small translucent on-screen popup ("Brightness 40%") for hotkeys.
//! Has its own state so painting never needs the main App borrow.

use super::util::{hinst, rgb, wide};
use std::cell::RefCell;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Dwm::{
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW,
    EndPaint, FillRect, HFONT, InvalidateRect, PAINTSTRUCT, SelectObject, SetBkMode, SetTextColor,
    TRANSPARENT,
};
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

struct State {
    hwnd: HWND,
    font: HFONT,
    text: String,
    frac: Option<f32>,
}

thread_local! {
    static OSD: RefCell<State> = const { RefCell::new(State { hwnd: null_mut(), font: null_mut(), text: String::new(), frac: None }) };
}

const CLASS: &str = "GlintOsd";
const TIMER_HIDE: usize = 1;

fn scale(v: i32) -> i32 {
    (v as f32 * unsafe { GetDpiForSystem() } as f32 / 96.0) as i32
}

pub fn init(font: HFONT) {
    unsafe {
        let class = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_),
            hInstance: hinst(),
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE,
            class.as_ptr(),
            wide("").as_ptr(),
            WS_POPUP,
            0,
            0,
            scale(260),
            scale(64),
            null_mut(),
            null_mut(),
            hinst(),
            null(),
        );
        SetLayeredWindowAttributes(hwnd, 0, 235, LWA_ALPHA);
        let pref = DWMWCP_ROUND;
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &pref as *const _ as *const _,
            4,
        );
        OSD.with(|o| {
            let mut o = o.borrow_mut();
            o.hwnd = hwnd;
            o.font = font;
        });
    }
}

pub fn show(text: &str, frac: Option<f32>) {
    let hwnd = OSD.with(|o| {
        let mut o = o.borrow_mut();
        o.text = text.to_string();
        o.frac = frac.map(|f| f.clamp(0.0, 1.0));
        o.hwnd
    });
    if hwnd.is_null() {
        return;
    }
    unsafe {
        let mut wa: RECT = std::mem::zeroed();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut _, 0);
        let (w, h) = (scale(260), scale(64));
        let x = wa.left + (wa.right - wa.left - w) / 2;
        let y = wa.bottom - h - scale(72);
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            w,
            h,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
        InvalidateRect(hwnd, null(), 1);
        SetTimer(hwnd, TIMER_HIDE, 1300, None);
    }
}

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_ERASEBKGND => 1,
            WM_TIMER if wp == TIMER_HIDE => {
                KillTimer(hwnd, TIMER_HIDE);
                ShowWindow(hwnd, SW_HIDE);
                0
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                let mut rc: RECT = std::mem::zeroed();
                GetClientRect(hwnd, &mut rc);
                let bg = CreateSolidBrush(rgb(30, 30, 30));
                FillRect(hdc, &rc, bg);
                DeleteObject(bg);
                let (text, frac, font) = OSD.with(|o| {
                    let o = o.borrow();
                    (o.text.clone(), o.frac, o.font)
                });
                let old = SelectObject(hdc, font);
                SetBkMode(hdc, TRANSPARENT as i32);
                SetTextColor(hdc, rgb(240, 240, 240));
                let mut trc = rc;
                if frac.is_some() {
                    trc.bottom -= scale(18);
                }
                if !text.is_empty() {
                    // NUL-terminated buffer: an empty Vec's pointer is dangling.
                    let mut t = wide(&text);
                    DrawTextW(
                        hdc,
                        t.as_mut_ptr(),
                        -1,
                        &mut trc,
                        DT_CENTER | DT_VCENTER | DT_SINGLELINE,
                    );
                }
                if let Some(f) = frac {
                    let pad = scale(20);
                    let bar = RECT {
                        left: pad,
                        right: rc.right - pad,
                        top: rc.bottom - scale(22),
                        bottom: rc.bottom - scale(16),
                    };
                    let track = CreateSolidBrush(rgb(70, 70, 70));
                    FillRect(hdc, &bar, track);
                    DeleteObject(track);
                    let fill_rc = RECT {
                        right: bar.left + ((bar.right - bar.left) as f32 * f) as i32,
                        ..bar
                    };
                    let fill = CreateSolidBrush(rgb(250, 204, 21));
                    FillRect(hdc, &fill_rc, fill);
                    DeleteObject(fill);
                }
                SelectObject(hdc, old);
                EndPaint(hwnd, &ps);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
