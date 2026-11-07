use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    // Common Controls v6 (modern trackbars/combos) + per-monitor-v2 DPI awareness.
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_manifest(new_manifest("Glint")).expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
