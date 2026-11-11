//! Tray application: owns the worker, config, hotkeys, schedule and the panel.
//!
//! Re-entrancy rule: window procedures reach the App through `with_app`, which
//! uses `try_borrow_mut`. Anything that runs a modal loop (menus, message
//! boxes) happens *outside* the borrow via deferred commands.

use super::flyout::{self, Flyout};
use super::util::{hinst, hiword, log, loword, wide};
use super::{autostart, gfx, icon, osd, theme};
use crate::backend::MonitorInfo;
use crate::config::{self, Action, Config};
use crate::ddc_win::WinBackend;
use crate::worker::{Event, Worker};
use crate::{hotkey, profile, schedule, vcp};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::time::{Instant, SystemTime};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, GetMonitorInfoW, HDC, HFONT,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MONITORINFOEXW, MonitorFromPoint, PAINTSTRUCT,
    SetBkColor, SetTextColor,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Controls::{
    DRAWITEMSTRUCT, ICC_BAR_CLASSES, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX,
    InitCommonControlsEx, NMCUSTOMDRAW, NMHDR,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NIN_SELECT, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

const MAIN_CLASS: &str = "GlintMain";
const WM_TRAY: u32 = WM_APP + 1;
const WM_WORKER: u32 = WM_APP + 2;
const WM_SHOW_PANEL: u32 = WM_APP + 3;
const TIMER_TICK: usize = 1;
const TIMER_SCREENS_OFF: usize = 2;
const TIMER_REENUM: usize = 3;
const TRAY_ID: u32 = 1;
const MOD_NOREPEAT: u32 = 0x4000;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    static PANEL: Cell<HWND> = const { Cell::new(null_mut()) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

struct App {
    hwnd: HWND,
    worker: Worker,
    cfg: Config,
    cfg_path: PathBuf,
    cfg_mtime: Option<SystemTime>,
    panel: Option<Flyout>,
    icon: HICON,
    bold: HFONT,
    advanced: bool,
    last_hide: Instant,
    last_schedule: Option<usize>,
    schedule_started: bool,
    hotkeys: Vec<(i32, Action)>,
}

pub fn run(args: Vec<String>) -> i32 {
    unsafe {
        let mutex = CreateMutexW(null(), 1, wide("Local\\GlintMonitorControl").as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let other = FindWindowW(wide(MAIN_CLASS).as_ptr(), null());
            if !other.is_null() {
                PostMessageW(other, WM_SHOW_PANEL, 0, 0);
            }
            return 0;
        }
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_BAR_CLASSES | ICC_STANDARD_CLASSES,
        };
        InitCommonControlsEx(&icc);
        let class = wide(MAIN_CLASS);
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(main_proc),
            hInstance: hinst(),
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);
        flyout::register(Some(panel_proc));
        TASKBAR_CREATED.set(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()));
        let hwnd = CreateWindowExW(
            0,
            class.as_ptr(),
            wide("Glint").as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinst(),
            null(),
        );
        let cfg_path = config::default_path();
        let (cfg, cfg_err) = match config::load_or_create(&cfg_path) {
            Ok(c) => (c, None),
            Err(e) => (Config::builtin(), Some(e)),
        };
        gfx::startup();
        autostart::migrate();
        theme::apply(&cfg.theme);
        let (big, _font) = theme::init_fonts();
        osd::init(big);
        let cfg_mtime = std::fs::metadata(&cfg_path).and_then(|m| m.modified()).ok();

        let hw = hwnd as isize;
        let backend = WinBackend::new(Some(config::config_dir().join("caps.toml")));
        let worker = Worker::spawn(Box::new(backend), move |ev| {
            let code = match ev {
                Event::Monitors => 1,
                Event::Values => 2,
                Event::Error => 3,
            };
            PostMessageW(hw as HWND, WM_WORKER, code, 0);
        });
        let app = App {
            hwnd,
            worker,
            cfg,
            cfg_path,
            cfg_mtime,
            panel: None,
            icon: icon::make(GetSystemMetrics(SM_CXSMICON).max(16)),
            bold: theme::bold(),
            advanced: false,
            last_hide: Instant::now() - std::time::Duration::from_secs(5),
            last_schedule: None,
            schedule_started: false,
            hotkeys: Vec::new(),
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        with_app(|a| {
            a.tray_add();
            a.register_hotkeys();
        });
        SetTimer(hwnd, TIMER_TICK, 5000, None);
        if let Some(e) = cfg_err {
            log(&format!("config error: {e}"));
            message_box(
                &format!("Config file has an error, using defaults:\n\n{e}"),
                MB_ICONWARNING,
            );
        }
        if args.iter().any(|a| a == "--show") {
            PostMessageW(hwnd, WM_SHOW_PANEL, 0, 0);
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            let panel = PANEL.get();
            if !panel.is_null() && IsDialogMessageW(panel, &msg) != 0 {
                continue;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        with_app(|a| a.tray_remove());
        APP.with(|a| a.borrow_mut().take()); // joins the worker thread
        CloseHandle(mutex);
        0
    }
}

fn message_box(text: &str, flags: MESSAGEBOX_STYLE) -> MESSAGEBOX_RESULT {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(text).as_ptr(),
            wide("Glint").as_ptr(),
            flags | MB_TOPMOST | MB_SETFOREGROUND,
        )
    }
}

