//! Real-monitor tests. They change settings briefly and restore them.
//! Run with: `cargo test --test hardware -- --ignored --test-threads 1`

#![cfg(windows)]

use glint::backend::Backend;
use glint::ddc_win::WinBackend;
use glint::vcp;

fn backend() -> WinBackend {
    WinBackend::new(None)
}

#[test]
#[ignore]
fn enumerates_ddc_monitors_with_caps() {
    let mut b = backend();
    let mons = b.enumerate().expect("enumerate");
    assert!(!mons.is_empty(), "no DDC/CI monitors");
    for w in mons.windows(2) {
        assert!(w[0].left <= w[1].left, "sorted left to right");
    }
    for m in &mons {
        assert!(
            m.caps.supports(vcp::BRIGHTNESS),
            "{} caps: {}",
            m.short_id(),
            m.caps_raw
        );
        assert!(m.short_id().starts_with("UID"), "{}", m.id);
    }
}

#[test]
#[ignore]
fn reads_every_catalog_feature() {
    let mut b = backend();
    for m in b.enumerate().unwrap() {
        for f in vcp::FEATURES {
            let (cur, max) = b
                .get(&m.id, f.code)
                .unwrap_or_else(|e| panic!("{} {}: {e}", m.short_id(), f.key));
            if f.kind == vcp::Kind::Continuous {
                assert!(
                    cur <= max && max > 0,
                    "{} {} = {cur}/{max}",
                    m.short_id(),
                    f.key
                );
            }
        }
    }
}

#[test]
#[ignore]
fn write_read_back_and_restore() {
    let mut b = backend();
    for m in b.enumerate().unwrap() {
        for code in [vcp::BRIGHTNESS, vcp::CONTRAST, vcp::SHARPNESS] {
            let (orig, max) = b.get(&m.id, code).unwrap();
            // +-10: the AOC quantizes sharpness to steps of 10
            let probe = if orig + 10 <= max {
                orig + 10
            } else {
                orig - 10
            };
            b.set(&m.id, code, probe).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(100));
            let (now, _) = b.get(&m.id, code).unwrap();
            b.set(&m.id, code, orig).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert_eq!(now, probe, "{} 0x{code:02X} read-back", m.short_id());
            assert_eq!(
                b.get(&m.id, code).unwrap().0,
                orig,
                "{} 0x{code:02X} restored",
                m.short_id()
            );
        }
    }
}

#[test]
#[ignore]
fn missing_monitor_is_an_error_not_a_crash() {
    let mut b = backend();
    assert!(b.get("nope", vcp::BRIGHTNESS).is_err());
    assert!(b.set("nope", vcp::BRIGHTNESS, 1).is_err());
}
