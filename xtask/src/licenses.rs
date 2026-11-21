use std::fs;
use std::path::Path;

use crate::{Result, cargo, workspace_root};

const BUNDLED: &[&str] = &["LICENSE", "THIRD_PARTY_NOTICES.md"];
const LIBM_CRATE: &str = "crates/asrs-libm-lgpl";
const LIBM_STANDALONE_MANIFEST: &str = concat!(
    r#"[package]
name = "asrs-libm-lgpl"
version = ""#,
    env!("CARGO_PKG_VERSION"),
    r#""
edition = "2024"
license = "LGPL-2.1-or-later"
publish = false

[lib]
name = "asrs_libm_lgpl"
crate-type = ["cdylib"]

[workspace]
"#
);

pub fn run() -> Result {
    let root = workspace_root();
    let dest = root.join("target/licenses");
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| format!("clearing {}: {e}", dest.display()))?;
    }
    fs::create_dir_all(&dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    let listing = dest.join("THIRD_PARTY_LICENSES.txt");
    let listing_arg = listing.to_string_lossy().into_owned();
    cargo(&[
        "about",
        "generate",
        "--locked",
        "--fail",
        "--config",
        "about.toml",
        "--manifest-path",
        "crates/asrs-app/Cargo.toml",
        "--output-file",
        &listing_arg,
        "about.hbs",
    ])
    .map_err(|e| {
        format!("{e}\n(needs cargo-about: `cargo install cargo-about --locked --features cli`)")
    })?;
    for name in BUNDLED {
        fs::copy(root.join(name), dest.join(name)).map_err(|e| format!("{name}: {e}"))?;
    }
    let texts = copy_dir(&root.join("licenses"), &dest.join("licenses"))?;
    copy_libm_source(&root.join(LIBM_CRATE), &dest.join("asrs-libm-lgpl"))?;
    eprintln!(
        "wrote {} with {texts} license texts for release bundles",
        dest.display()
    );
    Ok(())
}

fn copy_libm_source(src: &Path, dst: &Path) -> Result {
    copy_dir(&src.join("src"), &dst.join("src"))?;
    for name in ["LICENSE", "README.md"] {
        fs::copy(src.join(name), dst.join(name)).map_err(|e| format!("{name}: {e}"))?;
    }
    fs::write(dst.join("Cargo.toml"), LIBM_STANDALONE_MANIFEST).map_err(|e| e.to_string())
}

fn copy_dir(src: &Path, dst: &Path) -> Result<usize> {
    let err = |e: std::io::Error| format!("{}: {e}", src.display());
    fs::create_dir_all(dst).map_err(err)?;
    let mut count = 0;
    for entry in fs::read_dir(src).map_err(err)? {
        let entry = entry.map_err(err)?;
        fs::copy(entry.path(), dst.join(entry.file_name())).map_err(err)?;
        count += 1;
    }
    Ok(count)
}