fn local_minutes() -> u32 {
    let mut t = unsafe { std::mem::zeroed() };
    unsafe { GetLocalTime(&mut t) };
    u32::from(t.wHour) * 60 + u32::from(t.wMinute)
}

fn open_in_notepad(path: &std::path::Path) {
    let arg = wide(&format!("\"{}\"", path.display()));
    unsafe {
        ShellExecuteW(
            null_mut(),
            wide("open").as_ptr(),
            wide("notepad.exe").as_ptr(),
            arg.as_ptr(),
            null(),
            SW_SHOWNORMAL,
        );
    }
}

impl App {
    // ---------- tray ----------
    fn nid(&self) -> NOTIFYICONDATAW {
        let mut n: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        n.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        n.hWnd = self.hwnd;
        n.uID = TRAY_ID;
        n
    }

    fn fill_tip(&self, n: &mut NOTIFYICONDATAW) {
        let tip = self.tip_text();
        for (dst, src) in n.szTip.iter_mut().zip(tip.encode_utf16().take(127)) {
            *dst = src;
        }
    }

    fn tip_text(&self) -> String {
        let s = self.worker.lock();
        let mut t = String::from("Glint");
        for (i, m) in s.monitors.iter().enumerate() {
            if let Some((v, _)) = s.value(&m.id, vcp::BRIGHTNESS) {
                t.push_str(&format!("\nMonitor {}: {v}%", i + 1));
            }
        }
        t
    }

    fn tray_add(&self) {
        let mut n = self.nid();
        n.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        n.uCallbackMessage = WM_TRAY;
        n.hIcon = self.icon;
        self.fill_tip(&mut n);
        unsafe {
            Shell_NotifyIconW(NIM_ADD, &n);
            n.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            Shell_NotifyIconW(NIM_SETVERSION, &n);
        }
    }

    fn tray_tip(&self) {
        let mut n = self.nid();
        n.uFlags = NIF_TIP | NIF_SHOWTIP;
        self.fill_tip(&mut n);
        unsafe { Shell_NotifyIconW(NIM_MODIFY, &n) };
    }

    fn tray_remove(&self) {
        let n = self.nid();
        unsafe { Shell_NotifyIconW(NIM_DELETE, &n) };
    }

    // ---------- config / hotkeys / schedule ----------
    fn save_config(&mut self) {
        if let Err(e) = config::save(&self.cfg_path, &self.cfg) {
            log(&format!("save config: {e}"));
            osd::show("Could not save config", None);
        }
        self.cfg_mtime = std::fs::metadata(&self.cfg_path)
            .and_then(|m| m.modified())
            .ok();
    }

