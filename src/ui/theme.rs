//! Light/dark theme following Windows, plus owner-drawn buttons, toggle
//! switches and custom-drawn sliders in the Windows 11 style.
//!
//! Everything here reads thread-local state only, so painting never needs
//! the App borrow (paint messages can arrive while it is held).

use super::gfx::Canvas;
use super::util::wide;
use std::cell::{Cell, RefCell};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, SIZE, WPARAM};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::*;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
use windows_sys::Win32::UI::Controls::{DRAWITEMSTRUCT, NMCUSTOMDRAW, SetWindowTheme};
use windows_sys::Win32::UI::HiDpi::GetDpiForSystem;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent,
};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

#[derive(Clone, Copy, PartialEq)]
pub struct Theme {
    pub dark: bool,
    pub bg: u32,
    pub surface: u32,
    pub hover: u32,
    pub pressed: u32,
    pub line: u32,
    pub rail: u32,
    pub text: u32,
    pub muted: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub thumb: u32,
}

/// Colours are 0xRRGGBB.
const LIGHT: Theme = Theme {
    dark: false,
    bg: 0xF9F9F9,
    surface: 0xFFFFFF,
    hover: 0xF0F0F0,
    pressed: 0xE6E6E6,
    line: 0xE3E3E3,
    rail: 0xC8C8C8,
    text: 0x1B1B1B,
    muted: 0x616161,
    accent: 0x005FB8,
    on_accent: 0xFFFFFF,
    thumb: 0xFFFFFF,
};

const DARK: Theme = Theme {
    dark: true,
    bg: 0x202020,
    surface: 0x2D2D2D,
    hover: 0x383838,
    pressed: 0x333333,
    line: 0x3A3A3A,
    rail: 0x6E6E6E,
    text: 0xF3F3F3,
    muted: 0xA6A6A6,
    accent: 0x60CDFF,
    on_accent: 0x000000,
    thumb: 0x454545,
};

struct Res {
    bg: HBRUSH,
    surface: HBRUSH,
    font: HFONT,
    bold: HFONT,
    icon: HFONT,
}

thread_local! {
    static THEME: Cell<Theme> = const { Cell::new(LIGHT) };
    static RES: RefCell<Res> = const { RefCell::new(Res { bg: null_mut(), surface: null_mut(), font: null_mut(), bold: null_mut(), icon: null_mut() }) };
}

pub fn get() -> Theme {
    THEME.with(|t| t.get())
}

/// GDI COLORREF (0x00BBGGRR) from 0xRRGGBB.
pub fn cref(rgb: u32) -> u32 {
    ((rgb & 0xFF) << 16) | (rgb & 0xFF00) | ((rgb >> 16) & 0xFF)
}

pub fn px(v: f32) -> f32 {
    v * unsafe { GetDpiForSystem() } as f32 / 96.0
}

fn system_is_dark() -> bool {
    let mut val = 1u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize").as_ptr(),
            wide("AppsUseLightTheme").as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut val as *mut _ as *mut _,
            &mut size,
        )
    };
    rc == 0 && val == 0
}

/// Apply the configured theme ("system", "light" or "dark"). Returns true if it changed.
pub fn apply(pref: &str) -> bool {
    let dark = match pref.to_ascii_lowercase().as_str() {
        "dark" => true,
        "light" => false,
        _ => system_is_dark(),
    };
    let t = if dark { DARK } else { LIGHT };
    let changed = get() != t || RES.with(|r| r.borrow().bg.is_null());
    THEME.with(|c| c.set(t));
    RES.with(|r| {
        let mut r = r.borrow_mut();
        unsafe {
            if !r.bg.is_null() {
                DeleteObject(r.bg);
                DeleteObject(r.surface);
            }
            r.bg = CreateSolidBrush(cref(t.bg));
            r.surface = CreateSolidBrush(cref(t.surface));
        }
    });
    dark_menus(pref);
    changed
}

/// Context menus follow the theme (undocumented uxtheme ordinals 135/136, as used by Explorer).
fn dark_menus(pref: &str) {
    let mode = match pref.to_ascii_lowercase().as_str() {
        "dark" => 2,
        "light" => 3,
        _ => 1,
    };
    unsafe {
        let lib = LoadLibraryW(wide("uxtheme.dll").as_ptr());
        if lib.is_null() {
            return;
        }
        if let Some(f) = GetProcAddress(lib, 135usize as *const u8) {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(f);
            set(mode);
        }
        if let Some(f) = GetProcAddress(lib, 136usize as *const u8) {
            let flush: unsafe extern "system" fn() = std::mem::transmute(f);
            flush();
        }
    }
}

