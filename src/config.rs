//! TOML config: profiles, hotkeys, schedule and UI options.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_TOML: &str = r#"# Glint config. Edited from the tray menu -> "Edit config...".
# Saved automatically when you change options in the app (comments are not kept then).

theme = "system"           # system, light or dark
link_brightness = true     # brightness slider/hotkeys move every monitor together
step = 10                  # brightness hotkey step (percent)
show_osd = true            # on-screen popup for hotkeys
apply_schedule_on_start = false

# Profiles. Keys: brightness, contrast, sharpness, red, green, blue, volume,
# color_preset, game_mode, input, or any raw VCP code like "0x87".
# `all` applies to every monitor; `monitors.<sel>` overrides one monitor,
# where <sel> is its number ("1" = leftmost) or id fragment ("UID4353").
# RGB gains live in the User preset (11); Glint switches to User to write them,
# then to the profile's color_preset.
[[profiles]]
name = "Day"
all = { brightness = 60, contrast = 50, color_preset = 5 }

[[profiles]]
name = "Night"
all = { brightness = 10, contrast = 50, color_preset = 11, red = 50, green = 40, blue = 28 }

[[profiles]]
name = "Gaming"
all = { brightness = 85, contrast = 55, game_mode = 11 }

# Actions: brightness_up, brightness_down, profile:<name>, screens_off, toggle_panel
[[hotkeys]]
keys = "Ctrl+Alt+PageUp"
action = "brightness_up"

[[hotkeys]]
keys = "Ctrl+Alt+PageDown"
action = "brightness_down"

[[hotkeys]]
keys = "Ctrl+Alt+F1"
action = "profile:Day"

[[hotkeys]]
keys = "Ctrl+Alt+F2"
action = "profile:Night"

[[hotkeys]]
keys = "Ctrl+Alt+F3"
action = "profile:Gaming"

[[hotkeys]]
keys = "Ctrl+Alt+F12"
action = "screens_off"

[[hotkeys]]
keys = "Ctrl+Alt+M"
action = "toggle_panel"

