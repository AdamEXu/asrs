#![cfg(target_os = "linux")]

mod dom_text;

use std::collections::BTreeMap;

use asrs_canvas::text::linux::Fontations;
use asrs_canvas::text::lut::{self, LcdGammaLut, MaskGammaLut};
use asrs_canvas::text::{FontSpec, TextBackend, TextMetrics, measure_text, platform_backend};
use asrs_js::math::pow;
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;
use sha2::{Digest, Sha256};

/// `SK_GAMMA_CONTRAST` and `SK_GAMMA_EXPONENT` of Chromium's Linux Skia, the latter as
/// `SkScalerContextRec` holds it, in 2.6 fixed point.
const CONTRAST: f64 = 0.2;
const GAMMA: f64 = 76.0 / 64.0;

/// `SkTMaskGamma_build_correcting_lut`.
fn skia_row(bucket: usize) -> Vec<u8> {
    let base = bucket << 5;
    let src = ((base | base >> 3 | base >> 6) & 255) as f64 / 255.0;
    let dst = 1.0 - src;
    let (lin_src, lin_dst) = (pow(src, GAMMA), pow(dst, GAMMA));
    (0..256)
        .map(|i| {
            let raw = f64::from(i) / 255.0;
            let a = raw + (1.0 - raw) * (CONTRAST * lin_dst) * raw;
            let out = pow(lin_src * a + (1.0 - a) * lin_dst, 1.0 / GAMMA);
            (255.0 * (out - dst) / (src - dst) + 0.5).floor() as u8
        })
        .collect()
}

#[test]
fn embedded_lut_is_skias_table() {
    let lut = MaskGammaLut::embedded().expect("lut-linux.bin is committed");
    for k in 0..lut::ROWS {
        assert_eq!(lut.row(k).as_slice(), skia_row(k), "row {k}");
    }
}

#[test]
fn embedded_lcd_lut_is_skias_table() {
    let lcd = LcdGammaLut::embedded().expect("lut-linux-lcd.bin is committed");
    dom_text::check_lcd_lut(&lcd, &MaskGammaLut::embedded().unwrap());
}