pub fn bg_brush() -> HBRUSH {
    RES.with(|r| r.borrow().bg)
}

pub fn surface_brush() -> HBRUSH {
    RES.with(|r| r.borrow().surface)
}

/// Fonts: Segoe UI Variable on Windows 11 (falls back to the system message font),
/// and Segoe Fluent Icons / MDL2 Assets for glyphs.
pub fn init_fonts() -> (HFONT, HFONT) {
    unsafe {
        let mut ncm: NONCLIENTMETRICSW = std::mem::zeroed();
        ncm.cbSize = size_of::<NONCLIENTMETRICSW>() as u32;
        SystemParametersInfoW(
            SPI_GETNONCLIENTMETRICS,
            ncm.cbSize,
            &mut ncm as *mut _ as *mut _,
            0,
        );
        let base = ncm.lfMessageFont;
        let make = |face: &str, weight: i32, height: i32| {
            let mut lf = base;
            lf.lfWeight = weight;
            lf.lfHeight = height;
            lf.lfQuality = CLEARTYPE_QUALITY;
            let w: Vec<u16> = face.encode_utf16().collect();
            if !w.is_empty() {
                lf.lfFaceName = [0; 32];
                lf.lfFaceName[..w.len().min(31)].copy_from_slice(&w[..w.len().min(31)]);
            }
            CreateFontIndirectW(&lf)
        };
        let ui = if font_exists("Segoe UI Variable Text") {
            "Segoe UI Variable Text"
        } else {
            ""
        };
        let icons = if font_exists("Segoe Fluent Icons") {
            "Segoe Fluent Icons"
        } else {
            "Segoe MDL2 Assets"
        };
        let h = base.lfHeight;
        let font = make(ui, 400, h);
        let bold = make(ui, 600, h);
        let icon = make(icons, 400, h * 6 / 5);
        let big = make(ui, 600, h * 3 / 2);
        RES.with(|r| {
            let mut r = r.borrow_mut();
            r.font = font;
            r.bold = bold;
            r.icon = icon;
        });
        (big, font)
    }
}

fn font_exists(face: &str) -> bool {
    unsafe {
        let hdc = GetDC(null_mut());
        let mut lf: LOGFONTW = std::mem::zeroed();
        let w: Vec<u16> = face.encode_utf16().collect();
        lf.lfFaceName[..w.len().min(31)].copy_from_slice(&w[..w.len().min(31)]);
        lf.lfCharSet = DEFAULT_CHARSET;
        let mut found = false;
        unsafe extern "system" fn cb(
            _: *const LOGFONTW,
            _: *const TEXTMETRICW,
            _: u32,
            l: LPARAM,
        ) -> i32 {
            unsafe { *(l as *mut bool) = true };
            0
        }
        EnumFontFamiliesExW(hdc, &lf, Some(cb), &mut found as *mut bool as LPARAM, 0);
        ReleaseDC(null_mut(), hdc);
        found
    }
}

pub fn font() -> HFONT {
    RES.with(|r| r.borrow().font)
}

pub fn bold() -> HFONT {
    RES.with(|r| r.borrow().bold)
}

/// Theme native controls (combos, edits) to match light/dark.
pub fn style_native(hwnd: HWND) {
    let name = if get().dark {
        "DarkMode_CFD"
    } else {
        "Explorer"
    };
    unsafe { SetWindowTheme(hwnd, wide(name).as_ptr(), null()) };
}

/// Rounded corners + border colour on Windows 11 (ignored on 10).
pub fn style_window(hwnd: HWND) {
    unsafe {
        let round: u32 = 2; // DWMWCP_ROUND
        DwmSetWindowAttribute(hwnd, 33, &round as *const _ as *const _, 4);
        let border = cref(get().line);
        DwmSetWindowAttribute(hwnd, 34, &border as *const _ as *const _, 4);
        let dark: i32 = get().dark as i32;
        DwmSetWindowAttribute(hwnd, 20, &dark as *const _ as *const _, 4);
    }
}

// ---------- owner-drawn buttons ----------

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    /// Rounded, bordered button.
    Button = 1,
    /// Glyph-only button (Segoe icon font).
    Icon = 2,
    /// Toggle switch with a label.
    Toggle = 3,
    /// Quiet text button with a chevron.
    Disclosure = 4,
}

const ODS_SELECTED: u32 = 0x1;
const ODS_FOCUS: u32 = 0x10;
const ODS_NOFOCUSRECT: u32 = 0x200;
const FLAG_ON: isize = 0x100;
const FLAG_HOT: isize = 0x200;

