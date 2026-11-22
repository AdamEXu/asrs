#![allow(clippy::disallowed_methods)]
#![cfg(target_os = "macos")]

use asrs_canvas::text::dom::{self, Style};
use asrs_canvas::text::platform_backend;
use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};

const SIZE_TOLERANCE: f64 = 0.05;
const UA_SIZE_TOLERANCE: f64 = 1.0;
const RUN_TOLERANCE: f64 = 1.0 / 64.0;

fn string(r: &Record, k: &str) -> String {
    r.string(k).unwrap_or_else(|| panic!("no {k}"))
}

fn number(r: &Record, k: &str) -> Option<f64> {
    r.field(k).and_then(Json::as_f64)
}

fn rect(v: &Json) -> [f64; 4] {
    let a = v.as_array().expect("rect");
    [0, 1, 2, 3].map(|i| a[i].as_f64().expect("rect number"))
}

/// The left, top, right and bottom of `[x, y, width, height]`.
fn edges(r: [f64; 4]) -> [f64; 4] {
    [r[0], r[1], r[0] + r[2], r[1] + r[3]]
}

fn close(a: [f64; 4], b: [f64; 4], tolerance: f64) -> bool {
    edges(a)
        .iter()
        .zip(edges(b))
        .all(|(p, q)| (p - q).abs() <= tolerance)
}

/// Every light-scene box of `cargo xtask text`, laid out by `text::dom` against its DOM border
/// box and text client rects from Chromium at 1x and 2x.
#[test]
fn dom_text_boxes_match_chromium() {
    let records = vectors::load("text/boxes-macos").unwrap();
    assert!(records.len() > 150, "{} records", records.len());
    let mut backend = platform_backend().unwrap();
    let mut failures = Vec::new();
    let (mut max_size, mut runs_checked) = (0f64, 0);
    for r in &records {
        let id = format!(
            "{}@{}x {}",
            string(r, "scene"),
            number(r, "scale").unwrap(),
            string(r, "id")
        );
        let keys = [
            ("font", "font"),
            ("line-height", "lineHeight"),
            ("text-align", "align"),
            ("direction", "direction"),
            ("white-space", "whiteSpace"),
            ("text-overflow", "overflow"),
        ];
        let values: Vec<(&str, String)> = keys.iter().map(|&(c, k)| (c, string(r, k))).collect();
        let style = Style::from_css(values.iter().map(|(k, v)| (*k, v.as_str()))).unwrap();
        let scale = number(r, "scale").unwrap() as f32;
        let available = number(r, "width").map(|w| dom::lu_round(w as f32 * scale));
        let text = string(r, "text");
        let layout = dom::layout(backend.as_mut(), &style, &text, available, scale);
        let dom_rect = rect(r.field("rect").unwrap());
        let exact = !matches!(r.field("exact"), Some(Json::Bool(false)));
        let emoji = string(r, "scene").starts_with("text-emoji");
        for (k, ours, i) in [
            ("width", layout.width_px(), 2),
            ("height", layout.height_px(), 3),
        ] {
            if number(r, k).is_none() {
                let err = (ours - dom_rect[i]).abs();
                if emoji && err != 0.0 {
                    failures.push(format!(
                        "{id}: {k} {ours} vs DOM {}, not exact",
                        dom_rect[i]
                    ));
                }
                if exact {
                    max_size = max_size.max(err);
                }
                if err
                    > if exact {
                        SIZE_TOLERANCE
                    } else {
                        UA_SIZE_TOLERANCE
                    }
                {
                    failures.push(format!("{id}: {k} {ours} vs DOM {}", dom_rect[i]));
                }
            }
        }
        let px = |lu: dom::Lu| f64::from(lu) / 64.0 / f64::from(scale);
        let origin = (
            dom::lu_round(number(r, "x").unwrap() as f32 * scale),
            dom::lu_round(number(r, "y").unwrap() as f32 * scale),
        );
        let mut ours = Vec::new();
        for line in &layout.lines {
            for f in line.fragments.iter().filter(|f| f.text != "\u{2026}") {
                ours.push((
                    f.text.clone(),
                    [
                        px(origin.0 + f.x),
                        px(origin.1 + line.baseline - layout.ascent),
                        px(f.width),
                        px(layout.ascent + layout.descent),
                    ],
                ));
            }
        }
        let dom_runs: Vec<[f64; 4]> = r
            .field("lines")
            .and_then(Json::as_array)
            .unwrap_or_default()
            .iter()
            .map(rect)
            .filter(|d| d[2] > 0.0)
            .collect();
        for (t, o) in &ours {
            runs_checked += 1;
            if emoji && !dom_runs.iter().any(|d| d[2] == o[2] && d[3] == o[3]) {
                failures.push(format!(
                    "{id}: run {t:?} {o:?} size not exactly in {dom_runs:?}"
                ));
            }
            if exact && !dom_runs.iter().any(|d| close(*o, *d, RUN_TOLERANCE)) {
                failures.push(format!("{id}: run {t:?} {o:?} not in {dom_runs:?}"));
            }
        }
        let inside = |d: &[f64; 4]| {
            d[0] >= dom_rect[0] - 0.5 && d[0] + d[2] <= dom_rect[0] + dom_rect[2] + 0.5
        };
        for d in dom_runs.iter().filter(|d| inside(d)) {
            if exact && !ours.iter().any(|(_, o)| close(*o, *d, RUN_TOLERANCE)) {
                failures.push(format!("{id}: DOM text rect {d:?} not in {ours:?}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures (max size error {max_size}):\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(runs_checked > 200, "{runs_checked} runs");
    assert!(max_size <= SIZE_TOLERANCE, "max size error {max_size}");
}
