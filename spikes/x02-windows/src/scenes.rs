use std::collections::HashMap;
use std::fs;
use std::path::Path;

use asrs_canvas::text::TextBackend;
use asrs_canvas::text::coverage::{TextOp, render_coverage};
use asrs_canvas::text::lut::{self, MaskGammaLut};
use asrs_canvas::{Call, ImageDataSource, Pixmap, Raster, TextRaster, pixels, trace};
use asrs_js::value::JsValue;
use asrs_parity::json::{self, Json};
use asrs_parity::pixel;

use crate::dwrite::{DWrite, FaceKey, Variant};
use crate::shape;

pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, reference, out] if cmd == "lut" => fit_lut(Path::new(reference), Path::new(out)),
        [cmd, scenes, out, lut, wanted @ ..] if cmd == "render" => {
            render_scenes(Path::new(scenes), Path::new(out), Path::new(lut), wanted)
        }
        [cmd, file] if cmd == "measure" => measure(Path::new(file)),
        [cmd, dir, out] if cmd == "dom" => crate::scenes::dom(Path::new(dir), Path::new(out)),
        _ => Err(format!("bad arguments {args:?}")),
    }
}

fn backend() -> DWrite {
    let variant = Variant::from_env();
    eprintln!("variant {variant:?}");
    DWrite::new(variant)
}

fn js_number(v: Option<&JsValue>, what: &str) -> Result<f64, String> {
    v.and_then(JsValue::as_f64)
        .ok_or_else(|| format!("{what} is not a number"))
}

fn js_text(v: Option<&JsValue>) -> Option<String> {
    v.and_then(JsValue::as_str).map(|s| s.to_string_lossy())
}

