#![allow(clippy::disallowed_methods)]

//! The wheel-calibration scenario on the port, headless, against upstream's timeline ranges: on
//! Linux `wheel-calibration-linux`, with the fractions of a tick a high-resolution wheel sends, on
//! Windows `wheel-calibration-windows`, whose events are the real wheel's, 120 units a notch.
//! `parity/scenarios/inputs/drive.wpilog` is the log the Electron driver generates for
//! `app:drive` with upstream's `WPILOGEncoder` (oracle/src/app/logs.ts), copied from its
//! `inputs/`.

use std::path::Path;

use asrs_app::scenario;
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;

struct Calibration {
    scenario: &'static str,
    captures: &'static [&'static str],
    /// `selection.getTimelineRange()` in each capture, per scale, in the light theme.
    upstream: [(u32, &'static [[f64; 2]]); 2],
}

/// From `cargo xtask app run wheel-calibration` on macOS with no stray pointer input.
const MAC: Calibration = Calibration {
    scenario: "wheel-calibration",
    captures: &["before", "notched", "trackpad"],
    upstream: [
        (
            1,
            &[
                [0.0, 10.0],
                [0.15738966007338107, 9.842610339926617],
                [0.04477582253141499, 9.955224177468585],
            ],
        ),
        (
            2,
            &[
                [0.0, 10.0],
                [0.1573896600733855, 9.842610339926612],
                [0.04477582253141499, 9.955224177468585],
            ],
        ),
    ],
};

/// From `cargo xtask app run wheel-calibration-linux` on Linux x86_64 under Xvfb and xfwm4.
const LINUX: Calibration = Calibration {
    scenario: "wheel-calibration-linux",
    captures: &["before", "notched", "trackpad", "hires"],
    upstream: [
        (
            1,
            &[
                [0.0, 10.0],
                [0.15738966007338284, 9.842610339926615],
                [0.04477582253141499, 9.955224177468585],
                [0.03486041895230052, 9.965139581047698],
            ],
        ),
        (
            2,
            &[
                [0.0, 10.0],
                [0.1573896600733793, 9.842610339926619],
                [0.04477582253141499, 9.955224177468585],
                [0.03734112836573367, 9.962658871634265],
            ],
        ),
    ],
};

/// From `cargo xtask app run wheel-calibration-windows` on the Windows x86_64 VM.
const WINDOWS: Calibration = Calibration {
    scenario: "wheel-calibration-windows",
    captures: &["before", "notched", "one-40", "units-40"],
    upstream: [
        (
            1,
            &[
                [0.0, 10.0],
                [0.15738966007338107, 9.842610339926617],
                [0.30982501912715854, 9.69017498087284],
                [0.4574620283120101, 9.542537971687988],
            ],
        ),
        (
            2,
            &[
                [0.0, 10.0],
                [0.15738966007338195, 9.842610339926615],
                [0.3098250191271603, 9.690174980872838],
                [0.4574620283120172, 9.542537971687981],
            ],
        ),
    ],
};

fn timeline_range(path: &Path) -> [f64; 2] {
    let record = json::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    let range = record
        .get("timelineRange")
        .and_then(Json::as_array)
        .unwrap_or_else(|| panic!("{}: no timelineRange", path.display()));
    [range[0].as_f64().unwrap(), range[1].as_f64().unwrap()]
}

fn main() {
    let calibration = if cfg!(target_os = "macos") {
        MAC
    } else if cfg!(target_os = "linux") {
        LINUX
    } else if cfg!(windows) {
        if asrs_parity::ci::skip_in_ci("wheel scenario", "a desktop") {
            return;
        }
        WINDOWS
    } else {
        println!("wheel scenario: skipped, no recorded ranges for this platform");
        return;
    };
    let root = vectors::workspace_root();
    let out = std::env::temp_dir().join(format!("asrs-wheel-scenario-{}", std::process::id()));
    let args: Vec<String> = [
        "--parity-scenario",
        root.join(format!("parity/scenarios/{}.json", calibration.scenario))
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
    scenario::run(&args).unwrap_or_else(|e| panic!("{}: {e}", calibration.scenario));

    for (scale, ranges) in calibration.upstream {
        for (capture, upstream) in calibration.captures.iter().zip(ranges) {
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
