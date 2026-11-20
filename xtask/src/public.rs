use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::Result;
use crate::oracle::Paths;

const MANIFEST: &str = "parity/corpus/public.toml";
const REQUIRED: [&str; 8] = [
    "name", "file", "url", "sha256", "size", "format", "source", "license",
];

struct PublicLog {
    name: String,
    file: String,
    url: String,
    sha256: String,
    size: u64,
}

fn cache_dir(paths: &Paths) -> PathBuf {
    paths.cache.join("public-logs")
}

fn load(root: &Path) -> Result<Vec<PublicLog>> {
    let path = root.join(MANIFEST);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let table: toml::Table = text
        .parse()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(key) = table.keys().find(|k| *k != "log") {
        return Err(format!("{MANIFEST}: unknown top-level key {key:?}"));
    }
    let entries = table
        .get("log")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| format!("{MANIFEST}: needs [[log]] entries"))?;
    let mut logs: Vec<PublicLog> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let at = format!("{MANIFEST} [[log]] #{}", i + 1);
        let entry = entry
            .as_table()
            .ok_or_else(|| format!("{at}: not a table"))?;
        for key in REQUIRED {
            if !entry.contains_key(key) {
                return Err(format!("{at}: missing {key:?}"));
            }
        }
        if let Some(key) = entry
            .keys()
            .find(|k| !REQUIRED.contains(&k.as_str()) && *k != "merge")
        {
            return Err(format!("{at}: unknown key {key:?}"));
        }
        let text = |key: &str| {
            entry
                .get(key)
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("{at}: {key:?} must be a string"))
        };
        let log = PublicLog {
            name: text("name")?,
            file: text("file")?,
            url: text("url")?,
            sha256: text("sha256")?,
            size: entry
                .get("size")
                .and_then(toml::Value::as_integer)
                .and_then(|n| u64::try_from(n).ok())
                .ok_or_else(|| format!("{at}: \"size\" must be a non-negative integer"))?,
        };
        if log.sha256.len() != 64
            || !log
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(format!("{at}: sha256 must be 64 lowercase hex digits"));
        }
        if !log.url.starts_with("https://") {
            return Err(format!("{at}: url must be https"));
        }
        let file = Path::new(&log.file);
        if file.is_absolute()
            || file
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(format!(
                "{at}: file must be a plain relative path, got {:?}",
                log.file
            ));
        }
        if logs
            .iter()
            .any(|l| l.name == log.name || l.file == log.file)
        {
            return Err(format!(
                "{at}: duplicate name or file ({}, {})",
                log.name, log.file
            ));
        }
        logs.push(log);
    }
    Ok(logs)
}

pub(crate) fn fetch(paths: &Paths, args: &[String]) -> Result {
    let offline = args.iter().any(|a| a == "--offline");
    if let Some(flag) = args
        .iter()
        .find(|a| a.starts_with("--") && *a != "--offline")
    {
        return Err(format!("corpus fetch: unknown option {flag}"));
    }
    let wanted: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let logs = load(&paths.root)?;
    for name in &wanted {
        if !logs.iter().any(|l| &l.name == *name) {
            return Err(format!("corpus fetch: no public log named {name:?}"));
        }
    }
    let dir = cache_dir(paths);
    let mut failures = Vec::new();
    let (mut cached, mut downloaded) = (0, 0);
    for log in logs
        .iter()
        .filter(|l| wanted.is_empty() || wanted.contains(&&l.name))
    {
        let dest = dir.join(&log.file);
        if verify(&dest, log).is_ok() {
            cached += 1;
            continue;
        }
        if offline {
            failures.push(format!("{}: {}", log.name, verify(&dest, log).unwrap_err()));
            continue;
        }
        eprintln!("==> {} ({} bytes)", log.name, log.size);
        match download(&dest, log) {
            Ok(()) => downloaded += 1,
            Err(e) => failures.push(format!("{}: {e}", log.name)),
        }
    }
    eprintln!(
        "corpus fetch: {cached} cached, {downloaded} downloaded, {} failed, in {}",
        failures.len(),
        dir.display()
    );
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

fn download(dest: &Path, log: &PublicLog) -> Result {
    let parent = dest.parent().expect("cache file has a parent");
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    let part = dest.with_file_name(format!(
        "{}.part",
        dest.file_name().expect("file name").to_string_lossy()
    ));
    let status = Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error"])
        .args(["--retry", "3", "--output"])
        .arg(&part)
        .arg(&log.url)
        .status()
        .map_err(|e| format!("running curl: {e}"))?;
    if !status.success() {
        let _ = fs::remove_file(&part);
        return Err(format!("curl {} failed ({status})", log.url));
    }
    if let Err(e) = verify(&part, log) {
        let _ = fs::remove_file(&part);
        return Err(format!("downloaded file rejected: {e}"));
    }
    fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))
}

fn verify(path: &Path, log: &PublicLog) -> Result {
    let size = fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if size != log.size {
        return Err(format!("size {size}, expected {}", log.size));
    }
    let actual = sha256_file(path)?;
    if actual != log.sha256 {
        return Err(format!("sha256 {actual}, expected {}", log.sha256));
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(sha256: &str, size: u64) -> PublicLog {
        PublicLog {
            name: "t".into(),
            file: "t.bin".into(),
            url: "https://example.invalid/t.bin".into(),
            sha256: sha256.into(),
            size,
        }
    }

    #[test]
    fn verify_checks_size_and_hash() {
        let dir = std::env::temp_dir().join(format!("asrs-public-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abc.bin");
        fs::write(&path, b"abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify(&path, &log(abc, 3)).is_ok());
        assert!(verify(&path, &log(abc, 4)).unwrap_err().contains("size"));
        let wrong = "0".repeat(64);
        assert!(
            verify(&path, &log(&wrong, 3))
                .unwrap_err()
                .contains("sha256")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn committed_manifest_parses() {
        let logs = load(&crate::workspace_root()).unwrap();
        assert!(!logs.is_empty());
    }
}
