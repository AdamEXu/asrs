#![allow(clippy::disallowed_methods)]

//! Tab and Shift+Tab through the real hub window, headless. `data/focus_order.jsonl` is what
//! upstream's `document.activeElement` was after each key of `parity/scenarios/focus-order.json`,
//! as `cargo xtask app run` recorded it on macOS at 1x in the light theme.

use std::path::Path;

use asrs_app::scenario;
use asrs_parity::json::{self, Json};
use asrs_parity::{focus, vectors};

const RECORDED: &str = include_str!("data/focus_order.jsonl");
const SCENARIO: &str = "focus-order";

fn run(root: &Path, out: &Path) {
    let mut args: Vec<String> = ["--no-captures", "--scale", "1", "--theme", "light"]
        .map(str::to_owned)
        .to_vec();
    let scenarios = root.join("parity/scenarios");
    let paths = [
        (
            "--parity-scenario",
            scenarios.join(format!("{SCENARIO}.json")),
        ),
        ("--out", out.to_owned()),
        ("--inputs", scenarios.join("inputs")),
        (
            "--bundled-assets",
            root.join("reference/AdvantageScope/bundledAssets"),
        ),
    ];
    for (option, path) in paths {
        args.extend([option.to_owned(), path.to_string_lossy().into_owned()]);
    }
    scenario::run(&args).unwrap_or_else(|e| panic!("{SCENARIO}: {e}"));
}

fn text(record: &Json, key: &str) -> Option<String> {
    Some(record.get(key)?.as_str()?.to_string_lossy())
}

/// `hub.html` gives its scroll overlays `tabindex="-1"`: presses focus them and Tab does not.
fn overlay(focused: &Json) -> bool {
    text(focused, "element").is_some_and(|element| element.ends_with("-scroll"))
}

fn main() {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        println!("hub tab order: skipped, recorded on macOS only");
        return;
    }
    let root = vectors::workspace_root();
    let assets = root.join("reference/AdvantageScope/bundledAssets").is_dir();
    let dir = std::env::temp_dir().join(format!("asrs-hub-tab-order-{}", std::process::id()));
    run(&root, &dir);
    let (mut compared, mut without_assets) = (0, 0);
    let mut differences = Vec::new();
    for line in RECORDED.lines() {
        let record = json::parse(line).unwrap();
        let traverse = text(&record, "traverse").unwrap();
        let index = record.get("index").and_then(Json::as_f64).unwrap() as usize;
        let expected = record.get("focused").unwrap();
        let at = format!("1x-light/files/focus/{traverse}.json");
        let kept = std::fs::read_to_string(dir.join(at)).unwrap();
        let focused = json::parse(kept.lines().nth(index).expect("a key")).unwrap();
        // The field select is as wide as the names of upstream's bundled fields.
        if !assets && text(expected, "element").as_deref() == Some("select.field-select") {
            assert_ne!(focused, Json::Null, "{traverse} {index}");
            without_assets += 1;
            continue;
        }
        let expected = if overlay(expected) {
            &Json::Null
        } else {
            expected
        };
        if let Some(difference) = focus::difference(expected, &focused) {
            differences.push(format!("{traverse} {index}: {difference}"));
        }
        compared += 1;
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    println!("hub tab order: {compared} equal, {without_assets} not compared without the assets");
}
