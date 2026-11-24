use std::collections::{BTreeMap, HashMap};
use std::fmt::Write;
use std::fs;
use std::path::Path;

use asrs_parity::json::{self, Json};

use crate::{Result, cargo, workspace_root};

/// The programs a release ships, and the listing of the crates compiled into each.
const PROGRAMS: &[(&str, &str)] = &[
    ("asrs-app", "THIRD_PARTY_LICENSES.txt"),
    ("asrs-webview", "THIRD_PARTY_LICENSES-webview.txt"),
];

const BUNDLED: &[&str] = &["LICENSE", "THIRD_PARTY_NOTICES.md"];
/// Where the license texts of crates whose packages have none are kept.
const CRATE_TEXTS: &str = "licenses/crates";
/// Crates cargo-about finds no license text for, so that it falls back to the SPDX template, and
/// the file with the crate's own.
const SUPPLIED: &[(&str, &str, &str)] = &[
    ("block2", "0.6.2", "objc2-md.txt"),
    ("dpi", "0.1.2", "dpi.txt"),
    ("exr", "1.74.3", "exr.txt"),
    ("harfrust", "0.5.2", "harfrust.txt"),
    ("libm", "0.2.16", "libm.txt"),
    ("objc-sys", "0.3.5", "objc2.txt"),
    ("objc2", "0.5.3", "objc2.txt"),
    ("objc2", "0.6.5", "objc2-md.txt"),
    ("objc2-app-kit", "0.2.2", "objc2.txt"),
    ("objc2-encode", "4.1.0", "objc2-md.txt"),
    ("objc2-foundation", "0.2.2", "objc2.txt"),
    ("objc2-foundation", "0.3.2", "objc2-md.txt"),
    ("rust-i18n-macro", "4.2.4", "rust-i18n.txt"),
    ("rust-i18n-support", "4.2.4", "rust-i18n.txt"),
    ("seahash", "4.1.0", "seahash.txt"),
    ("sha1_smol", "1.0.1", "sha1_smol.txt"),
    ("taffy", "0.15.1", "taffy.txt"),
    (
        "wayland-protocols-plasma",
        "0.3.12",
        "wayland-protocols-plasma.txt",
    ),
    ("webview2-com", "0.38.2", "webview2-com.txt"),
    ("webview2-com-macros", "0.8.1", "webview2-com.txt"),
    ("webview2-com-sys", "0.38.2", "webview2-com.txt"),
    ("xim-ctext", "0.3.0", "xim-rs.txt"),
    ("xim-parser", "0.2.2", "xim-rs.txt"),
];
/// What the SPDX templates leave for the copyright holder to fill in.
const PLACEHOLDERS: &[&str] = &[
    "<year>",
    "<owner>",
    "<copyright holders>",
    "<copyright holder>",
    "<name of author>",
    "[year]",
    "[fullname]",
];

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
    let mut reports = Vec::new();
    for (package, listing) in PROGRAMS {
        let report = Report::read(&about(package, &dest)?, &root)?;
        let listing = dest.join(listing);
        fs::write(&listing, listing_text(&report))
            .map_err(|e| format!("{}: {e}", listing.display()))?;
        reports.push(report);
    }
    let window = dest.join("licenses.json");
    fs::write(&window, window_list(&reports, &root)?.to_compact())
        .map_err(|e| format!("{}: {e}", window.display()))?;
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

/// cargo-about's report on the crates in `package`.
fn about(package: &str, dir: &Path) -> Result<Json> {
    let report = dir.join("about.json");
    cargo(&[
        "about",
        "generate",
        "--locked",
        "--fail",
        "--format",
        "json",
        "--config",
        "about.toml",
        "--manifest-path",
        &format!("crates/{package}/Cargo.toml"),
        "--output-file",
        &report.to_string_lossy(),
    ])
    .map_err(|e| {
        format!("{e}\n(needs cargo-about: `cargo install cargo-about --locked --features cli`)")
    })?;
    let text = fs::read_to_string(&report).map_err(|e| format!("{}: {e}", report.display()))?;
    fs::remove_file(&report).map_err(|e| format!("{}: {e}", report.display()))?;
    json::parse(&text).map_err(|e| format!("cargo-about's report: {e}"))
}

fn list<'a>(json: &'a Json, key: &str) -> Result<&'a [Json]> {
    json.get(key)
        .and_then(Json::as_array)
        .ok_or_else(|| format!("cargo-about's report has no list {key:?}"))
}

fn text(json: &Json, key: &str) -> Result<String> {
    json.get(key)
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| format!("cargo-about's report has no text {key:?}"))
}

struct Crate {
    name: String,
    version: String,
    repository: Option<String>,
}

struct License {
    name: String,
    id: String,
    text: String,
    used_by: Vec<Crate>,
}

struct Report {
    overview: Vec<(String, f64)>,
    licenses: Vec<License>,
}

