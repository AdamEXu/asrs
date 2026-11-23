use crate::{Result, cargo, gpui, harness, layout, tables};

const PARITY_FEATURES: &str = "asrs-ui/parity,asrs-app/parity";

pub fn run() -> Result {
    layout::run()?;
    harness::run(&[])?;
    gpui::verify_vendor()?;
    gpui::check_features()?;
    tables::run(&["units".to_owned(), "--check".to_owned()])?;
    tables::run(&["spark".to_owned(), "--check".to_owned()])?;
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
    cargo(&[
        "test",
        "-p",
        "asrs-app",
        "--locked",
        "--features",
        "parity",
        "--test",
        "parity_scenario",
    ])?;
    eprintln!("check passed");
    Ok(())
}
