use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("../asrs-libm-lgpl/src");
    println!("cargo:rerun-if-changed={}", src.display());
    println!("cargo:rerun-if-env-changed=ASRS_LIBM_LGPL_SHIP");
    if env::var_os("ASRS_LIBM_LGPL_SHIP").is_some() {
        return;
    }

    let target = env::var("TARGET").unwrap();
    let file = match env::var("CARGO_CFG_TARGET_OS").unwrap().as_str() {
        "windows" => "asrs_libm_lgpl.dll",
        "macos" | "ios" => "libasrs_libm_lgpl.dylib",
        _ => "libasrs_libm_lgpl.so",
    };
    let lib = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join(file);
    let mut rustc = Command::new(env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()));
    rustc
        .args(["--crate-name", "asrs_libm_lgpl", "--crate-type", "cdylib"])
        .args([
            "--edition",
            "2024",
            "-C",
            "opt-level=3",
            "--target",
            &target,
        ])
        .arg(src.join("lib.rs"))
        .arg("-o")
        .arg(&lib);
    if let Some(linker) = env::var_os("RUSTC_LINKER") {
        let mut arg = OsString::from("linker=");
        arg.push(linker);
        rustc.arg("-C").arg(arg);
    }
    let status = rustc.status().expect("failed to run rustc");
    assert!(
        status.success(),
        "building asrs-libm-lgpl failed ({status})"
    );
    println!("cargo:rustc-env=ASRS_LIBM_LGPL_DEV={}", lib.display());
}
