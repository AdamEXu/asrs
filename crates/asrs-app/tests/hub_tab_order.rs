#![allow(clippy::disallowed_methods)]

//! Tab and Shift+Tab through the real hub and its tabs, headless: what has the focus after each
//! key of the `traverse` scenarios is what has it in upstream's app.

use std::path::Path;

use asrs_app::scenario;
use asrs_parity::json::{self, Json};
use asrs_parity::{focus, vectors};

struct Expected {
    scenario: String,
    traverse: String,
    index: usize,
    focused: Json,
}

fn unhex(text: &str) -> Vec<u8> {
    let digit = |at: usize| u8::from_str_radix(&text[at..at + 2], 16).expect("hex");
    (0..text.len()).step_by(2).map(digit).collect()
}

fn run(name: &str, inputs: &Path, out: &Path) {
    let root = vectors::workspace_root();
    let file = root.join(format!("parity/scenarios/{name}.json"));
    let mut args: Vec<String> = ["--no-captures", "--scale", "1", "--theme", "light"]
        .map(str::to_owned)
        .to_vec();
    let paths = [
        ("--parity-scenario", file),
        ("--out", out.join(name)),
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

fn main() {
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        println!("hub tab order: skipped, recorded on macOS only");
        return;
    }
    let dir = std::env::temp_dir().join(format!("asrs-hub-tab-order-{}", std::process::id()));
    let inputs = dir.join("inputs");
    std::fs::create_dir_all(&inputs).unwrap();
    let mut expected = Vec::new();
    for record in vectors::load(&vectors::platform_vector("focus/order"))
        .unwrap()
        .iter()
    {
        if let Some(log) = record.string("log") {
            std::fs::write(inputs.join(log), unhex(&record.string("bytes").unwrap())).unwrap();
            continue;
        }
        expected.push(Expected {
            scenario: record.string("scenario").unwrap(),
            traverse: record.string("traverse").unwrap(),
            index: record.field("index").and_then(Json::as_f64).unwrap() as usize,
            focused: record.field("focused").unwrap().clone(),
        });
    }
    let mut scenarios: Vec<&str> = expected.iter().map(|e| e.scenario.as_str()).collect();
    scenarios.dedup();
    let mut differences = Vec::new();
    for name in scenarios {
        run(name, &inputs, &dir);
        for e in expected.iter().filter(|e| e.scenario == name) {
            let at = format!("{name}/1x-light/files/focus/{}.json", e.traverse);
            let records = std::fs::read_to_string(dir.join(at)).unwrap();
            let focused = json::parse(records.lines().nth(e.index).unwrap()).unwrap();
            if let Some(difference) = focus::difference(&e.focused, &focused) {
                differences.push(format!("{name} {} {}: {difference}", e.traverse, e.index));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    println!("hub tab order: {} records equal", expected.len());
}
