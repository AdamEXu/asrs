#![allow(clippy::disallowed_methods)]

//! Tab and Shift+Tab, the arrow keys in inputs and the pointer leaving through the real windows,
//! headless, against what `cargo xtask app run` recorded of upstream at 1x in the light theme:
//! `document.activeElement` at each `traverse` step of the scenarios and the hovered time at each
//! `hover`. `data/focus_macos.jsonl` is of `focus-order` on macOS, from before the recorder kept
//! `stop` and values. `data/focus_linux.jsonl` is of every scenario here on Linux x86_64 under the
//! render host's fonts. The logs in `parity/scenarios/inputs` are what the driver generates.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use asrs_app::scenario;
use asrs_parity::json::{self, Json};
use asrs_parity::{focus, vectors};

const MACOS: &str = include_str!("data/focus_macos.jsonl");
const LINUX: &str = include_str!("data/focus_linux.jsonl");

/// Upstream's classic scrollbars on Linux take space the port gives to what they are beside (#268).
const SCROLLBAR: f64 = 15.0;

fn run(root: &Path, inputs: &Path, name: &str, out: &Path) {
    let mut args: Vec<String> = ["--no-captures", "--scale", "1", "--theme", "light"]
        .map(str::to_owned)
        .to_vec();
    let paths = [
        (
            "--parity-scenario",
            root.join(format!("parity/scenarios/{name}.json")),
        ),
        ("--out", out.to_owned()),
        ("--inputs", inputs.to_owned()),
        (
            "--bundled-assets",
            root.join("reference/AdvantageScope/bundledAssets"),
        ),
    ];
    for (option, path) in paths {
        args.extend([option.to_owned(), path.to_string_lossy().into_owned()]);
    }
    scenario::run(&args).unwrap_or_else(|e| panic!("{name}: {e}"));
}

fn text(record: &Json, key: &str) -> Option<String> {
    Some(record.get(key)?.as_str()?.to_string_lossy())
}

/// `hub.html` gives its scroll overlays `tabindex="-1"`: presses focus them and Tab does not.
fn overlay(focused: &Json) -> bool {
    text(focused, "element").is_some_and(|element| element.ends_with("-scroll"))
}

fn inputs(root: &Path, dir: &Path) -> PathBuf {
    let inputs = dir.join("inputs");
    // The clip of `traverse-video`, whose frames do not matter to the focus.
    std::fs::create_dir_all(inputs.join("wide")).unwrap();
    for entry in std::fs::read_dir(root.join("parity/scenarios/inputs")).unwrap() {
        let path = entry.unwrap().path();
        std::fs::copy(&path, inputs.join(path.file_name().unwrap())).unwrap();
    }
    inputs
}

fn main() {
    let recorded = if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        MACOS
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        LINUX
    } else {
        println!("hub tab order: skipped, not recorded on this platform");
        return;
    };
    let root = vectors::workspace_root();
    let assets = root.join("reference/AdvantageScope/bundledAssets").is_dir();
    let dir = std::env::temp_dir().join(format!("asrs-hub-tab-order-{}", std::process::id()));
    let inputs = inputs(&root, &dir);
    let mut scenarios: BTreeMap<String, Vec<Json>> = BTreeMap::new();
    for line in recorded.lines() {
        let record = json::parse(line).unwrap();
        let name = text(&record, "scenario").unwrap();
        scenarios.entry(name).or_default().push(record);
    }
    let (mut compared, mut without_assets) = (0, 0);
    let mut differences = Vec::new();
    for (name, records) in &scenarios {
        let out = dir.join(name);
        run(&root, &inputs, name, &out);
        let files = out.join("1x-light/files");
        for record in records {
            if let Some(hover) = text(record, "hover") {
                let kept = std::fs::read_to_string(files.join(format!("hover/{hover}.json")));
                let expected = record.get("time").unwrap();
                let kept = json::parse(kept.unwrap().trim()).unwrap();
                if kept != *expected {
                    differences.push(format!("{name} {hover}: {expected:?} upstream, {kept:?}"));
                }
                compared += 1;
                continue;
            }
            let traverse = text(record, "traverse").unwrap();
            let index = record.get("index").and_then(Json::as_f64).unwrap() as usize;
            let expected = record.get("focused").unwrap();
            let kept = std::fs::read_to_string(files.join(format!("focus/{traverse}.json")));
            let kept = kept.unwrap();
            let focused = json::parse(kept.lines().nth(index).expect("a key")).unwrap();
            // The field select is as wide as the names of upstream's bundled fields.
            if !assets && text(expected, "element").as_deref() == Some("select.field-select") {
                assert_ne!(focused, Json::Null, "{name} {traverse} {index}");
                without_assets += 1;
                continue;
            }
            let expected = if overlay(expected) {
                &Json::Null
            } else {
                expected
            };
            if let Some(difference) = focus::difference(expected, &focused) {
                differences.push(format!("{name} {traverse} {index}: {difference}"));
            }
            compared += 1;
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    println!("hub tab order: {compared} equal, {without_assets} not compared without the assets");
}
