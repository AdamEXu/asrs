use std::fs;
use std::path::Path;
use std::process::Command;

use crate::{Result, libm, licenses, workspace_root};

const USAGE: &str = "usage: cargo xtask bundle [--target <triple>]

Builds a release directory in target/bundle/<triple or host>/: the advantagescope binary
(built with ASRS_LIBM_LGPL_SHIP=1, so it only loads the library next to it or from
ASRS_LIBM_LGPL), the asrs-libm-lgpl shared library, and the `cargo xtask licenses` bundle
including the library's LGPL text and source. Needs cargo-about.";

pub fn run(args: &[String]) -> Result {
    let target = match args {
        [] => None,
        [flag, triple] if flag == "--target" => Some(triple.as_str()),
        _ => return Err(USAGE.to_owned()),
    };
    let root = workspace_root();
    let mut cargo_args = vec!["build", "--locked", "--release", "-p", "asrs-app"];
    if let Some(target) = target {
        cargo_args.extend(["--target", target]);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    eprintln!("$ ASRS_LIBM_LGPL_SHIP=1 cargo {}", cargo_args.join(" "));
    let status = Command::new(cargo)
        .args(&cargo_args)
        .env("ASRS_LIBM_LGPL_SHIP", "1")
        .env_remove("ASRS_LIBM_LGPL_DEV")
        .current_dir(&root)
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("building asrs-app failed ({status})"));
    }
    let library = libm::build_library(true, target)?;
    licenses::run()?;

    let mut release = libm::target_dir();
    if let Some(target) = target {
        release.push(target);
    }
    release.push("release");
    let exe = if target.map_or(cfg!(windows), |t| t.contains("windows")) {
        "advantagescope.exe"
    } else {
        "advantagescope"
    };
    let dest = root.join("target/bundle").join(target.unwrap_or("host"));
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| format!("clearing {}: {e}", dest.display()))?;
    }
    copy_tree(&root.join("target/licenses"), &dest)?;
    copy(&release.join(exe), &dest.join(exe))?;
    copy(&library, &dest.join(libm::file_name(target)))?;
    eprintln!("wrote {}", dest.display());
    Ok(())
}

fn copy(src: &Path, dst: &Path) -> Result {
    fs::copy(src, dst)
        .map(drop)
        .map_err(|e| format!("{}: {e}", src.display()))
}

fn copy_tree(src: &Path, dst: &Path) -> Result {
    fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let to = dst.join(path.file_name().unwrap_or_default());
        if path.is_dir() {
            copy_tree(&path, &to)?;
        } else {
            copy(&path, &to)?;
        }
    }
    Ok(())
}
