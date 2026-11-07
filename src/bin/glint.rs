#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Glint tray app. `glint --show` opens the panel on start.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(glint::ui::run(args));
}
