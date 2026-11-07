//! Turn profiles into ordered VCP writes, and snapshot current values into profiles.

use crate::backend::MonitorInfo;
use crate::config::Profile;
use crate::vcp;
use std::collections::{BTreeMap, HashMap};

pub type Write = (String, u8, u32);

/// After the colour preset/RGB block: game mode, everything else, input last
/// (switching input may take the screen away).
fn order(code: u8) -> (u8, u8) {
    let group = match code {
        vcp::COLOR_PRESET => 0,
        vcp::GAME_MODE => 1,
        vcp::INPUT => 3,
        _ => 2,
    };
    (group, code)
}

const USER_PRESET: u32 = 0x0B;
const RGB: [u8; 3] = [vcp::RED, vcp::GREEN, vcp::BLUE];

/// Resolve a profile into writes for `monitors` (left-to-right order).
/// Returns the writes plus human-readable warnings for skipped entries.
pub fn resolve(p: &Profile, monitors: &[MonitorInfo]) -> (Vec<Write>, Vec<String>) {
    let mut writes = Vec::new();
    let mut warnings = Vec::new();

    for map in std::iter::once(&p.all).chain(p.monitors.values()) {
        for k in map.keys().filter(|k| vcp::parse_code(k).is_none()) {
            warnings.push(format!("profile '{}': unknown setting '{k}'", p.name));
        }
    }
    for sel in p.monitors.keys() {
        if !monitors.iter().enumerate().any(|(i, m)| m.matches(i, sel)) {
            warnings.push(format!("profile '{}': no monitor matches '{sel}'", p.name));
        }
    }

    fn add(vals: &mut BTreeMap<u8, u32>, map: &BTreeMap<String, u32>) {
        for (k, v) in map {
            if let Some(code) = vcp::parse_code(k) {
                vals.insert(code, *v);
            }
        }
    }

    for (i, m) in monitors.iter().enumerate() {
        let mut vals: BTreeMap<u8, u32> = BTreeMap::new();
        add(&mut vals, &p.all);
        for (sel, map) in &p.monitors {
            if m.matches(i, sel) {
                add(&mut vals, map);
            }
        }

        // RGB gains live in the User preset (AOC reports/keeps them there even
        // while another preset is active). To restore them without losing the
        // chosen preset: switch to User, write RGB, then switch to the target.
        let rgb: Vec<(u8, u32)> = RGB
            .iter()
            .filter_map(|c| vals.remove(c).map(|v| (*c, v)))
            .collect();
        let preset = vals.remove(&vcp::COLOR_PRESET);
        let mut list: Vec<(u8, u32)> = Vec::new();
        match preset {
            Some(p) if p != USER_PRESET && !rgb.is_empty() => {
                list.push((vcp::COLOR_PRESET, USER_PRESET));
                list.extend(&rgb);
                list.push((vcp::COLOR_PRESET, p));
            }
            _ => {
                list.extend(preset.map(|p| (vcp::COLOR_PRESET, p)));
                list.extend(&rgb);
            }
        }
        let mut rest: Vec<(u8, u32)> = vals.into_iter().collect();
        rest.sort_by_key(|(c, _)| order(*c));
        list.extend(rest);
        writes.extend(list.into_iter().map(|(c, v)| (m.id.clone(), c, v)));
    }
    (writes, warnings)
}

