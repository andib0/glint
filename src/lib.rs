//! Glint: lightweight DDC/CI monitor control.
//!
//! Pure logic (capabilities parsing, VCP catalog, config, hotkeys, schedule,
//! profiles, the coalescing worker and the CLI) is platform independent and
//! unit tested against a mock backend. `ddc_win` and `ui` are the Win32 layers.

pub mod backend;
pub mod caps;
pub mod cli;
pub mod config;
pub mod hotkey;
pub mod profile;
pub mod schedule;
pub mod vcp;
pub mod worker;

#[cfg(windows)]
pub mod ddc_win;
#[cfg(windows)]
pub mod ui;
