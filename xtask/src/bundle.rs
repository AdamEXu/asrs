use std::fs;
use std::path::Path;
use std::process::Command;

use crate::{Result, docs, libm, licenses, workspace_root};

const USAGE: &str = "usage: cargo xtask bundle [--target <triple>]

Builds a release directory in target/bundle/<triple or host>/: the asrs binary
(built with ASRS_LIBM_LGPL_SHIP=1, so it only loads the library next to it or from
ASRS_LIBM_LGPL, and ASRS_PACKAGED=1, so it runs as a packaged app with bundledAssets/ beside
it), the asrs-libm-lgpl shared library, on Linux asrs-webview (the only binary that links
WebKitGTK), upstream's bundledAssets/ (from assets/ or
reference/AdvantageScope), the documentation `cargo xtask docs` built (as docs/build), and the
`cargo xtask licenses` bundle including the library's LGPL
text and source. Needs cargo-about.";

const WEBVIEW: &str = "asrs-webview";

pub fn run(args: &[String]) -> Result {
    let target = match args {
        [] => None,
        [flag, triple] if flag == "--target" => Some(triple.as_str()),
        _ => return Err(USAGE.to_owned()),
    };
    let root = workspace_root();
    let bundled_assets = [
        root.join("assets/bundledAssets"),
        root.join("reference/AdvantageScope/bundledAssets"),
    ]
    .into_iter()
    .find(|dir| dir.is_dir())
    .ok_or("upstream bundledAssets not found; link reference/AdvantageScope or run `cargo xtask assets`")?;
    let documentation = docs::build_dir();
    if !documentation.join("index.html").is_file() {
        return Err("the documentation is not built; run `cargo xtask docs`".to_owned());
    }
    let linux = target.map_or(cfg!(target_os = "linux"), |t| t.contains("linux"));
    let mut cargo_args = vec!["build", "--locked", "--release", "-p", "asrs-app"];
    if linux {
        cargo_args.extend(["-p", WEBVIEW]);
    }
    if let Some(target) = target {
        cargo_args.extend(["--target", target]);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    eprintln!(
        "$ ASRS_LIBM_LGPL_SHIP=1 ASRS_PACKAGED=1 cargo {}",
        cargo_args.join(" ")
    );
    let status = Command::new(cargo)
        .args(&cargo_args)
        .env("ASRS_LIBM_LGPL_SHIP", "1")
        .env("ASRS_PACKAGED", "1")
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
        "asrs.exe"
    } else {
        "asrs"
    };
    let dest = root.join("target/bundle").join(target.unwrap_or("host"));
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| format!("clearing {}: {e}", dest.display()))?;
    }
    copy_tree(&root.join("target/licenses"), &dest)?;
    copy(&release.join(exe), &dest.join(exe))?;
    if linux {
        check_startup_libraries(&dest.join(exe))?;
        copy(&release.join(WEBVIEW), &dest.join(WEBVIEW))?;
    }
    copy(&library, &dest.join(libm::file_name(target)))?;
    copy_tree(&bundled_assets, &dest.join("bundledAssets"))?;
    copy_tree(&documentation, &dest.join("docs/build"))?;
    eprintln!("wrote {}", dest.display());
    Ok(())
}

/// asrs must start on machines without WebKitGTK, so it may not name it as a needed library.
fn check_startup_libraries(exe: &Path) -> Result {
    let bytes = fs::read(exe).map_err(|e| format!("{}: {e}", exe.display()))?;
    for library in ["libwebkit2gtk-", "libgtk-3.so", "libjavascriptcoregtk-"] {
        if bytes
            .windows(library.len())
            .any(|window| window == library.as_bytes())
        {
            return Err(format!("{} links {library}", exe.display()));
        }
    }
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
        if path.file_name().is_some_and(|n| n == ".DS_Store") {
            continue;
        }
        let to = dst.join(path.file_name().unwrap_or_default());
        if path.is_dir() {
            copy_tree(&path, &to)?;
        } else {
            copy(&path, &to)?;
        }
    }
    Ok(())
}
