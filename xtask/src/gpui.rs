use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::oracle::Paths;
use crate::{Result, workspace_root};

const USAGE: &str = "usage: cargo xtask gpui <command>

commands:
  verify          vendor/gpui-pre and vendor/gpui-pre-scheduler equal the crates.io
                  archives plus their patches in patches/ (gpui-pre's examples/ is not
                  vendored)
  vendor <crate> <dir>
                  write the crates.io archive of gpui-pre or gpui-pre-scheduler plus its
                  patch to <dir>, which must not exist (cargo cannot load the workspace
                  without the vendored crates, so a new vendor tree is generated
                  elsewhere and moved into place)
  features        no GPUI test-support in a normal build; present with `parity`
  release-audit   build asrs-app in release with and without `parity` and check that
                  test-support code reaches the artifacts only with it

The crates.io archives come from cargo's registry cache or is downloaded to
$ASRS_CACHE/crates and are checked against their sha256.";

struct Vendored {
    name: &'static str,
    version: &'static str,
    sha256: &'static str,
    not_vendored: &'static [&'static str],
}

const VENDORED: &[Vendored] = &[
    Vendored {
        name: "gpui-pre",
        version: "0.3.12",
        sha256: "29db89613527079737f53752eab27fb65c9a7dd94979256c65a0e590c9fd34d8",
        not_vendored: &["examples/"],
    },
    Vendored {
        name: "gpui-pre-scheduler",
        version: "0.3.12",
        sha256: "b1d3fa97eeb4ea233635b756684affe96db9e29fc2af8cdc269268bd7ff649fc",
        not_vendored: &[],
    },
];

// rlib metadata lists the names of cfg-stripped items (`TestPlatform`, `VisualTestPlatform`, ...)
// even without test-support, so markers must be names and literals that only compiled code carries.
const TEST_SUPPORT_MARKERS: &[&str] = &["HeadlessAppContext", "no HeadlessRenderer configured"];

pub fn run(args: &[String]) -> Result {
    match args.first().map(String::as_str) {
        Some("verify") => verify_vendor(),
        Some("vendor") => match &args[1..] {
            [name, dir] => write_vendor(name, Path::new(dir)),
            _ => Err(USAGE.to_owned()),
        },
        Some("features") => check_features(),
        Some("release-audit") => release_audit(),
        _ => Err(USAGE.to_owned()),
    }
}

