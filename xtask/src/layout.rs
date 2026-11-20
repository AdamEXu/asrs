use std::fs;
use std::path::Path;

use crate::{Result, workspace_root};

const OPT_OUT: &str = "#![allow(clippy::disallowed_methods)]";
const GPUI_CRATE: &str = "asrs-ui";
const UNSAFE_ROOTS: &[&str] = &[
    "asrs-ui/src/platform/",
    "asrs-canvas/src/text/mac",
    "asrs-canvas/src/text/win",
    "asrs-canvas/src/text/linux",
];

struct Rule {
    name: &'static str,
    parity: bool,
    may_depend_on: &'static [&'static str],
}

const RULES: &[Rule] = &[
    rule("asrs-js", false, &[]),
    rule("asrs-log", true, &["asrs-js"]),
    rule("asrs-formats", true, &["asrs-js", "asrs-log"]),
    rule("asrs-live", true, &["asrs-js", "asrs-log", "asrs-formats"]),
    rule("asrs-canvas", false, &["asrs-js"]),
    rule(
        "asrs-model",
        true,
        &["asrs-js", "asrs-log", "asrs-formats", "asrs-live"],
    ),
    rule(
        "asrs-tabs",
        true,
        &[
            "asrs-js",
            "asrs-log",
            "asrs-formats",
            "asrs-live",
            "asrs-model",
            "asrs-canvas",
        ],
    ),
    rule(
        "asrs-hub",
        true,
        &[
            "asrs-js",
            "asrs-log",
            "asrs-formats",
            "asrs-live",
            "asrs-model",
            "asrs-canvas",
            "asrs-tabs",
        ],
    ),
    rule(
        "asrs-services",
        true,
        &[
            "asrs-js",
            "asrs-log",
            "asrs-formats",
            "asrs-live",
            "asrs-model",
        ],
    ),
    rule(
        "asrs-field3d",
        false,
        &[
            "asrs-js",
            "asrs-log",
            "asrs-formats",
            "asrs-live",
            "asrs-model",
            "asrs-canvas",
        ],
    ),
    rule("asrs-ui", false, &["asrs-js", "asrs-canvas"]),
    rule(
        "asrs-app",
        false,
        &[
            "asrs-js",
            "asrs-log",
            "asrs-formats",
            "asrs-live",
            "asrs-canvas",
            "asrs-model",
            "asrs-tabs",
            "asrs-hub",
            "asrs-services",
            "asrs-field3d",
            "asrs-ui",
        ],
    ),
    rule(
        "asrs-parity",
        false,
        &[
            "asrs-js",
            "asrs-log",
            "asrs-formats",
            "asrs-live",
            "asrs-canvas",
            "asrs-model",
            "asrs-tabs",
            "asrs-hub",
            "asrs-services",
        ],
    ),
    rule(
        "asrs-cli",
        false,
        &[
            "asrs-js",
            "asrs-log",
            "asrs-formats",
            "asrs-live",
            "asrs-canvas",
            "asrs-model",
            "asrs-tabs",
            "asrs-hub",
            "asrs-services",
            "asrs-field3d",
            "asrs-parity",
        ],
    ),
];

