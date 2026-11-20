use std::fs;
use std::path::Path;
use std::process::Command;

use crate::{Result, workspace_root};

const COPIED: &[&str] = &[
    "www/symbols",
    "www/textures",
    "icons",
    "bundledAssets",
    "licenses",
];

pub fn run() -> Result {
    let root = workspace_root();
    let upstream = root.join("reference/AdvantageScope");
    let dest = root.join("assets");
    if !upstream.is_dir() {
        return Err(format!(
            "upstream checkout not found at {}",
            upstream.display()
        ));
    }
    if dest.exists() {
        fs::remove_dir_all(&dest).map_err(|e| format!("clearing {}: {e}", dest.display()))?;
    }
    let mut files = 0;
    for rel in COPIED {
        files += copy_tree(&upstream.join(rel), &dest.join(rel))?;
    }
    let commit = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(&upstream)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned());
    fs::write(dest.join("UPSTREAM"), format!("{commit}\n")).map_err(|e| e.to_string())?;
    eprintln!(
        "copied {files} files from upstream {commit} into {}",
        dest.display()
    );
    Ok(())
}

fn copy_tree(src: &Path, dst: &Path) -> Result<usize> {
    let err = |e: std::io::Error| format!("{}: {e}", src.display());
    fs::create_dir_all(dst).map_err(err)?;
    let mut count = 0;
    for entry in fs::read_dir(src).map_err(err)? {
        let entry = entry.map_err(err)?;
        let name = entry.file_name();
        if name == ".DS_Store" {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&name);
        if entry.file_type().map_err(err)?.is_dir() {
            count += copy_tree(&from, &to)?;
        } else {
            fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
            count += 1;
        }
    }
    Ok(count)
}
