mod ft;

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use asrs_canvas::text::coverage::{TextOp, render_coverage};
use asrs_canvas::text::lut::{self, MaskGammaLut};
use asrs_canvas::text::{FontSpec, TextBackend};
use asrs_canvas::{Call, ImageDataSource, Pixmap, Raster, TextRaster, pixels, trace};
use asrs_js::value::JsValue;
use asrs_parity::json::{self, Json};

use asrs_parity::pixel;

const USAGE: &str = "x03-linux <command>
  info                                  FreeType version and resolved font files
  lut <reference dir> <out.bin>         fit the mask-gamma table from calib-<k> scenes
  render <scenes dir> <out dir> [<scene>...]
                                        rasterize scenes with the FreeType backend (X03_LUT=<bin>)
  measure <measure.jsonl>               compare widths with Chromium's measureText
  dump <reference dir> <scene> <out.png>
                                        raw text coverage of one scene, before the table
env: X03_HINT=none|slight|normal X03_AUTOHINT=0|1 X03_SUBPIXEL=0|1 X03_ROUNDSIZE=0|1
     X03_SKPATH=1|2|3 (fill the hinted outline: Skia analytic AA, tiny-skia, exact area)
     X03_LCDAVG=1 (LCD render averaged to gray)";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [c] if c == "info" => info(),
        [c, reference, out] if c == "lut" => fit_lut(Path::new(reference), Path::new(out)),
        [c, scenes, out, wanted @ ..] if c == "render" => {
            render_scenes(Path::new(scenes), Path::new(out), wanted)
        }
        [c, file] if c == "measure" => measure(Path::new(file)),
        [c, reference, scene, out] if c == "dump" => dump(Path::new(reference), scene, Path::new(out)),
        _ => Err(USAGE.to_owned()),
    };
    if let Err(e) = result {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

fn backend() -> ft::FreeType {
    let options = ft::Options::from_env();
    eprintln!("options: {options:?}");
    ft::FreeType::new(options)
}

fn info() -> Result<(), String> {
    let mut b = backend();
    println!("freetype {}", b.version());
    for w in [200, 400, 700] {
        let spec = FontSpec { size: 12.0, weight: w };
        println!("weight {w}: {}", b.font_path(&spec));
    }
    Ok(())
}

fn js_number(v: Option<&JsValue>, what: &str) -> Result<f64, String> {
    v.and_then(JsValue::as_f64)
        .ok_or_else(|| format!("{what} is not a number"))
}

fn js_text(v: Option<&JsValue>) -> Option<String> {
    v.and_then(JsValue::as_str).map(|s| s.to_string_lossy())
}

fn load_lut() -> Result<MaskGammaLut, String> {
    match std::env::var("X03_LUT") {
        Ok(path) => MaskGammaLut::from_bytes(&fs::read(&path).map_err(|e| format!("{path}: {e}"))?)
            .ok_or("bad lut".to_owned()),
        Err(_) => Ok(MaskGammaLut::identity()),
    }
}

fn render_scenes(scenes: &Path, out: &Path, wanted: &[String]) -> Result<(), String> {
    let mut backend = backend();
    let lut = load_lut()?;
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
        let start = std::time::Instant::now();
        let pixmap = render_list(width, height, cmds, &mut images, &mut backend, &lut)
            .map_err(|e| format!("{name}: {e}"))?;
        eprintln!("{name}: {:?}", start.elapsed());
        let rgba = pixel::Rgba {
            width,
            height,
            data: pixels::to_straight_rgba(&pixmap),
        };
        pixel::write_png(&out.join(format!("{name}.png")), &rgba)?;
    }
    Ok(())
}

