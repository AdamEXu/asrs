#![cfg(target_os = "macos")]

use std::collections::BTreeMap;

use asrs_canvas::text::lut::{self, MaskGammaLut};
use asrs_canvas::text::{FontSpec, platform_backend};
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

const S2_FONTS: [&str; 4] = ["12px ", "16px ", "200 12px ", "200 14px "];
const NOT_IN_S2_CORPUS: [&str; 12] = [
    "",
    "Drag to zoom",
    "Click to add a field",
    "\u{b5}s 45\u{b0} \u{b1}",
    "caf\u{e9} na\u{ef}ve",
    "\u{65e5}\u{672c}\u{8a9e}",
    "\u{1f916} robot",
    "A\u{301}",
    "\t tab",
    "fi fl ffi",
    "1.23e+21",
    "-0",
];

#[test]
fn s2_corpus_widths_match_chromium() {
    let records = vectors::load("canvas/measure-macos").unwrap();
    let mut backend = platform_backend().unwrap();
    let mut worst: BTreeMap<String, (f64, usize, usize)> = BTreeMap::new();
    for r in &records {
        let font = r.string("font").unwrap();
        let text = r.string("text").unwrap();
        if !S2_FONTS.iter().any(|f| font.starts_with(f))
            || NOT_IN_S2_CORPUS.contains(&text.as_str())
            || r.string("align").as_deref() != Some("left")
            || r.string("baseline").as_deref() != Some("alphabetic")
        {
            continue;
        }
        let expected = r
            .value("metrics")
            .unwrap()
            .get("width")
            .unwrap()
            .as_f64()
            .unwrap();
        let ours = backend.shape(&FontSpec::parse(&font), &text).width;
        let entry = worst.entry(font).or_default();
        let error = (ours - expected).abs();
        if error > entry.0 {
            entry.0 = error;
        }
        entry.1 += usize::from((ours / 10.0).ceil() != (expected / 10.0).ceil());
        entry.2 += 1;
    }
    let report = format!("{worst:#?}");
    for (font, (max, flips, n)) in &worst {
        assert_eq!(*n, 41, "{font}");
        let limit = if font.starts_with("200 ") { 0.03 } else { 0.0 };
        assert!(*max <= limit, "{font}: width off by {max}\n{report}");
        assert_eq!(*flips, 0, "{font}: ceil(w/10) flips\n{report}");
    }
    assert_eq!(worst.len(), 4, "{report}");
}
