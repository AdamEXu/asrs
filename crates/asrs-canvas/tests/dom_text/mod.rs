#![allow(clippy::disallowed_methods)]

use std::path::Path;

use asrs_canvas::text::dom::{self, LcdExtent, LcdInk};
use asrs_canvas::text::lut::{self, LcdGammaLut, MaskGammaLut};
use asrs_canvas::text::{FontSpec, TextBackend};
use asrs_parity::json::{self, Json};
use asrs_parity::pixel::{self, Rgb};

const SYSTEM: &str = "system-ui";

pub const FONTS: [&str; 7] = [
    "12px system-ui",
    "200 12px system-ui",
    "600 12px system-ui",
    "bold 12px system-ui",
    "900 12px system-ui",
    "15px Courier, monospace",
    "bold 15px Courier, monospace",
];

const DOM_SAMPLES: [&str; 4] = [
    "Robot/Drive/LeftVelocity",
    "Teleoperated 0123456789 +4.37s",
    "AVWAY Tij fi Wg",
    "NT:/SmartDashboard/Auto Chooser",
];

/// A row of `oracle/src/canvas/dom.ts` as its font, text and, where it has one, colour.
fn dom_row(id: &str, dark: bool) -> (String, String, Option<[u8; 3]>) {
    let (style, index) = id.split_once('/').unwrap();
    let index: usize = index.parse().unwrap();
    if style == "color" {
        let last = if dark { "#ffffff" } else { "#000000" };
        let color = ["#2b66a2", "#af2437", "#e5b31b", "#3fa34d", last][index];
        let channel = |i: usize| u8::from_str_radix(&color[i..i + 2], 16).unwrap();
        let rgb = [channel(1), channel(3), channel(5)];
        return (
            format!("13px {SYSTEM}"),
            format!("Color {color}"),
            Some(rgb),
        );
    }
    let (size, rest) = style.split_once('-').unwrap_or((style, ""));
    let css = match rest {
        "" => format!("{size} {SYSTEM}"),
        "mono" => format!("{size} Courier, monospace"),
        "mono-bold" => format!("bold {size} Courier, monospace"),
        weight => format!("{weight} {size} {SYSTEM}"),
    };
    (css, DOM_SAMPLES[index].to_owned(), None)
}

/// The measured LCD table is Skia's: each channel's rows are those of the A8 table, cut to the 5
/// bits LCD masks blend with.
pub fn check_lcd_lut(lcd: &LcdGammaLut, gray: &MaskGammaLut) {
    for channel in 0..3 {
        for bucket in 0..lut::ROWS {
            let cut = |row: &[u8; 256]| row.map(|v| v >> 3);
            assert_eq!(
                cut(lcd.row(channel, bucket)),
                cut(gray.row(bucket)),
                "channel {channel}, row {bucket}"
            );
        }
    }
}

struct Ink<'a> {
    text: &'a str,
    spec: &'a FontSpec,
    lcd: bool,
    color: [u8; 3],
    alpha: u8,
}

/// Draws a run onto the flat colour `image` has at its origin, as `asrs_ui::Text` does.
fn draw(
    backend: &mut dyn TextBackend,
    (lut, lcd_lut): (&MaskGammaLut, &LcdGammaLut),
    ink: &Ink<'_>,
    (x, baseline): (f64, i32),
    scale: f32,
    image: &mut Rgb,
) {
    let glyphs = backend.shape_run(ink.spec, ink.text, scale).glyphs;
    let origin = dom::lu_round(x as f32);
    let (x, fraction) = (origin.div_euclid(64), origin.rem_euclid(64));
    let (width, height) = (image.width as i32, image.height as i32);
    let inside = |x: i32, y: i32| (0..width).contains(&x) && (0..height).contains(&y);
    let at = move |x: i32, y: i32| (y * width + x) as usize;
    let extent = dom::lcd_extent(backend, &glyphs, fraction, scale);
    assert_eq!(
        matches!(extent, LcdExtent::Rect(_)),
        ink.lcd,
        "{}: {extent:?}",
        ink.text
    );
    if ink.lcd {
        let background = image.data[at(x, baseline)];
        let run = dom::rasterize_lcd(
            backend,
            (lut, lcd_lut),
            &glyphs,
            fraction,
            scale,
            LcdInk {
                color: ink.color,
                alpha: ink.alpha,
                background,
            },
        )
        .expect("a run");
        for (i, p) in run.rgba.as_chunks::<4>().0.iter().enumerate() {
            let (col, row) = (i as u32 % run.width, i as u32 / run.width);
            let (px, py) = (x + run.left + col as i32, baseline + run.top + row as i32);
            if p[3] != 0 && inside(px, py) {
                image.data[at(px, py)] = [p[0], p[1], p[2]];
            }
        }
        return;
    }
    let run = dom::rasterize(backend, lut, &glyphs, fraction, scale, ink.color).expect("a run");
    for (i, a) in run.alpha.iter().enumerate() {
        let (col, row) = (i as u32 % run.width, i as u32 / run.width);
        let (px, py) = (x + run.left + col as i32, baseline + run.top + row as i32);
        if !inside(px, py) {
            continue;
        }
        let i = at(px, py);
        let a = f64::from(*a) / 255.0 * f64::from(ink.alpha) / 255.0;
        let under = image.data[i];
        image.data[i] = [0, 1, 2].map(|c| {
            let (d, v) = (f64::from(under[c]), f64::from(ink.color[c]));
            (d + (v - d) * a).round() as u8
        });
    }
}

