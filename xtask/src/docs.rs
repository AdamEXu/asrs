use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Result, oracle, sha256_hex, workspace_root};

const USAGE: &str = "usage: cargo xtask docs

Builds upstream's embedded documentation (`npm run docs:build-embed`) in a copy under
target/docs/src and writes it to target/docs/build, which `cargo xtask bundle` ships as
docs/build and development builds of asrs show. An oracle install gets a copy too. Two images
upstream's license does not cover are replaced (see THIRD_PARTY_NOTICES.md), and
THIRD_PARTY_LICENSES.txt lists every npm package compiled in. Two builds have the same pages and
files but not the same bytes: Docusaurus names variables and numbers modules differently. Needs
Node and reference/AdvantageScope.";

const EMBED: &str = "docusaurus-embed.config.ts";
const LICENSES: &str = "asrs.config.ts";
const LISTING: &str = "THIRD_PARTY_LICENSES.txt";
const SKIPPED: &[&str] = &["node_modules", "build", ".docusaurus", ".DS_Store"];

/// Images upstream's license does not cover, and what is built in their place.
const REPLACED: &[(&str, &str)] = &[
    ("src/theme/NotFound/Content/404.webp", "404.webp"),
    (
        "docs/tab-reference/3d-field/img/app-store.svg",
        "app-store.svg",
    ),
];

/// License texts of bundled npm packages whose archives omit them.
const SUPPLIED_LICENSES: &[&str] = &["infima"];

pub(crate) fn build_dir() -> PathBuf {
    workspace_root().join("target/docs/build")
}

pub fn run(args: &[String]) -> Result {
    if !args.is_empty() {
        return Err(USAGE.to_owned());
    }
    let root = workspace_root();
    let paths = oracle::Paths::new()?;
    oracle::check_reference(&paths)?;
    let work = root.join("target/docs/src");
    let build = build_dir();

    remove_sources(&work)?;
    copy_sources(&root.join("reference/AdvantageScope/docs"), &work)?;
    for (upstream, ours) in REPLACED {
        let to = work.join(upstream);
        if !to.is_file() {
            return Err(format!("{upstream} is no longer in upstream's docs"));
        }
        copy(&root.join("xtask/docs").join(ours), &to)?;
    }
    copy(
        &root.join("xtask/docs").join(LICENSES),
        &work.join(LICENSES),
    )?;
    for name in SUPPLIED_LICENSES {
        let file = format!("{name}.txt");
        let dir = work.join("asrs-licenses");
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        copy(&root.join("licenses").join(&file), &dir.join(&file))?;
    }

    let lock = fs::read(work.join("package-lock.json")).map_err(|e| e.to_string())?;
    let lock = sha256_hex(&lock);
    let stamp = work.join("node_modules/.asrs-lock");
    if !fs::read_to_string(&stamp).is_ok_and(|s| s == lock) {
        oracle::npm_ci(&paths, &work)?;
        fs::write(&stamp, lock).map_err(|e| format!("{}: {e}", stamp.display()))?;
    }

    let licenses = root.join("target/docs/licenses");
    docusaurus(&work, EMBED, &build)?;
    docusaurus(&work, LICENSES, &licenses)?;
    copy(&licenses.join(LISTING), &build.join(LISTING))?;
    if paths.upstream.is_dir() {
        copy_sources(&build, &paths.upstream.join("docs/build"))?;
    }
    eprintln!("wrote {}", build.display());
    Ok(())
}

fn docusaurus(work: &Path, config: &str, out: &Path) -> Result {
    let status = Command::new("node")
        .arg(work.join("node_modules/@docusaurus/core/bin/docusaurus.mjs"))
        .args(["build", "--config", config, "--out-dir"])
        .arg(out)
        .env("DO_NOT_TRACK", "1")
        .current_dir(work)
        .status()
        .map_err(|e| format!("failed to run node: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("building the docs with {config} failed ({status})"))
    }
}

fn copy(from: &Path, to: &Path) -> Result {
    fs::copy(from, to)
        .map(drop)
        .map_err(|e| format!("{} to {}: {e}", from.display(), to.display()))
}

fn remove_sources(work: &Path) -> Result {
    let Ok(entries) = fs::read_dir(work) else {
        return Ok(());
    };
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name() == "node_modules" {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        }
        .map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

fn copy_sources(src: &Path, dst: &Path) -> Result {
    fs::create_dir_all(dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    for entry in fs::read_dir(src).map_err(|e| format!("{}: {e}", src.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if SKIPPED.iter().any(|s| name == *s) {
            continue;
        }
        let (from, to) = (entry.path(), dst.join(&name));
        if from.is_dir() {
            copy_sources(&from, &to)?;
        } else {
            copy(&from, &to)?;
        }
    }
    Ok(())
}