    fn check_config(&mut self) {
        let mtime = std::fs::metadata(&self.cfg_path)
            .and_then(|m| m.modified())
            .ok();
        if mtime == self.cfg_mtime {
            return;
        }
        self.cfg_mtime = mtime;
        match config::load_or_create(&self.cfg_path) {
            Ok(c) if c != self.cfg => {
                self.cfg = c;
                theme::apply(&self.cfg.theme);
                self.rebuild_panel();
                self.register_hotkeys();
                self.last_schedule = None;
                self.refresh_panel();
                log("config reloaded");
            }
            Ok(_) => {}
            Err(e) => {
                log(&format!("config error: {e}"));
                osd::show("Config error – see glint.log", None);
            }
        }
    }

    fn register_hotkeys(&mut self) {
        for (id, _) in self.hotkeys.drain(..) {
            unsafe { UnregisterHotKey(self.hwnd, id) };
        }
        let mut errs = Vec::new();
        for (i, h) in self.cfg.hotkeys.iter().enumerate() {
            let Some(act) = Action::parse(&h.action) else {
                errs.push(format!("unknown action '{}'", h.action));
                continue;
            };
            let (mods, vk) = match hotkey::parse(&h.keys) {
                Ok(x) => x,
                Err(e) => {
                    errs.push(e);
                    continue;
                }
            };
            let repeat = matches!(act, Action::BrightnessUp | Action::BrightnessDown);
            let id = i as i32 + 1;
            let mods = mods | if repeat { 0 } else { MOD_NOREPEAT };
            if unsafe { RegisterHotKey(self.hwnd, id, mods, vk) } == 0 {
                errs.push(format!("{} is already used by another app", h.keys));
                continue;
            }
            self.hotkeys.push((id, act));
        }
        for e in &errs {
            log(&format!("hotkey: {e}"));
        }
        if let Some(e) = errs.first() {
            osd::show(&format!("Hotkey: {e}"), None);
        }
    }

    fn check_schedule(&mut self, startup: bool) {
        let times: Vec<Option<u32>> = self
            .cfg
            .schedule
            .iter()
            .map(|e| schedule::parse_hhmm(&e.time))
            .collect();
        let idx = schedule::active_index(&times, local_minutes());
        if idx == self.last_schedule {
            return;
        }
        let first = self.last_schedule.is_none() && startup;
        self.last_schedule = idx;
        let Some(i) = idx else { return };
        if first && !self.cfg.apply_schedule_on_start {
            return;
        }
        let name = self.cfg.schedule[i].profile.clone();
        log(&format!(
            "schedule: {} -> {name}",
            self.cfg.schedule[i].time
        ));
        self.apply_profile(&name);
    }

    // ---------- actions ----------
    fn run_action(&mut self, act: &Action) {
        match act {
            Action::BrightnessUp => self.step_brightness(i64::from(self.cfg.step)),
            Action::BrightnessDown => self.step_brightness(-i64::from(self.cfg.step)),
            Action::Profile(n) => self.apply_profile(n),
            Action::ScreensOff => unsafe {
                // short delay so releasing the hotkey doesn't wake the screens
                SetTimer(self.hwnd, TIMER_SCREENS_OFF, 700, None);
            },
            Action::TogglePanel => self.toggle_panel(),
        }
    }

    fn monitor_under_cursor(&self, mons: &[MonitorInfo]) -> Option<String> {
        unsafe {
            let mut pt = POINT { x: 0, y: 0 };
            GetCursorPos(&mut pt);
            let hm = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let mut mi: MONITORINFOEXW = std::mem::zeroed();
            mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(hm, &mut mi as *mut _ as *mut MONITORINFO) == 0 {
                return None;
            }
            let n = mi.szDevice.iter().position(|&c| c == 0).unwrap_or(32);
            let dev = String::from_utf16_lossy(&mi.szDevice[..n]);
            mons.iter()
                .find(|m| m.device.eq_ignore_ascii_case(&dev))
                .map(|m| m.id.clone())
        }
    }