pub fn verify_vendor() -> Result {
    let problems: Vec<String> = VENDORED
        .iter()
        .filter_map(|krate| krate.verify().err())
        .collect();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

fn write_vendor(name: &str, dir: &Path) -> Result {
    let krate = VENDORED
        .iter()
        .find(|krate| krate.name == name)
        .ok_or_else(|| format!("{name} is not a vendored crate\n\n{USAGE}"))?;
    if dir.exists() {
        return Err(format!("{} exists; remove it first", dir.display()));
    }
    let tree = krate.patched_tree()?;
    for (path, bytes) in &tree {
        let out = dir.join(path);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        fs::write(&out, bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    eprintln!("wrote {} files to {}", tree.len(), dir.display());
    Ok(())
}

impl Vendored {
    fn vendor_dir(&self) -> String {
        format!("vendor/{}", self.name)
    }

    fn patch(&self) -> String {
        format!("patches/{}-{}.patch", self.name, self.version)
    }

    fn verify(&self) -> Result {
        let expected = self.patched_tree()?;
        let vendor_dir = self.vendor_dir();
        let dir = workspace_root().join(&vendor_dir);
        let mut actual = BTreeMap::new();
        collect_tree(&dir, &dir, &mut actual)?;
        let mut problems = Vec::new();
        for (path, bytes) in &expected {
            match actual.get(path) {
                None => problems.push(format!("missing {path}")),
                Some(found) if found != bytes => problems.push(format!("differs {path}")),
                Some(_) => {}
            }
        }
        for path in actual.keys().filter(|p| !expected.contains_key(*p)) {
            problems.push(format!("extra {path}"));
        }
        let (name, version, patch) = (self.name, self.version, self.patch());
        if problems.is_empty() {
            eprintln!(
                "{vendor_dir} ok: {} files equal {name} {version} plus {patch}",
                expected.len()
            );
            Ok(())
        } else {
            Err(format!(
                "{vendor_dir} is not {name} {version} plus {patch}:\n  {}",
                problems.join("\n  ")
            ))
        }
    }

    fn patched_tree(&self) -> Result<BTreeMap<String, Vec<u8>>> {
        let mut tree = self.crate_tree(&self.crate_archive()?)?;
        let patch_name = self.patch();
        let patch_path = workspace_root().join(&patch_name);
        let patch = fs::read_to_string(&patch_path)
            .map_err(|e| format!("{}: {e}", patch_path.display()))?;
        apply_patch(&mut tree, &patch).map_err(|e| format!("{patch_name}: {e}"))?;
        Ok(tree)
    }

    fn crate_archive(&self) -> Result<Vec<u8>> {
        let file = format!("{}-{}.crate", self.name, self.version);
        let mut candidates = registry_caches()
            .into_iter()
            .map(|dir| dir.join(&file))
            .collect::<Vec<_>>();
        let download = Paths::new()?.cache.join("crates").join(&file);
        candidates.push(download.clone());
        for path in candidates.iter().filter(|p| p.is_file()) {
            let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            if sha256_hex(&bytes) == self.sha256 {
                return Ok(bytes);
            }
            eprintln!("ignoring {}: sha256 mismatch", path.display());
        }
        let url = format!("https://static.crates.io/crates/{}/{file}", self.name);
        eprintln!("downloading {url}");
        if let Some(parent) = download.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let partial = download.with_extension("crate.part");
        let status = Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&partial)
            .arg(&url)
            .status()
            .map_err(|e| format!("failed to run curl: {e}"))?;
        if !status.success() {
            return Err(format!("downloading {url} failed ({status})"));
        }
        let bytes = fs::read(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        if sha256_hex(&bytes) != self.sha256 {
            return Err(format!("{url}: sha256 is not {}", self.sha256));
        }
        fs::rename(&partial, &download).map_err(|e| format!("{}: {e}", download.display()))?;
        Ok(bytes)
    }

    fn crate_tree(&self, archive: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
        let prefix = format!("{}-{}/", self.name, self.version);
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
        let mut tree = BTreeMap::new();
        for entry in tar.entries().map_err(|e| format!("crate archive: {e}"))? {
            let mut entry = entry.map_err(|e| format!("crate archive: {e}"))?;
            let path = entry
                .path()
                .map_err(|e| format!("crate archive: {e}"))?
                .to_string_lossy()
                .replace('\\', "/");
            let Some(rel) = path.strip_prefix(&prefix) else {
                return Err(format!("crate archive: unexpected entry {path}"));
            };
            if entry.header().entry_type().is_dir() {
                continue;
            }
            if !entry.header().entry_type().is_file() {
                return Err(format!("crate archive: {path} is not a regular file"));
            }
            if self.not_vendored.iter().any(|p| rel.starts_with(p)) {
                continue;
            }
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("crate archive: {path}: {e}"))?;
            tree.insert(rel.to_owned(), bytes);
        }
        Ok(tree)
    }
}

fn registry_caches() -> Vec<PathBuf> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".cargo")));
    let Some(cache) = cargo_home.map(|home| home.join("registry/cache")) else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&cache) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn collect_tree(base: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) -> Result {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect_tree(base, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(base)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            out.insert(rel, bytes);
        }
    }
    Ok(())
}

