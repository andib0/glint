//! `glintctl` command implementation (backend-agnostic so it is testable).

use crate::backend::{Backend, MonitorInfo, select};
use crate::config::{self, Config};
use crate::profile;
use crate::vcp::{self, Kind};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

pub const HELP: &str = "\
glintctl - DDC/CI monitor control

USAGE:
  glintctl list                        monitors (number, id, device, model)
  glintctl caps   [MON]                raw + parsed capabilities
  glintctl dump   [MON]                every known setting with its value
  glintctl get    MON SETTING
  glintctl set    MON SETTING VALUE    VALUE: 40 | +10 | -10 | 0x11 | hdmi1 | srgb
  glintctl profiles                    list profiles from config
  glintctl apply  PROFILE
  glintctl save   PROFILE              save current state as a profile
  glintctl config                      print config path

MON: all | 1 | 2 (left to right) | UID4353 | display1     (default: all)
SETTING: brightness contrast sharpness red green blue volume
         color_preset game_mode input  | raw code like 0x87
";

type Out<'a> = &'a mut dyn Write;

macro_rules! w {
    ($out:expr, $($arg:tt)*) => { writeln!($out, $($arg)*).map_err(|e| e.to_string()) };
}

pub fn run(
    args: &[String],
    backend: &mut dyn Backend,
    cfg_path: &Path,
    out: Out,
) -> Result<(), String> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match a.as_slice() {
        [] | ["help"] | ["-h"] | ["--help"] => w!(out, "{HELP}"),
        ["config"] => w!(out, "{}", cfg_path.display()),
        ["profiles"] => {
            let cfg = config::load_or_create(cfg_path)?;
            for p in &cfg.profiles {
                let n = p.all.len() + p.monitors.values().map(|m| m.len()).sum::<usize>();
                w!(out, "{:<16} {n} setting(s)", p.name)?;
            }
            Ok(())
        }
        ["list"] => {
            let mons = backend.enumerate()?;
            if mons.is_empty() {
                return Err("no DDC/CI monitors found".into());
            }
            for (i, m) in mons.iter().enumerate() {
                w!(
                    out,
                    "{}  {:<8} {:<12} {:<10} {}x{} at ({},{}){}",
                    i + 1,
                    m.short_id(),
                    m.device,
                    m.model(),
                    m.width,
                    m.height,
                    m.left,
                    m.top,
                    if m.primary { "  primary" } else { "" }
                )?;
            }
            Ok(())
        }
        ["caps"] => caps(backend, "all", out),
        ["caps", sel] => caps(backend, sel, out),
        ["dump"] => dump(backend, "all", out),
        ["dump", sel] => dump(backend, sel, out),
        ["get", sel, setting] => {
            let code = code(setting)?;
            for (i, m) in pick(backend, sel)? {
                let (cur, max) = backend.get(&m.id, code)?;
                w!(
                    out,
                    "{} {}: {}",
                    i + 1,
                    vcp::key_for(code),
                    fmt_value(code, cur, max)
                )?;
            }
            Ok(())
        }
        ["set", sel, setting, value] => {
            let code = code(setting)?;
            for (i, m) in pick(backend, sel)? {
                let target = if let Some(delta) = parse_delta(value) {
                    let (cur, max) = backend.get(&m.id, code)?;
                    (i64::from(cur) + delta).clamp(0, i64::from(max)) as u32
                } else {
                    vcp::parse_value(code, value).ok_or_else(|| format!("bad value '{value}'"))?
                };
                backend.set(&m.id, code, target)?;
                std::thread::sleep(std::time::Duration::from_millis(vcp::settle_ms(code)));
                w!(out, "{} {} = {}", i + 1, vcp::key_for(code), target)?;
            }
            Ok(())
        }
        ["apply", name] => {
            let cfg = config::load_or_create(cfg_path)?;
            let p = cfg
                .profile(name)
                .ok_or_else(|| format!("no profile '{name}'"))?;
            let mons = backend.enumerate()?;
            let (writes, warnings) = profile::resolve(p, &mons);
            for wr in warnings {
                w!(out, "warning: {wr}")?;
            }
            let mut errors = 0;
            for (id, code, v) in &writes {
                match backend.set(id, *code, *v) {
                    Ok(()) => {
                        std::thread::sleep(std::time::Duration::from_millis(vcp::settle_ms(*code)))
                    }
                    Err(e) => {
                        errors += 1;
                        w!(out, "error: {} 0x{code:02X}: {e}", short(id))?;
                    }
                }
            }
            w!(
                out,
                "applied '{}' ({} write(s), {errors} error(s))",
                p.name,
                writes.len()
            )?;
            if errors > 0 {
                Err("some writes failed".into())
            } else {
                Ok(())
            }
        }
        ["save", name] => {
            let mut cfg: Config = config::load_or_create(cfg_path)?;
            let mons = backend.enumerate()?;
            let values = read_all(backend, &mons);
            let p = profile::snapshot(name, &mons, &values);
            let n: usize = p.monitors.values().map(|m| m.len()).sum();
            cfg.upsert_profile(p);
            config::save(cfg_path, &cfg)?;
            w!(out, "saved profile '{name}' ({n} setting(s))")
        }
        _ => Err(format!("unknown command: {}\n\n{HELP}", a.join(" "))),
    }
}

