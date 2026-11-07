//! Monitor backend abstraction (real Win32 DDC/CI or an in-memory mock).

use crate::caps::Capabilities;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct MonitorInfo {
    /// Stable device interface path, e.g. `\\?\DISPLAY#AOC3202#...&UID4353#{...}`.
    pub id: String,
    /// GDI device, e.g. `\\.\DISPLAY1`.
    pub device: String,
    pub description: String,
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
    pub primary: bool,
    pub caps_raw: String,
    pub caps: Capabilities,
}

impl MonitorInfo {
    /// Short human id: the `UIDnnnn` part of the device path when present.
    pub fn short_id(&self) -> String {
        let upper = self.id.to_ascii_uppercase();
        if let Some(pos) = upper.find("UID") {
            let digits: String = upper[pos + 3..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if !digits.is_empty() {
                return format!("UID{digits}");
            }
        }
        let tail: String = self
            .device
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        tail.chars().rev().collect()
    }

    pub fn model(&self) -> String {
        self.caps
            .model
            .clone()
            .unwrap_or_else(|| self.description.clone())
    }

    /// Does selector `sel` pick this monitor? `index` is 0-based (left to right).
    pub fn matches(&self, index: usize, sel: &str) -> bool {
        let sel = sel.trim().to_ascii_lowercase();
        if sel.is_empty() {
            return false;
        }
        if sel.chars().all(|c| c.is_ascii_digit()) {
            return sel == (index + 1).to_string();
        }
        sel == "all"
            || self.short_id().to_ascii_lowercase() == sel
            || self.device.to_ascii_lowercase().trim_start_matches(r"\\.\") == sel
            || (sel.len() >= 3 && self.id.to_ascii_lowercase().contains(&sel))
    }
}

pub fn select<'a>(monitors: &'a [MonitorInfo], sel: &str) -> Vec<(usize, &'a MonitorInfo)> {
    monitors
        .iter()
        .enumerate()
        .filter(|(i, m)| m.matches(*i, sel))
        .collect()
}

pub trait Backend: Send {
    /// Enumerate monitors, sorted left to right.
    fn enumerate(&mut self) -> Result<Vec<MonitorInfo>, String>;
    /// `(current, maximum)`.
    fn get(&mut self, id: &str, code: u8) -> Result<(u32, u32), String>;
    fn set(&mut self, id: &str, code: u8, value: u32) -> Result<(), String>;
}

pub mod mock {
    use super::*;
    use crate::caps;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    pub const AOC_CAPS: &str = "(prot(monitor)type(LCD)model(C32G2ZE)cmds(01 02 03 07 0C E3 F3)vcp(02 04 05 08 0C 10 12 14(01 05 06 08 0B) 16 18 1A 52 60(0F 11 12 )62 86(01 02 05 0B 0C 0D 0E 0F 10 11 12 13 14) AC AE B2 B6 C6 C8 CA CC(01 02 03 04 05 06 07 09 0A 0B 0C 0D 0E 12 14 16 1E) D6(01 04 05) DC(00 0B 0C 0D 0E 0F 10) DF ED FF)mswhql(1)asset_eep(40)mccs_ver(2.2))";

    pub type Write = (String, u8, u32);
    pub type Values = HashMap<(String, u8), (u32, u32)>;

    pub struct MockBackend {
        pub monitors: Vec<MonitorInfo>,
        pub values: Arc<Mutex<Values>>,
        pub writes: Arc<Mutex<Vec<Write>>>,
        pub delay: Duration,
        pub fail_set: Vec<u8>,
    }

    impl MockBackend {
        /// Two AOC C32G2ZE monitors mirroring the user's real setup.
        pub fn two_aoc() -> Self {
            let mk = |n: i32, uid: u32, primary: bool| MonitorInfo {
                id: format!(r"\\?\DISPLAY#AOC3202#5&24a51c0f&0&UID{uid}#{{e6f07b5f}}"),
                device: format!(r"\\.\DISPLAY{}", n + 1),
                description: "Generic PnP Monitor".into(),
                left: n * 1920,
                top: 0,
                width: 1920,
                height: 1080,
                primary,
                caps_raw: AOC_CAPS.into(),
                caps: caps::parse(AOC_CAPS),
            };
            let monitors = vec![mk(0, 4353, true), mk(1, 4354, false)];
            let mut values = HashMap::new();
            for (m, (b, preset, input)) in monitors.iter().zip([(19, 5, 0x0F), (0, 11, 0x11)]) {
                for (code, v) in [
                    (0x10, b),
                    (0x12, 50),
                    (0x14, preset),
                    (0x16, 50),
                    (0x18, 50),
                    (0x1A, 50),
                    (0x60, input),
                    (0x62, 50),
                    (0x87, 50),
                    (0xDC, 0),
                ] {
                    values.insert((m.id.clone(), code), (v, 100));
                }
            }
            MockBackend {
                monitors,
                values: Arc::new(Mutex::new(values)),
                writes: Arc::new(Mutex::new(Vec::new())),
                delay: Duration::ZERO,
                fail_set: Vec::new(),
            }
        }
    }

    impl Backend for MockBackend {
        fn enumerate(&mut self) -> Result<Vec<MonitorInfo>, String> {
            Ok(self.monitors.clone())
        }

        fn get(&mut self, id: &str, code: u8) -> Result<(u32, u32), String> {
            self.values
                .lock()
                .unwrap()
                .get(&(id.to_string(), code))
                .copied()
                .ok_or_else(|| format!("VCP 0x{code:02X} not supported"))
        }

        fn set(&mut self, id: &str, code: u8, value: u32) -> Result<(), String> {
            std::thread::sleep(self.delay);
            if self.fail_set.contains(&code) {
                return Err(format!("set 0x{code:02X} failed"));
            }
            self.writes
                .lock()
                .unwrap()
                .push((id.to_string(), code, value));
            let mut vals = self.values.lock().unwrap();
            let max = vals
                .get(&(id.to_string(), code))
                .map(|v| v.1)
                .unwrap_or(100);
            vals.insert((id.to_string(), code), (value, max));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockBackend;
    use super::*;

    #[test]
    fn short_id_and_selectors() {
        let b = MockBackend::two_aoc();
        let m = &b.monitors;
        assert_eq!(m[0].short_id(), "UID4353");
        assert_eq!(m[0].model(), "C32G2ZE");
        assert_eq!(select(m, "all").len(), 2);
        assert_eq!(select(m, "2")[0].1.short_id(), "UID4354");
        assert_eq!(select(m, "uid4353")[0].0, 0);
        assert_eq!(select(m, "display2")[0].0, 1);
        assert!(select(m, "3").is_empty());
        assert!(select(m, "").is_empty());

        let bare = MonitorInfo {
            id: "x".into(),
            device: r"\\.\DISPLAY7".into(),
            ..Default::default()
        };
        assert_eq!(bare.short_id(), "DISPLAY7");
    }
}