pub fn make_button(hwnd: HWND, kind: Kind, on: bool) {
    unsafe {
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            kind as isize | if on { FLAG_ON } else { 0 },
        );
        SetWindowSubclass(hwnd, Some(hover_proc), 1, 0);
    }
}

pub fn is_on(hwnd: HWND) -> bool {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) & FLAG_ON != 0 }
}

pub fn set_on(hwnd: HWND, on: bool) {
    unsafe {
        let d = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            if on { d | FLAG_ON } else { d & !FLAG_ON },
        );
        InvalidateRect(hwnd, null(), 0);
    }
}

unsafe extern "system" fn hover_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    unsafe {
        match msg {
            WM_MOUSEMOVE => {
                let d = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if d & FLAG_HOT == 0 {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, d | FLAG_HOT);
                    let mut tme = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut tme);
                    InvalidateRect(hwnd, null(), 0);
                }
            }
            0x02A3 => {
                // WM_MOUSELEAVE
                let d = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, d & !FLAG_HOT);
                InvalidateRect(hwnd, null(), 0);
            }
            // owner-draw buttons repaint on WM_PAINT; skip the flicker-y erase
            WM_ERASEBKGND => return 1,
            _ => {}
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

fn text_out(hdc: HDC, s: &str, rc: &mut RECT, color: u32, font: HFONT, flags: DRAW_TEXT_FORMAT) {
    unsafe {
        let old = SelectObject(hdc, font);
        SetBkMode(hdc, TRANSPARENT as i32);
        SetTextColor(hdc, cref(color));
        let mut w = wide(s);
        DrawTextW(
            hdc,
            w.as_mut_ptr(),
            -1,
            rc,
            flags | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX,
        );
        SelectObject(hdc, old);
    }
}

pub const GLYPH_REFRESH: &str = "\u{E72C}";
pub const GLYPH_SETTINGS: &str = "\u{E713}";
const GLYPH_DOWN: &str = "\u{E70D}";
const GLYPH_UP: &str = "\u{E70E}";

/// WM_DRAWITEM for our owner-drawn buttons.
pub fn draw_button(dis: &DRAWITEMSTRUCT) {
    let t = get();
    let data = unsafe { GetWindowLongPtrW(dis.hwndItem, GWLP_USERDATA) };
    let on = data & FLAG_ON != 0;
    let hot = data & FLAG_HOT != 0;
    let pressed = dis.itemState & ODS_SELECTED != 0;
    let focus = dis.itemState & ODS_FOCUS != 0 && dis.itemState & ODS_NOFOCUSRECT == 0;
    let hdc = dis.hDC;
    let rc = dis.rcItem;
    let (x, y, w, h) = (
        rc.left as f32,
        rc.top as f32,
        (rc.right - rc.left) as f32,
        (rc.bottom - rc.top) as f32,
    );
    let label = super::util::window_text(dis.hwndItem);
    unsafe { FillRect(hdc, &rc, bg_brush()) };

    let Some(cv) = Canvas::new(hdc) else { return };
    let r = px(5.0);
    match (data & 0xFF) as u8 {
        k if k == Kind::Button as u8 => {
            let fill = if pressed {
                t.pressed
            } else if hot {
                t.hover
            } else {
                t.surface
            };
            cv.fill_round(x, y, w, h, r, fill);
            cv.stroke_round(x, y, w, h, r, t.line, 1.0);
            drop(cv);
            let mut trc = rc;
            text_out(hdc, &label, &mut trc, t.text, font(), DT_CENTER);
        }
        k if k == Kind::Icon as u8 => {
            if hot || pressed {
                cv.fill_round(x, y, w, h, r, if pressed { t.pressed } else { t.hover });
            }
            drop(cv);
            let mut trc = rc;
            let icon = RES.with(|r| r.borrow().icon);
            text_out(
                hdc,
                &label,
                &mut trc,
                if hot { t.text } else { t.muted },
                icon,
                DT_CENTER,
            );
        }
        k if k == Kind::Toggle as u8 => {
            let (sw, sh) = (px(36.0), px(18.0));
            let sy = y + (h - sh) / 2.0;
            if on {
                cv.fill_round(x + 1.0, sy, sw, sh, sh / 2.0, t.accent);
                cv.fill_circle(
                    x + 1.0 + sw - sh / 2.0,
                    sy + sh / 2.0,
                    sh / 2.0 - px(4.0),
                    t.on_accent,
                );
            } else {
                if hot {
                    cv.fill_round(x + 1.0, sy, sw, sh, sh / 2.0, t.hover);
                }
                cv.stroke_round(x + 1.0, sy, sw, sh, sh / 2.0, t.muted, 1.0);
                cv.fill_circle(
                    x + 1.0 + sh / 2.0,
                    sy + sh / 2.0,
                    sh / 2.0 - px(4.5),
                    t.muted,
                );
            }
            drop(cv);
            let mut trc = RECT {
                left: rc.left + (sw + px(10.0)) as i32,
                ..rc
            };
            text_out(hdc, &label, &mut trc, t.text, font(), DT_LEFT);
        }
        _ => {
            // Disclosure
            drop(cv);
            let color = if hot { t.text } else { t.muted };
            let mut trc = rc;
            text_out(hdc, &label, &mut trc, color, font(), DT_RIGHT);
            let mut size: SIZE = unsafe { std::mem::zeroed() };
            unsafe {
                let old = SelectObject(hdc, font());
                let wl: Vec<u16> = label.encode_utf16().collect();
                GetTextExtentPoint32W(hdc, wl.as_ptr(), wl.len() as i32, &mut size);
                SelectObject(hdc, old);
            }
            // glyph sits left of the label
            let mut grc = RECT {
                right: rc.right - size.cx - px(6.0) as i32,
                ..rc
            };
            let icon = RES.with(|r| r.borrow().icon);
            text_out(
                hdc,
                if on { GLYPH_UP } else { GLYPH_DOWN },
                &mut grc,
                color,
                icon,
                DT_RIGHT,
            );
        }
    }
    if focus && let Some(cv) = Canvas::new(hdc) {
        cv.stroke_round(x, y, w, h, r, t.accent, px(1.5));
    }
}