fn code(s: &str) -> Result<u8, String> {
    vcp::parse_code(s).ok_or_else(|| format!("unknown setting '{s}'"))
}

fn parse_delta(s: &str) -> Option<i64> {
    if s.starts_with('+') || (s.starts_with('-') && s.len() > 1) {
        s.parse().ok()
    } else {
        None
    }
}

fn pick(backend: &mut dyn Backend, sel: &str) -> Result<Vec<(usize, MonitorInfo)>, String> {
    let mons = backend.enumerate()?;
    let hits: Vec<_> = select(&mons, sel)
        .into_iter()
        .map(|(i, m)| (i, m.clone()))
        .collect();
    if hits.is_empty() {
        return Err(format!("no monitor matches '{sel}' (try `glintctl list`)"));
    }
    Ok(hits)
}

fn fmt_value(code: u8, cur: u32, max: u32) -> String {
    match vcp::by_code(code).map(|f| f.kind) {
        Some(Kind::Choice(_)) => format!("{} ({})", vcp::choice_label(code, cur & 0xFF), cur),
        _ => format!("{cur}/{max}"),
    }
}

fn caps(backend: &mut dyn Backend, sel: &str, out: Out) -> Result<(), String> {
    for (i, m) in pick(backend, sel)? {
        w!(out, "== {} {} ({})", i + 1, m.short_id(), m.model())?;
        w!(out, "raw: {}", m.caps_raw)?;
        for (code, vals) in &m.caps.vcp {
            let name = vcp::by_code(*code).map(|f| f.key).unwrap_or("");
            let vals: Vec<String> = vals.iter().map(|v| format!("{v:02X}")).collect();
            w!(out, "  0x{code:02X} {name:<13} {}", vals.join(" "))?;
        }
    }
    Ok(())
}

fn dump(backend: &mut dyn Backend, sel: &str, out: Out) -> Result<(), String> {
    for (i, m) in pick(backend, sel)? {
        w!(out, "== {} {} ({})", i + 1, m.short_id(), m.model())?;
        for f in vcp::FEATURES {
            match backend.get(&m.id, f.code) {
                Ok((cur, max)) => w!(out, "  {:<13} {}", f.key, fmt_value(f.code, cur, max))?,
                Err(_) => w!(out, "  {:<13} -", f.key)?,
            }
        }
    }
    Ok(())
}

fn read_all(backend: &mut dyn Backend, mons: &[MonitorInfo]) -> HashMap<(String, u8), (u32, u32)> {
    let mut values = HashMap::new();
    for m in mons {
        for f in vcp::FEATURES {
            if let Ok(v) = backend.get(&m.id, f.code) {
                values.insert((m.id.clone(), f.code), v);
            }
        }
    }
    values
}

