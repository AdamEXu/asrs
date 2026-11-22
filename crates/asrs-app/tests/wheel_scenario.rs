#![allow(clippy::disallowed_methods)]

//! The wheel-calibration scenario on the port, headless, against upstream's timeline ranges.
//! `parity/scenarios/inputs/drive.wpilog` is the log the Electron driver generates for
//! `app:drive` with upstream's `WPILOGEncoder` (oracle/src/app/logs.ts), copied from its
//! `inputs/`.

use std::path::Path;

use asrs_app::scenario;
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;

/// `selection.getTimelineRange()` in the captures of `cargo xtask app run wheel-calibration` on
/// macOS with no stray pointer input, light theme: `before`, `notched` and `trackpad`.
const UPSTREAM: [(u32, [[f64; 2]; 3]); 2] = [
    (
        1,
        [
            [0.0, 10.0],
            [0.15738966007338107, 9.842610339926617],
            [0.04477582253141499, 9.955224177468585],
        ],
    ),
    (
        2,
        [
            [0.0, 10.0],
            [0.1573896600733855, 9.842610339926612],
            [0.04477582253141499, 9.955224177468585],
        ],
    ),
];

const CAPTURES: [&str; 3] = ["before", "notched", "trackpad"];

fn timeline_range(path: &Path) -> [f64; 2] {
    let record = json::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    let range = record
        .get("timelineRange")
        .and_then(Json::as_array)
        .unwrap_or_else(|| panic!("{}: no timelineRange", path.display()));
    [range[0].as_f64().unwrap(), range[1].as_f64().unwrap()]
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("wheel scenario: skipped, no headless renderer on this platform");
        return;
    }
    let root = vectors::workspace_root();
    let out = std::env::temp_dir().join(format!("asrs-wheel-scenario-{}", std::process::id()));
    let args: Vec<String> = [
        "--parity-scenario",
        root.join("parity/scenarios/wheel-calibration.json")
            .to_str()
            .unwrap(),
        "--out",
        out.to_str().unwrap(),
        "--inputs",
        root.join("parity/scenarios/inputs").to_str().unwrap(),
        "--bundled-assets",
        root.join("reference/AdvantageScope/bundledAssets")
            .to_str()
            .unwrap(),
        "--scale",
        "1",
        "--scale",
        "2",
        "--theme",
        "light",
    ]
    .map(str::to_owned)
    .to_vec();
    scenario::run(&args).unwrap_or_else(|e| panic!("wheel-calibration: {e}"));

    for (scale, ranges) in UPSTREAM {
        for (capture, upstream) in CAPTURES.into_iter().zip(ranges) {
            let port = timeline_range(&out.join(format!("{scale}x-light/captures/{capture}.json")));
            let tolerance = 1e-9 * (upstream[1] - upstream[0]);
            assert!(
                (port[0] - upstream[0]).abs() <= tolerance
                    && (port[1] - upstream[1]).abs() <= tolerance,
                "{scale}x {capture}: timeline range {port:?}, upstream {upstream:?}"
            );
            println!("{scale}x {capture}: {port:?}");
        }
    }
    let _ = std::fs::remove_dir_all(&out);
    println!("wheel_scenario: ok");
}