# Time-of-day schedule (24h). Uncomment to use:
# [[schedule]]
# time = "07:30"
# profile = "Day"
#
# [[schedule]]
# time = "21:00"
# profile = "Night"
"#;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    pub link_brightness: bool,
    pub step: u32,
    pub show_osd: bool,
    pub apply_schedule_on_start: bool,
    pub profiles: Vec<Profile>,
    pub hotkeys: Vec<Hotkey>,
    pub schedule: Vec<ScheduleEntry>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct Profile {
    pub name: String,
    pub all: BTreeMap<String, u32>,
    pub monitors: BTreeMap<String, BTreeMap<String, u32>>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Hotkey {
    pub keys: String,
    pub action: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct ScheduleEntry {
    pub time: String,
    pub profile: String,
}

impl Default for Config {
    /// Field fallbacks for partial files. Must not parse TOML (serde calls this).
    fn default() -> Self {
        Config {
            theme: "system".into(),
            link_brightness: true,
            step: 10,
            show_osd: true,
            apply_schedule_on_start: false,
            profiles: Vec::new(),
            hotkeys: Vec::new(),
            schedule: Vec::new(),
        }
    }
}

impl Config {
    /// The shipped config (default profiles + hotkeys).
    pub fn builtin() -> Config {
        toml::from_str(DEFAULT_TOML).expect("default config is valid")
    }

    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
    }

    /// Insert or replace a profile by name (case-insensitive).
    pub fn upsert_profile(&mut self, p: Profile) {
        match self
            .profiles
            .iter_mut()
            .find(|x| x.name.eq_ignore_ascii_case(&p.name))
        {
            Some(slot) => *slot = p,
            None => self.profiles.push(p),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    BrightnessUp,
    BrightnessDown,
    Profile(String),
    ScreensOff,
    TogglePanel,
}

impl Action {
    pub fn parse(s: &str) -> Option<Action> {
        let s = s.trim();
        if let Some(name) = s.strip_prefix("profile:") {
            let name = name.trim();
            return (!name.is_empty()).then(|| Action::Profile(name.to_string()));
        }
        match s.to_ascii_lowercase().as_str() {
            "brightness_up" => Some(Action::BrightnessUp),
            "brightness_down" => Some(Action::BrightnessDown),
            "screens_off" => Some(Action::ScreensOff),
            "toggle_panel" | "toggle_flyout" => Some(Action::TogglePanel),
            _ => None,
        }
    }
}

pub fn parse(s: &str) -> Result<Config, String> {
    toml::from_str(s).map_err(|e| e.to_string())
}

pub fn to_string(c: &Config) -> Result<String, String> {
    toml::to_string_pretty(c).map_err(|e| e.to_string())
}

/// `%APPDATA%\Glint` (falls back to the current dir).
pub fn config_dir() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Glint")
}

pub fn default_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// Load config, writing the commented default file on first run.
pub fn load_or_create(path: &Path) -> Result<Config, String> {
    if !path.exists() {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, DEFAULT_TOML).map_err(|e| e.to_string())?;
        return Ok(Config::builtin());
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn save(path: &Path, c: &Config) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, to_string(c)?).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_parses_with_expected_content() {
        let c = Config::builtin();
        assert!(c.link_brightness);
        assert_eq!(c.step, 10);
        assert_eq!(c.profiles.len(), 3);
        assert_eq!(c.profile("night").unwrap().all["color_preset"], 11);
        assert_eq!(c.hotkeys.len(), 7);
        assert!(c.schedule.is_empty());
        for h in &c.hotkeys {
            assert!(
                Action::parse(&h.action).is_some(),
                "bad action {}",
                h.action
            );
            assert!(crate::hotkey::parse(&h.keys).is_ok(), "bad keys {}", h.keys);
        }
    }

    #[test]
    fn roundtrip_and_partial_files() {
        let mut c = Config::builtin();
        c.schedule.push(ScheduleEntry {
            time: "07:00".into(),
            profile: "Day".into(),
        });
        let mut p = Profile {
            name: "Mine".into(),
            ..Default::default()
        };
        p.monitors.insert(
            "UID4353".into(),
            BTreeMap::from([("brightness".into(), 33)]),
        );
        c.upsert_profile(p);
        let back = parse(&to_string(&c).unwrap()).unwrap();
        assert_eq!(back, c);

        // Missing fields fall back to defaults of the field type.
        let partial = parse("step = 5").unwrap();
        assert_eq!(partial.step, 5);
        assert!(parse("step = \"x\"").is_err());
    }

    #[test]
    fn upsert_replaces_case_insensitively() {
        let mut c = Config::builtin();
        let n = c.profiles.len();
        c.upsert_profile(Profile {
            name: "DAY".into(),
            ..Default::default()
        });
        assert_eq!(c.profiles.len(), n);
        assert!(c.profile("day").unwrap().all.is_empty());
    }

    #[test]
    fn actions() {
        assert_eq!(Action::parse("brightness_up"), Some(Action::BrightnessUp));
        assert_eq!(
            Action::parse("profile: Night "),
            Some(Action::Profile("Night".into()))
        );
        assert_eq!(Action::parse("profile:"), None);
        assert_eq!(Action::parse("toggle_flyout"), Some(Action::TogglePanel));
        assert_eq!(Action::parse("explode"), None);
    }

    #[test]
    fn load_creates_file_then_reads_it() {
        let dir = std::env::temp_dir().join(format!("glint-test-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let c = load_or_create(&path).unwrap();
        assert!(path.exists());
        assert_eq!(c, Config::builtin());
        let mut c2 = c.clone();
        c2.step = 3;
        save(&path, &c2).unwrap();
        assert_eq!(load_or_create(&path).unwrap().step, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
