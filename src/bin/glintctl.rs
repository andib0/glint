//! Console companion: `glintctl list | get | set | dump | apply | save ...`

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cache = glint::config::config_dir().join("caps.toml");
    let mut backend = glint::ddc_win::WinBackend::new(Some(cache));
    let cfg = glint::config::default_path();
    let mut out = std::io::stdout();
    if let Err(e) = glint::cli::run(&args, &mut backend, &cfg, &mut out) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