impl Report {
    fn read(about: &Json, root: &Path) -> Result<Report> {
        let mut overview = Vec::new();
        for kind in list(about, "overview")? {
            let count = kind.get("count").and_then(Json::as_f64).unwrap_or(0.0);
            overview.push((text(kind, "name")?, count));
        }
        let mut licenses = Vec::new();
        for license in list(about, "licenses")? {
            let (name, id) = (text(license, "name")?, text(license, "id")?);
            let found = text(license, "text")?;
            let mut used_by = Vec::new();
            for user in list(license, "used_by")? {
                let package = user
                    .get("crate")
                    .ok_or("cargo-about's report names no crate")?;
                used_by.push(Crate {
                    name: text(package, "name")?,
                    version: text(package, "version")?,
                    repository: text(package, "repository").ok(),
                });
            }
            if !is_template(&found) {
                licenses.push(License {
                    name,
                    id,
                    text: found,
                    used_by,
                });
                continue;
            }
            for user in used_by {
                let text = supplied(&user, root)?;
                let (name, id) = (name.clone(), id.clone());
                let used_by = vec![user];
                licenses.push(License {
                    name,
                    id,
                    text,
                    used_by,
                });
            }
        }
        Ok(Report { overview, licenses })
    }
}

fn is_template(text: &str) -> bool {
    PLACEHOLDERS.iter().any(|p| text.contains(p))
}

fn supplied(user: &Crate, root: &Path) -> Result<String> {
    let Crate { name, version, .. } = user;
    let (.., file) = SUPPLIED
        .iter()
        .find(|(n, v, _)| n == name && v == version)
        .ok_or_else(|| {
            format!(
                "{name} {version} has no license text of its own: add it to {CRATE_TEXTS} and to \
                 SUPPLIED in xtask/src/licenses.rs"
            )
        })?;
    let path = root.join(CRATE_TEXTS).join(file);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = text.replace("\r\n", "\n");
    if is_template(&text) {
        return Err(format!("{} has a placeholder", path.display()));
    }
    Ok(text)
}

fn listing_text(report: &Report) -> String {
    let mut out = String::from(
        "Third-party licenses for crates compiled into this asrs (AdvantageScope port) binary.\n\
         Generated by `cargo xtask licenses` with cargo-about. Do not edit.\n\n\
         Ported and derived sources that are not crates are listed in THIRD_PARTY_NOTICES.md.\n\n",
    );
    for (name, count) in &report.overview {
        let _ = writeln!(out, "{name} ({count})");
    }
    let (double, single) = ("=".repeat(80), "-".repeat(80));
    for license in &report.licenses {
        let License { name, id, .. } = license;
        let _ = writeln!(out, "\n{double}\n{name} ({id})\nUsed by:");
        for user in &license.used_by {
            let Crate { name, version, .. } = user;
            let _ = match &user.repository {
                Some(repository) => writeln!(out, "  {name} {version} ({repository})"),
                None => writeln!(out, "  {name} {version}"),
            };
        }
        let _ = writeln!(out, "{single}\n{}", license.text);
    }
    out
}

/// The Licenses window's list, in the shape of upstream's licenses.json: asrs, the crates compiled
/// into it, and the projects its ported sources come from.
fn window_list(reports: &[Report], root: &Path) -> Result<Json> {
    let read = |path: &Path| {
        fs::read_to_string(path)
            .map(|text| text.replace("\r\n", "\n"))
            .map_err(|e| format!("{}: {e}", path.display()))
    };
    let mut entries = vec![
        ("asrs".to_owned(), read(&root.join("LICENSE"))?),
        (
            "Third-party notices".to_owned(),
            read(&root.join("THIRD_PARTY_NOTICES.md"))?,
        ),
    ];
    let mut crates: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for license in reports.iter().flat_map(|report| &report.licenses) {
        for user in &license.used_by {
            let key = (user.name.clone(), user.version.clone());
            let texts = crates.entry(key).or_default();
            if !texts.contains(&license.text) {
                texts.push(license.text.clone());
            }
        }
    }
    let mut versions: HashMap<String, usize> = HashMap::new();
    for (name, _) in crates.keys() {
        *versions.entry(name.clone()).or_default() += 1;
    }
    for ((name, version), texts) in crates {
        let module = if versions[&name] > 1 {
            format!("{name} {version}")
        } else {
            name
        };
        entries.push((module, texts.join("\n")));
    }
    let dir = root.join("licenses");
    let mut names: Vec<_> = fs::read_dir(&dir)
        .and_then(|files| files.map(|f| Ok(f?.path())).collect::<std::io::Result<_>>())
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    names.retain(|path| path.extension().is_some_and(|e| e == "txt"));
    names.sort();
    for path in names {
        let name = path.file_stem().unwrap_or_default();
        entries.push((name.to_string_lossy().into_owned(), read(&path)?));
    }
    let entry = |(module, text): (String, String)| {
        Json::object([
            ("module", Json::string(&module)),
            ("text", Json::string(&text)),
        ])
    };
    Ok(Json::Array(entries.into_iter().map(entry).collect()))
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
        if entry.path().is_dir() {
            continue;
        }
        fs::copy(entry.path(), dst.join(entry.file_name())).map_err(err)?;
        count += 1;
    }
    Ok(count)
}
