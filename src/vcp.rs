//! Catalog of user-facing VCP features and their value names.

use crate::caps::Capabilities;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Continuous,
    Choice(&'static [(u8, &'static str)]),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Feature {
    pub code: u8,
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
}

pub const BRIGHTNESS: u8 = 0x10;
pub const CONTRAST: u8 = 0x12;
pub const COLOR_PRESET: u8 = 0x14;
pub const RED: u8 = 0x16;
pub const GREEN: u8 = 0x18;
pub const BLUE: u8 = 0x1A;
pub const INPUT: u8 = 0x60;
pub const VOLUME: u8 = 0x62;
pub const SHARPNESS: u8 = 0x87;
pub const GAME_MODE: u8 = 0xDC;
pub const RESTORE_COLOR: u8 = 0x08;

/// MCCS 2.2 colour presets. AOC labels its 6500/7500/9300K presets Warm/Normal/Cool.
pub const COLOR_PRESETS: &[(u8, &str)] = &[
    (0x01, "sRGB"),
    (0x02, "Native"),
    (0x03, "4000K"),
    (0x04, "5000K"),
    (0x05, "Warm (6500K)"),
    (0x06, "Normal (7500K)"),
    (0x07, "8200K"),
    (0x08, "Cool (9300K)"),
    (0x09, "10000K"),
    (0x0A, "11500K"),
    (0x0B, "User"),
    (0x0C, "User 2"),
    (0x0D, "User 3"),
];

/// 0xDC display mode. 0x0B-0x10 are manufacturer specific; AOC maps them to its game modes.
pub const GAME_MODES: &[(u8, &str)] = &[
    (0x00, "Off"),
    (0x0B, "FPS"),
    (0x0C, "RTS"),
    (0x0D, "Racing"),
    (0x0E, "Gamer 1"),
    (0x0F, "Gamer 2"),
    (0x10, "Gamer 3"),
];

pub const INPUTS: &[(u8, &str)] = &[
    (0x01, "VGA"),
    (0x03, "DVI"),
    (0x0F, "DisplayPort"),
    (0x10, "DisplayPort 2"),
    (0x11, "HDMI 1"),
    (0x12, "HDMI 2"),
];

/// Display order in the UI: sliders first, then choices.
pub const FEATURES: &[Feature] = &[
    Feature {
        code: BRIGHTNESS,
        key: "brightness",
        label: "Brightness",
        kind: Kind::Continuous,
    },
    Feature {
        code: CONTRAST,
        key: "contrast",
        label: "Contrast",
        kind: Kind::Continuous,
    },
    Feature {
        code: SHARPNESS,
        key: "sharpness",
        label: "Sharpness",
        kind: Kind::Continuous,
    },
    Feature {
        code: RED,
        key: "red",
        label: "Red",
        kind: Kind::Continuous,
    },
    Feature {
        code: GREEN,
        key: "green",
        label: "Green",
        kind: Kind::Continuous,
    },
    Feature {
        code: BLUE,
        key: "blue",
        label: "Blue",
        kind: Kind::Continuous,
    },
    Feature {
        code: VOLUME,
        key: "volume",
        label: "Volume",
        kind: Kind::Continuous,
    },
    Feature {
        code: COLOR_PRESET,
        key: "color_preset",
        label: "Color",
        kind: Kind::Choice(COLOR_PRESETS),
    },
    Feature {
        code: GAME_MODE,
        key: "game_mode",
        label: "Game mode",
        kind: Kind::Choice(GAME_MODES),
    },
    Feature {
        code: INPUT,
        key: "input",
        label: "Input",
        kind: Kind::Choice(INPUTS),
    },
];

/// Milliseconds to wait after writing `code` before the next write. Preset and
/// mode switches make the monitor ignore writes for a moment (measured on AOC).
pub fn settle_ms(code: u8) -> u64 {
    match code {
        COLOR_PRESET | GAME_MODE => 350,
        0x04 | 0x05 | RESTORE_COLOR => 500,
        _ => 0,
    }
}

pub fn by_code(code: u8) -> Option<&'static Feature> {
    FEATURES.iter().find(|f| f.code == code)
}

pub fn by_key(key: &str) -> Option<&'static Feature> {
    FEATURES.iter().find(|f| f.key.eq_ignore_ascii_case(key))
}

/// Accepts a feature key (`brightness`), hex (`0x10`, `10h`) or decimal (`16`).
pub fn parse_code(s: &str) -> Option<u8> {
    let s = s.trim();
    if let Some(f) = by_key(s) {
        return Some(f.code);
    }
    let lower = s.to_ascii_lowercase();
    if let Some(h) = lower.strip_prefix("0x") {
        return u8::from_str_radix(h, 16).ok();
    }
    if let Some(h) = lower.strip_suffix('h') {
        return u8::from_str_radix(h, 16).ok();
    }
    lower.parse().ok()
}