fn render_scenes(scenes: &Path, out: &Path, lut: &Path, wanted: &[String]) -> Result<(), String> {
    let mut backend = backend();
    let lut = MaskGammaLut::from_bytes(&fs::read(lut).map_err(|e| e.to_string())?)
        .ok_or("bad lut")?;
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
        let t = std::time::Instant::now();
        let pixmap = render_list(width, height, cmds, &mut images, &mut backend, &lut)
            .map_err(|e| format!("{name}: {e}"))?;
        eprintln!("{name}: {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
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
    let mut total_off = 0usize;
    let mut total = 0usize;
    for k in 0..lut::ROWS {
        let name = format!("calib-{k}");
        let scene = text_scene(&reference.join(format!("scenes/{name}.json")))?;
        let chromium = pixel::read_png(&reference.join(format!("ref/{name}.cpu.png")))?;
        if (chromium.width, chromium.height) != (scene.width, scene.height) {
            return Err(format!("{name}: reference size differs from the scene"));
        }
        let raw = render_coverage(scene.width, scene.height, &scene.ops, &mut backend);
        if let Some(dir) = std::env::var_os("X02_DUMP") {
            let data = raw.iter().flat_map(|&v| [0, 0, 0, v]).collect();
            pixel::write_png(
                &Path::new(&dir).join(format!("{name}.raw.png")),
                &pixel::Rgba { width: scene.width, height: scene.height, data },
            )?;
        }
        let samples: Vec<(u8, u8)> = raw
            .iter()
            .zip(chromium.data.as_chunks::<4>().0)
            .filter(|(r, c)| **r != 0 || c[3] != 0)
            .map(|(&r, c)| (r, c[3]))
            .collect();
        let only_ref = samples.iter().filter(|(r, c)| *r == 0 && *c != 0).count();
        let only_ours = samples.iter().filter(|(r, c)| *r != 0 && *c == 0).count();
        let bucket = lut::luminance_bucket(scene.fill);
        let row = lut::fit_row(&samples);
        let off: Vec<u8> = samples
            .iter()
            .map(|&(r, c)| row[r as usize].abs_diff(c))
            .collect();
        let over2 = off.iter().filter(|&&d| d > 2).count();
        total_off += over2;
        total += samples.len();
        eprintln!(
            "  {name}: bucket {bucket}, {} inked samples, max residual {}, {} over 2, ink only in Chromium {only_ref}, only ours {only_ours}",
            samples.len(),
            off.iter().max().copied().unwrap_or(0),
            over2,
        );
        rows[bucket] = Some(row);
    }
    eprintln!("  total: {total_off} of {total} samples over 2 levels");
    let mut table = [[0u8; 256]; lut::ROWS];
    for (k, row) in rows.into_iter().enumerate() {
        table[k] = row.ok_or_else(|| format!("no calibration scene for luminance bucket {k}"))?;
    }
    let bytes = MaskGammaLut::from_rows(table).to_bytes();
    fs::write(out, &bytes).map_err(|e| format!("{}: {e}", out.display()))
}

pub struct Css {
    pub key: FaceKey,
    pub size: f64,
}

pub fn parse_css(css: &str) -> Css {
    let mut size = 10.0;
    let mut weight = 400;
    let mut italic = false;
    for token in css.split_whitespace() {
        if let Some(px) = token.strip_suffix("px") {
            size = px.parse().unwrap_or(10.0);
            break;
        }
        match token {
            "bold" => weight = 700,
            "italic" => italic = true,
            _ => {
                if let Ok(w) = token.parse() {
                    weight = w;
                }
            }
        }
    }
    let family = if css.contains("system-ui") {
        "Segoe UI"
    } else {
        "Arial"
    };
    Css {
        key: FaceKey {
            family: family.to_owned(),
            weight,
            italic,
        },
        size,
    }
}

fn hex_f64(v: &Json) -> Option<f64> {
    let n = v.get("n")?.as_str()?.to_string_lossy();
    u64::from_str_radix(&n, 16).ok().map(f64::from_bits)
}

fn measure(file: &Path) -> Result<(), String> {
    let mut backend = backend();
    let text = fs::read_to_string(file).map_err(|e| e.to_string())?;
    let mut by_font: std::collections::BTreeMap<String, (usize, f64, f64, usize, usize)> =
        Default::default();
    for line in text.lines() {
        let r = json::parse(line).map_err(|e| e.to_string())?;
        let s = |k: &str| r.get(k).and_then(Json::as_str).map(|v| v.to_string_lossy());
        if s("align").as_deref() != Some("left") || s("baseline").as_deref() != Some("alphabetic") {
            continue;
        }
        let font = s("font").unwrap_or_default();
        let label = s("text").unwrap_or_default();
        let metrics = r.get("metrics").and_then(|m| m.get("o")).and_then(Json::as_array).ok_or("metrics")?;
        let width = metrics
            .iter()
            .find_map(|kv| {
                let kv = kv.as_array()?;
                (kv[0].as_str()?.to_string_lossy() == "width").then(|| hex_f64(&kv[1]))?
            })
            .ok_or("width")?;
        let css = parse_css(&font);
        let face = backend.face(&css.key);
        let run = shape::shape(&face.data, face.face_index(), face.upem, css.size, &label);
        let err = (run.width - width).abs();
        let entry = by_font.entry(font).or_default();
        entry.0 += 1;
        entry.1 = entry.1.max(err);
        entry.2 += err;
        if err > 0.0 {
            entry.3 += 1;
        }
        if (run.width / 10.0).ceil() != (width / 10.0).ceil() {
            entry.4 += 1;
        }
        if err > 0.0 {
            eprintln!("  {label:?} {:?}: ours {} chromium {width}", css.key, run.width);
        }
    }
    for (font, (n, max, sum, inexact, flips)) in by_font {
        println!(
            "{font}: {n} strings, max abs {max:.6}, mean {:.6}, inexact {inexact}, ceil(w/10) flips {flips}",
            sum / n as f64
        );
    }
    Ok(())
}

pub fn dom(dir: &Path, out: &Path) -> Result<(), String> {
    crate::dom::run(dir, out)
}