// ---------- custom-drawn trackbar ----------

const CDDS_PREPAINT: u32 = 1;
const CDDS_ITEMPREPAINT: u32 = 0x10001;
const CDRF_DODEFAULT: LRESULT = 0;
const CDRF_SKIPDEFAULT: LRESULT = 4;
const CDRF_NOTIFYITEMDRAW: LRESULT = 0x20;
const TBCD_THUMB: usize = 2;
const TBCD_CHANNEL: usize = 3;
const TBM_GETTHUMBRECT: u32 = 0x419;
const CDIS_SELECTED: u32 = 1;
const CDIS_HOT: u32 = 0x40;

/// NM_CUSTOMDRAW from a trackbar: thin rail, accent fill, round thumb.
pub fn draw_slider(nm: &NMCUSTOMDRAW) -> LRESULT {
    match nm.dwDrawStage {
        CDDS_PREPAINT => CDRF_NOTIFYITEMDRAW,
        CDDS_ITEMPREPAINT => {
            let t = get();
            let hdc = nm.hdc;
            match nm.dwItemSpec {
                TBCD_CHANNEL => {
                    let mut thumb: RECT = unsafe { std::mem::zeroed() };
                    unsafe {
                        SendMessageW(
                            nm.hdr.hwndFrom,
                            TBM_GETTHUMBRECT,
                            0,
                            &mut thumb as *mut _ as LPARAM,
                        )
                    };
                    let cy = (thumb.top + thumb.bottom) as f32 / 2.0;
                    let cx = (thumb.left + thumb.right) as f32 / 2.0;
                    let rh = px(4.0);
                    let (l, r) = (nm.rc.left as f32 + px(2.0), nm.rc.right as f32 - px(2.0));
                    if let Some(cv) = Canvas::new(hdc) {
                        cv.fill_round(l, cy - rh / 2.0, r - l, rh, rh / 2.0, t.rail);
                        if cx > l {
                            cv.fill_round(l, cy - rh / 2.0, cx - l, rh, rh / 2.0, t.accent);
                        }
                    }
                    CDRF_SKIPDEFAULT
                }
                TBCD_THUMB => {
                    let rc = nm.rc;
                    let cx = (rc.left + rc.right) as f32 / 2.0;
                    let cy = (rc.top + rc.bottom) as f32 / 2.0;
                    let active = nm.uItemState & (CDIS_HOT | CDIS_SELECTED) != 0;
                    if let Some(cv) = Canvas::new(hdc) {
                        let outer = px(9.5);
                        cv.fill_circle(cx, cy, outer, t.thumb);
                        cv.stroke_circle(cx, cy, outer - 0.5, t.line, 1.0);
                        cv.fill_circle(cx, cy, if active { px(6.5) } else { px(5.0) }, t.accent);
                    }
                    CDRF_SKIPDEFAULT
                }
                _ => CDRF_SKIPDEFAULT, // no tick marks
            }
        }
        _ => CDRF_DODEFAULT,
    }
}
