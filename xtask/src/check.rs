use crate::{Result, cargo, gpui, layout};

const PARITY_FEATURES: &str = "asrs-ui/parity,asrs-app/parity";

pub fn run() -> Result {
    layout::run()?;
    gpui::verify_vendor()?;
    gpui::check_features()?;
    cargo(&["fmt", "--all", "--check"])?;
    cargo(&[
        "clippy",
        "--workspace",
        "--all-targets",
        "--locked",
        "--",
        "-D",
        "warnings",
    ])?;
    cargo(&[
        "clippy",
        "-p",
        "asrs-ui",
        "-p",
        "asrs-app",
        "--all-targets",
        "--locked",
        "--features",
        PARITY_FEATURES,
        "--",
        "-D",
        "warnings",
    ])?;
    cargo(&["test", "--workspace", "--locked"])?;
    cargo(&["test", "-p", "asrs-ui", "--locked", "--features", "parity"])?;
    eprintln!("check passed");
    Ok(())
}
