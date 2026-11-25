#![cfg(windows)]
#![allow(clippy::disallowed_methods)]

mod dom_text;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use asrs_canvas::text::caret;
use asrs_canvas::text::lut::{self, LcdGammaLut, MaskGammaLut};
use asrs_canvas::text::win::DirectWrite;
use asrs_canvas::text::win::settings::{Edging, TextSettings};
use asrs_canvas::text::{FontSpec, TextBackend, TextMetrics, measure_text};
use asrs_parity::json::{self, Json};
use asrs_parity::vectors;
use sha2::{Digest, Sha256};

#[test]
fn embedded_lut_is_skias_table() {
    let lut = MaskGammaLut::embedded().expect("lut-windows.bin is committed");
    let skia = MaskGammaLut::skia(MaskGammaLut::WINDOWS_CONTRAST, MaskGammaLut::WINDOWS_GAMMA);
    for k in 0..lut::ROWS {
        assert_eq!(lut.row(k), skia.row(k), "row {k}");
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

fn manifest() -> Json {
    json::parse(include_str!("../../../parity/vectors/canvas/windows.json")).unwrap()
}

/// The recorded hash of each font file of the render host, by lowercase file name.
fn recorded_fonts() -> BTreeMap<String, String> {
    let manifest = manifest();
    let Some(Json::Object(fonts)) = manifest.get("host").and_then(|h| h.get("fonts")) else {
        panic!("windows.json records no host fonts");
    };
    fonts
        .iter()
        .filter_map(|(path, hash)| {
            let path = path.to_string_lossy().to_lowercase();
            let name = path.rsplit('/').next()?.to_owned();
            Some((name, hash.as_str()?.to_string_lossy()))
        })
        .collect()
}

/// The fonts are Microsoft's and cannot be installed for a test, so a machine whose files differ
/// from the render host's skips, past libtest's output capture.
fn skipped(reason: &str) {
    let test = std::thread::current();
    let test = test.name().expect("libtest names its threads");
    let line = if std::env::var_os("ASRS_CI").is_some() {
        format!("\n::warning title=Windows text vectors not run::{test}: {reason}\n")
    } else {
        format!("\nskipped {test}: {reason}\n")
    };
    let _ = std::io::stdout().write_all(line.as_bytes());
}

fn file_problem(path: &Path, recorded: &BTreeMap<String, String>) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let Some(want) = recorded.get(&name) else {
        return Some(format!("{name} is not a font of the render host"));
    };
    let hash: String = Sha256::digest(std::fs::read(path).unwrap())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    (*want != hash).then(|| format!("{name} is not the render host's file"))
}

const FALLBACKS: &str = include_str!("../../../parity/vectors/canvas/fallback-windows.json");

struct Probe {
    font: String,
    text: String,
    metrics: Vec<f64>,
}

fn probes(key: &str) -> Vec<Probe> {
    let vectors = json::parse(FALLBACKS).unwrap();
    let rows = vectors.get(key).and_then(Json::as_array).unwrap();
    let text = |r: &Json, key: &str| r.get(key).unwrap().as_str().unwrap().to_string_lossy();
    rows.iter()
        .map(|r| Probe {
            font: text(r, "font"),
            text: text(r, "text"),
            metrics: r
                .get("metrics")
                .and_then(Json::as_array)
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect(),
        })
        .collect()
}

/// The backend with the render host's ClearType settings, or `None` where a font of `fonts` or a
/// font they fall back to is not the file the vectors were recorded with.
fn pinned_backend(fonts: &[&str]) -> Option<DirectWrite> {
    pinned_backend_for(fonts, &[])
}

fn pinned_backend_for(fonts: &[&str], characters: &[char]) -> Option<DirectWrite> {
    let mut backend = DirectWrite::with_settings(Some(TextSettings::DEFAULT)).unwrap();
    let mut files = BTreeSet::new();
    for css in fonts {
        let spec = FontSpec::parse(css).unwrap();
        files.insert(backend.font_file(&spec).unwrap().0);
        for c in ['日', 'か', '\u{1f5fa}'].iter().chain(characters) {
            files.extend(backend.fallback_file(&spec, *c, false));
        }
        files.extend(backend.fallback_file(&spec, '\u{1f916}', true));
    }
    let recorded = recorded_fonts();
    let problems: Vec<String> = files
        .iter()
        .filter_map(|f| file_problem(f, &recorded))
        .collect();
    if problems.is_empty() {
        return Some(backend);
    }
    skipped(&problems.join(", "));
    None
}

const SYSTEM: &str = "system-ui";

#[test]
fn fonts_resolve_as_chromium_resolves_them() {
    let expected = [
        ("12px system-ui", "segoeui.ttf"),
        ("200 12px system-ui", "segoeuil.ttf"),
        ("italic 12px system-ui", "segoeuii.ttf"),
        ("bold 12px system-ui", "segoeuib.ttf"),
        ("600 12px system-ui", "seguisb.ttf"),
        ("900 12px system-ui", "seguibl.ttf"),
        ("10px sans-serif", "arial.ttf"),
        ("italic 10px sans-serif", "ariali.ttf"),
        ("bold 10px sans-serif", "arialbd.ttf"),
        ("10px serif", "times.ttf"),
        ("10px monospace", "consola.ttf"),
        ("10px Courier, monospace", "cour.ttf"),
    ];
    let fonts: Vec<&str> = expected.iter().map(|(css, _)| *css).collect();
    let Some(mut backend) = pinned_backend(&fonts) else {
        return;
    };
    for (css, name) in expected {
        let (path, _) = backend.font_file(&FontSpec::parse(css).unwrap()).unwrap();
        let file = path.file_name().unwrap().to_string_lossy().to_lowercase();
        assert_eq!(file, name, "{css}");
    }
}

#[test]
fn characters_fall_back_as_chromium_falls_back() {
    let Some(mut backend) = pinned_backend(&["12px system-ui"]) else {
        return;
    };
    let spec = FontSpec::parse("12px system-ui").unwrap();
    for (c, emoji, name) in [
        ('日', false, "msyh.ttc"),
        ('か', false, "yugothr.ttc"),
        ('\u{1f916}', true, "seguiemj.ttf"),
        ('\u{2699}', false, "seguisym.ttf"),
    ] {
        let path = backend.fallback_file(&spec, c, emoji).unwrap();
        let file = path.file_name().unwrap().to_string_lossy().to_lowercase();
        assert_eq!(file, name, "{c}");
    }
    assert!(backend.fallback_file(&spec, '\u{e000}', false).is_none());
}

/// Every measured field of the Windows vectors (11 fonts, 58 strings, 3 aligns, 4 baselines),
/// emoji and missing glyphs included.
#[test]
fn text_metrics_are_bit_exact_against_chromium() {
    let records = vectors::load("canvas/measure-windows").unwrap();
    assert_eq!(records.len(), 11 * 58 * 12);
    let mut fonts: Vec<String> = records.iter().map(|r| r.string("font").unwrap()).collect();
    fonts.dedup();
    let names: Vec<&str> = fonts.iter().map(String::as_str).collect();
    let Some(mut backend) = pinned_backend(&names) else {
        return;
    };
    let mut wrong = Vec::new();
    for r in &records {
        let css = r.string("font").unwrap();
        let (text, align) = (r.string("text").unwrap(), r.string("align").unwrap());
        let baseline = r.string("baseline").unwrap();
        let font = FontSpec::parse(&css).unwrap();
        let ours = measure_text(&mut backend, &font, &align, &baseline, &text);
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
        &wrong[..wrong.len().min(40)]
    );
}

fn check_probes(backend: &mut DirectWrite, probes: &[Probe]) {
    let mut wrong = Vec::new();
    for p in probes {
        let font = FontSpec::parse(&p.font).unwrap();
        let m = measure_text(backend, &font, "left", "alphabetic", &p.text);
        assert_eq!(p.metrics.len(), 5, "{} {}", p.font, p.text);
        for (name, want) in FIELDS.iter().zip(&p.metrics) {
            let got = field(&m, name);
            if got != *want {
                wrong.push(format!("{} {} {name}: {got} vs {want}", p.font, p.text));
            }
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Sizes of the joysticks and swerve painters: Blink's font cache keys sizes to 1/100 px.
#[test]
fn fractional_sizes_are_floored_as_blink_floors_them() {
    let Some(mut backend) = pinned_backend(&["10px sans-serif"]) else {
        return;
    };
    let probes = probes("sizes");
    assert_eq!(probes.len(), 10);
    check_probes(&mut backend, &probes);
}

/// The em-height baselines at those sizes, the button numbers' of the 2x `joysticks` scenario first.
#[test]
fn baselines_follow_the_floored_size() {
    let Some(mut backend) = pinned_backend(&["10px sans-serif"]) else {
        return;
    };
    let probes = probes("baselines");
    assert_eq!(probes.len(), 12);
    let mut wrong = Vec::new();
    for p in &probes {
        let font = FontSpec::parse(&p.font).unwrap();
        let got = ["top", "middle", "bottom"].map(|baseline| {
            measure_text(&mut backend, &font, "left", baseline, &p.text).alphabetic_baseline
        });
        if got[..] != p.metrics[..] {
            wrong.push(format!("{}: {got:?} vs {:?}", p.font, p.metrics));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

/// Text no primary font covers, in scripts of most of Blink's fallback families, upright, bold
/// and italic.
#[test]
fn fallback_text_measures_as_chromium_measures_it() {
    let probes = probes("fallback");
    assert_eq!(probes.len(), 4 * 13);
    let fonts: Vec<&str> = probes.iter().map(|p| p.font.as_str()).collect();
    let mut characters: Vec<char> = probes.iter().flat_map(|p| p.text.chars()).collect();
    characters.sort_unstable();
    characters.dedup();
    let Some(mut backend) = pinned_backend_for(&fonts, &characters) else {
        return;
    };
    check_probes(&mut backend, &probes);
}

#[test]
fn zero_size_font_measures_nothing() {
    let mut backend = DirectWrite::new().unwrap();
    for css in ["0px sans-serif", "0px system-ui", "200 0px system-ui"] {
        let font = FontSpec::parse(css).unwrap();
        let m = measure_text(&mut backend, &font, "center", "top", "Robot");
        assert_eq!(m, TextMetrics::default(), "{css}");
    }
}

/// Whatever the machine's fonts, the canvas mask is the average of the ClearType channels, and
/// text too big for LCD has no three-channel mask.
#[test]
fn gray_masks_average_the_lcd_channels() {
    let mut backend = DirectWrite::with_settings(Some(TextSettings::DEFAULT)).unwrap();
    let font = FontSpec::parse(&format!("16px {SYSTEM}")).unwrap();
    let shaped = backend.shape(&font, "Robot", 1.0);
    assert_eq!(shaped.glyphs.len(), 5);
    for g in &shaped.glyphs {
        for phase in 0..4 {
            let gray = backend.raw_mask(g.glyph, 1.5, 1.0, phase).unwrap();
            let lcd = backend.raw_lcd_mask(g.glyph, 1.5, 1.0, phase).unwrap();
            assert_eq!((gray.left, gray.top), (lcd.left, lcd.top));
            assert_eq!((gray.width, gray.height), (lcd.width, lcd.height));
            let averages = lcd.data.as_chunks::<3>().0;
            let average = |p: &[u8; 3]| (p.iter().map(|&v| u32::from(v)).sum::<u32>() / 3) as u8;
            assert!(gray.data.iter().copied().eq(averages.iter().map(average)));
            assert!(gray.data.iter().any(|&v| v > 128));
        }
    }
    let glyph = shaped.glyphs[0].glyph;
    assert!(backend.raw_lcd_mask(glyph, 3.0, 1.0, 0).is_some());
    assert!(backend.raw_lcd_mask(glyph, 3.01, 1.0, 0).is_none());
}

#[test]
fn text_settings_pick_the_masks() {
    let with = |settings| DirectWrite::with_settings(Some(settings)).unwrap();
    let font = FontSpec::parse(&format!("16px {SYSTEM}")).unwrap();
    let mask = |settings: TextSettings| {
        let mut backend = with(settings);
        let glyph = backend.shape(&font, "R", 1.0).glyphs[0].glyph;
        let lcd = backend.raw_lcd_mask(glyph, 1.0, 1.0, 0);
        (backend.raw_mask(glyph, 1.0, 1.0, 0).unwrap(), lcd)
    };
    let (gray, rgb) = mask(TextSettings::DEFAULT);
    let (same, bgr) = mask(TextSettings {
        bgr: true,
        ..TextSettings::DEFAULT
    });
    assert_eq!(gray, same);
    let (rgb, bgr) = (rgb.unwrap(), bgr.unwrap());
    assert_ne!(rgb, bgr);
    let swapped = rgb
        .data
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| [p[2], p[1], p[0]]);
    assert!(bgr.data.as_chunks::<3>().0.iter().copied().eq(swapped));
    let (aliased, none) = mask(TextSettings {
        edging: Edging::Alias,
        ..TextSettings::DEFAULT
    });
    assert!(none.is_none() && aliased.data.iter().all(|&v| v == 0 || v == 255));
    let (smooth, none) = mask(TextSettings {
        edging: Edging::AntiAlias,
        ..TextSettings::DEFAULT
    });
    assert!(none.is_none() && smooth != gray);
}

/// Segoe UI Symbol has no bold face, so its bold is drawn from outlines.
#[test]
fn synthetic_bold_follows_the_text_settings() {
    let font = FontSpec::parse(&format!("bold 24px {SYSTEM}")).unwrap();
    let mask = |edging| {
        let settings = TextSettings {
            edging,
            ..TextSettings::DEFAULT
        };
        let mut backend = DirectWrite::with_settings(Some(settings)).unwrap();
        let shaped = backend.shape(&font, "\u{2699}", 1.0);
        backend
            .raw_mask(shaped.glyphs[0].glyph, 1.0, 1.0, 0)
            .unwrap()
    };
    let partial = |v: &u8| !matches!(v, 0 | 255);
    let aliased = mask(Edging::Alias);
    assert!(!aliased.data.iter().any(partial));
    assert!(aliased.data.contains(&255));
    assert!(mask(Edging::AntiAlias).data.iter().any(partial));
}

/// The caret positions text selection uses end at the run's width, and its space is the one
/// `measureText` reports, whatever the machine's fonts.
#[test]
fn carets_follow_shaped_text() {
    let mut backend = DirectWrite::with_settings(Some(TextSettings::DEFAULT)).unwrap();
    for css in [
        "12px system-ui",
        "bold 13px monospace",
        "39.109px sans-serif",
    ] {
        let font = FontSpec::parse(css).unwrap();
        let space = measure_text(&mut backend, &font, "left", "alphabetic", " ").width;
        assert_eq!(
            f64::from(caret::space_width(&mut backend, &font, 1.0)),
            space
        );
        for text in ["/Drive/Left 1.5", "naïve e\u{301}x", "a 日本か \u{1f916} b"] {
            for scale in [1.0f32, 1.5, 2.0] {
                let carets = caret::carets(&mut backend, &font, text, scale);
                let width = backend.shape(&font, text, scale).width * scale;
                assert_eq!(carets.first(), Some(&(0, 0.0)));
                assert_eq!(carets.last(), Some(&(text.len(), width)), "{css} {text}");
                assert!(carets.windows(2).all(|w| w[0].1 < w[1].1), "{css} {text}");
            }
        }
    }
}

/// DOM text of the render host, redrawn from three-channel masks at the DOM's own box positions.
#[test]
fn lcd_masks_reproduce_dom_text() {
    let Some(mut backend) = pinned_backend(&dom_text::FONTS) else {
        return;
    };
    dom_text::check_redraw(&mut backend, "scenes-windows", Some(2));
    dom_text::check_eligibility(&mut backend, "scenes-windows", 2);
}

#[test]
fn embedded_lcd_lut_is_skias_table() {
    let lcd = LcdGammaLut::embedded().expect("lut-windows-lcd.bin is committed");
    dom_text::check_lcd_lut(&lcd, &MaskGammaLut::embedded().unwrap());
}
