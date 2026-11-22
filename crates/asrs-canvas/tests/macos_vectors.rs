#![cfg(target_os = "macos")]

use std::collections::BTreeMap;

use asrs_canvas::text::layout::place_text;
use asrs_canvas::text::lut::{self, MaskGammaLut};
use asrs_canvas::text::{FontSpec, TextMetrics, measure_text, platform_backend};
use asrs_parity::vectors;

#[test]
fn embedded_lut_has_the_four_macos_rows() {
    let lut = MaskGammaLut::embedded().expect("lut-macos.bin is committed");
    let mut distinct: Vec<&[u8; 256]> = (0..lut::ROWS).map(|k| lut.row(k)).collect();
    distinct.dedup();
    assert_eq!(distinct.len(), 4);
    for k in 0..lut::ROWS {
        let row = lut.row(k);
        assert_eq!((row[0], row[255]), (0, 255));
        assert!(row.windows(2).all(|w| w[0] <= w[1]), "row {k} is monotonic");
    }
}

const FIELDS: [&str; 10] = [
    "width",
    "actualBoundingBoxLeft",
    "actualBoundingBoxRight",
    "actualBoundingBoxAscent",
    "actualBoundingBoxDescent",
    "fontBoundingBoxAscent",
    "fontBoundingBoxDescent",
    "hangingBaseline",
    "alphabeticBaseline",
    "ideographicBaseline",
];

/// Strings Chromium draws with a fallback font it picks differently from CoreText: CJK next to
/// Helvetica or a `wght` instance (Chromium matches the fallback's weight).
const FALLBACK: [&str; 1] = ["\u{65e5}\u{672c}\u{8a9e}"];

fn field(m: &TextMetrics, name: &str) -> f64 {
    match name {
        "width" => m.width,
        "actualBoundingBoxLeft" => m.actual_bounding_box_left,
        "actualBoundingBoxRight" => m.actual_bounding_box_right,
        "actualBoundingBoxAscent" => m.actual_bounding_box_ascent,
        "actualBoundingBoxDescent" => m.actual_bounding_box_descent,
        "fontBoundingBoxAscent" => m.font_bounding_box_ascent,
        "fontBoundingBoxDescent" => m.font_bounding_box_descent,
        "hangingBaseline" => m.hanging_baseline,
        "alphabeticBaseline" => m.alphabetic_baseline,
        _ => m.ideographic_baseline,
    }
}

#[derive(Debug, Default)]
struct Error {
    cases: usize,
    inexact: usize,
    max: f64,
    flips: usize,
    worst: String,
}

type Errors = BTreeMap<String, Error>;

/// Every measured field of the macOS width vectors (9 fonts, 58 strings, 3 aligns, 4
/// baselines) against `measure_text`, keyed by `font / field`, fallback strings apart.
fn measure_errors() -> (Errors, Errors) {
    let records = vectors::load("canvas/measure-macos").unwrap();
    let mut backend = platform_backend().unwrap();
    let (mut main, mut fallback) = (Errors::new(), Errors::new());
    for r in &records {
        let css = r.string("font").unwrap();
        let text = r.string("text").unwrap();
        let align = r.string("align").unwrap();
        let baseline = r.string("baseline").unwrap();
        let font = FontSpec::parse(&css).unwrap();
        let ours = measure_text(backend.as_mut(), &font, &align, &baseline, &text);
        let expected = r.value("metrics").unwrap();
        let errors = if FALLBACK.contains(&text.as_str()) {
            &mut fallback
        } else {
            &mut main
        };
        for name in FIELDS {
            let want = expected.get(name).unwrap().as_f64().unwrap();
            let got = field(&ours, name);
            let e: &mut Error = errors.entry(format!("{css} / {name}")).or_default();
            e.cases += 1;
            if got.to_bits() != want.to_bits() {
                e.inexact += 1;
                let diff = (got - want).abs();
                if diff >= e.max {
                    e.max = diff;
                    e.worst = format!("{text:?} {align} {baseline}: {got} vs {want}");
                }
            }
            if name == "width" {
                e.flips += usize::from((got / 10.0).ceil() != (want / 10.0).ceil());
            }
        }
    }
    (main, fallback)
}

fn report(errors: &Errors) -> String {
    errors
        .iter()
        .filter(|(_, e)| e.inexact > 0)
        .map(|(k, e)| {
            format!(
                "{k}: {}/{} inexact, max {:.6}, {} flips, worst {}\n",
                e.inexact, e.cases, e.max, e.flips, e.worst
            )
        })
        .collect()
}

#[test]
fn text_metrics_are_bit_exact_against_chromium() {
    let (main, fallback) = measure_errors();
    let report = report(&main);
    assert_eq!(main.len(), 9 * FIELDS.len(), "{report}");
    for (key, e) in &main {
        assert_eq!(e.cases, 57 * 12, "{key}");
        assert_eq!(e.inexact, 0, "{key}\n{report}");
    }
    let s2_fonts = ["12px ", "16px ", "200 12px ", "200 14px "];
    for (key, e) in &fallback {
        if s2_fonts.iter().any(|f| key.starts_with(f)) && key.ends_with("/ width") {
            assert_eq!(e.flips, 0, "{key}");
        }
    }
}

#[test]
fn zero_size_font_measures_and_draws_nothing() {
    let mut backend = platform_backend().unwrap();
    for css in ["0px sans-serif", "0px system-ui", "200 0px system-ui"] {
        let font = FontSpec::parse(css).unwrap();
        let m = measure_text(backend.as_mut(), &font, "center", "top", "Robot");
        assert_eq!(m, TextMetrics::default(), "{css}");
        let identity = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        let placed = place_text(
            backend.as_mut(),
            &font,
            "left",
            "alphabetic",
            "Robot",
            10.0,
            10.0,
            None,
            identity,
        );
        assert!(placed.is_empty(), "{css}");
    }
}
