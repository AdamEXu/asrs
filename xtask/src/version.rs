use std::fs;
use std::path::Path;

use asrs_model::app_identity::VERSION;
use toml::Value;

use crate::{Result, workspace_root};

const USAGE: &str = "usage: cargo xtask version [--tag <tag>]

Checks that every workspace package inherits the version of the root Cargo.toml, that Cargo.lock
has it and that it has the numeric forms packages need, and prints those. With --tag, also checks
that the tag is `v` and the version. Creates and publishes nothing.";

/// A version of the form `1.2.3`, `1.2.3-alpha.4`, `1.2.3-beta.4` or `1.2.3-rc.4`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    major: u16,
    minor: u16,
    patch: u16,
    build: u16,
}

/// The pre-releases in SemVer order, and what each adds to its number in the build number.
const STAGES: &[(&str, u16)] = &[("alpha", 0), ("beta", 1000), ("rc", 2000)];
const FINAL: u16 = 3000;
const MAX_PRE_RELEASE: u16 = 999;
/// `macos_build` makes one number of the major and the minor.
const MAX_MINOR: u16 = 999;

impl Version {
    pub fn current() -> Result<Version> {
        Version::parse(VERSION)
    }

    fn parse(text: &str) -> Result<Version> {
        let bad = || {
            format!(
                "the version {text:?} is not <major>.<minor>.<patch>, or that and -alpha.<n>, \
                 -beta.<n> or -rc.<n>, with n from 1 to {MAX_PRE_RELEASE} and a minor up to {MAX_MINOR}"
            )
        };
        let (numbers, pre_release) = match text.split_once('-') {
            Some((numbers, pre_release)) => (numbers, Some(pre_release)),
            None => (text, None),
        };
        let numbers: Option<Vec<u16>> = numbers.split('.').map(number).collect();
        let Some([major, minor, patch]) = numbers.and_then(|n| <[u16; 3]>::try_from(n).ok()) else {
            return Err(bad());
        };
        if minor > MAX_MINOR {
            return Err(bad());
        }
        let build = match pre_release {
            None => FINAL,
            Some(pre_release) => {
                let (stage, n) = pre_release.split_once('.').ok_or_else(bad)?;
                let (_, base) = STAGES
                    .iter()
                    .find(|(name, _)| *name == stage)
                    .ok_or_else(bad)?;
                match number(n) {
                    Some(n @ 1..=MAX_PRE_RELEASE) => base + n,
                    _ => return Err(bad()),
                }
            }
        };
        Ok(Version {
            major,
            minor,
            patch,
            build,
        })
    }

    /// `CFBundleShortVersionString`.
    pub fn numbers(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }

    /// `CFBundleVersion`. macOS reads its first three numbers only, so the major and the minor
    /// share one.
    pub fn macos_build(&self) -> String {
        let first = u32::from(self.major) * 1000 + u32::from(self.minor);
        format!("{first}.{}.{}", self.patch, self.build)
    }

    /// `FILEVERSION` and `PRODUCTVERSION`.
    pub fn windows(&self) -> String {
        format!("{}.{}", self.numbers(), self.build)
    }
}

fn number(text: &str) -> Option<u16> {
    let plain = text.bytes().all(|b| b.is_ascii_digit()) && (text == "0" || !text.starts_with('0'));
    plain.then(|| text.parse().ok()).flatten()
}

pub fn run(args: &[String]) -> Result {
    let tag = match args {
        [] => None,
        [flag, tag] if flag == "--tag" => Some(tag),
        _ => return Err(USAGE.to_owned()),
    };
    check_workspace(&workspace_root())?;
    let version = Version::current()?;
    if let Some(tag) = tag {
        check_tag(tag)?;
    }
    println!("version  {VERSION}");
    println!("tag      v{VERSION}");
    println!("macOS    {} ({})", version.numbers(), version.macos_build());
    println!("Windows  {}", version.windows());
    Ok(())
}

fn check_tag(tag: &str) -> Result {
    let expected = format!("v{VERSION}");
    if tag == expected {
        Ok(())
    } else {
        Err(format!(
            "the tag {tag} is not {expected}, the version in Cargo.toml"
        ))
    }
}