    fn step_brightness(&mut self, delta: i64) {
        let targets: Vec<(String, u32, u32)> = {
            let s = self.worker.lock();
            let only = if self.cfg.link_brightness {
                None
            } else {
                self.monitor_under_cursor(&s.monitors)
            };
            s.monitors
                .iter()
                .filter(|m| only.as_ref().is_none_or(|id| &m.id == id))
                .filter_map(|m| {
                    s.value(&m.id, vcp::BRIGHTNESS)
                        .map(|(c, x)| (m.id.clone(), c, x))
                })
                .collect()
        };
        let Some(&(_, c0, m0)) = targets.first() else {
            osd::show("No monitor reports brightness yet", None);
            return;
        };
        let lead = (i64::from(c0) + delta).clamp(0, i64::from(m0)) as u32;
        for (id, cur, max) in &targets {
            // linked: everyone follows the first monitor; otherwise step each
            let n = if self.cfg.link_brightness {
                lead.min(*max)
            } else {
                (i64::from(*cur) + delta).clamp(0, i64::from(*max)) as u32
            };
            self.worker.set(id, vcp::BRIGHTNESS, n);
        }
        if self.cfg.show_osd {
            osd::show(
                &format!("☀  Brightness  {lead}"),
                Some(lead as f32 / m0.max(1) as f32),
            );
        }
        self.refresh_panel();
        self.tray_tip();
    }

    fn set_all_brightness(&mut self, pct: u32) {
        let targets: Vec<(String, u32)> = {
            let s = self.worker.lock();
            s.monitors
                .iter()
                .filter_map(|m| {
                    s.value(&m.id, vcp::BRIGHTNESS)
                        .map(|(_, x)| (m.id.clone(), x))
                })
                .collect()
        };
        for (id, max) in targets {
            self.worker.set(&id, vcp::BRIGHTNESS, pct * max / 100);
        }
        self.refresh_panel();
        self.tray_tip();
    }

    fn apply_profile(&mut self, name: &str) {
        let Some(p) = self.cfg.profile(name).cloned() else {
            osd::show(&format!("No profile '{name}'"), None);
            return;
        };
        let mons = self.worker.lock().monitors.clone();
        let (writes, warnings) = profile::resolve(&p, &mons);
        for w in &warnings {
            log(w);
        }
        for (id, code, v) in writes {
            self.worker.set(&id, code, v);
        }
        if self.cfg.show_osd {
            osd::show(&format!("Profile  ·  {}", p.name), None);
        }
        self.refresh_panel();
        self.tray_tip();
    }

    // ---------- panel ----------
    fn panel_visible(&self) -> bool {
        self.panel.as_ref().is_some_and(|p| p.visible())
    }

    /// Rebuild the panel if monitors/features/profiles changed. Returns true if rebuilt.
    fn ensure_panel(&mut self) -> bool {
        let key = flyout::layout_key(&self.worker.lock(), &self.cfg, self.advanced);
        if self.panel.as_ref().is_some_and(|p| p.key == key) {
            return false;
        }
        let was_visible = self.panel_visible();
        if let Some(old) = self.panel.take() {
            PANEL.set(null_mut());
            old.destroy();
        }
        let p = Flyout::create(&self.worker.lock(), &self.cfg, self.advanced, self.bold);
        PANEL.set(p.hwnd);
        p.place();
        self.panel = Some(p);
        if was_visible {
            self.show_window();
        }
        true
    }

    /// Throw the panel away (theme change); rebuilt now if it was open, else on next show.
    fn rebuild_panel(&mut self) {
        let was_visible = self.panel_visible();
        if let Some(old) = self.panel.take() {
            PANEL.set(null_mut());
            old.destroy();
        }
        if was_visible {
            self.ensure_panel();
            self.show_window();
        }
    }

    fn show_window(&self) {
        if let Some(p) = &self.panel {
            p.place();
            unsafe {
                ShowWindow(p.hwnd, SW_SHOW);
                SetForegroundWindow(p.hwnd);
            }
        }
    }

    fn show_panel(&mut self) {
        self.ensure_panel();
        if let Some(p) = self.panel.as_mut() {
            p.update(&self.worker.lock());
        }
        self.show_window();
        self.worker.refresh(false);
    }

    fn hide_panel(&mut self) {
        if self.panel_visible() {
            if let Some(p) = &self.panel {
                unsafe { ShowWindow(p.hwnd, SW_HIDE) };
            }
            self.last_hide = Instant::now();
        }
    }

