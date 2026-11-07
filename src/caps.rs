//! MCCS capabilities string parser, e.g.
//! `(prot(monitor)type(LCD)model(C32G2ZE)vcp(10 12 14(01 05 06))mccs_ver(2.2))`.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capabilities {
    pub model: Option<String>,
    pub mccs_ver: Option<String>,
    /// VCP code -> allowed values (empty = continuous / unspecified).
    pub vcp: BTreeMap<u8, Vec<u8>>,
}

impl Capabilities {
    pub fn supports(&self, code: u8) -> bool {
        self.vcp.contains_key(&code)
    }

    pub fn values(&self, code: u8) -> Option<&[u8]> {
        self.vcp.get(&code).map(|v| v.as_slice())
    }

    pub fn is_empty(&self) -> bool {
        self.vcp.is_empty()
    }
}

pub fn parse(s: &str) -> Capabilities {
    let s = s.trim().trim_end_matches('\0').trim();
    let inner = s
        .strip_prefix('(')
        .and_then(|r| r.rfind(')').map(|i| &r[..i]))
        .unwrap_or(s);

    let mut caps = Capabilities::default();
    for (key, val) in top_level(inner) {
        match key.as_str() {
            "model" => caps.model = Some(val.trim().to_string()),
            "mccs_ver" => caps.mccs_ver = Some(val.trim().to_string()),
            "vcp" => caps.vcp = parse_vcp(&val),
            _ => {}
        }
    }
    caps
}

/// Split `key(value)key2(value2)` respecting nested parentheses.
fn top_level(s: &str) -> Vec<(String, String)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let start = i;
        while i < b.len() && b[i] != b'(' {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let key = s[start..i].trim().to_ascii_lowercase();
        i += 1;
        let vstart = i;
        let mut depth = 1;
        while i < b.len() && depth > 0 {
            match b[i] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
        let vend = if depth == 0 { i - 1 } else { i };
        out.push((key, s[vstart..vend].to_string()));
    }
    out
}

fn hex_bytes(tok: &str) -> Vec<u8> {
    if !tok.len().is_multiple_of(2) {
        return u8::from_str_radix(tok, 16).ok().into_iter().collect();
    }
    (0..tok.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&tok[i..i + 2], 16).ok())
        .collect()
}

fn flush(tok: &mut String, depth: u32, last: &mut Option<u8>, map: &mut BTreeMap<u8, Vec<u8>>) {
    if tok.is_empty() {
        return;
    }
    for b in hex_bytes(tok) {
        match depth {
            0 => {
                map.entry(b).or_default();
                *last = Some(b);
            }
            1 => {
                if let Some(code) = *last {
                    map.entry(code).or_default().push(b);
                }
            }
            _ => {}
        }
    }
    tok.clear();
}

fn parse_vcp(s: &str) -> BTreeMap<u8, Vec<u8>> {
    let mut map = BTreeMap::new();
    let mut last = None;
    let mut depth = 0u32;
    let mut tok = String::new();
    for c in s.chars() {
        match c {
            '(' => {
                flush(&mut tok, depth, &mut last, &mut map);
                depth += 1;
            }
            ')' => {
                flush(&mut tok, depth, &mut last, &mut map);
                depth = depth.saturating_sub(1);
            }
            c if c.is_ascii_hexdigit() => tok.push(c),
            _ => flush(&mut tok, depth, &mut last, &mut map),
        }
    }
    flush(&mut tok, depth, &mut last, &mut map);
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real string read from the user's AOC C32G2ZE over DDC/CI.
    pub const AOC: &str = "(prot(monitor)type(LCD)model(C32G2ZE)cmds(01 02 03 07 0C E3 F3)vcp(02 04 05 08 0C 10 12 14(01 05 06 08 0B) 16 18 1A 52 60(0F 11 12 )62 86(01 02 05 0B 0C 0D 0E 0F 10 11 12 13 14) AC AE B2 B6 C6 C8 CA CC(01 02 03 04 05 06 07 09 0A 0B 0C 0D 0E 12 14 16 1E) D6(01 04 05) DC(00 0B 0C 0D 0E 0F 10) DF ED FF)mswhql(1)asset_eep(40)mccs_ver(2.2))";

    #[test]
    fn parses_real_aoc_string() {
        let c = parse(AOC);
        assert_eq!(c.model.as_deref(), Some("C32G2ZE"));
        assert_eq!(c.mccs_ver.as_deref(), Some("2.2"));
        for code in [0x10, 0x12, 0x16, 0x18, 0x1A, 0x52, 0x62, 0xFF] {
            assert!(c.supports(code), "missing {code:#x}");
        }
        // Sharpness answers GET on this monitor but is not advertised.
        assert!(!c.supports(0x87));
        assert_eq!(c.values(0x14).unwrap(), &[0x01, 0x05, 0x06, 0x08, 0x0B]);
        assert_eq!(c.values(0x60).unwrap(), &[0x0F, 0x11, 0x12]);
        assert_eq!(
            c.values(0xDC).unwrap(),
            &[0x00, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x10]
        );
        assert_eq!(c.values(0xD6).unwrap(), &[0x01, 0x04, 0x05]);
        assert!(c.values(0x10).unwrap().is_empty());
        assert_eq!(c.vcp.len(), 28);
    }

    #[test]
    fn parses_without_spaces_and_outer_parens() {
        let c = parse("prot(monitor)vcp(021012(0102)16)model(X)");
        assert_eq!(c.model.as_deref(), Some("X"));
        assert_eq!(c.values(0x12).unwrap(), &[0x01, 0x02]);
        assert!(c.supports(0x02) && c.supports(0x10) && c.supports(0x16));
    }

    #[test]
    fn tolerates_garbage_and_nul() {
        assert!(parse("").is_empty());
        assert!(parse("\0\0").is_empty());
        let c = parse("(vcp(10 zz 12(01 02)\0");
        assert!(c.supports(0x10) && c.supports(0x12));
        assert_eq!(c.values(0x12).unwrap(), &[1, 2]);
    }
}