pub fn key_for(code: u8) -> String {
    by_code(code)
        .map(|f| f.key.to_string())
        .unwrap_or_else(|| format!("0x{code:02X}"))
}

pub fn choice_label(code: u8, value: u32) -> String {
    if let Some(Feature {
        kind: Kind::Choice(list),
        ..
    }) = by_code(code)
        && let Some((_, name)) = list.iter().find(|(v, _)| u32::from(*v) == value)
    {
        return (*name).to_string();
    }
    format!("0x{value:02X}")
}

/// Values selectable for a choice feature: what the monitor advertises if it
/// lists values, otherwise the full catalog list.
pub fn choices_for(feature: &Feature, caps: Option<&Capabilities>) -> Vec<(u8, String)> {
    let Kind::Choice(list) = feature.kind else {
        return Vec::new();
    };
    match caps
        .and_then(|c| c.values(feature.code))
        .filter(|v| !v.is_empty())
    {
        Some(adv) => adv
            .iter()
            .map(|&v| (v, choice_label(feature.code, u32::from(v))))
            .collect(),
        None => list.iter().map(|&(v, n)| (v, n.to_string())).collect(),
    }
}

/// Parse a value for a feature: number (dec/hex) or a choice label (case-insensitive).
pub fn parse_value(code: u8, s: &str) -> Option<u32> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    if let Some(h) = lower.strip_prefix("0x") {
        return u32::from_str_radix(h, 16).ok();
    }
    if let Ok(n) = lower.parse() {
        return Some(n);
    }
    if let Some(Feature {
        kind: Kind::Choice(list),
        ..
    }) = by_code(code)
    {
        let hit = list.iter().find(|(_, n)| {
            let n = n.to_ascii_lowercase();
            n == lower || n.split(' ').next() == Some(lower.as_str())
        });
        return hit.map(|(v, _)| u32::from(*v));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps;

    #[test]
    fn parse_code_forms() {
        assert_eq!(parse_code("brightness"), Some(0x10));
        assert_eq!(parse_code("Game_Mode"), Some(0xDC));
        assert_eq!(parse_code("0x14"), Some(0x14));
        assert_eq!(parse_code("14h"), Some(0x14));
        assert_eq!(parse_code("16"), Some(16));
        assert_eq!(parse_code("nope"), None);
        assert_eq!(parse_code("0x1FF"), None);
    }

    #[test]
    fn labels_and_keys() {
        assert_eq!(choice_label(COLOR_PRESET, 5), "Warm (6500K)");
        assert_eq!(choice_label(INPUT, 0x11), "HDMI 1");
        assert_eq!(choice_label(INPUT, 0x99), "0x99");
        assert_eq!(choice_label(BRIGHTNESS, 3), "0x03");
        assert_eq!(key_for(0x16), "red");
        assert_eq!(key_for(0xAC), "0xAC");
    }

    #[test]
    fn choices_filtered_by_caps() {
        let c = caps::parse("(vcp(14(01 05 0B) 60(0F 11 12) DC))");
        let preset = by_code(COLOR_PRESET).unwrap();
        let ch = choices_for(preset, Some(&c));
        assert_eq!(ch.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1, 5, 0x0B]);
        assert_eq!(ch[2].1, "User");
        // DC advertised without values -> full list
        let gm = by_code(GAME_MODE).unwrap();
        assert_eq!(choices_for(gm, Some(&c)).len(), GAME_MODES.len());
        assert_eq!(choices_for(gm, None).len(), GAME_MODES.len());
        assert!(choices_for(by_code(BRIGHTNESS).unwrap(), Some(&c)).is_empty());
    }

    #[test]
    fn parse_values() {
        assert_eq!(parse_value(BRIGHTNESS, "40"), Some(40));
        assert_eq!(parse_value(INPUT, "0x11"), Some(0x11));
        assert_eq!(parse_value(INPUT, "hdmi 2"), Some(0x12));
        assert_eq!(parse_value(COLOR_PRESET, "srgb"), Some(1));
        assert_eq!(parse_value(COLOR_PRESET, "warm"), Some(5));
        assert_eq!(parse_value(GAME_MODE, "fps"), Some(0x0B));
        assert_eq!(parse_value(BRIGHTNESS, "bright"), None);
    }

    #[test]
    fn feature_codes_unique() {
        let mut codes: Vec<u8> = FEATURES.iter().map(|f| f.code).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), FEATURES.len());
    }
}
