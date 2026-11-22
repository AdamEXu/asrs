#![allow(clippy::disallowed_methods)]

//! The wheel calibration: `parity/calibration/wheel-<platform>-<scale>x.json`, recorded from
//! Electron by `oracle/tools/wheelCalibration.cjs`, holds the overlay position Chromium reached
//! at each `scroll` event for a script of precise (trackpad) and notched wheel events. The same
//! events go through `ScrollSensor`: a precise event must produce a scroll event exactly when
//! Chromium's did, at the same position; a notched tick must animate (more than one scroll
//! event, as Chromium's did) and settle where Chromium's settled.

use asrs_model::scroll_sensor::{ScrollSensor, Wheel};
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;

const FRAME_MS: f64 = 1000.0 / 60.0;

fn numbers(value: &Json) -> Vec<f64> {
    value
        .as_array()
        .expect("array")
        .iter()
        .map(|n| n.as_f64().expect("number"))
        .collect()
}

fn run(file: &str) -> Result<(usize, usize), String> {
    let path = vectors::workspace_root()
        .join("parity/calibration")
        .join(file);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let root = json::parse(&text).map_err(|e| e.to_string())?;
    let dpr = root
        .get("devicePixelRatio")
        .and_then(Json::as_f64)
        .ok_or("devicePixelRatio")?;
    let overlay = numbers(root.get("overlay").ok_or("overlay")?);
    let cases = root.get("cases").ok_or("cases")?;
    let Json::Object(cases) = cases else {
        return Err("cases is not an object".to_owned());
    };
    let (mut precise_events, mut notches) = (0, 0);
    for (name, steps) in cases {
        let name = name.to_string_lossy();
        let mut sensor = ScrollSensor::default();
        sensor.set_layout([overlay[0], overlay[1]], dpr);
        let mut now = 0.0;
        sensor.periodic(now);
        now += FRAME_MS;
        sensor.scroll_step(now, true);
        for (i, step) in steps.as_array().ok_or("steps")?.iter().enumerate() {
            let at = |what: String| format!("{file} {name} step {i}: {what}");
            let delta = numbers(step.get("wheel").ok_or("wheel")?);
            let precise = match step.get("precise") {
                Some(Json::Bool(b)) => *b,
                other => return Err(at(format!("precise is {other:?}"))),
            };
            let scrolls: Vec<Vec<f64>> = step
                .get("scrolls")
                .and_then(Json::as_array)
                .ok_or_else(|| at("no scrolls".to_owned()))?
                .iter()
                .map(numbers)
                .collect();
            sensor.wheel(
                Wheel {
                    delta: [delta[0], delta[1]],
                    precise,
                },
                now,
            );
            let mut events = 0;
            loop {
                now += FRAME_MS;
                sensor.animate(now);
                if sensor.scroll_step(now, true).is_some() {
                    events += 1;
                }
                if !sensor.animating() {
                    break;
                }
            }
            let position = sensor.position().to_vec();
            let expected = scrolls.last().cloned();
            if precise {
                precise_events += 1;
                if events != scrolls.len() {
                    return Err(at(format!(
                        "{events} scroll events in the port, {} in Chromium",
                        scrolls.len()
                    )));
                }
                if let Some(expected) = expected
                    && expected != position
                {
                    return Err(at(format!(
                        "port at {position:?}, Chromium at {expected:?}"
                    )));
                }
            } else {
                notches += 1;
                let expected = expected.ok_or_else(|| at("Chromium never scrolled".to_owned()))?;
                if expected != position {
                    return Err(at(format!(
                        "port settled at {position:?}, Chromium at {expected:?}"
                    )));
                }
                if (scrolls.len() > 1) != (events > 1) {
                    return Err(at(format!(
                        "{events} scroll events in the port, {} in Chromium",
                        scrolls.len()
                    )));
                }
            }
        }
    }
    Ok((precise_events, notches))
}

#[test]
fn wheel_calibration_matches_chromium() {
    let mut failures = Vec::new();
    for file in ["wheel-darwin-1x.json", "wheel-darwin-2x.json"] {
        match run(file) {
            Ok((precise, notches)) => {
                assert!(precise > 0 && notches > 0, "{file} compared nothing");
                eprintln!(
                    "{file}: {precise} precise events, {notches} notched ticks, 0 mismatches"
                );
            }
            Err(e) => failures.push(e),
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