    fn toggle_panel(&mut self) {
        if self.panel_visible() {
            self.hide_panel();
        } else if self.last_hide.elapsed().as_millis() > 300 {
            // a click on the tray icon first deactivates (hides) the panel; don't reopen it
            self.show_panel();
        }
    }

    fn refresh_panel(&mut self) {
        if self.panel.is_none() {
            return;
        }
        if !self.ensure_panel()
            && let Some(p) = self.panel.as_mut()
        {
            p.update(&self.worker.lock());
        }
    }

    fn on_worker(&mut self, ev: usize) {
        if !self.schedule_started && self.worker.lock().generation > 0 {
            self.schedule_started = true;
            self.check_schedule(true);
        }
        if ev == 3
            && let Some(e) = self.worker.lock().last_error.clone()
        {
            log(&e);
        }
        self.refresh_panel();
        self.tray_tip();
    }
}

// ---------- deferred (run outside the App borrow) ----------

fn tray_menu(hwnd: HWND) {
    let Some((profiles, link, osd_on, cfg_path)) = with_app(|a| {
        (
            a.cfg
                .profiles
                .iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>(),
            a.cfg.link_brightness,
            a.cfg.show_osd,
            a.cfg_path.clone(),
        )
    }) else {
        return;
    };
    let auto = autostart::is_enabled();
    let cmd = unsafe {
        let menu = CreatePopupMenu();
        let add = |m: HMENU, flags: u32, id: usize, text: &str| {
            AppendMenuW(m, flags, id, wide(text).as_ptr());
        };
        add(menu, MF_STRING, 1, "Open panel");
        let prof = CreatePopupMenu();
        for (i, p) in profiles.iter().enumerate() {
            add(prof, MF_STRING, 100 + i, p);
        }
        AppendMenuW(
            menu,
            MF_POPUP | if profiles.is_empty() { MF_GRAYED } else { 0 },
            prof as usize,
            wide("Profiles").as_ptr(),
        );
        let bri = CreatePopupMenu();
        for (i, pct) in [0, 25, 50, 75, 100].iter().enumerate() {
            add(bri, MF_STRING, 200 + i, &format!("{pct}%"));
        }
        AppendMenuW(
            menu,
            MF_POPUP,
            bri as usize,
            wide("Brightness (all)").as_ptr(),
        );
        add(menu, MF_STRING, 2, "Screens off");
        add(menu, MF_SEPARATOR, 0, "");
        add(
            menu,
            MF_STRING | if link { MF_CHECKED } else { 0 },
            3,
            "Link brightness",
        );
        add(
            menu,
            MF_STRING | if osd_on { MF_CHECKED } else { 0 },
            4,
            "Show on-screen display",
        );
        add(
            menu,
            MF_STRING | if auto { MF_CHECKED } else { 0 },
            5,
            "Start with Windows",
        );
        add(menu, MF_STRING, 6, "Edit config…");
        add(menu, MF_STRING, 7, "Re-detect monitors");
        add(menu, MF_SEPARATOR, 0, "");
        add(menu, MF_STRING, 9, "Exit");

        let mut pt = POINT { x: 0, y: 0 };
        GetCursorPos(&mut pt);
        SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            pt.x,
            pt.y,
            0,
            hwnd,
            null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        cmd as usize
    };
    match cmd {
        0 => {}
        1 => {
            with_app(|a| a.show_panel());
        }
        2 => {
            with_app(|a| a.run_action(&Action::ScreensOff));
        }
        3 => {
            with_app(|a| {
                a.cfg.link_brightness = !a.cfg.link_brightness;
                a.save_config();
                a.refresh_panel();
            });
        }
        4 => {
            with_app(|a| {
                a.cfg.show_osd = !a.cfg.show_osd;
                a.save_config();
            });
        }
        5 => {
            if let Err(e) = autostart::set(!auto) {
                message_box(
                    &format!("Could not change startup setting: {e}"),
                    MB_ICONERROR,
                );
            }
        }
        6 => open_in_notepad(&cfg_path),
        7 => {
            with_app(|a| {
                a.worker.lock().last_error = None;
                a.worker.refresh(true);
            });
        }
        9 => unsafe {
            DestroyWindow(hwnd);
        },
        c if (100..200).contains(&c) => {
            if let Some(name) = profiles.get(c - 100) {
                with_app(|a| a.apply_profile(name));
            }
        }
        c if (200..205).contains(&c) => {
            let pct = [0, 25, 50, 75, 100][c - 200];
            with_app(|a| a.set_all_brightness(pct));
        }
        _ => {}
    }
}

