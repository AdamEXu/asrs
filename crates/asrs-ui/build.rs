#![allow(clippy::disallowed_methods)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

fn symbols_dir(root: &Path) -> PathBuf {
    let candidates = [
        root.join("reference/AdvantageScope/www/symbols"),
        root.join("reference/www/symbols"),
        root.join("assets/www/symbols"),
    ];
    for dir in &candidates {
        println!("cargo:rerun-if-changed={}", dir.display());
        if dir.join("add.svg").is_file() {
            return dir.clone();
        }
    }
    panic!(
        "upstream www/symbols not found; link reference/AdvantageScope or run `cargo xtask assets` (looked in {})",
        candidates.map(|c| c.display().to_string()).join(", ")
    );
}

fn svgs(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read symbols dir")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with(".svg"))
        .collect();
    names.sort();
    names
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let dir = symbols_dir(&root);
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    for name in svgs(&dir) {
        entries.push((name.clone(), dir.join(&name)));
    }
    for name in svgs(&dir.join("sourceList")) {
        entries.push((
            format!("sourceList/{name}"),
            dir.join("sourceList").join(&name),
        ));
    }
    let mut out = String::from("pub(crate) static EMBEDDED: &[(&str, &[u8])] = &[\n");
    for (name, path) in &entries {
        let path = fs::canonicalize(path).expect("canonical symbol path");
        writeln!(
            out,
            "    ({name:?}, include_bytes!({:?})),",
            path.display().to_string()
        )
        .unwrap();
    }
    out.push_str("];\n");
    for (i, (name, _)) in entries.iter().enumerate() {
        if let Some(stem) = name.strip_suffix(".svg").filter(|s| !s.contains('/')) {
            writeln!(
                out,
                "pub const {}: Symbol = Symbol({i});",
                stem.to_uppercase()
            )
            .unwrap();
        }
    }
    let dest = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("symbols.rs");
    fs::write(dest, out).expect("write symbols.rs");
}