fn render_list(
    width: u32,
    height: u32,
    cmds: &[JsValue],
    images: &mut HashMap<String, Pixmap>,
    backend: &mut dyn TextBackend,
    lut: &MaskGammaLut,
) -> Result<Pixmap, String> {
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
                let pixmap = render_list(w, h, inner, images, backend, lut)?;
                images.insert(id.ok_or("canvas: no id")?, pixmap);
            }
            _ => calls.push(trace::call_from_value(cmd)?),
        }
    }
    let mut raster = Raster::new(width, height)
        .ok_or("empty canvas")?
        .with_text(TextRaster { backend, lut });
    for (id, pixmap) in images.iter() {
        raster.add_image(id, pixmap.clone());
    }
    raster.run(&calls);
    Ok(raster.into_pixmap())
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
    let mut backend = backend();
    let mut rows: [Option<[u8; 256]>; lut::ROWS] = [None; lut::ROWS];
    for k in 0..lut::ROWS {
        let name = format!("calib-{k}");
        let scene = text_scene(&reference.join(format!("scenes/{name}.json")))?;
        let chromium = pixel::read_png(&reference.join(format!("ref/{name}.cpu.png")))?;
        let raw = render_coverage(scene.width, scene.height, &scene.ops, &mut backend);
        let pairs: Vec<(u8, u8)> = raw
            .iter()
            .zip(chromium.data.as_chunks::<4>().0)
            .filter(|(r, c)| **r != 0 || c[3] != 0)
            .map(|(&r, c)| (r, c[3]))
            .collect();
        let bucket = lut::luminance_bucket(scene.fill);
        let row = lut::fit_row(&pairs);
        let off: Vec<u8> = pairs
            .iter()
            .map(|&(r, c)| row[r as usize].abs_diff(c))
            .collect();
        let raw_ink = raw.iter().filter(|&&r| r != 0).count();
        let ref_ink = chromium.data.as_chunks::<4>().0.iter().filter(|c| c[3] != 0).count();
        let both = raw
            .iter()
            .zip(chromium.data.as_chunks::<4>().0)
            .filter(|(r, c)| **r != 0 && c[3] != 0)
            .count();
        eprintln!(
            "{name}: bucket {bucket}, ink ours {raw_ink} chromium {ref_ink} both {both}, max residual {}, {} over 2, {} over 8",
            off.iter().max().copied().unwrap_or(0),
            off.iter().filter(|&&d| d > 2).count(),
            off.iter().filter(|&&d| d > 8).count(),
        );
        rows[bucket] = Some(row);
    }
    let mut table = [[0u8; 256]; lut::ROWS];
    for (k, row) in rows.into_iter().enumerate() {
        table[k] = row.ok_or_else(|| format!("no calibration scene for luminance bucket {k}"))?;
    }
    fs::write(out, MaskGammaLut::from_rows(table).to_bytes()).map_err(|e| e.to_string())
}

fn measure(file: &Path) -> Result<(), String> {
    let mut backend = backend();
    let text = fs::read_to_string(file).map_err(|e| e.to_string())?;
    let mut per_font: HashMap<String, (usize, f64, usize, usize)> = HashMap::new();
    for record in asrs_parity::vectors::parse_records(&text)? {
        let font = record.string("font").ok_or("font")?;
        if font.starts_with("italic")
            || record.string("align").as_deref() != Some("left")
            || record.string("baseline").as_deref() != Some("alphabetic")
        {
            continue;
        }
        let s = record.string("text").ok_or("text")?;
        let width = record
            .value("metrics")
            .map_err(|e| format!("{e:?}"))?
            .get("width")
            .and_then(|v| v.as_f64())
            .ok_or("width")?;
        let spec = FontSpec::parse(&font);
        let ours = backend.shape(&spec, &s).width;
        let err = (ours - width).abs();
        let flip = usize::from((ours / 10.0).ceil() != (width / 10.0).ceil());
        let entry = per_font.entry(font.clone()).or_default();
        entry.0 += 1;
        entry.1 = entry.1.max(err);
        entry.2 += usize::from(err == 0.0);
        entry.3 += flip;
        if err > 0.01 && std::env::var("X03_VERBOSE").is_ok() {
            eprintln!("{font:?} {s:?}: chromium {width} ours {ours}");
        }
    }
    let mut fonts: Vec<_> = per_font.into_iter().collect();
    fonts.sort_by(|a, b| a.0.cmp(&b.0));
    for (font, (n, max, exact, flips)) in fonts {
        println!("{font:<70} n={n:<4} exact={exact:<4} max_err={max:.5} ceil10_flips={flips}");
    }
    Ok(())
}

fn dump(reference: &Path, name: &str, out: &Path) -> Result<(), String> {
    let mut backend = backend();
    let scene = text_scene(&reference.join(format!("scenes/{name}.json")))?;
    let raw = render_coverage(scene.width, scene.height, &scene.ops, &mut backend);
    let data = raw.iter().flat_map(|&c| [0, 0, 0, c]).collect();
    pixel::write_png(out, &pixel::Rgba { width: scene.width, height: scene.height, data })
}
