//! Parse hotkey strings like `Ctrl+Alt+PageUp` into Win32 RegisterHotKey args.

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
pub const MOD_NOREPEAT: u32 = 0x4000;

/// Returns `(modifiers, virtual_key)`. MOD_NOREPEAT is not included.
pub fn parse(s: &str) -> Result<(u32, u32), String> {
    let mut mods = 0;
    let mut key = None;
    for part in s.split('+').map(str::trim) {
        if part.is_empty() {
            return Err(format!("empty key in '{s}'"));
        }
        let m = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => MOD_CONTROL,
            "alt" => MOD_ALT,
            "shift" => MOD_SHIFT,
            "win" | "super" | "meta" => MOD_WIN,
            _ => 0,
        };
        if m != 0 {
            mods |= m;
            continue;
        }
        if key.is_some() {
            return Err(format!("more than one key in '{s}'"));
        }
        key = Some(vk(part).ok_or_else(|| format!("unknown key '{part}'"))?);
    }
    let key = key.ok_or_else(|| format!("no key in '{s}'"))?;
    Ok((mods, key))
}

fn vk(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    if n.len() == 1 {
        let c = n.as_bytes()[0];
        if c.is_ascii_alphanumeric() {
            return Some(u32::from(c.to_ascii_uppercase()));
        }
    }
    if let Some(num) = n.strip_prefix('f').and_then(|x| x.parse::<u32>().ok())
        && (1..=24).contains(&num)
    {
        return Some(0x70 + num - 1);
    }
    if let Some(num) = n.strip_prefix("numpad").and_then(|x| x.parse::<u32>().ok())
        && num <= 9
    {
        return Some(0x60 + num);
    }
    Some(match n.as_str() {
        "pageup" | "pgup" => 0x21,
        "pagedown" | "pgdn" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "insert" | "ins" => 0x2D,
        "delete" | "del" => 0x2E,
        "space" => 0x20,
        "pause" => 0x13,
        "scrolllock" => 0x91,
        "plus" | "=" => 0xBB,
        "minus" | "-" => 0xBD,
        "add" => 0x6B,
        "subtract" => 0x6D,
        "multiply" => 0x6A,
        "divide" => 0x6F,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_combos() {
        assert_eq!(parse("Ctrl+Alt+PageUp"), Ok((MOD_CONTROL | MOD_ALT, 0x21)));
        assert_eq!(
            parse("win + shift + b"),
            Ok((MOD_WIN | MOD_SHIFT, b'B' as u32))
        );
        assert_eq!(parse("Ctrl+F12"), Ok((MOD_CONTROL, 0x7B)));
        assert_eq!(parse("Alt+1"), Ok((MOD_ALT, b'1' as u32)));
        assert_eq!(parse("Ctrl+Numpad5"), Ok((MOD_CONTROL, 0x65)));
        assert_eq!(parse("F24"), Ok((0, 0x87)));
        assert_eq!(parse("Ctrl+Minus"), Ok((MOD_CONTROL, 0xBD)));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(parse("").is_err());
        assert!(parse("Ctrl+Alt").is_err());
        assert!(parse("Ctrl+A+B").is_err());
        assert!(parse("Ctrl+Banana").is_err());
        assert!(parse("Ctrl++").is_err());
        assert!(parse("F25").is_err());
    }
}
