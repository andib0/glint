//! Win32 tray app: hidden main window (tray, hotkeys, timers), panel flyout and OSD.

pub mod app;
mod autostart;
mod flyout;
mod icon;
mod osd;
mod util;

pub use app::run;
