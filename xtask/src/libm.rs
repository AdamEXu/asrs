use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crate::{Result, cargo, workspace_root};

const USAGE: &str = "usage: cargo xtask libm [--release] [--target <triple>] [--out <dir>]
       cargo xtask libm plant

Builds crates/asrs-libm-lgpl, the LGPL-2.1-or-later shared library with Math.sin and
Math.cos, into target/[<triple>/]<profile>/, and copies it into <dir> with --out. Release
builds (`cargo xtask bundle`) load it from next to their executable or from the path in
ASRS_LIBM_LGPL; development builds use the copy asrs-js's build script compiles from the
crate instead of the one next to the executable. To relink a release against a modified
library, change the crate and run this command with --release --out <its directory> (or
point ASRS_LIBM_LGPL at the result).

plant builds an unmodified copy and one with a table entry off by one bit, and requires the
committed math vectors to pass against the first and report differing js/math cases against
the second, proving the tests run the loaded library.";

const PLANT_FROM: &str = "0x3feaef78930bd275";
const PLANT_TO: &str = "0x3feaef78930bd274";

pub fn run(args: &[String]) -> Result {
    match args.first().map(String::as_str) {
        Some("plant") => plant(),
        Some("help" | "--help" | "-h") => Err(USAGE.to_owned()),
        _ => build(args),
    }
}

pub fn file_name(target: Option<&str>) -> &'static str {
    let os = target.map_or(std::env::consts::OS, |t| {
        if t.contains("windows") {
            "windows"
        } else if t.contains("apple") {
            "macos"
        } else {
            "linux"
        }
    });
    match os {
        "windows" => "asrs_libm_lgpl.dll",
        "macos" => "libasrs_libm_lgpl.dylib",
        _ => "libasrs_libm_lgpl.so",
    }
}

fn build(args: &[String]) -> Result {
    let mut release = false;
    let mut target = None;
    let mut out = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--release" => release = true,
            "--target" => target = Some(it.next().ok_or(USAGE)?.clone()),
            "--out" => out = Some(PathBuf::from(it.next().ok_or(USAGE)?)),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let path = build_library(release, target.as_deref())?;
    match out {
        Some(dir) => {
            fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let dest = dir.join(file_name(target.as_deref()));
            fs::copy(&path, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            eprintln!("wrote {}", dest.display());
        }
        None => eprintln!("built {}", path.display()),
    }
    Ok(())
}

pub fn build_library(release: bool, target: Option<&str>) -> Result<PathBuf> {
    let mut args = vec!["build", "--locked", "-p", "asrs-libm-lgpl"];
    if release {
        args.push("--release");
    }
    if let Some(target) = target {
        args.extend(["--target", target]);
    }
    cargo(&args)?;
    let mut dir = target_dir();
    if let Some(target) = target {
        dir.push(target);
    }
    dir.push(if release { "release" } else { "debug" });
    Ok(dir.join(file_name(target)))
}

/// Cargo's build directory, where `--out` and `cargo xtask bundle` read artifacts from.
pub fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
}

fn plant() -> Result {
    let root = workspace_root();
    let crate_src = root.join("crates/asrs-libm-lgpl/src");
    let table = fs::read_to_string(crate_src.join("sincostab.rs")).map_err(|e| e.to_string())?;
    if !table.contains(PLANT_FROM) {
        return Err(format!(
            "plant target {PLANT_FROM} not found in sincostab.rs"
        ));
    }
    let dir = root.join("target/libm-plant");
    let control = build_copy(&crate_src, &dir.join("control"), &table)?;
    let planted = build_copy(
        &crate_src,
        &dir.join("planted"),
        &table.replacen(PLANT_FROM, PLANT_TO, 1),
    )?;

    let out = math_vectors(&control)?;
    if !out.status.success() {
        return Err("the math vectors fail against the unmodified library".to_owned());
    }
    let out = math_vectors(&planted)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let caught = !out.status.success()
        && stdout
            .lines()
            .any(|l| l.contains("vectors js/math: ") && l.ends_with(" cases differ"));
    if !caught {
        return Err(if out.status.success() {
            "the planted library passed the math vectors".to_owned()
        } else {
            "the math vectors failed against the planted library without reporting differing \
             js/math cases"
                .to_owned()
        });
    }
    eprintln!("plant caught: js/math cases differ against the planted library");
    Ok(())
}

fn build_copy(crate_src: &Path, dir: &Path, table: &str) -> Result<PathBuf> {
    let src = dir.join("src");
    fs::create_dir_all(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    for name in ["lib.rs", "glibc.rs"] {
        fs::copy(crate_src.join(name), src.join(name)).map_err(|e| format!("{name}: {e}"))?;
    }
    fs::write(src.join("sincostab.rs"), table).map_err(|e| e.to_string())?;
    let lib = dir.join(file_name(None));
    compile(&src.join("lib.rs"), &lib)?;
    Ok(lib)
}

fn math_vectors(lib: &Path) -> Result<Output> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    eprintln!(
        "$ ASRS_LIBM_LGPL={} cargo test -p asrs-js --test math_vectors",
        lib.display()
    );
    let out = Command::new(cargo)
        .args([
            "test",
            "--locked",
            "-p",
            "asrs-js",
            "--test",
            "math_vectors",
        ])
        .env("ASRS_LIBM_LGPL", lib)
        .current_dir(workspace_root())
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    print!("{}", String::from_utf8_lossy(&out.stdout));
    Ok(out)
}

fn compile(lib_rs: &Path, out: &Path) -> Result {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".to_owned());
    let status = Command::new(rustc)
        .args(["--crate-name", "asrs_libm_lgpl", "--crate-type", "cdylib"])
        .args(["--edition", "2024", "-C", "opt-level=3"])
        .arg(lib_rs)
        .arg("-o")
        .arg(out)
        .current_dir(workspace_root())
        .status()
        .map_err(|e| format!("failed to run rustc: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("rustc failed ({status})"))
    }
}
