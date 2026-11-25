#![allow(clippy::disallowed_methods)]

use std::path::Path;

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

fn blend_gray(background: [u8; 3], color: [u8; 3], alpha: u8) -> [u8; 3] {
    let a = f64::from(alpha) / 255.0;
    [0, 1, 2].map(|c| {
        let (d, v) = (f64::from(background[c]), f64::from(color[c]));
        (d + (v - d) * a).round() as u8
    })
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
            let origin = (rect[0] * s) as f32;
            for g in backend.shape_run(&spec, &text, scale as f32).glyphs {
                let biased = origin + g.x * scale as f32 + 0.125;
                let ix = biased.floor();
                let phase = ((biased - ix) * 4.0) as u8;
                let lcd = backend.raw_lcd_mask(g.glyph, s, 1.0, phase);
                let gray = backend.raw_mask(g.glyph, s, 1.0, phase);
                let Some(gray) = gray.filter(|m| m.width > 0 && m.height > 0) else {
                    continue;
                };
                assert_eq!(lcd.is_some(), mode == "lcd", "{name} {id}: {mode}");
                let mut paint =
                    |mx: usize,
                     my: usize,
                     left: i32,
                     top: i32,
                     draw: &dyn Fn([u8; 3]) -> [u8; 3]| {
                        let (x, y) = (ix as i32 + left + mx as i32, baseline + top + my as i32);
                        if x < 0 || y < 0 || x >= ours.width as i32 || y >= ours.height as i32 {
                            return;
                        }
                        let dst = &mut ours.data[(y * reference.width as i32 + x) as usize];
                        *dst = draw(*dst);
                    };
                if let Some(mask) = &lcd {
                    for (i, raw) in mask.data.as_chunks::<3>().0.iter().enumerate() {
                        let width = mask.width as usize;
                        let out = lcd_lut.apply(color, *raw);
                        paint(i % width, i / width, mask.left, mask.top, &|dst| {
                            lut::blend_lcd(dst, color, out)
                        });
                    }
                } else {
                    for (i, raw) in gray.data.iter().enumerate() {
                        let width = gray.width as usize;
                        let out = lut.apply(color, *raw);
                        paint(i % width, i / width, gray.left, gray.top, &|dst| {
                            blend_gray(dst, color, out)
                        });
                    }
                }
            }
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
