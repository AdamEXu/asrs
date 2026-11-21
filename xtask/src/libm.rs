use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Result, cargo, workspace_root};

const USAGE: &str = "usage: cargo xtask libm [--release] [--target <triple>] [--out <dir>]
       cargo xtask libm plant

Builds crates/asrs-libm-lgpl, the LGPL-2.1-or-later shared library with Math.sin and
Math.cos, into target/[<triple>/]<profile>/ next to the asrs binaries, and copies it into
<dir> with --out. asrs loads it at runtime from next to its executable, or from the path in
ASRS_LIBM_LGPL. To relink asrs against a modified library, change the crate, run this
command, and put the result next to the executable (or point ASRS_LIBM_LGPL at it).

plant builds a copy with one table entry off by one bit and checks that the committed math
vectors fail against it, proving the tests run the loaded library.";

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
    let mut dir = workspace_root().join("target");
    if let Some(target) = target {
        dir.push(target);
    }
    dir.push(if release { "release" } else { "debug" });
    Ok(dir.join(file_name(target)))
}

fn plant() -> Result {
    let root = workspace_root();
    let dir = root.join("target/libm-plant");
    let src = dir.join("src");
    fs::create_dir_all(&src).map_err(|e| format!("{}: {e}", src.display()))?;
    let crate_src = root.join("crates/asrs-libm-lgpl/src");
    for name in ["lib.rs", "glibc.rs", "sincostab.rs"] {
        fs::copy(crate_src.join(name), src.join(name)).map_err(|e| format!("{name}: {e}"))?;
    }
    let table = src.join("sincostab.rs");
    let text = fs::read_to_string(&table).map_err(|e| e.to_string())?;
    if !text.contains(PLANT_FROM) {
        return Err(format!(
            "plant target {PLANT_FROM} not found in sincostab.rs"
        ));
    }
    fs::write(&table, text.replacen(PLANT_FROM, PLANT_TO, 1)).map_err(|e| e.to_string())?;
    let lib = dir.join(file_name(None));
    compile(&src.join("lib.rs"), &lib)?;

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    eprintln!(
        "$ ASRS_LIBM_LGPL={} cargo test -p asrs-js --test math_vectors",
        lib.display()
    );
    let status = Command::new(cargo)
        .args([
            "test",
            "--locked",
            "-p",
            "asrs-js",
            "--test",
            "math_vectors",
        ])
        .env("ASRS_LIBM_LGPL", &lib)
        .current_dir(&root)
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if status.success() {
        Err("the planted library passed the math vectors".to_owned())
    } else {
        eprintln!("plant caught: the math vectors fail against the planted library");
        Ok(())
    }
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