fn apply_patch(
    tree: &mut BTreeMap<String, Vec<u8>>,
    patch: &str,
) -> std::result::Result<(), String> {
    let lines: Vec<&str> = patch.split_inclusive('\n').collect();
    let mut i = 0;
    let mut patched = 0;
    while i < lines.len() {
        let Some(old) = lines[i].strip_prefix("--- ") else {
            i += 1;
            continue;
        };
        let new = lines
            .get(i + 1)
            .and_then(|l| l.strip_prefix("+++ "))
            .ok_or_else(|| format!("line {}: `---` without `+++`", i + 1))?;
        let old = old.trim_end();
        let path = new
            .trim_end()
            .strip_prefix("b/")
            .ok_or_else(|| format!("line {}: expected a b/ path", i + 2))?
            .to_owned();
        if old != "/dev/null" && old.strip_prefix("a/") != Some(path.as_str()) {
            return Err(format!("line {}: renames are not supported", i + 1));
        }
        i += 2;
        let original: Vec<String> = if old == "/dev/null" {
            if tree.contains_key(&path) {
                return Err(format!("{path}: created by the patch but already present"));
            }
            Vec::new()
        } else {
            let bytes = tree
                .get(&path)
                .ok_or_else(|| format!("{path}: not in the crate"))?;
            std::str::from_utf8(bytes)
                .map_err(|_| format!("{path}: not UTF-8"))?
                .split_inclusive('\n')
                .map(str::to_owned)
                .collect()
        };
        let mut output: Vec<String> = Vec::new();
        let mut cursor = 0;
        while let Some(header) = lines.get(i).and_then(|l| l.strip_prefix("@@ -")) {
            let (old_start, old_len, new_len) =
                hunk_header(header).ok_or_else(|| format!("line {}: bad hunk header", i + 1))?;
            i += 1;
            let start = if old_len == 0 {
                old_start
            } else {
                old_start - 1
            };
            if start < cursor || start > original.len() {
                return Err(format!("{path}: hunk at line {old_start} is out of order"));
            }
            output.extend_from_slice(&original[cursor..start]);
            cursor = start;
            let (mut seen_old, mut seen_new) = (0, 0);
            while seen_old < old_len || seen_new < new_len {
                let line = lines
                    .get(i)
                    .ok_or_else(|| format!("{path}: hunk at line {old_start} is truncated"))?;
                i += 1;
                let (kind, text) = line.split_at(1);
                let text = text.to_owned();
                let text = if lines.get(i).is_some_and(|l| l.starts_with('\\')) {
                    i += 1;
                    text.strip_suffix('\n').unwrap_or(&text).to_owned()
                } else {
                    text
                };
                match kind {
                    " " | "-" => {
                        if original.get(cursor) != Some(&text) {
                            return Err(format!(
                                "{path}: hunk at line {old_start} does not match line {}",
                                cursor + 1
                            ));
                        }
                        cursor += 1;
                        seen_old += 1;
                        if kind == " " {
                            output.push(text);
                            seen_new += 1;
                        }
                    }
                    "+" => {
                        output.push(text);
                        seen_new += 1;
                    }
                    _ => return Err(format!("{path}: bad hunk line {line:?}")),
                }
            }
        }
        output.extend_from_slice(&original[cursor..]);
        tree.insert(path, output.concat().into_bytes());
        patched += 1;
    }
    if patched == 0 {
        return Err("no file diffs".to_owned());
    }
    Ok(())
}

fn hunk_header(header: &str) -> Option<(usize, usize, usize)> {
    let (ranges, _) = header.split_once(" @@")?;
    let (old, new) = ranges.split_once(" +")?;
    let range = |r: &str| -> Option<(usize, usize)> {
        match r.split_once(',') {
            Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
            None => Some((r.parse().ok()?, 1)),
        }
    };
    let (old_start, old_len) = range(old)?;
    let (_, new_len) = range(new)?;
    Some((old_start, old_len, new_len))
}

pub fn check_features() -> Result {
    let normal = enabled_features(&["--workspace"])?;
    let leaks: Vec<&str> = normal
        .iter()
        .filter(|(_, features)| features.iter().any(|f| f == "test-support"))
        .map(|(package, _)| package.as_str())
        .collect();
    if !leaks.is_empty() {
        return Err(format!(
            "test-support is enabled in a normal workspace build for: {}",
            leaks.join(", ")
        ));
    }
    let parity = enabled_features(&["-p", "asrs-app", "--features", "parity"])?;
    for package in ["gpui-pre", "gpui-pre-platform"] {
        let on = parity
            .iter()
            .any(|(p, f)| p == package && f.iter().any(|f| f == "test-support"));
        if !on {
            return Err(format!(
                "asrs-app --features parity does not enable {package}/test-support"
            ));
        }
    }
    eprintln!("gpui features ok: test-support only with `parity`");
    Ok(())
}

fn enabled_features(selection: &[&str]) -> Result<Vec<(String, Vec<String>)>> {
    let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo_bin)
        .args(["tree", "--locked", "--target", "all", "-e", "normal,build"])
        .args(["--prefix", "none", "--format", "{p}|{f}"])
        .args(selection)
        .current_dir(workspace_root())
        .output()
        .map_err(|e| format!("failed to run cargo tree: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo tree failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (package, features) = line.split_once('|')?;
            let name = package.split_whitespace().next()?.to_owned();
            let features = features
                .split(',')
                .filter(|f| !f.is_empty())
                .map(str::to_owned)
                .collect();
            Some((name, features))
        })
        .collect())
}