fn run_panel_cmd(cmd: flyout::Cmd) {
    use flyout::Cmd;
    match cmd {
        Cmd::Hide => {
            with_app(|a| a.hide_panel());
        }
        Cmd::Refresh => {
            with_app(|a| {
                a.worker.lock().last_error = None;
                a.worker.refresh(true);
                a.refresh_panel();
            });
        }
        Cmd::Settings => {
            if let Some(path) = with_app(|a| {
                a.hide_panel();
                a.cfg_path.clone()
            }) {
                open_in_notepad(&path);
            }
        }
        Cmd::SaveProfile(name) => {
            with_app(|a| {
                let p = {
                    let s = a.worker.lock();
                    profile::snapshot(&name, &s.monitors, &s.values)
                };
                a.cfg.upsert_profile(p);
                a.save_config();
                osd::show(&format!("Saved profile  ·  {name}"), None);
                a.refresh_panel();
            });
        }
        Cmd::ApplyProfile(name) => {
            with_app(|a| a.apply_profile(&name));
        }
        Cmd::ToggleAdvanced => {
            with_app(|a| {
                a.advanced = !a.advanced;
                a.ensure_panel();
                a.show_window();
            });
        }
        Cmd::LinkChanged(on) => {
            with_app(|a| {
                a.cfg.link_brightness = on;
                a.save_config();
            });
        }
        Cmd::ConfirmInput {
            id,
            mon_no,
            value,
            label,
        } => {
            let ok = message_box(
                &format!(
                    "Switch monitor {mon_no} to {label}?\n\nIf nothing is connected to that input the screen goes dark. \
                     Switch back with the monitor's own buttons."
                ),
                MB_OKCANCEL | MB_ICONWARNING,
            ) == IDOK;
            with_app(|a| {
                if ok {
                    a.worker.set(&id, vcp::INPUT, value);
                }
                a.refresh_panel();
            });
        }
    }
}

