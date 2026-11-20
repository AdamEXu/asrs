mod frames;
mod headless;
#[cfg(target_os = "macos")]
mod mac;
mod shell;

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(
        std::env::var("S1_OUT").unwrap_or_else(|_| "/Volumes/T9/asrs/target/spikes/s1-out".into()),
    );
    match args.get(1).map(String::as_str) {
        Some("auto") => shell::run_auto(out),
        Some("headless") => headless::run(out),
        Some("image-exact") => headless::image_exactness(),
        Some("frames") => {
            let w = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(2200);
            let h = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(1300);
            let secs = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(10.0);
            let drop_old = args.get(5).map(|s| s != "nodrop").unwrap_or(true);
            frames::run(w, h, secs, drop_old);
        }
        _ => shell::run_interactive(),
    }
}