fn release_audit() -> Result {
    let without = release_artifacts(&[])?;
    let found = scan(&without)?;
    if !found.is_empty() {
        return Err(format!(
            "release build without parity contains test-support code:\n  {}",
            found.join("\n  ")
        ));
    }
    eprintln!(
        "release without parity: none of {TEST_SUPPORT_MARKERS:?} in {} artifacts",
        without.len()
    );
    let with = release_artifacts(&["--features", "parity"])?;
    let found = scan(&with)?;
    if found.is_empty() {
        return Err("release build with parity shows no test-support marker; the audit cannot see what it checks for".to_owned());
    }
    eprintln!(
        "release with parity: markers found in {} artifacts (control)",
        found.len()
    );
    Ok(())
}

fn release_artifacts(features: &[&str]) -> Result<Vec<PathBuf>> {
    let mut args = vec![
        "build",
        "--release",
        "--locked",
        "-p",
        "asrs-app",
        "--message-format=json-render-diagnostics",
    ];
    args.extend_from_slice(features);
    eprintln!("$ cargo {}", args.join(" "));
    let cargo_bin = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo_bin)
        .args(&args)
        .current_dir(workspace_root())
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| format!("failed to run cargo build: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo {} failed ({})",
            args.join(" "),
            output.status
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();
    for line in stdout
        .lines()
        .filter(|l| l.contains("\"reason\":\"compiler-artifact\""))
    {
        let Some(start) = line.find("\"filenames\":[") else {
            continue;
        };
        let rest = &line[start + "\"filenames\":[".len()..];
        let Some(end) = rest.find(']') else {
            continue;
        };
        for name in rest[..end].split("\",\"") {
            let name = name.trim_matches('"').replace("\\\\", "\\");
            let path = PathBuf::from(name);
            let keep = path
                .extension()
                .is_none_or(|e| e == "rlib" || e == "exe" || e == "dylib" || e == "so");
            if keep && path.is_file() {
                files.push(path);
            }
        }
    }
    if !files
        .iter()
        .any(|p| p.file_stem().is_some_and(|s| s == "asrs"))
    {
        return Err("cargo build reported no asrs binary".to_owned());
    }
    Ok(files)
}

pub fn scan(files: &[PathBuf]) -> Result<Vec<String>> {
    let mut found = Vec::new();
    for path in files {
        let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        for marker in TEST_SUPPORT_MARKERS {
            if bytes.windows(marker.len()).any(|w| w == marker.as_bytes()) {
                found.push(format!("{marker} in {}", path.display()));
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        files
            .iter()
            .map(|(p, s)| ((*p).to_owned(), s.as_bytes().to_vec()))
            .collect()
    }

    const PATCH_TEXT: &str = "diff --git a/f.rs b/f.rs
--- a/f.rs
+++ b/f.rs
@@ -1,3 +1,4 @@
 a
+x
 b
 c
@@ -5,2 +6,1 @@
 e
-f
--- /dev/null
+++ b/new.rs
@@ -0,0 +1,1 @@
+n
";

    #[test]
    fn applies_hunks_and_new_files() {
        let mut t = tree(&[("f.rs", "a\nb\nc\nd\ne\nf\n")]);
        apply_patch(&mut t, PATCH_TEXT).unwrap();
        assert_eq!(t["f.rs"], b"a\nx\nb\nc\nd\ne\n");
        assert_eq!(t["new.rs"], b"n\n");
    }

    #[test]
    fn rejects_mismatched_context() {
        let mut t = tree(&[("f.rs", "a\nB\nc\nd\ne\nf\n")]);
        assert!(apply_patch(&mut t, PATCH_TEXT).is_err());
    }

    #[test]
    fn handles_missing_final_newline() {
        let patch =
            "--- a/f.rs\n+++ b/f.rs\n@@ -1,1 +1,1 @@\n-a\n\\ No newline at end of file\n+b\n";
        let mut t = tree(&[("f.rs", "a")]);
        apply_patch(&mut t, patch).unwrap();
        assert_eq!(t["f.rs"], b"b\n");
    }
}