fn short(id: &str) -> String {
    MonitorInfo {
        id: id.to_string(),
        ..Default::default()
    }
    .short_id()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;
    use std::path::PathBuf;

    fn tmp_cfg(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("glint-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("config.toml")
    }

    fn run_str(b: &mut MockBackend, cfg: &Path, args: &str) -> Result<String, String> {
        let args: Vec<String> = args.split_whitespace().map(String::from).collect();
        let mut out = Vec::new();
        run(&args, b, cfg, &mut out)?;
        Ok(String::from_utf8(out).unwrap())
    }

    #[test]
    fn list_get_dump_caps() {
        let mut b = MockBackend::two_aoc();
        let cfg = tmp_cfg("list");
        let out = run_str(&mut b, &cfg, "list").unwrap();
        assert!(out.contains("1  UID4353") && out.contains("primary") && out.contains("C32G2ZE"));
        assert_eq!(
            run_str(&mut b, &cfg, "get 2 input").unwrap().trim(),
            "2 input: HDMI 1 (17)"
        );
        assert_eq!(
            run_str(&mut b, &cfg, "get 1 brightness").unwrap().trim(),
            "1 brightness: 19/100"
        );
        let d = run_str(&mut b, &cfg, "dump uid4354").unwrap();
        assert!(d.contains("color_preset  User (11)"));
        let c = run_str(&mut b, &cfg, "caps 1").unwrap();
        assert!(c.contains("0x14 color_preset  01 05 06 08 0B"));
    }

    #[test]
    fn set_absolute_relative_and_labels() {
        let mut b = MockBackend::two_aoc();
        let cfg = tmp_cfg("set");
        run_str(&mut b, &cfg, "set all brightness 40").unwrap();
        run_str(&mut b, &cfg, "set 1 brightness +70").unwrap();
        run_str(&mut b, &cfg, "set 2 brightness -5").unwrap();
        run_str(&mut b, &cfg, "set 2 color_preset srgb").unwrap();
        run_str(&mut b, &cfg, "set 1 0x87 0x20").unwrap();
        let v = b.values.lock().unwrap();
        let ids: Vec<String> = b.monitors.iter().map(|m| m.id.clone()).collect();
        assert_eq!(v[&(ids[0].clone(), 0x10)].0, 100, "clamped to max");
        assert_eq!(v[&(ids[1].clone(), 0x10)].0, 35);
        assert_eq!(v[&(ids[1].clone(), 0x14)].0, 1);
        assert_eq!(v[&(ids[0].clone(), 0x87)].0, 0x20);
    }

    #[test]
    fn errors_are_reported() {
        let mut b = MockBackend::two_aoc();
        let cfg = tmp_cfg("err");
        assert!(
            run_str(&mut b, &cfg, "get 3 brightness")
                .unwrap_err()
                .contains("no monitor")
        );
        assert!(
            run_str(&mut b, &cfg, "get 1 sparkle")
                .unwrap_err()
                .contains("unknown setting")
        );
        assert!(
            run_str(&mut b, &cfg, "set 1 input hdmi9")
                .unwrap_err()
                .contains("bad value")
        );
        assert!(
            run_str(&mut b, &cfg, "frobnicate")
                .unwrap_err()
                .contains("unknown command")
        );
        assert!(
            run_str(&mut b, &cfg, "apply Nope")
                .unwrap_err()
                .contains("no profile")
        );
        assert!(run_str(&mut b, &cfg, "help").unwrap().contains("USAGE"));
    }

    #[test]
    fn save_then_apply_profile() {
        let mut b = MockBackend::two_aoc();
        let cfg = tmp_cfg("save");
        assert!(run_str(&mut b, &cfg, "profiles").unwrap().contains("Night"));
        run_str(&mut b, &cfg, "set all brightness 33").unwrap();
        assert!(
            run_str(&mut b, &cfg, "save Mine")
                .unwrap()
                .contains("saved profile 'Mine'")
        );
        run_str(&mut b, &cfg, "set all brightness 90").unwrap();
        let out = run_str(&mut b, &cfg, "apply mine").unwrap();
        assert!(out.contains("0 error(s)"), "{out}");
        let ids: Vec<String> = b.monitors.iter().map(|m| m.id.clone()).collect();
        for id in ids {
            assert_eq!(b.values.lock().unwrap()[&(id, 0x10)].0, 33);
        }
        let out = run_str(&mut b, &cfg, "apply Night").unwrap();
        assert!(out.contains("applied 'Night'"));
        let _ = std::fs::remove_dir_all(cfg.parent().unwrap());
    }
}
