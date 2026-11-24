#![allow(clippy::disallowed_methods)]

//! Tab and Shift+Tab through the real hub and its tabs, headless: what has the focus after each
//! key of the `traverse` scenarios is what has it in upstream's app.

use std::path::Path;

use asrs_app::scenario;
use asrs_parity::json::{self, Json};
use asrs_parity::tally::Tally;
use asrs_parity::vectors::Record;
use asrs_parity::{focus, vectors};

const VECTOR: &str = "focus/order";

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
    let id = vectors::platform_vector(VECTOR);
    let records = vectors::load(&id).unwrap();
    let is_log = |record: &Record| record.field("log").is_some();
    let kind = |record: &Record| if is_log(record) { "log" } else { "focused" }.to_owned();
    let mut tally = Tally::new(&id, &records, kind);
    for record in records.iter().filter(|r| is_log(r)) {
        let log = record.string("log").unwrap();
        std::fs::write(inputs.join(log), unhex(&record.string("bytes").unwrap())).unwrap();
        tally.compared("log");
    }
    let mut ran = Vec::new();
    let mut differences = Vec::new();
    for record in records.iter().filter(|r| !is_log(r)) {
        let name = record.string("scenario").unwrap();
        if !ran.contains(&name) {
            run(&name, &inputs, &dir);
            ran.push(name.clone());
        }
        let compared: Result<(), String> = tally.compare("focused", record, |fields| {
            let traverse = fields.string("traverse")?;
            let index = fields.field("index").and_then(Json::as_f64);
            let index = index.ok_or("no index")? as usize;
            let expected = fields.field("focused").ok_or("no focused")?;
            let at = format!(
                "{}/1x-light/files/focus/{traverse}.json",
                fields.string("scenario")?
            );
            let kept = std::fs::read_to_string(dir.join(at)).map_err(|e| e.to_string())?;
            let line = kept
                .lines()
                .nth(index)
                .ok_or("the port pressed fewer keys")?;
            match focus::difference(expected, &json::parse(line).map_err(|e| e.to_string())?) {
                Some(difference) => Err(format!("{name} {traverse} {index}: {difference}")),
                None => Ok(()),
            }
        });
        differences.extend(compared.err());
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(differences.is_empty(), "{}", differences.join("\n"));
    println!("hub tab order: {} records equal", tally.finish());
}