/// Redraws the `dom-text` scenes of `dir` with the port's masks, in the antialiasing mode the
/// scene records per text box, and holds them to the reference. A box the reference draws in LCD
/// is one the backend has LCD masks for, and the other way round.
pub fn check_redraw(backend: &mut dyn TextBackend, dir: &str, max_channel: u8) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../parity/vectors/canvas");
    let lut = MaskGammaLut::embedded().unwrap();
    let lcd_lut = LcdGammaLut::embedded().unwrap();
    for (theme, scale) in [("light", 1u32), ("light", 2), ("dark", 1), ("dark", 2)] {
        let name = format!("dom-text-{theme}@{scale}x");
        let dark = theme == "dark";
        let background = if dark { [0x22; 3] } else { [0xff; 3] };
        let text_color = if dark { [0xee; 3] } else { [0x22; 3] };
        let read = |ext: &str| root.join(format!("{dir}/{name}.{ext}"));
        let png = pixel::read_png(&read("png")).unwrap();
        let reference = pixel::composite(&png, background);
        let scene = json::parse(&std::fs::read_to_string(read("json")).unwrap()).unwrap();
        let rows = scene.get("rows").and_then(Json::as_array).unwrap();
        assert_eq!(rows.len(), 65);
        backend.set_device_scale(scale as f32);
        let mut ours = Rgb {
            width: reference.width,
            height: reference.height,
            data: vec![background; reference.data.len()],
        };
        let mut in_box = vec![false; reference.data.len()];
        let s = f64::from(scale);
        for row in rows {
            let string = |key: &str| row.get(key).unwrap().as_str().unwrap().to_string_lossy();
            let (id, mode) = (string("id"), string("mode"));
            assert!(matches!(&*mode, "lcd" | "grayscale"), "{id}: {mode}");
            let (css, text, color) = dom_row(&id, dark);
            let color = color.unwrap_or(text_color);
            let rect: Vec<f64> = row
                .get("rect")
                .and_then(Json::as_array)
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            let spec = FontSpec::parse(&css).unwrap();
            let device = FontSpec {
                size: spec.size * scale as f32,
                ..spec
            };
            let metrics = backend.metrics(&device);
            let content = f64::from(metrics.ascent + metrics.descent);
            let leading = ((rect[3] * s - content) / 2.0).floor();
            let baseline = (rect[1] * s + leading + f64::from(metrics.ascent)).round() as i32;
            let (x0, x1) = (
                (rect[0] * s).floor() as i32,
                ((rect[0] + rect[2]) * s).ceil() as i32,
            );
            let (y0, y1) = (
                (rect[1] * s).floor() as i32,
                ((rect[1] + rect[3]) * s).ceil() as i32,
            );
            for y in y0..y1 {
                for x in x0..x1 {
                    in_box[(y * ours.width as i32 + x) as usize] = true;
                }
            }
            let at = (rect[0] * s, baseline);
            let ink = Ink {
                text: &text,
                spec: &spec,
                lcd: mode == "lcd",
                color,
                alpha: 255,
            };
            draw(backend, (&lut, &lcd_lut), &ink, at, scale as f32, &mut ours);
        }
        let compared = pixel::compare(&reference, &ours, background).unwrap();
        let mut boxed = 0;
        let mut differing = 0;
        let (mut max, mut at) = (0, 0);
        for i in (0..ours.data.len()).filter(|&i| in_box[i]) {
            boxed += 1;
            differing += u32::from(compared.mask[i]);
            for c in 0..3 {
                let off = ours.data[i][c].abs_diff(reference.data[i][c]);
                if off > max {
                    (max, at) = (off, i);
                }
            }
        }
        let width = ours.width as usize;
        let at = (at % width, at / width);
        let percent = 100.0 * f64::from(differing) / f64::from(boxed);
        eprintln!(
            "{dir}/{name}: {differing} of {boxed} ({percent:.4} %), max channel {max} at {at:?}"
        );
        let gate = if scale == 1 { 0.2 } else { 0.003 };
        assert!(percent <= gate, "{name}: {percent:.4} %");
        assert!(max <= max_channel, "{name}: max channel {max}");
    }
}