fn check_workspace(root: &Path) -> Result {
    let workspace = manifest(&root.join("Cargo.toml"))?;
    let version = workspace
        .get("workspace")
        .and_then(|w| w.get("package")?.get("version")?.as_str());
    if version != Some(VERSION) {
        return Err(format!(
            "Cargo.toml has the version {version:?} but xtask was built from {VERSION}; rebuild xtask"
        ));
    }
    let lock = manifest(&root.join("Cargo.lock"))?;
    let locked = lock.get("package").and_then(Value::as_array);
    let locked = locked.ok_or("Cargo.lock has no packages")?;
    let mut members = vec![root.join("xtask")];
    let crates = root.join("crates");
    for entry in fs::read_dir(&crates).map_err(|e| format!("{}: {e}", crates.display()))? {
        members.push(entry.map_err(|e| e.to_string())?.path());
    }
    let mut problems = Vec::new();
    for path in members.iter().map(|dir| dir.join("Cargo.toml")) {
        if !path.is_file() {
            continue;
        }
        let member = manifest(&path)?;
        let package = member.get("package");
        let inherits = package
            .and_then(|p| p.get("version")?.get("workspace")?.as_bool())
            .unwrap_or(false);
        if !inherits {
            problems.push(format!("{}: no version.workspace = true", path.display()));
        }
        let name = package.and_then(|p| p.get("name")?.as_str());
        let in_lock = locked.iter().any(|p| {
            p.get("source").is_none()
                && p.get("name").and_then(Value::as_str) == name
                && p.get("version").and_then(Value::as_str) == Some(VERSION)
        });
        if !in_lock {
            problems.push(format!("Cargo.lock: no {name:?} {VERSION}"));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems.join("\n"))
    }
}

fn manifest(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_workspace_has_one_version() {
        check_workspace(&workspace_root()).expect("one version");
        Version::current().expect("numeric forms");
    }

    #[test]
    fn numeric_forms() {
        for (text, short, macos, windows) in [
            ("0.1.0-alpha.1", "0.1.0", "1.0.1", "0.1.0.1"),
            ("0.1.0-beta.2", "0.1.0", "1.0.1002", "0.1.0.1002"),
            ("0.1.0-rc.999", "0.1.0", "1.0.2999", "0.1.0.2999"),
            ("0.1.0", "0.1.0", "1.0.3000", "0.1.0.3000"),
            ("2.30.4", "2.30.4", "2030.4.3000", "2.30.4.3000"),
        ] {
            let version = Version::parse(text).expect(text);
            assert_eq!(version.numbers(), short);
            assert_eq!(version.macos_build(), macos);
            assert_eq!(version.windows(), windows);
        }
    }

    #[test]
    fn numeric_forms_grow_with_the_version() {
        let fields = |text: String| -> Vec<u32> {
            text.split('.')
                .map(|field| field.parse().expect("a number"))
                .collect()
        };
        let ordered = [
            "0.1.0-alpha.1",
            "0.1.0-alpha.2",
            "0.1.0-alpha.10",
            "0.1.0-alpha.999",
            "0.1.0-beta.1",
            "0.1.0-rc.1",
            "0.1.0",
            "0.1.1-alpha.1",
            "0.1.1",
            "0.2.0-alpha.1",
            "0.999.65535",
            "1.0.0-alpha.1",
            "1.0.0",
        ]
        .map(|text| Version::parse(text).expect(text));
        for pair in ordered.windows(2) {
            assert!(pair[0] < pair[1]);
            assert!(fields(pair[0].windows()) < fields(pair[1].windows()));
            assert!(fields(pair[0].macos_build()) < fields(pair[1].macos_build()));
        }
    }

    #[test]
    fn versions_without_numeric_forms_are_refused() {
        for text in [
            "",
            "0.1",
            "0.1.0.0",
            "00.1.0",
            "0.1.0-",
            "0.1.0-alpha",
            "0.1.0-alpha.0",
            "0.1.0-alpha.01",
            "0.1.0-alpha.1000",
            "0.1.0-alpha-1",
            "0.1.0-nightly.1",
            "0.1.0-alpha.1+x",
            "0.1.0+x",
            "0.1000.0",
            "65536.0.0",
            "v0.1.0",
        ] {
            assert!(Version::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn a_tag_is_v_and_the_version() {
        check_tag(&format!("v{VERSION}")).expect("the tag");
        for tag in [
            VERSION,
            "v0.0.1",
            &format!("v{VERSION}.1"),
            &format!("V{VERSION}"),
        ] {
            assert!(check_tag(tag).is_err(), "{tag}");
        }
    }
}
