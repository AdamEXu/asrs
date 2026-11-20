use std::collections::HashMap;
use std::fs;
use std::path::Path;

use asrs_canvas::text::coverage::{TextOp, render_coverage};
use asrs_canvas::text::lut::{self, MaskGammaLut};
use asrs_canvas::text::{TextBackend, platform_backend};
use asrs_canvas::{Call, ImageDataSource, Pixmap, Raster, RasterState, TextRaster, pixels, trace};
use asrs_js::value::JsValue;
use asrs_parity::json::{self, Json};
use asrs_parity::pixel;

const USAGE: &str = "usage: asrs-cli canvas <command>

commands:
  lut <reference dir> <out.bin>   fit the mask-gamma table from the oracle's calibration scenes
                                  (scenes/calib-<k>.json and ref/calib-<k>.cpu.png)
  render <scenes dir> <out dir> [<scene>...]
                                  rasterize scene display lists to <out dir>/<scene>.png, straight
                                  RGBA as getImageData returns it (`cargo xtask canvas run`)";

pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, reference, out] if cmd == "lut" => fit_lut(Path::new(reference), Path::new(out)),
        [cmd, scenes, out, wanted @ ..] if cmd == "render" => {
            render_scenes(Path::new(scenes), Path::new(out), wanted)
        }
        _ => Err(USAGE.to_owned()),
    }
}

fn js_number(v: Option<&JsValue>, what: &str) -> Result<f64, String> {
    v.and_then(JsValue::as_f64)
        .ok_or_else(|| format!("{what} is not a number"))
}

fn js_text(v: Option<&JsValue>) -> Option<String> {
    v.and_then(JsValue::as_str).map(|s| s.to_string_lossy())
}

fn render_scenes(scenes: &Path, out: &Path, wanted: &[String]) -> Result<(), String> {
    let mut glyphs = platform_backend().zip(MaskGammaLut::embedded());
    if glyphs.is_none() {
        eprintln!(
            "no text backend or mask-gamma table on this platform: skipping scenes with text"
        );
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(scenes).map_err(|e| format!("{}: {e}", scenes.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
            && (wanted.is_empty() || wanted.iter().any(|w| w == name))
        {
            names.push(name.to_owned());
        }
    }
    names.sort_unstable();
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    for name in names {
        let path = scenes.join(format!("{name}.json"));
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let scene = asrs_js::json::parse_str(&text).map_err(|e| format!("{name}: {e:?}"))?;
        if js_text(scene.get("gate")).as_deref() == Some("calibration") {
            continue;
        }
        let dpr = js_number(scene.get("dpr"), "dpr")?;
        let width = (js_number(scene.get("width"), "width")? * dpr) as u32;
        let height = (js_number(scene.get("height"), "height")? * dpr) as u32;
        let cmds = scene
            .get("cmds")
            .and_then(JsValue::as_array)
            .ok_or("cmds is not an array")?;
        let mut images = HashMap::new();
        if glyphs.is_none() && uses_text(cmds) {
            continue;
        }
        let pixmap = render_list(width, height, cmds, &mut images, &mut glyphs)
            .map_err(|e| format!("{name}: {e}"))?;
        let rgba = pixel::Rgba {
            width,
            height,
            data: pixels::to_straight_rgba(&pixmap),
        };
        pixel::write_png(&out.join(format!("{name}.png")), &rgba)?;
    }
    Ok(())
}

fn uses_text(cmds: &[JsValue]) -> bool {
    cmds.iter().any(|cmd| {
        let items = cmd.as_array().unwrap_or_default();
        match js_text(items.first()).as_deref() {
            Some("fillText") => true,
            Some("canvas") => items
                .get(4)
                .and_then(JsValue::as_array)
                .is_some_and(|inner| uses_text(inner)),
            _ => false,
        }
    })
}

/// Replays one scene command list. The scene-only commands `image` (a source from straight
/// RGBA) and `canvas` (a nested canvas) only define sources, so they run before the rest;
/// `frame` (`canvas.width = canvas.width`) and `resize` start a new frame on the same canvas.
fn render_list(
    width: u32,
    height: u32,
    cmds: &[JsValue],
    images: &mut HashMap<String, Pixmap>,
    glyphs: &mut Option<(Box<dyn TextBackend>, MaskGammaLut)>,
) -> Result<Pixmap, String> {
    let mut frames: Vec<((u32, u32), Vec<Call>)> = Vec::new();
    let mut frame_size = (width, height);
    let mut calls: Vec<Call> = Vec::new();
    for cmd in cmds {
        let items = cmd.as_array().ok_or("a command is not an array")?;
        let name = js_text(items.first()).unwrap_or_default();
        let id = js_text(items.get(1));
        let size = || -> Result<(u32, u32), String> {
            Ok((
                js_number(items.get(2), "width")? as u32,
                js_number(items.get(3), "height")? as u32,
            ))
        };
        match name.as_str() {
            "image" => {
                let (w, h) = size()?;
                let spec = JsValue::Object(
                    [
                        ("width", JsValue::Number(f64::from(w))),
                        ("height", JsValue::Number(f64::from(h))),
                        ("rgba", items.get(4).cloned().unwrap_or_default()),
                    ]
                    .into_iter()
                    .collect(),
                );
                let ImageDataSource::Inline(data) = trace::image_data_from(&spec)? else {
                    return Err("image: no pixels".to_owned());
                };
                let pixmap =
                    pixels::pixmap_from_straight_rgba(w, h, &data.data).ok_or("image: bad size")?;
                images.insert(id.ok_or("image: no id")?, pixmap);
            }
            "canvas" => {
                let (w, h) = size()?;
                let inner = items
                    .get(4)
                    .and_then(JsValue::as_array)
                    .ok_or("canvas: no commands")?;
                let pixmap = render_list(w, h, inner, images, glyphs)?;
                images.insert(id.ok_or("canvas: no id")?, pixmap);
            }
            "frame" => frames.push((frame_size, std::mem::take(&mut calls))),
            "resize" => {
                frames.push((frame_size, std::mem::take(&mut calls)));
                frame_size = (
                    js_number(items.get(1), "width")? as u32,
                    js_number(items.get(2), "height")? as u32,
                );
            }
            _ => calls.push(trace::call_from_value(cmd)?),
        }
    }
    frames.push((frame_size, calls));
    let mut state = RasterState::default();
    let mut last = None;
    for ((w, h), calls) in frames {
        let Some(raster) = Raster::new(w, h) else {
            state = RasterState::default();
            continue;
        };
        let mut raster = raster.with_state(state);
        if let Some((backend, lut)) = glyphs.as_mut() {
            raster = raster.with_text(TextRaster {
                backend: backend.as_mut(),
                lut,
            });
        }
        for (id, pixmap) in images.iter() {
            raster.add_image(id, pixmap.clone());
        }
        raster.run(&calls);
        state = raster.state();
        last = Some(raster.into_pixmap());
    }
    if frame_size != (width, height) {
        return Err(format!(
            "the last frame is {}x{}, not the scene size",
            frame_size.0, frame_size.1
        ));
    }
    last.ok_or_else(|| "empty canvas".to_owned())
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