/// DOM text under the font profile in use: `gray.conf`, or the stock rules, which are LCD.
#[test]
fn masks_reproduce_dom_text() {
    if !fonts_are_pinned(&dom_text::FONTS) {
        return;
    }
    let gray = std::env::var("FONTCONFIG_FILE").is_ok_and(|rules| rules.ends_with("/gray.conf"));
    let dir = if gray {
        "scenes-linux-gray"
    } else {
        "scenes-linux-stock"
    };
    dom_text::check_redraw(&mut Fontations::new().unwrap(), dir, 3);
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

/// The recorded hash of each font file of the render host, by file name.
fn recorded_fonts() -> BTreeMap<String, String> {
    let text = include_str!("../../../parity/vectors/canvas/linux.json");
    let manifest = json::parse(text).unwrap();
    let Some(Json::Object(fonts)) = manifest.get("host").and_then(|h| h.get("fonts")) else {
        panic!("linux.json records no host fonts");
    };
    fonts
        .iter()
        .map(|(path, hash)| {
            let path = path.to_string_lossy();
            let name = path.rsplit('/').next().unwrap().to_owned();
            (name, hash.as_str().unwrap().to_string_lossy())
        })
        .collect()
}

/// Whether fontconfig resolves `css` to the files the vectors were recorded with. CI installs
/// them, so there anything else fails; elsewhere the vectors do not apply.
fn fonts_are_pinned(fonts: &[&str]) -> bool {
    let recorded = recorded_fonts();
    let problem = Fontations::new().map_or(Some("no fonts".to_owned()), |mut backend| {
        fonts.iter().find_map(|css| {
            let spec = FontSpec::parse(css).unwrap();
            let (path, _) = backend.font_file(&spec).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let hash: String = Sha256::digest(std::fs::read(&path).unwrap())
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect();
            (recorded.get(&name) != Some(&hash))
                .then(|| format!("{css} resolves to {}", path.display()))
        })
    });
    if let Some(problem) = &problem {
        assert!(std::env::var_os("ASRS_CI").is_none(), "{problem}");
        eprintln!("skipped: {problem}, not a font of the render host (FONTCONFIG_FILE)");
    }
    problem.is_none()
}

#[test]
fn fonts_resolve_as_chromium_resolves_them() {
    let expected = [
        ("12px system-ui", "DejaVuSans.ttf"),
        ("200 12px system-ui", "DejaVuSans.ttf"),
        ("italic 12px system-ui", "DejaVuSans.ttf"),
        ("bold 12px system-ui", "DejaVuSans-Bold.ttf"),
        ("600 12px system-ui", "DejaVuSans-Bold.ttf"),
        ("10px sans-serif", "LiberationSans-Regular.ttf"),
        ("italic 10px sans-serif", "LiberationSans-Italic.ttf"),
        ("bold 10px sans-serif", "LiberationSans-Bold.ttf"),
        ("10px serif", "LiberationSerif-Regular.ttf"),
        ("10px monospace", "DejaVuSansMono.ttf"),
        ("10px Courier, monospace", "LiberationMono-Regular.ttf"),
    ];
    let fonts: Vec<&str> = expected.iter().map(|(css, _)| *css).collect();
    if !fonts_are_pinned(&fonts) {
        return;
    }
    let mut backend = Fontations::new().unwrap();
    for (css, name) in expected {
        let (path, _) = backend.font_file(&FontSpec::parse(css).unwrap()).unwrap();
        assert_eq!(path.file_name().unwrap(), name, "{css}");
    }
}

/// Every measured field of the Linux vectors (11 fonts, 58 strings, 3 aligns, 4 baselines), emoji
/// and missing glyphs included.
#[test]
fn text_metrics_are_bit_exact_against_chromium() {
    let records = vectors::load("canvas/measure-linux").unwrap();
    assert_eq!(records.len(), 11 * 58 * 12);
    let mut fonts: Vec<String> = records.iter().map(|r| r.string("font").unwrap()).collect();
    fonts.dedup();
    let names: Vec<&str> = fonts.iter().map(String::as_str).collect();
    if !fonts_are_pinned(&names) {
        return;
    }
    let mut backend = platform_backend().unwrap();
    let mut wrong = Vec::new();
    for r in &records {
        let css = r.string("font").unwrap();
        let (text, align) = (r.string("text").unwrap(), r.string("align").unwrap());
        let baseline = r.string("baseline").unwrap();
        let font = FontSpec::parse(&css).unwrap();
        let ours = measure_text(backend.as_mut(), &font, &align, &baseline, &text);
        let expected = r.value("metrics").unwrap();
        for name in FIELDS {
            let want = expected.get(name).unwrap().as_f64().unwrap();
            let got = field(&ours, name);
            if got.to_bits() != want.to_bits() {
                wrong.push(format!(
                    "{css} {text:?} {align} {baseline} {name}: {got} vs {want}"
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{}: {:#?}",
        wrong.len(),
        &wrong[..wrong.len().min(20)]
    );
}

#[test]
fn zero_size_font_measures_nothing() {
    let Some(mut backend) = platform_backend() else {
        assert!(std::env::var_os("ASRS_CI").is_none(), "CI installs fonts");
        return;
    };
    for css in ["0px sans-serif", "0px system-ui", "200 0px system-ui"] {
        let font = FontSpec::parse(css).unwrap();
        let m = measure_text(backend.as_mut(), &font, "center", "top", "Robot");
        assert_eq!(m, TextMetrics::default(), "{css}");
    }
}

/// Chromium draws text unhinted on a display scaled above 1, which moves ink off the pixel grid
/// the light autohinter snaps it to, and lays the descent out a pixel deeper where it rounds down,
/// but for the fonts a page made before it knew.
#[test]
fn a_scaled_display_turns_hinting_off() {
    if !fonts_are_pinned(&["10px sans-serif"]) {
        return;
    }
    let mut backend = platform_backend().unwrap();
    let font = FontSpec::parse("10px sans-serif").unwrap();
    let hinted = backend.metrics(&font);
    let glyph = backend.shape(&font, "x", 1.0).glyphs[0].glyph;
    let mask = backend.raw_mask(glyph, 1.0, 1.0, 0).unwrap();
    assert!(!backend.set_device_scale(1.0));
    assert!(backend.set_device_scale(1.5));
    assert!(!backend.set_device_scale(2.0));
    let scaled = backend.metrics(&font);
    assert_eq!((hinted.ascent, hinted.descent), (9.0, 2.0));
    assert_eq!((scaled.ascent, scaled.descent), (8.0, 3.0));
    assert_ne!(*backend.raw_mask(glyph, 1.0, 1.0, 0).unwrap(), *mask);
    let page = FontSpec {
        hinted: true,
        ..font
    };
    let glyph_of_page = backend.shape(&page, "x", 1.0).glyphs[0].glyph;
    assert_eq!(
        *backend.raw_mask(glyph_of_page, 1.0, 1.0, 0).unwrap(),
        *mask
    );
    let kept = backend.metrics(&page);
    assert_eq!((kept.ascent, kept.descent), (9.0, 2.0));
    let label = FontSpec::parse("12px system-ui").unwrap();
    let descent = |backend: &mut dyn TextBackend| {
        measure_text(backend, &label, "center", "top", "+3.3s").actual_bounding_box_descent
    };
    assert_eq!(descent(backend.as_mut()), 10.125);
    assert!(backend.set_device_scale(1.0));
    assert_eq!(descent(backend.as_mut()), 9.125);
    assert_eq!(*backend.raw_mask(glyph, 1.0, 1.0, 0).unwrap(), *mask);
}

/// `measureText("Robot 0").width` of the joysticks and swerve painters' sizes in Electron 39.2.7,
/// each font's first use in its page: Blink's font cache keys sizes to 1/100 px.
#[test]
fn fractional_sizes_are_floored_as_blink_floors_them() {
    if !fonts_are_pinned(&["10px sans-serif"]) {
        return;
    }
    let mut backend = platform_backend().unwrap();
    for (size, width) in [
        ("39.10909090909091", 136.92356872558594),
        ("24.443181818181817", 85.59091186523438),
        ("19.554545454545455", 68.46183776855469),
        ("26.072727272727278", 91.28237915039062),
        ("46.1337890625", 161.55010986328125),
        ("22.21276595744681", 77.76519775390625),
        ("13.882978723404253", 48.59637451171875),
        ("14.829545454545453", 51.879913330078125),
        ("10.1953125", 35.681121826171875),
        ("23.727272727272727", 83.07353210449219),
    ] {
        let font = FontSpec::parse(&format!("{size}px sans-serif")).unwrap();
        let m = measure_text(backend.as_mut(), &font, "left", "alphabetic", "Robot 0");
        assert_eq!(m.width, width, "{size}");
    }
}