const fn rule(name: &'static str, parity: bool, may_depend_on: &'static [&'static str]) -> Rule {
    Rule {
        name,
        parity,
        may_depend_on,
    }
}

pub fn run() -> Result {
    let crates_dir = workspace_root().join("crates");
    let mut problems = Vec::new();
    let mut seen = Vec::new();
    for entry in fs::read_dir(&crates_dir).map_err(|e| e.to_string())? {
        let dir = entry.map_err(|e| e.to_string())?.path();
        if !dir.join("Cargo.toml").is_file() {
            continue;
        }
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        let Some(rule) = RULES.iter().find(|r| r.name == name) else {
            problems.push(format!("{name}: not listed in xtask layout rules"));
            continue;
        };
        seen.push(name);
        check_crate(rule, &dir, &mut problems)?;
    }
    for rule in RULES {
        if !seen.iter().any(|s| s == rule.name) {
            problems.push(format!("{}: listed in layout rules but missing", rule.name));
        }
    }
    if problems.is_empty() {
        eprintln!("layout ok");
        Ok(())
    } else {
        Err(format!("layout check failed:\n  {}", problems.join("\n  ")))
    }
}

fn check_crate(rule: &Rule, dir: &Path, problems: &mut Vec<String>) -> Result {
    let manifest = fs::read_to_string(dir.join("Cargo.toml")).map_err(|e| e.to_string())?;
    for (dep, dev) in dependencies(&manifest) {
        if dep.starts_with("gpui") && rule.name != GPUI_CRATE {
            problems.push(format!(
                "{}: depends on {dep}; GPUI is only allowed in {GPUI_CRATE}",
                rule.name
            ));
        }
        let internal = dep.starts_with("asrs-");
        let allowed = rule.may_depend_on.contains(&dep.as_str()) || (dev && dep == "asrs-parity");
        if internal && !allowed {
            problems.push(format!("{}: may not depend on {dep}", rule.name));
        }
    }

    let mut sources = Vec::new();
    collect_rs(&dir.join("src"), &mut sources)?;
    let mut opted_out = false;
    for path in &sources {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        if text.contains("clippy::disallowed_methods") || text.contains("clippy::disallowed_types")
        {
            if rule.parity {
                problems.push(format!(
                    "{}: parity crates may not silence disallowed lints ({})",
                    rule.name,
                    path.display()
                ));
            }
            opted_out |= text.lines().any(|l| l.trim() == OPT_OUT);
        }
    }
    if !rule.parity && !opted_out {
        problems.push(format!(
            "{}: crate root must start with {OPT_OUT}",
            rule.name
        ));
    }

    for extra in ["tests", "examples", "benches", "build.rs"] {
        let path = dir.join(extra);
        if path.is_dir() {
            collect_rs(&path, &mut sources)?;
        } else if path.is_file() {
            sources.push(path);
        }
    }
    for path in &sources {
        let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
        if lifts_unsafe_lint(&text) && !unsafe_allowed(rule.name, dir, path) {
            problems.push(format!(
                "{}: unsafe_code may only be allowed under {} ({})",
                rule.name,
                UNSAFE_ROOTS.join(", "),
                path.display()
            ));
        }
    }
    Ok(())
}

fn lifts_unsafe_lint(text: &str) -> bool {
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    ["allow(", "expect(", "warn("].iter().any(|level| {
        compact.match_indices(level).any(|(i, _)| {
            compact[i + level.len()..]
                .split(')')
                .next()
                .is_some_and(|lints| lints.split(',').any(|l| l == "unsafe_code"))
        })
    })
}

fn unsafe_allowed(crate_name: &str, crate_dir: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(crate_dir) else {
        return false;
    };
    let relative = format!(
        "{crate_name}/{}",
        relative.to_string_lossy().replace('\\', "/")
    );
    UNSAFE_ROOTS.iter().any(|root| relative.starts_with(root))
}

fn dependencies(manifest: &str) -> Vec<(String, bool)> {
    let mut deps = Vec::new();
    let mut section: Option<bool> = None;
    for line in manifest.lines().map(str::trim) {
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let dev = header.contains("dev-dependencies");
            section = None;
            match header.split_once("dependencies") {
                Some((_, "")) => section = Some(dev),
                Some((_, rest)) => {
                    if let Some(name) = rest.strip_prefix('.') {
                        deps.push((name.to_owned(), dev));
                    }
                }
                None => {}
            }
            continue;
        }
        if let Some(dev) = section {
            let name = line.split(['=', '.', ' ']).next().unwrap_or_default();
            if !name.is_empty() && !name.starts_with('#') {
                deps.push((name.to_owned(), dev));
            }
        }
    }
    deps
}

fn collect_rs(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect_rs(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
    Ok(())
}
