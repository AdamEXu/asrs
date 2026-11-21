//! Loader for `asrs-libm-lgpl`, the LGPL-2.1-or-later shared library that holds V8's glibc
//! `sin` and `cos`. Keeping it a separately loaded library lets users swap in a modified build
//! without relinking asrs (LGPL-2.1 section 6).
//!
//! The library is looked up, first match wins:
//! 1. at the path in `ASRS_LIBM_LGPL`, if set (and nowhere else),
//! 2. next to the running executable, which is where releases ship it,
//! 3. in development builds, the copy `asrs-js`'s build script compiled from
//!    `crates/asrs-libm-lgpl` (left out when built with `ASRS_LIBM_LGPL_SHIP` set).

#![allow(unsafe_code)]

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use libloading::Library;

pub const PATH_ENV: &str = "ASRS_LIBM_LGPL";
pub const ABI_VERSION: u32 = 1;

type UnaryFn = extern "C" fn(f64) -> f64;

struct Libm {
    sin: UnaryFn,
    cos: UnaryFn,
    path: PathBuf,
    _library: Library,
}

static LIBM: OnceLock<Result<Libm, String>> = OnceLock::new();

/// Loads the library if needed and returns the path it was loaded from.
pub fn load() -> Result<&'static Path, &'static str> {
    match LIBM.get_or_init(open) {
        Ok(libm) => Ok(&libm.path),
        Err(e) => Err(e),
    }
}

pub fn file_name() -> OsString {
    libloading::library_filename("asrs_libm_lgpl")
}

fn libm() -> &'static Libm {
    match LIBM.get_or_init(open) {
        Ok(libm) => libm,
        Err(e) => panic!("{e}"),
    }
}

pub(super) fn sin(x: f64) -> f64 {
    (libm().sin)(x)
}

pub(super) fn cos(x: f64) -> f64 {
    (libm().cos)(x)
}

fn candidates() -> Vec<PathBuf> {
    if let Some(path) = env::var_os(PATH_ENV) {
        return vec![PathBuf::from(path)];
    }
    let mut paths = Vec::new();
    if let Some(dir) = env::current_exe().ok().as_deref().and_then(Path::parent) {
        paths.push(dir.join(file_name()));
    }
    if let Some(dev) = option_env!("ASRS_LIBM_LGPL_DEV") {
        paths.push(PathBuf::from(dev));
    }
    paths
}

fn open() -> Result<Libm, String> {
    let paths = candidates();
    match paths.iter().find(|p| p.is_file()) {
        Some(path) => open_at(path).map_err(|e| format!("loading {}: {e}", path.display())),
        None => Err(format!(
            "the asrs-libm-lgpl library ({}) was not found; looked at: {}. Build it with \
             `cargo xtask libm` or set {PATH_ENV} to its path.",
            file_name().to_string_lossy(),
            paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

fn open_at(path: &Path) -> Result<Libm, String> {
    // SAFETY: loading runs the library's initializers; asrs-libm-lgpl has none, and a
    // replacement is trusted the same way as the executable next to it.
    let library = unsafe { Library::new(path) }.map_err(|e| e.to_string())?;
    // SAFETY: the symbol types are the C ABI documented in crates/asrs-libm-lgpl/README.md,
    // checked through asrs_libm_lgpl_abi_version before the math symbols are used.
    let (abi_version, sin, cos) = unsafe {
        let abi_version = *library
            .get::<extern "C" fn() -> u32>(b"asrs_libm_lgpl_abi_version\0")
            .map_err(|e| e.to_string())?;
        let sin = *library
            .get::<UnaryFn>(b"asrs_libm_lgpl_sin\0")
            .map_err(|e| e.to_string())?;
        let cos = *library
            .get::<UnaryFn>(b"asrs_libm_lgpl_cos\0")
            .map_err(|e| e.to_string())?;
        (abi_version, sin, cos)
    };
    let version = abi_version();
    if version != ABI_VERSION {
        return Err(format!(
            "ABI version {version}, but asrs needs version {ABI_VERSION}"
        ));
    }
    Ok(Libm {
        sin,
        cos,
        path: path.to_owned(),
        _library: library,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_the_library() {
        let path = load().unwrap();
        assert!(path.ends_with(file_name()), "{}", path.display());
        assert_eq!(sin(1.0).to_bits(), 0x3feaed548f090cee);
        assert_eq!(cos(1.0).to_bits(), 0x3fe14a280fb5068c);
    }
}