/// Redraws the boxes of the `dom-lcd` scenes of `dir` that differ in size and in what is under or
/// in the text, each in the mode it has there.
pub fn check_eligibility(backend: &mut dyn TextBackend, dir: &str, max_channel: u8) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../parity/vectors/canvas");
    let luts = (
        &MaskGammaLut::embedded().unwrap(),
        &LcdGammaLut::embedded().unwrap(),
    );
    for (theme, scale) in [("light", 1u32), ("light", 2), ("dark", 1), ("dark", 2)] {
        let name = format!("dom-lcd-{theme}@{scale}x");
        let dark = theme == "dark";
        let page = if dark { [0x22; 3] } else { [0xff; 3] };
        let color = if dark { [0xee; 3] } else { [0x22; 3] };
        let edge = if dark { 0xff } else { 0x00 };
        let read = |ext: &str| root.join(format!("{dir}/{name}.{ext}"));
        let png = pixel::read_png(&read("png")).unwrap();
        let reference = pixel::composite(&png, page);
        let scene = json::parse(&std::fs::read_to_string(read("json")).unwrap()).unwrap();
        backend.set_device_scale(scale as f32);
        let (mut checked, mut max) = (0, 0);
        for row in scene.get("rows").and_then(Json::as_array).unwrap() {
            let string = |key: &str| row.get(key).unwrap().as_str().unwrap().to_string_lossy();
            let (id, mode) = (string("id"), string("mode"));
            let size = id.strip_prefix("size-");
            let (color, alpha, behind) = match &*id {
                "opaque-box" => (color, 255, [if dark { 0x44 } else { 0xcc }; 3]),
                "translucent-box" => {
                    let tint = page[0] as f64 + (f64::from(edge) - f64::from(page[0])) * 0.05;
                    (color, 255, [tint.round() as u8; 3])
                }
                "translucent-text" => ([edge; 3], 64, page),
                _ if size.is_some() => (color, 255, page),
                _ => continue,
            };
            let rect: Vec<f64> = row
                .get("rect")
                .and_then(Json::as_array)
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect();
            let text = if size.is_some() {
                "Wolf 47"
            } else {
                "Auto Chooser 4.37"
            };
            let spec = FontSpec::parse(&format!("{}px {SYSTEM}", size.unwrap_or("14"))).unwrap();
            let s = f64::from(scale);
            let device = FontSpec {
                size: spec.size * scale as f32,
                ..spec
            };
            let ascent = f64::from(backend.metrics(&device).ascent);
            let baseline = (rect[1] * s + ascent).round() as i32;
            let mut ours = Rgb {
                data: vec![behind; reference.data.len()],
                ..reference
            };
            let ink = Ink {
                text,
                spec: &spec,
                lcd: mode == "lcd",
                color,
                alpha,
            };
            let at = (rect[0] * s, baseline);
            draw(backend, luts, &ink, at, scale as f32, &mut ours);
            let columns = (rect[0] * s).floor() as usize..((rect[0] + rect[2]) * s) as usize;
            for y in (rect[1] * s).ceil() as usize..((rect[1] + rect[3]) * s) as usize {
                for x in columns.clone() {
                    let i = y * reference.width as usize + x;
                    for c in 0..3 {
                        let off = ours.data[i][c].abs_diff(reference.data[i][c]);
                        assert!(
                            off <= max_channel,
                            "{dir}/{name} {id}: {:?} vs {:?} at {x},{y}",
                            ours.data[i],
                            reference.data[i]
                        );
                        max = max.max(off);
                    }
                }
            }
            checked += 1;
        }
        assert_eq!(checked, 14);
        eprintln!("{dir}/{name}: {checked} boxes, max channel {max}");
    }
}
