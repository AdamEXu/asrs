use crate::{Result, cargo, layout};

pub fn run() -> Result {
    layout::run()?;
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
    cargo(&["test", "--workspace", "--locked"])?;
    eprintln!("check passed");
    Ok(())
}