// ---------- window procedures ----------

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_TRAY => {
                match loword(lp as usize) {
                    NIN_SELECT | NIN_KEYSELECT => {
                        with_app(|a| a.toggle_panel());
                    }
                    WM_CONTEXTMENU => tray_menu(hwnd),
                    _ => {}
                }
                0
            }
            WM_WORKER => {
                with_app(|a| a.on_worker(wp));
                0
            }
            WM_SHOW_PANEL => {
                with_app(|a| a.show_panel());
                0
            }
            WM_HOTKEY => {
                let act = with_app(|a| {
                    a.hotkeys
                        .iter()
                        .find(|h| h.0 as usize == wp)
                        .map(|h| h.1.clone())
                })
                .flatten();
                if let Some(act) = act {
                    with_app(|a| a.run_action(&act));
                }
                0
            }
            WM_TIMER => {
                match wp {
                    TIMER_TICK => {
                        with_app(|a| {
                            a.check_config();
                            if a.schedule_started {
                                a.check_schedule(false);
                            }
                        });
                    }
                    TIMER_SCREENS_OFF => {
                        KillTimer(hwnd, TIMER_SCREENS_OFF);
                        DefWindowProcW(hwnd, WM_SYSCOMMAND, SC_MONITORPOWER as usize, 2);
                    }
                    TIMER_REENUM => {
                        KillTimer(hwnd, TIMER_REENUM);
                        with_app(|a| a.worker.refresh(true));
                    }
                    _ => {}
                }
                0
            }
            // monitors plugged/unplugged, resolution change, resume from sleep
            WM_DISPLAYCHANGE | WM_DEVICECHANGE => {
                SetTimer(hwnd, TIMER_REENUM, 2000, None);
                0
            }
            WM_POWERBROADCAST => {
                if wp == 0x12 {
                    SetTimer(hwnd, TIMER_REENUM, 3000, None);
                }
                1
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            WM_SETTINGCHANGE => {
                if lp != 0 {
                    let p = lp as *const u16;
                    let mut n = 0;
                    while n < 64 && *p.add(n) != 0 {
                        n += 1;
                    }
                    let name = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                    if name == "ImmersiveColorSet" {
                        with_app(|a| {
                            if theme::apply(&a.cfg.theme) {
                                a.rebuild_panel();
                            }
                        });
                    }
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            m if m != 0 && m == TASKBAR_CREATED.get() => {
                with_app(|a| a.tray_add());
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

unsafe extern "system" fn panel_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        match msg {
            WM_HSCROLL if lp != 0 => {
                with_app(|a| {
                    let link = a.cfg.link_brightness;
                    if let Some(p) = a.panel.as_mut() {
                        p.on_hscroll(lp as HWND, &a.worker, link);
                    }
                });
                a_tip();
                0
            }
            WM_COMMAND => {
                let (id, code) = (loword(wp), hiword(wp));
                let cmd = with_app(|a| {
                    a.panel
                        .as_mut()
                        .and_then(|p| p.on_command(id, code, &a.worker))
                })
                .flatten();
                if let Some(c) = cmd {
                    run_panel_cmd(c);
                }
                0
            }
            WM_ACTIVATE => {
                if loword(wp) == WA_INACTIVE {
                    with_app(|a| a.hide_panel());
                }
                0
            }
            WM_CTLCOLORSTATIC | WM_CTLCOLORBTN => {
                let id = GetDlgCtrlID(lp as HWND) as u32;
                SetTextColor(wp as HDC, theme::cref(flyout::static_text_color(id)));
                SetBkColor(wp as HDC, theme::cref(theme::get().bg));
                theme::bg_brush() as LRESULT
            }
            WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => {
                SetTextColor(wp as HDC, theme::cref(theme::get().text));
                SetBkColor(wp as HDC, theme::cref(theme::get().surface));
                theme::surface_brush() as LRESULT
            }
            WM_ERASEBKGND => {
                let mut rc = std::mem::zeroed();
                GetClientRect(hwnd, &mut rc);
                FillRect(wp as HDC, &rc, theme::bg_brush());
                1
            }
            WM_PAINT => {
                let mut ps: PAINTSTRUCT = std::mem::zeroed();
                let hdc = BeginPaint(hwnd, &mut ps);
                let line = CreateSolidBrush(theme::cref(theme::get().line));
                for r in flyout::lines() {
                    FillRect(hdc, &r, line);
                }
                DeleteObject(line);
                let frames = flyout::frames();
                if !frames.is_empty()
                    && let Some(cv) = gfx::Canvas::new(hdc)
                {
                    let t = theme::get();
                    let r = theme::px(5.0);
                    for f in frames {
                        let (x, y) = (f.left as f32, f.top as f32);
                        let (w, h) = ((f.right - f.left) as f32, (f.bottom - f.top) as f32);
                        cv.fill_round(x, y, w, h, r, t.surface);
                        cv.stroke_round(x, y, w, h, r, t.line, 1.0);
                    }
                }
                EndPaint(hwnd, &ps);
                0
            }
            WM_DRAWITEM => {
                theme::draw_button(&*(lp as *const DRAWITEMSTRUCT));
                1
            }
            WM_NOTIFY => {
                let hdr = &*(lp as *const NMHDR);
                // NM_CUSTOMDRAW (-12) from a trackbar
                if hdr.code == 0xFFFF_FFF4 && is_trackbar(hdr.hwndFrom) {
                    return theme::draw_slider(&*(lp as *const NMCUSTOMDRAW));
                }
                DefWindowProcW(hwnd, msg, wp, lp)
            }
            WM_CLOSE => {
                with_app(|a| a.hide_panel());
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

fn is_trackbar(hwnd: HWND) -> bool {
    let mut buf = [0u16; 32];
    let n = unsafe { GetClassNameW(hwnd, buf.as_mut_ptr(), 32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize]).eq_ignore_ascii_case("msctls_trackbar32")
}

fn a_tip() {
    with_app(|a| a.tray_tip());
}
