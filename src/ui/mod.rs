//! Win32 tray app: hidden main window (tray, hotkeys, timers), panel flyout and OSD.

pub mod app;
mod autostart;
mod flyout;
mod gfx;
mod icon;
mod osd;
mod theme;
mod util;

pub use app::run;
