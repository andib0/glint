//! The control panel popup: one column per monitor with sliders and dropdowns
//! for every feature the monitor answers, profiles, and options. Raw VCP access
//! lives behind the "Advanced" disclosure to keep the default view quiet.

use super::theme::{self, Kind};
use super::util::{hinst, wide, window_text};
use crate::config::Config;
use crate::vcp::{self, Kind as FKind};
use crate::worker::{Shared, Worker};
use std::cell::RefCell;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::SIZE;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::Graphics::Gdi::{
    GetDC, GetTextExtentPoint32W, HFONT, ReleaseDC, SelectObject,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetCapture, GetFocus, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

pub const CLASS: &str = "GlintPanel";

// Control styles / messages (numeric to stay independent of windows-sys typing).
const TBM_GETPOS: u32 = 0x400;
const TBM_SETPOS: u32 = 0x405;
const TBM_SETRANGE: u32 = 0x406;
const TBM_SETPAGESIZE: u32 = 0x415;
const TBM_SETTHUMBLENGTH: u32 = 0x41B;
const TBS_NOTICKS: u32 = 0x10;
const TBS_FIXEDLENGTH: u32 = 0x40;
const CBS_DROPDOWNLIST: u32 = 3;
const CB_ADDSTRING: u32 = 0x143;
const CB_GETCURSEL: u32 = 0x147;
const CB_SETCURSEL: u32 = 0x14E;
const CB_SETITEMHEIGHT: u32 = 0x153;
const CBN_SELCHANGE: u32 = 1;
const BS_OWNERDRAW: u32 = 0xB;
const ES_AUTOHSCROLL: u32 = 0x80;
const EM_SETCUEBANNER: u32 = 0x1501;
const SS_RIGHT: u32 = 2;
const SS_NOPREFIX: u32 = 0x80;
const SS_ENDELLIPSIS: u32 = 0x4000;
const IDOK_: u32 = 1;
const IDCANCEL_: u32 = 2;

const ID_LINK: u32 = 10;
const ID_REFRESH: u32 = 11;
const ID_SETTINGS: u32 = 12;
const ID_SAVE: u32 = 13;
const ID_ADVANCED: u32 = 14;
const ID_COMBO: u32 = 1000;
const ID_ADV_GET: u32 = 2000;
const ID_ADV_SET: u32 = 2100;
const ID_RESET: u32 = 2200;
const ID_PROFILE: u32 = 3000;
/// Static controls with ids above this are drawn in the muted text colour.
pub const ID_DIM: u32 = 4000;

// Layout in DIPs.
const M: i32 = 18;
const GAP: i32 = 28;
const COLW: i32 = 300;
const LBL: i32 = 84;
const VALW: i32 = 30;

thread_local! {
    /// Separator lines painted by the panel's WM_PAINT (no App borrow needed).
    static LINES: RefCell<Vec<RECT>> = const { RefCell::new(Vec::new()) };
    /// Rounded frames painted behind borderless text fields.
    static FRAMES: RefCell<Vec<RECT>> = const { RefCell::new(Vec::new()) };
}

pub fn lines() -> Vec<RECT> {
    LINES.with(|l| l.borrow().clone())
}

pub fn frames() -> Vec<RECT> {
    FRAMES.with(|l| l.borrow().clone())
}

pub enum Cmd {
    Hide,
    Refresh,
    Settings,
    SaveProfile(String),
    ApplyProfile(String),
    LinkChanged(bool),
    ToggleAdvanced,
    ConfirmInput {
        id: String,
        mon_no: usize,
        value: u32,
        label: String,
    },
}

struct Slider {
    hwnd: HWND,
    val: HWND,
    mon: String,
    code: u8,
}

struct Combo {
    hwnd: HWND,
    mon: String,
    mon_no: usize,
    code: u8,
    values: Vec<u8>,
}

struct Adv {
    code: HWND,
    val: HWND,
    mon: String,
}

/// Rebuild the panel whenever this changes.
pub type LayoutKey = (u64, Vec<(String, Vec<u8>)>, Vec<String>, bool, bool);

pub fn layout_key(s: &Shared, cfg: &Config, advanced: bool) -> LayoutKey {
    let mons = s
        .monitors
        .iter()
        .map(|m| {
            let codes = vcp::FEATURES
                .iter()
                .filter(|f| s.values.contains_key(&(m.id.clone(), f.code)))
                .map(|f| f.code)
                .collect();
            (m.id.clone(), codes)
        })
        .collect();
    (
        s.generation,
        mons,
        cfg.profiles.iter().map(|p| p.name.clone()).collect(),
        s.ready,
        advanced,
    )
}

pub struct Flyout {
    pub hwnd: HWND,
    pub key: LayoutKey,
    sliders: Vec<Slider>,
    combos: Vec<Combo>,
    advs: Vec<Adv>,
    adv_pending: Option<(usize, u8)>,
    link: HWND,
    save_edit: HWND,
    status: HWND,
    status_note: Option<String>,
    profiles: Vec<String>,
    width: i32,
    height: i32,
}

fn px(v: i32) -> i32 {
    theme::px(v as f32).round() as i32
}

struct Builder {
    parent: HWND,
    dim_next: u32,
}

impl Builder {
    #[allow(clippy::too_many_arguments)]
    fn ctl(
        &self,
        class: &str,
        text: &str,
        style: u32,
        ex: u32,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        id: u32,
    ) -> HWND {
        unsafe {
            let hwnd = CreateWindowExW(
                ex,
                wide(class).as_ptr(),
                wide(text).as_ptr(),
                WS_CHILD | WS_VISIBLE | style,
                px(x),
                px(y),
                px(w),
                px(h),
                self.parent,
                id as usize as HMENU,
                hinst(),
                null(),
            );
            SendMessageW(hwnd, WM_SETFONT, theme::font() as usize, 1);
            hwnd
        }
    }

    fn label(&mut self, text: &str, x: i32, y: i32, w: i32, muted: bool, right: bool) -> HWND {
        let id = if muted {
            self.dim_next += 1;
            self.dim_next
        } else {
            0
        };
        let align = if right { SS_RIGHT } else { 0 };
        self.ctl(
            "STATIC",
            text,
            SS_NOPREFIX | SS_ENDELLIPSIS | align,
            0,
            x,
            y,
            w,
            18,
            id,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn button(
        &self,
        kind: Kind,
        on: bool,
        text: &str,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        id: u32,
    ) -> HWND {
        let b = self.ctl("BUTTON", text, WS_TABSTOP | BS_OWNERDRAW, 0, x, y, w, h, id);
        theme::make_button(b, kind, on);
        b
    }

    /// Borderless edit inside a painted rounded frame (28 DIPs tall, like buttons).
    fn edit(&self, cue: &str, x: i32, y: i32, w: i32) -> HWND {
        FRAMES.with(|f| {
            f.borrow_mut().push(RECT {
                left: px(x),
                top: px(y),
                right: px(x + w),
                bottom: px(y + 28),
            })
        });
        let h = self.ctl(
            "EDIT",
            "",
            WS_TABSTOP | ES_AUTOHSCROLL,
            0,
            x + 8,
            y + 6,
            w - 16,
            17,
            0,
        );
        let cue = wide(cue);
        unsafe { SendMessageW(h, EM_SETCUEBANNER, 0, cue.as_ptr() as isize) };
        h
    }
}

/// Width in DIPs of `text` in `font`.
fn text_width(text: &str, font: HFONT) -> i32 {
    unsafe {
        let dc = GetDC(null_mut());
        let old = SelectObject(dc, font);
        let w: Vec<u16> = text.encode_utf16().collect();
        let mut size: SIZE = std::mem::zeroed();
        GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32, &mut size);
        SelectObject(dc, old);
        ReleaseDC(null_mut(), dc);
        (size.cx as f32 / theme::px(1.0)).ceil() as i32
    }
}

pub fn register(proc_: WNDPROC) {
    unsafe {
        let class = wide(CLASS);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DROPSHADOW,
            lpfnWndProc: proc_,
            hInstance: hinst(),
            lpszClassName: class.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_ARROW),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
    }
}

impl Flyout {
    pub fn create(s: &Shared, cfg: &Config, advanced: bool, bold: HFONT) -> Flyout {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_CONTROLPARENT,
                wide(CLASS).as_ptr(),
                wide("Glint").as_ptr(),
                WS_POPUP | WS_CLIPCHILDREN,
                0,
                0,
                100,
                100,
                null_mut(),
                null_mut(),
                hinst(),
                null(),
            )
        };
        theme::style_window(hwnd);
        FRAMES.with(|f| f.borrow_mut().clear());
        let mut b = Builder {
            parent: hwnd,
            dim_next: ID_DIM,
        };
        let mut f = Flyout {
            hwnd,
            key: layout_key(s, cfg, advanced),
            sliders: Vec::new(),
            combos: Vec::new(),
            advs: Vec::new(),
            adv_pending: None,
            link: null_mut(),
            save_edit: null_mut(),
            status: null_mut(),
            status_note: None,
            profiles: cfg.profiles.iter().map(|p| p.name.clone()).collect(),
            width: 0,
            height: 0,
        };
        let mut lines = Vec::new();

        let ncols = s.monitors.len().max(1) as i32;
        let width = 2 * M + ncols * COLW + (ncols - 1) * GAP;
        let mut col_bottom = M;

        if s.monitors.is_empty() {
            let msg = if s.ready {
                "No DDC/CI monitors found. Turn on DDC/CI in your monitor's menu."
            } else {
                "Looking for monitors…"
            };
            b.label(msg, M, M, COLW, true, false);
            col_bottom = M + 26;
        }

        for (mi, m) in s.monitors.iter().enumerate() {
            let x0 = M + mi as i32 * (COLW + GAP);
            if mi > 0 {
                let sx = x0 - GAP / 2;
                lines.push(RECT {
                    left: px(sx),
                    top: px(M),
                    right: px(sx) + 1,
                    bottom: 0,
                });
            }
            let mut y = M;
            let head = b.label(
                &format!("Monitor {}", mi + 1),
                x0,
                y,
                COLW - 120,
                false,
                false,
            );
            unsafe { SendMessageW(head, WM_SETFONT, bold as usize, 1) };
            if let Some((v, _)) = s.value(&m.id, vcp::INPUT) {
                b.label(
                    &vcp::choice_label(vcp::INPUT, v & 0xFF),
                    x0 + COLW - 120,
                    y,
                    120,
                    true,
                    true,
                );
            }
            y += 20;
            let sub = format!(
                "{}  ·  {}×{}{}",
                m.model(),
                m.width,
                m.height,
                if m.primary { "  ·  primary" } else { "" }
            );
            b.label(&sub, x0, y, COLW, true, false);
            y += 30;

            if !s.ready
                && !vcp::FEATURES
                    .iter()
                    .any(|ft| s.value(&m.id, ft.code).is_some())
            {
                b.label("Reading settings…", x0, y, COLW, true, false);
                y += 26;
            }

            let mut had_slider = false;
            for ft in vcp::FEATURES {
                let Some((cur, max)) = s.value(&m.id, ft.code) else {
                    continue;
                };
                match ft.kind {
                    FKind::Continuous => {
                        had_slider = true;
                        b.label(ft.label, x0, y + 6, LBL, false, false);
                        let tb = b.ctl(
                            "msctls_trackbar32",
                            "",
                            WS_TABSTOP | TBS_NOTICKS | TBS_FIXEDLENGTH,
                            0,
                            x0 + LBL - 4,
                            y,
                            COLW - LBL - VALW,
                            30,
                            0,
                        );
                        unsafe {
                            SendMessageW(tb, TBM_SETTHUMBLENGTH, px(20) as usize, 0);
                            SendMessageW(tb, TBM_SETRANGE, 1, (max.max(1) as isize) << 16);
                            SendMessageW(tb, TBM_SETPAGESIZE, 0, 10);
                            SendMessageW(tb, TBM_SETPOS, 1, cur as isize);
                        }
                        let val =
                            b.label(&cur.to_string(), x0 + COLW - VALW, y + 6, VALW, true, true);
                        f.sliders.push(Slider {
                            hwnd: tb,
                            val,
                            mon: m.id.clone(),
                            code: ft.code,
                        });
                        y += 32;
                    }
                    FKind::Choice(_) => {
                        if had_slider {
                            y += 10;
                            had_slider = false;
                        }
                        b.label(ft.label, x0, y + 5, LBL, false, false);
                        let idx = f.combos.len() as u32;
                        let cb = b.ctl(
                            "COMBOBOX",
                            "",
                            WS_TABSTOP | WS_VSCROLL | CBS_DROPDOWNLIST,
                            0,
                            x0 + LBL,
                            y,
                            COLW - LBL,
                            320,
                            ID_COMBO + idx,
                        );
                        theme::style_native(cb);
                        unsafe { SendMessageW(cb, CB_SETITEMHEIGHT, usize::MAX, px(22) as isize) };
                        let mut choices = vcp::choices_for(ft, Some(&m.caps));
                        let curv = (cur & 0xFF) as u8;
                        if !choices.iter().any(|c| c.0 == curv) {
                            choices.push((curv, vcp::choice_label(ft.code, u32::from(curv))));
                        }
                        for (_, name) in &choices {
                            let w = wide(name);
                            unsafe { SendMessageW(cb, CB_ADDSTRING, 0, w.as_ptr() as isize) };
                        }
                        let sel = choices
                            .iter()
                            .position(|c| c.0 == curv)
                            .map(|p| p as isize)
                            .unwrap_or(-1);
                        unsafe { SendMessageW(cb, CB_SETCURSEL, sel as usize, 0) };
                        f.combos.push(Combo {
                            hwnd: cb,
                            mon: m.id.clone(),
                            mon_no: mi + 1,
                            code: ft.code,
                            values: choices.iter().map(|c| c.0).collect(),
                        });
                        y += 36;
                    }
                }
            }

            if advanced {
                y += 8;
                b.label("VCP code", x0, y + 5, LBL, false, false);
                let code = b.edit("0x87", x0 + LBL, y, 56);
                let val = b.edit("value", x0 + LBL + 62, y, 56);
                b.button(
                    Kind::Button,
                    false,
                    "Get",
                    x0 + LBL + 124,
                    y,
                    44,
                    28,
                    ID_ADV_GET + mi as u32,
                );
                b.button(
                    Kind::Button,
                    false,
                    "Set",
                    x0 + LBL + 172,
                    y,
                    44,
                    28,
                    ID_ADV_SET + mi as u32,
                );
                f.advs.push(Adv {
                    code,
                    val,
                    mon: m.id.clone(),
                });
                y += 34;
                if m.caps.supports(vcp::RESTORE_COLOR) || m.caps.is_empty() {
                    b.button(
                        Kind::Button,
                        false,
                        "Reset colors",
                        x0 + LBL,
                        y,
                        110,
                        28,
                        ID_RESET + mi as u32,
                    );
                    y += 34;
                }
                b.label(
                    &format!("Id {}", m.short_id()),
                    x0 + LBL,
                    y,
                    COLW - LBL,
                    true,
                    false,
                );
                y += 22;
            }
            col_bottom = col_bottom.max(y);
        }
        for l in lines.iter_mut() {
            l.bottom = px(col_bottom - 6);
        }

        // Footer: profiles, then options.
        let mut y = col_bottom + 8;
        lines.push(RECT {
            left: 0,
            top: px(y),
            right: px(width),
            bottom: px(y) + 1,
        });
        y += 16;

        let mut x = M;
        for (i, name) in f.profiles.iter().enumerate() {
            let w = (text_width(name, theme::font()) + 28).max(60);
            if x + w > width - M {
                x = M;
                y += 36;
            }
            b.button(
                Kind::Button,
                false,
                name,
                x,
                y,
                w,
                28,
                ID_PROFILE + i as u32,
            );
            x += w + 8;
        }
        let save_w = 150 + 8 + 64;
        if x + save_w > width - M {
            x = M;
            y += 36;
        } else {
            x = width - M - save_w;
        }
        f.save_edit = b.edit("New profile name", x, y, 150);
        b.button(Kind::Button, false, "Save", x + 158, y, 64, 28, ID_SAVE);
        y += 44;

        f.link = b.button(
            Kind::Toggle,
            cfg.link_brightness,
            "Link brightness",
            M,
            y,
            180,
            28,
            ID_LINK,
        );
        let adv_w = text_width("Advanced", theme::font()) + 30;
        b.button(
            Kind::Icon,
            false,
            theme::GLYPH_SETTINGS,
            width - M - 32,
            y,
            32,
            28,
            ID_SETTINGS,
        );
        b.button(
            Kind::Icon,
            false,
            theme::GLYPH_REFRESH,
            width - M - 68,
            y,
            32,
            28,
            ID_REFRESH,
        );
        b.button(
            Kind::Disclosure,
            advanced,
            "Advanced",
            width - M - 76 - adv_w,
            y,
            adv_w,
            28,
            ID_ADVANCED,
        );
        y += 32;
        f.status = b.label("", M, y + 4, width - 2 * M, true, false);
        y += 24;

        LINES.with(|l| *l.borrow_mut() = lines);
        f.width = px(width);
        f.height = px(y + M - 10);
        unsafe {
            // hide focus rectangles until the keyboard is used
            SendMessageW(
                hwnd,
                WM_CHANGEUISTATE,
                (UIS_SET | (UISF_HIDEFOCUS << 16)) as usize,
                0,
            );
        }
        f.update(s);
        f
    }

    pub fn destroy(self) {
        unsafe { DestroyWindow(self.hwnd) };
    }

    pub fn visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd) != 0 }
    }

    /// Bottom-right of the primary work area (next to the tray).
    pub fn place(&self) {
        unsafe {
            let mut wa: RECT = std::mem::zeroed();
            SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut wa as *mut _ as *mut _, 0);
            let gap = px(12);
            let x = (wa.right - self.width - gap).max(wa.left);
            let y = (wa.bottom - self.height - gap).max(wa.top);
            SetWindowPos(
                self.hwnd,
                HWND_TOPMOST,
                x,
                y,
                self.width,
                self.height,
                SWP_NOACTIVATE,
            );
        }
    }

    /// Sync controls with the cache (skips a slider the user is dragging).
    pub fn update(&mut self, s: &Shared) {
        unsafe {
            let capture = GetCapture();
            for sl in &self.sliders {
                if sl.hwnd == capture {
                    continue;
                }
                if let Some((cur, _)) = s.value(&sl.mon, sl.code) {
                    if SendMessageW(sl.hwnd, TBM_GETPOS, 0, 0) as u32 != cur {
                        SendMessageW(sl.hwnd, TBM_SETPOS, 1, cur as isize);
                    }
                    set_text(sl.val, &cur.to_string());
                }
            }
            for c in &self.combos {
                if let Some((cur, _)) = s.value(&c.mon, c.code) {
                    let want = c
                        .values
                        .iter()
                        .position(|&v| u32::from(v) == cur & 0xFF)
                        .map(|p| p as isize)
                        .unwrap_or(-1);
                    if SendMessageW(c.hwnd, CB_GETCURSEL, 0, 0) != want {
                        SendMessageW(c.hwnd, CB_SETCURSEL, want as usize, 0);
                    }
                }
            }
        }
        if let Some((i, code)) = self.adv_pending
            && let Some(r) = s.adhoc.get(&(self.advs[i].mon.clone(), code))
        {
            match r {
                Ok((cur, max)) => {
                    set_text(self.advs[i].val, &cur.to_string());
                    self.status_note = Some(format!(
                        "Monitor {}: VCP 0x{code:02X} = {cur} (max {max})",
                        i + 1
                    ));
                }
                Err(e) => {
                    set_text(self.advs[i].val, "");
                    self.status_note = Some(format!("Monitor {}: {e}", i + 1));
                }
            }
            self.adv_pending = None;
        }
        let status = match (&s.last_error, &self.status_note) {
            (Some(e), _) => e.clone(),
            (None, Some(n)) => n.clone(),
            (None, None) if !s.ready => "Reading monitors…".into(),
            _ => String::new(),
        };
        set_text(self.status, &status);
    }

    pub fn on_hscroll(&mut self, ctl: HWND, worker: &Worker, link: bool) {
        let Some(i) = self.sliders.iter().position(|s| s.hwnd == ctl) else {
            return;
        };
        let pos = unsafe { SendMessageW(ctl, TBM_GETPOS, 0, 0) } as u32;
        let (mon, code) = (self.sliders[i].mon.clone(), self.sliders[i].code);
        set_text(self.sliders[i].val, &pos.to_string());
        send_if_changed(worker, &mon, code, pos);
        if link && code == vcp::BRIGHTNESS {
            for other in self
                .sliders
                .iter()
                .filter(|s| s.code == vcp::BRIGHTNESS && s.hwnd != ctl)
            {
                unsafe { SendMessageW(other.hwnd, TBM_SETPOS, 1, pos as isize) };
                set_text(other.val, &pos.to_string());
                send_if_changed(worker, &other.mon, code, pos);
            }
        }
    }

    pub fn on_command(&mut self, id: u32, notify: u32, worker: &Worker) -> Option<Cmd> {
        match id {
            IDCANCEL_ => Some(Cmd::Hide),
            IDOK_ if unsafe { GetFocus() } == self.save_edit => self.save_cmd(),
            ID_LINK => {
                let on = !theme::is_on(self.link);
                theme::set_on(self.link, on);
                Some(Cmd::LinkChanged(on))
            }
            ID_ADVANCED => Some(Cmd::ToggleAdvanced),
            ID_REFRESH => {
                self.status_note = None;
                Some(Cmd::Refresh)
            }
            ID_SETTINGS => Some(Cmd::Settings),
            ID_SAVE => self.save_cmd(),
            _ if (ID_PROFILE..ID_PROFILE + 100).contains(&id) => self
                .profiles
                .get((id - ID_PROFILE) as usize)
                .map(|n| Cmd::ApplyProfile(n.clone())),
            _ if (ID_COMBO..ID_COMBO + 100).contains(&id) && notify == CBN_SELCHANGE => {
                let c = self.combos.get((id - ID_COMBO) as usize)?;
                let sel = unsafe { SendMessageW(c.hwnd, CB_GETCURSEL, 0, 0) };
                let v = u32::from(*c.values.get(usize::try_from(sel).ok()?)?);
                if c.code == vcp::INPUT {
                    return Some(Cmd::ConfirmInput {
                        id: c.mon.clone(),
                        mon_no: c.mon_no,
                        value: v,
                        label: vcp::choice_label(vcp::INPUT, v),
                    });
                }
                worker.set(&c.mon, c.code, v);
                None
            }
            _ if (ID_ADV_GET..ID_ADV_GET + 100).contains(&id) => {
                let i = (id - ID_ADV_GET) as usize;
                let adv = self.advs.get(i)?;
                match vcp::parse_code(&window_text(adv.code)) {
                    Some(code) => {
                        worker.read(&adv.mon, code);
                        self.adv_pending = Some((i, code));
                        self.note(format!("Reading VCP 0x{code:02X}…"));
                    }
                    None => self.note("Enter a VCP code like 0x87, or a name like contrast".into()),
                }
                None
            }
            _ if (ID_ADV_SET..ID_ADV_SET + 100).contains(&id) => {
                let i = (id - ID_ADV_SET) as usize;
                let adv = self.advs.get(i)?;
                let code = vcp::parse_code(&window_text(adv.code));
                let val = code.and_then(|c| vcp::parse_value(c, &window_text(adv.val)));
                match (code, val) {
                    (Some(code), Some(v)) => {
                        worker.set(&adv.mon, code, v);
                        self.note(format!("Monitor {}: VCP 0x{code:02X} set to {v}", i + 1));
                    }
                    _ => self.note("Enter a VCP code and a value".into()),
                }
                None
            }
            _ if (ID_RESET..ID_RESET + 100).contains(&id) => {
                let mon = self.advs.get((id - ID_RESET) as usize)?.mon.clone();
                worker.set(&mon, vcp::RESTORE_COLOR, 1);
                worker.refresh(false);
                self.note("Colour settings restored to factory defaults".into());
                None
            }
            _ => None,
        }
    }

    fn note(&mut self, s: String) {
        set_text(self.status, &s);
        self.status_note = Some(s);
    }

    fn save_cmd(&mut self) -> Option<Cmd> {
        let name = window_text(self.save_edit).trim().to_string();
        if name.is_empty() {
            self.note("Type a name for the profile first".into());
            unsafe { SetFocus(self.save_edit) };
            return None;
        }
        set_text(self.save_edit, "");
        Some(Cmd::SaveProfile(name))
    }
}

fn send_if_changed(worker: &Worker, mon: &str, code: u8, v: u32) {
    let cached = worker.lock().value(mon, code).map(|x| x.0);
    if cached != Some(v) {
        worker.set(mon, code, v);
    }
}

fn set_text(hwnd: HWND, s: &str) {
    if !hwnd.is_null() {
        unsafe { SetWindowTextW(hwnd, wide(s).as_ptr()) };
    }
}

/// Text colour (0xRRGGBB) for a static control id.
pub fn static_text_color(ctl_id: u32) -> u32 {
    let t = theme::get();
    if ctl_id > ID_DIM { t.muted } else { t.text }
}
