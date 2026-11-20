use std::fs;
use std::path::Path;

use asrs_canvas::text::coverage::{TextOp, render_coverage};
use asrs_canvas::text::lut::{self, MaskGammaLut};
use asrs_canvas::text::platform_backend;
use asrs_parity::json::{self, Json};
use asrs_parity::pixel;

const USAGE: &str = "usage: asrs-cli canvas <command>

commands:
  lut <reference dir> <out.bin>   fit the mask-gamma table from the oracle's calibration scenes
                                  (scenes/calib-<k>.json and ref/calib-<k>.cpu.png)";

pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, reference, out] if cmd == "lut" => fit_lut(Path::new(reference), Path::new(out)),
        _ => Err(USAGE.to_owned()),
    }
}

struct TextScene {
    width: u32,
    height: u32,
    fill: [u8; 3],
    ops: Vec<TextOp>,
}

fn number(v: &Json) -> Result<f64, String> {
    v.as_f64()
        .ok_or_else(|| format!("expected a number, got {v:?}"))
}

fn string(v: &Json) -> Result<String, String> {
    v.as_str()
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| format!("expected a string, got {v:?}"))
}

fn hex_colour(css: &str) -> Result<[u8; 3], String> {
    let hex = css.strip_prefix('#').filter(|h| h.len() == 6);
    let channel = |i: usize| {
        hex.and_then(|h| u8::from_str_radix(&h[i..i + 2], 16).ok())
            .ok_or_else(|| format!("calibration fillStyle {css:?} is not #rrggbb"))
    };
    Ok([channel(0)?, channel(2)?, channel(4)?])
}

fn text_scene(path: &Path) -> Result<TextScene, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let scene = json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let field = |k: &str| scene.get(k).ok_or_else(|| format!("scene has no {k}"));
    let dpr = number(field("dpr")?)?;
    let width = (number(field("width")?)? * dpr) as u32;
    let height = (number(field("height")?)? * dpr) as u32;
    let mut fill = None;
    let mut ops = Vec::new();
    let cmds = field("cmds")?.as_array().ok_or("cmds is not an array")?;
    for cmd in cmds {
        let cmd = cmd.as_array().ok_or("a command is not an array")?;
        let name = string(&cmd[0])?;
        let arg = |i: usize| {
            cmd.get(i)
                .ok_or_else(|| format!("{name}: missing argument {i}"))
        };
        match name.as_str() {
            "=" => {
                let value = arg(2)?;
                match string(arg(1)?)?.as_str() {
                    "fillStyle" => fill = Some(hex_colour(&string(value)?)?),
                    "font" => ops.push(TextOp::Font(string(value)?)),
                    "textAlign" => ops.push(TextOp::TextAlign(string(value)?)),
                    "textBaseline" => ops.push(TextOp::TextBaseline(string(value)?)),
                    other => return Err(format!("unsupported property {other} in a text scene")),
                }
            }
            "scale" => ops.push(TextOp::Scale(number(arg(1)?)?, number(arg(2)?)?)),
            "fillText" => ops.push(TextOp::FillText {
                text: string(arg(1)?)?,
                x: number(arg(2)?)?,
                y: number(arg(3)?)?,
                max_width: cmd.get(4).map(number).transpose()?,
            }),
            other => return Err(format!("unsupported call {other} in a text scene")),
        }
    }
    Ok(TextScene {
        width,
        height,
        fill: fill.ok_or("calibration scene sets no fillStyle")?,
        ops,
    })
}

fn fit_lut(reference: &Path, out: &Path) -> Result<(), String> {
    let mut backend =
        platform_backend().ok_or("no text backend for this platform yet (units R12, R13)")?;
    let mut rows: [Option<[u8; 256]>; lut::ROWS] = [None; lut::ROWS];
    let mut residuals = Vec::new();
    for k in 0..lut::ROWS {
        let name = format!("calib-{k}");
        let scene = text_scene(&reference.join(format!("scenes/{name}.json")))?;
        let chromium = pixel::read_png(&reference.join(format!("ref/{name}.cpu.png")))?;
        if (chromium.width, chromium.height) != (scene.width, scene.height) {
            return Err(format!("{name}: reference size differs from the scene"));
        }
        let raw = render_coverage(scene.width, scene.height, &scene.ops, backend.as_mut());
        let samples: Vec<(u8, u8)> = raw
            .iter()
            .zip(chromium.data.as_chunks::<4>().0)
            .filter(|(r, c)| **r != 0 || c[3] != 0)
            .map(|(&r, c)| (r, c[3]))
            .collect();
        let bucket = lut::luminance_bucket(scene.fill);
        if rows[bucket].is_some() {
            return Err(format!(
                "{name}: luminance bucket {bucket} calibrated twice"
            ));
        }
        let row = lut::fit_row(&samples);
        let off: Vec<u8> = samples
            .iter()
            .map(|&(r, c)| row[r as usize].abs_diff(c))
            .collect();
        residuals.push(format!(
            "{name}: bucket {bucket}, {} inked samples, max residual {}, {} over 2",
            samples.len(),
            off.iter().max().copied().unwrap_or(0),
            off.iter().filter(|&&d| d > 2).count()
        ));
        rows[bucket] = Some(row);
    }
    let mut table = [[0u8; 256]; lut::ROWS];
    for (k, row) in rows.into_iter().enumerate() {
        table[k] = row.ok_or_else(|| format!("no calibration scene for luminance bucket {k}"))?;
    }
    let bytes = MaskGammaLut::from_rows(table).to_bytes();
    fs::write(out, &bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    for line in residuals {
        eprintln!("  {line}");
    }
    Ok(())
}
