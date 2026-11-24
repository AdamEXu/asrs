#![allow(clippy::disallowed_methods)]

//! Every `title` of the port's windows against what upstream's real windows give AppKit
//! (`cargo xtask app tooltips`, `parity/tooltips`): the scenarios run headless, and at each
//! capture the pointer visits the middle and the corner of every titled element and leaves the
//! window. The titles, their boxes, the texts given at each move and the titles on a grid over
//! the window must equal the recording.

use std::path::{Path, PathBuf};

use asrs_app::scenario;
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;

/// On each edge of a box, in CSS px.
const TOLERANCE: f64 = 1.0;

fn record(path: &Path) -> Json {
    json::parse(
        &std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .expect("JSON")
}

fn array<'a>(value: &'a Json, key: &str) -> &'a [Json] {
    value.get(key).and_then(Json::as_array).unwrap_or_default()
}

fn texts(value: &Json, key: &str) -> Vec<String> {
    let texts = array(value, key).iter();
    texts
        .map(|t| t.as_str().expect("a string").to_string_lossy())
        .collect()
}

struct Titled<'a> {
    title: String,
    rect: Vec<f64>,
    value: &'a Json,
}

fn titles(capture: &Json) -> Vec<Titled<'_>> {
    (array(capture, "titles").iter())
        .map(|value| Titled {
            title: value
                .get("title")
                .and_then(Json::as_str)
                .expect("a title")
                .to_string_lossy(),
            rect: array(value, "rect")
                .iter()
                .filter_map(Json::as_f64)
                .collect(),
            value,
        })
        .collect()
}

fn near(a: &[f64], b: &[f64]) -> bool {
    let edges = |r: &[f64]| [r[0], r[1], r[0] + r[2], r[1] + r[3]];
    edges(a)
        .iter()
        .zip(edges(b))
        .all(|(a, b)| (a - b).abs() <= TOLERANCE)
}

/// The title at each point of the grid; `Err` near an edge.
fn grid(capture: &Json) -> Vec<Vec<Result<Option<String>, ()>>> {
    let names = texts(capture, "texts");
    let cell = |c: char| match c {
        '?' => Err(()),
        '.' => Ok(None),
        c => Ok(Some(names[c as usize - 'A' as usize].clone())),
    };
    let rows = texts(capture, "grid");
    rows.iter()
        .map(|row| row.chars().map(cell).collect())
        .collect()
}

fn compare(at: &str, upstream: &Json, port: &Json, wrong: &mut Vec<String>) -> [usize; 2] {
    let (expected, mut actual) = (titles(upstream), titles(port));
    for want in &expected {
        let found =
            (actual.iter()).position(|got| got.title == want.title && near(&got.rect, &want.rect));
        let Some(found) = found else {
            wrong.push(format!("{at}: no {:?} at {:?}", want.title, want.rect));
            continue;
        };
        let got = actual.remove(found);
        for key in ["shown", "corner"] {
            let (given, recorded) = (texts(got.value, key), texts(want.value, key));
            if given != recorded {
                let title = &want.title;
                wrong.push(format!(
                    "{at}: {title:?} {key}: {given:?}, upstream {recorded:?}"
                ));
            }
        }
    }
    for extra in actual {
        wrong.push(format!(
            "{at}: {:?} at {:?}, not upstream",
            extra.title, extra.rect
        ));
    }
    if texts(upstream, "left") != texts(port, "left") {
        wrong.push(format!("{at}: leaving the window"));
    }
    let (want, got) = (grid(upstream), grid(port));
    assert_eq!(want.len(), got.len(), "{at}: grid rows");
    let mut points = 0;
    for (y, (want, got)) in want.iter().zip(&got).enumerate() {
        assert_eq!(want.len(), got.len(), "{at}: grid columns");
        for (x, (want, got)) in want.iter().zip(got).enumerate() {
            if let (Ok(want), Ok(got)) = (want, got) {
                points += 1;
                if want != got {
                    let [x, y] = [x, y].map(|i| i * 24 + 12);
                    wrong.push(format!("{at}: ({x}, {y}) is {got:?}, upstream {want:?}"));
                }
            }
        }
    }
    [expected.len(), points]
}

fn main() {
    if asrs_parity::ci::skip_in_ci("tooltips", "a GPU") {
        return;
    }
    if !cfg!(target_os = "macos") {
        println!("tooltips: skipped, the recordings are of macOS's layout");
        return;
    }
    let root = vectors::workspace_root();
    let references = root.join("parity/tooltips");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&references)
        .expect("parity/tooltips")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    let only: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let out = std::env::temp_dir().join(format!("asrs-tooltips-{}", std::process::id()));
    let mut wrong = Vec::new();
    let [mut captures, mut compared, mut points] = [0; 3];
    for file in files {
        let name = file
            .file_stem()
            .and_then(|s| s.to_str())
            .expect("a name")
            .to_owned();
        if !only.is_empty() && !only.contains(&name) {
            continue;
        }
        let dir = out.join(&name);
        let args: Vec<String> = [
            "--parity-scenario",
            root.join(format!("parity/scenarios/{name}.json"))
                .to_str()
                .unwrap(),
            "--out",
            dir.to_str().unwrap(),
            "--inputs",
            references.join("inputs").to_str().unwrap(),
            "--bundled-assets",
            root.join("reference/AdvantageScope/bundledAssets")
                .to_str()
                .unwrap(),
            "--scale",
            "1",
            "--theme",
            "light",
            "--tooltips",
            file.to_str().unwrap(),
        ]
        .map(str::to_owned)
        .to_vec();
        scenario::run(&args).unwrap_or_else(|e| panic!("{name}: {e}"));
        let reference = record(&file);
        let Json::Object(recorded) = &reference else {
            panic!("{name}: not an object");
        };
        for (capture, upstream) in recorded {
            let capture = capture.to_string_lossy();
            let file = dir.join(format!("1x-light/captures/{capture}.tooltips.json"));
            let [titles, grid] = compare(
                &format!("{name} {capture}"),
                upstream,
                &record(&file),
                &mut wrong,
            );
            captures += 1;
            compared += titles;
            points += grid;
        }
    }
    let _ = std::fs::remove_dir_all(&out);
    println!("tooltips: {captures} captures, {compared} titles, {points} points");
    assert!(compared > 0, "nothing was compared");
    for line in &wrong {
        println!("{line}");
    }
    assert!(wrong.is_empty(), "{} differences", wrong.len());
}