/// Capture the current state of every monitor as a profile (input excluded).
pub fn snapshot(
    name: &str,
    monitors: &[MonitorInfo],
    values: &HashMap<(String, u8), (u32, u32)>,
) -> Profile {
    let mut p = Profile {
        name: name.to_string(),
        ..Default::default()
    };
    for m in monitors {
        let get = |code: u8| values.get(&(m.id.clone(), code)).map(|v| v.0);
        let mut map = BTreeMap::new();
        for f in vcp::FEATURES {
            if f.code == vcp::INPUT {
                continue;
            }
            if let Some(v) = get(f.code) {
                map.insert(f.key.to_string(), v);
            }
        }
        if !map.is_empty() {
            p.monitors.insert(m.short_id(), map);
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use crate::config::Config;

    fn mons() -> Vec<MonitorInfo> {
        MockBackend::two_aoc().monitors
    }

    #[test]
    fn night_profile_orders_preset_before_rgb() {
        let c = Config::builtin();
        let (w, warn) = resolve(c.profile("Night").unwrap(), &mons());
        assert!(warn.is_empty(), "{warn:?}");
        let m0: Vec<u8> = w
            .iter()
            .filter(|x| x.0.contains("UID4353"))
            .map(|x| x.1)
            .collect();
        assert_eq!(m0[0], vcp::COLOR_PRESET);
        assert!(m0.contains(&vcp::RED) && m0.contains(&vcp::BRIGHTNESS));
        assert_eq!(w.len(), m0.len() * 2, "applies to both monitors");
    }

    #[test]
    fn per_monitor_overrides_and_input_last() {
        let mut p = Profile {
            name: "X".into(),
            ..Default::default()
        };
        p.all.insert("brightness".into(), 50);
        p.all.insert("input".into(), 0x11);
        p.all.insert("game_mode".into(), 0x0B);
        p.monitors
            .insert("2".into(), BTreeMap::from([("brightness".into(), 20)]));
        p.monitors
            .insert("UID4353".into(), BTreeMap::from([("0x87".into(), 70)]));
        let (w, warn) = resolve(&p, &mons());
        assert!(warn.is_empty(), "{warn:?}");
        let m = mons();
        let get = |id: &str, code| w.iter().find(|x| x.0 == id && x.1 == code).map(|x| x.2);
        assert_eq!(get(&m[0].id, vcp::BRIGHTNESS), Some(50));
        assert_eq!(get(&m[1].id, vcp::BRIGHTNESS), Some(20));
        assert_eq!(get(&m[0].id, vcp::SHARPNESS), Some(70));
        assert_eq!(get(&m[1].id, vcp::SHARPNESS), None);
        let m0: Vec<u8> = w.iter().filter(|x| x.0 == m[0].id).map(|x| x.1).collect();
        assert_eq!(m0.first(), Some(&vcp::GAME_MODE));
        assert_eq!(m0.last(), Some(&vcp::INPUT));
    }

    #[test]
    fn warnings_for_bad_entries() {
        let mut p = Profile {
            name: "Bad".into(),
            ..Default::default()
        };
        p.all.insert("sparkle".into(), 1);
        p.monitors.insert("9".into(), BTreeMap::new());
        let (_, warn) = resolve(&p, &mons());
        assert!(warn.iter().any(|s| s.contains("'9'")));
        assert_eq!(
            warn.iter().filter(|s| s.contains("sparkle")).count(),
            1,
            "warned once"
        );
    }

    #[test]
    fn rgb_with_non_user_preset_goes_through_user() {
        let mut p = Profile {
            name: "Warm+rgb".into(),
            ..Default::default()
        };
        p.all.insert("color_preset".into(), 5);
        p.all.insert("red".into(), 30);
        p.all.insert("brightness".into(), 40);
        let m = mons();
        let (w, warn) = resolve(&p, &m[..1]);
        assert!(warn.is_empty());
        let seq: Vec<(u8, u32)> = w.iter().map(|x| (x.1, x.2)).collect();
        assert_eq!(seq, vec![(0x14, 11), (0x16, 30), (0x14, 5), (0x10, 40)]);
    }

    #[test]
    fn snapshot_roundtrips_through_resolve() {
        let b = MockBackend::two_aoc();
        let vals = b.values.lock().unwrap().clone();
        let p = snapshot("Now", &b.monitors, &vals);
        let m0 = &p.monitors["UID4353"];
        assert_eq!(m0["brightness"], 19);
        assert!(!m0.contains_key("input"));
        assert!(m0.contains_key("red") && p.monitors["UID4354"].contains_key("red"));
        let (w, warn) = resolve(&p, &b.monitors);
        assert!(warn.is_empty(), "{warn:?}");
        // Replaying the writes over a scrambled state must reproduce the snapshot.
        let mut state: HashMap<(String, u8), u32> =
            vals.iter().map(|(k, v)| (k.clone(), v.0 ^ 0x7)).collect();
        for (id, code, v) in w {
            state.insert((id, code), v);
        }
        for (k, v) in &vals {
            if k.1 != vcp::INPUT {
                assert_eq!(state[k], v.0, "{k:?}");
            }
        }
    }
}
