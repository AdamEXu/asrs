use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use asrs_canvas::text::coverage::{TextOp, render_coverage};
use asrs_canvas::text::dom::{self, Style};
use asrs_canvas::text::lut::{self, LcdGammaLut, MaskGammaLut};
use asrs_canvas::text::{LcdMask, TextBackend, platform_backend};
use asrs_canvas::{Call, ImageDataSource, Pixmap, Raster, RasterState, TextRaster, pixels, trace};
use asrs_js::value::JsValue;
use asrs_parity::json::{self, Json};
use asrs_parity::pixel;

const USAGE: &str = "usage: asrs-cli canvas <command>

commands:
  lut <reference dir> <out.bin>   fit the mask-gamma table from the oracle's calibration scenes
                                  (scenes/calib-<k>.json and ref/calib-<k>.cpu.png)
  lcd <reference dir> <out.bin>   fit the per-channel table of LCD text from the oracle's DOM
                                  calibration scenes (scenes/lcd-*.json and lcd-*@<scale>x.*)
  render <scenes dir> <out dir> [<scene>...]
                                  rasterize scene display lists to <out dir>/<scene>.png, straight
                                  RGBA as getImageData returns it (`cargo xtask canvas run`)";

pub fn run(args: &[String]) -> Result<(), String> {
    match args {
        [cmd, reference, out] if cmd == "lut" => fit_lut(Path::new(reference), Path::new(out)),
        [cmd, reference, out] if cmd == "lcd" => fit_lcd(Path::new(reference), Path::new(out)),
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
                .is_some_and(uses_text),
            _ => false,
        }
    })
}

struct Source {
    pixmap: Arc<Pixmap>,
    element: bool,
}

/// Replays one scene command list. The scene-only commands `image` (a canvas source from straight
/// RGBA), `img` (an `<img>` of them) and `canvas` (a nested canvas) only define sources, so they
/// run before the rest; `frame` (`canvas.width = canvas.width`) and `resize` start a new frame on the same canvas.
fn render_list(
    width: u32,
    height: u32,
    cmds: &[JsValue],
    images: &mut HashMap<String, Source>,
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
            "image" | "img" => {
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
                    pixels::pixmap_from_straight_rgba(w, h, data.data).ok_or("image: bad size")?;
                let source = Source {
                    pixmap: Arc::new(pixmap),
                    element: name == "img",
                };
                images.insert(id.ok_or("image: no id")?, source);
            }
            "canvas" => {
                let (w, h) = size()?;
                let inner = items
                    .get(4)
                    .and_then(JsValue::as_array)
                    .ok_or("canvas: no commands")?;
                let pixmap = render_list(w, h, inner, images, glyphs)?;
                let source = Source {
                    pixmap: Arc::new(pixmap),
                    element: false,
                };
                images.insert(id.ok_or("canvas: no id")?, source);
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
        for (id, source) in images.iter() {
            if source.element {
                raster.add_image_element(id, source.pixmap.clone());
            } else {
                raster.add_image(id, source.pixmap.clone());
            }
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
    let mut backend = platform_backend().ok_or("no text backend on this machine")?;
    let mut rows: [Option<[u8; 256]>; lut::ROWS] = [None; lut::ROWS];
    // DirectWrite's averaged coverage skips a few values, which Skia's own table then fills.
    let prior = cfg!(windows)
        .then(|| MaskGammaLut::skia(MaskGammaLut::WINDOWS_CONTRAST, MaskGammaLut::WINDOWS_GAMMA));
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
        let row = lut::fit_row(&samples, prior.as_ref().map(|p| p.row(bucket)));
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

/// Raw LCD coverage of the one glyph that inks each pixel, `None` where several do.
struct LcdPlane {
    width: u32,
    height: u32,
    pixels: HashMap<(i32, i32), Option<[u8; 3]>>,
}

impl LcdPlane {
    fn draw(&mut self, mask: &LcdMask, x: i32, y: i32) {
        for (i, raw) in mask.data.as_chunks::<3>().0.iter().enumerate() {
            let at = (
                x + mask.left + (i as u32 % mask.width) as i32,
                y + mask.top + (i as u32 / mask.width) as i32,
            );
            let inside =
                (0..self.width as i32).contains(&at.0) && (0..self.height as i32).contains(&at.1);
            if inside && *raw != [0; 3] {
                self.pixels
                    .entry(at)
                    .and_modify(|owner| *owner = None)
                    .or_insert(Some(*raw));
            }
        }
    }

    /// Draws a one-line text box at its DOM rectangle as Blink paints it.
    fn draw_box(
        &mut self,
        backend: &mut dyn TextBackend,
        css: &str,
        text: &str,
        rect: &[f64],
        scale: f32,
    ) -> Result<(), String> {
        let style = Style::from_css([("font", css), ("white-space", "pre")])
            .ok_or_else(|| format!("bad font {css:?}"))?;
        let layout = dom::layout(backend, &style, text, None, scale);
        let [line] = layout.lines.as_slice() else {
            return Err(format!("{text:?} is not one line"));
        };
        let device = |v: f64| dom::lu_round((v * f64::from(scale)) as f32);
        let baseline = (f64::from(device(rect[1]) + line.baseline) / 64.0).round() as i32;
        for fragment in &line.fragments {
            let origin = (device(rect[0]) + fragment.x) as f32 / 64.0;
            for g in &fragment.glyphs {
                let biased = origin + g.x * scale + 0.125;
                let x = biased.floor();
                let phase = ((biased - x) * 4.0) as u8;
                if let Some(mask) = backend.raw_lcd_mask(g.glyph, f64::from(scale), 1.0, phase) {
                    self.draw(&mask, x as i32, baseline);
                }
            }
        }
        Ok(())
    }
}

/// The 5-bit coverage that blends `color` onto `background` into `pixel`.
fn lcd_coverage(background: u8, color: u8, pixel: u8) -> Option<u8> {
    (0..32).find(|&m| lut::blend_lcd_channel(background, color, m) == pixel)
}

const LCD_SCALES: [u32; 2] = [1, 2];

fn fit_lcd(reference: &Path, out: &Path) -> Result<(), String> {
    let mut backend = platform_backend().ok_or("no text backend on this machine")?;
    let prior = MaskGammaLut::embedded().ok_or("no mask-gamma table for this platform")?;
    let mut samples: Vec<Vec<(u8, u8)>> = vec![Vec::new(); 3 * lut::ROWS];
    let mut scenes = Vec::new();
    let dir = reference.join("scenes");
    for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        scenes.push(entry.map_err(|e| e.to_string())?.path());
    }
    scenes.sort();
    if scenes.len() != 2 * lut::ROWS {
        return Err(format!("{} LCD calibration scenes", scenes.len()));
    }
    let read = |path: &Path| {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    };
    let list = |v: &Json, k: &str| {
        let array = v.get(k).and_then(Json::as_array).map(<[Json]>::to_vec);
        array.ok_or_else(|| format!("no {k}"))
    };
    let text = |v: &Json, k: &str| string(v.get(k).ok_or_else(|| format!("no {k}"))?);
    let mut captures = Vec::new();
    for path in &scenes {
        let scene = read(path)?;
        let name = text(&scene, "name")?;
        let color = hex_colour(&text(&scene, "color")?)?;
        let background = hex_colour(&text(&scene, "background")?)?;
        for scale in LCD_SCALES {
            let base = reference.join(format!("{name}@{scale}x"));
            let image = pixel::read_png(&base.with_extension("png"))?;
            let captured = read(&base.with_extension("json"))?;
            let boxes = list(&captured, "rows")?;
            let rows = list(&scene, "rows")?;
            if rows.len() != boxes.len() {
                return Err(format!("{name}@{scale}x: {} text boxes", boxes.len()));
            }
            let mut plane = LcdPlane {
                width: image.width,
                height: image.height,
                pixels: HashMap::new(),
            };
            for (row, dom) in rows.iter().zip(&boxes) {
                let id = text(row, "id")?;
                if text(dom, "id")? != id || text(dom, "mode")? != "lcd" {
                    return Err(format!("{name}@{scale}x {id}: not LCD text"));
                }
                let rect: Vec<f64> = list(dom, "rect")?.iter().filter_map(Json::as_f64).collect();
                plane.draw_box(
                    backend.as_mut(),
                    &text(row, "font")?,
                    &text(row, "text")?,
                    &rect,
                    scale as f32,
                )?;
            }
            let mut inked = Vec::new();
            let mut undecoded = 0usize;
            for (&(x, y), raw) in &plane.pixels {
                let Some(raw) = raw else { continue };
                let at = (y as u32 * image.width + x as u32) as usize * 4;
                for c in 0..3 {
                    let pixel = image.data[at + c];
                    match lcd_coverage(background[c], color[c], pixel) {
                        Some(m) => inked.push((c, raw[c], m << 3 | m >> 2)),
                        None => undecoded += 1,
                    }
                }
            }
            for &(c, raw, out) in &inked {
                samples[c * lut::ROWS + usize::from(color[c] >> 5)].push((raw, out));
            }
            captures.push((format!("{name}@{scale}x"), color, inked, undecoded));
        }
    }
    let mut rows = [[[0u8; 256]; lut::ROWS]; 3];
    for (i, samples) in samples.iter().enumerate() {
        let bucket = i % lut::ROWS;
        let prior = prior.row(bucket).map(|v| (v >> 3) << 3 | v >> 5);
        rows[i / lut::ROWS][bucket] = lut::fit_row(samples, Some(&prior));
    }
    let table = LcdGammaLut::from_rows(rows);
    for (name, color, inked, undecoded) in &captures {
        let off = |c: usize, raw: u8, out: u8| {
            let row = table.row(c, usize::from(color[c] >> 5));
            (row[usize::from(raw)] >> 3).abs_diff(out >> 3)
        };
        let off: Vec<u8> = inked.iter().map(|&(c, r, o)| off(c, r, o)).collect();
        eprintln!(
            "  {name}: {} samples, {} off, by at most {} of 31, {undecoded} not a blend",
            off.len(),
            off.iter().filter(|&&d| d > 0).count(),
            off.iter().max().copied().unwrap_or(0)
        );
    }
    fs::write(out, table.to_bytes()).map_err(|e| format!("{}: {e}", out.display()))
}

#[cfg(test)]
mod lcd_tests {
    use super::*;

    #[test]
    fn coverage_is_read_back_from_the_blend() {
        for (background, color) in [(255, 0), (0, 255), (255, 0x6d), (0, 0x92)] {
            for m in 0..32 {
                let pixel = lut::blend_lcd_channel(background, color, m);
                let found = lcd_coverage(background, color, pixel).unwrap();
                assert_eq!(found, m, "{color:#x} on {background:#x}");
            }
        }
        assert_eq!(lcd_coverage(255, 0, 254), None);
    }
}

#[cfg(all(test, any(target_os = "linux", windows)))]
mod tests {
    use std::io::Write;
    use std::path::PathBuf;

    use asrs_canvas::text::FontSpec;
    use sha2::{Digest, Sha256};

    use super::*;

    const MAX_CHANNEL: u8 = 24;

    const FONTS: [&str; 5] = [
        "12px system-ui",
        "200 12px system-ui",
        "bold 12px system-ui",
        "10px sans-serif",
        "italic 10px sans-serif",
    ];

    #[cfg(target_os = "linux")]
    fn font_files() -> Option<Vec<PathBuf>> {
        let mut backend = asrs_canvas::text::linux::Fontations::new()?;
        let file = |css: &&str| backend.font_file(&FontSpec::parse(css).unwrap());
        FONTS.iter().map(file).map(|f| f.map(|f| f.0)).collect()
    }

    #[cfg(windows)]
    fn font_files() -> Option<Vec<PathBuf>> {
        let mut backend = asrs_canvas::text::win::DirectWrite::new()?;
        let mut files = Vec::new();
        for css in FONTS {
            let spec = FontSpec::parse(css).unwrap();
            files.push(backend.font_file(&spec)?.0);
            files.extend(backend.fallback_file(&spec, '\u{1f4d6}', true));
        }
        Some(files)
    }

    #[cfg(target_os = "linux")]
    fn text() -> Option<(Box<dyn TextBackend>, MaskGammaLut)> {
        platform_backend().zip(MaskGammaLut::embedded())
    }

    /// The machine's fonts with the render host's ClearType settings.
    #[cfg(windows)]
    fn text() -> Option<(Box<dyn TextBackend>, MaskGammaLut)> {
        use asrs_canvas::text::cache::Cached;
        use asrs_canvas::text::win::{DirectWrite, settings::TextSettings};
        let backend = DirectWrite::with_settings(Some(TextSettings::DEFAULT))?;
        let backend: Box<dyn TextBackend> = Box::new(Cached::new(backend));
        Some(backend).zip(MaskGammaLut::embedded())
    }

    /// A font of the scenes that is not the file the manifest records for the render host.
    fn unpinned_font(manifest: &Json) -> Option<String> {
        let fonts = manifest.get("host").and_then(|h| h.get("fonts")).unwrap();
        let Json::Object(fonts) = fonts else {
            panic!("the manifest records no host fonts");
        };
        let Some(files) = font_files() else {
            return Some("no fonts".to_owned());
        };
        files.iter().find_map(|path| {
            let hash = format!("{:x}", Sha256::digest(fs::read(path).unwrap()));
            let recorded = fonts
                .iter()
                .any(|(_, h)| h.as_str().is_some_and(|h| h.to_string_lossy() == hash));
            (!recorded).then(|| format!("{} is not a font of the render host", path.display()))
        })
    }

    /// CI installs the Linux render host's fonts. Windows' cannot be installed, so a runner whose
    /// files differ says so past libtest's output capture.
    fn skip(problem: &str) {
        if std::env::var_os("ASRS_CI").is_none() {
            eprintln!("skipped: {problem} (FONTCONFIG_FILE)");
            return;
        }
        if !cfg!(windows) {
            panic!("{problem}");
        }
        let line = format!("\n::warning title=Windows text scenes not run::{problem}\n");
        let _ = std::io::stdout().write_all(line.as_bytes());
    }

    /// The committed text scenes against Chromium's CPU canvas on the platform's render
    /// host, at the gate `cargo xtask canvas diff` holds text to, wherever its fonts are
    /// installed.
    #[test]
    fn text_scenes_match_the_render_host() {
        let os = std::env::consts::OS;
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../parity/vectors/canvas");
        let manifest =
            json::parse(&fs::read_to_string(root.join(format!("{os}.json"))).unwrap()).unwrap();
        let scenes = manifest.get("scenes").and_then(Json::as_array).unwrap();
        assert!(scenes.len() >= 12);
        if let Some(problem) = unpinned_font(&manifest) {
            skip(&problem);
            return;
        }
        let mut glyphs = text();
        for entry in scenes {
            let name = string(entry.get("scene").unwrap()).unwrap();
            if name.starts_with("dom-") {
                continue;
            }
            let file = |ext: &str| root.join(format!("scenes-{os}/{name}.{ext}"));
            let scene =
                asrs_js::json::parse_str(&fs::read_to_string(file("json")).unwrap()).unwrap();
            let number = |k: &str| js_number(scene.get(k), k).unwrap();
            let (dpr, text) = (number("dpr"), js_text(scene.get("gate")).unwrap() == "text");
            let (width, height) = (
                (number("width") * dpr) as u32,
                (number("height") * dpr) as u32,
            );
            let cmds = scene.get("cmds").and_then(JsValue::as_array).unwrap();
            let pixmap =
                render_list(width, height, cmds, &mut HashMap::new(), &mut glyphs).unwrap();
            let ours = pixel::Rgba {
                width,
                height,
                data: pixels::to_straight_rgba(&pixmap),
            };
            let background = scene.get("background").and_then(JsValue::as_array).unwrap();
            let channel = |i: usize| js_number(background.get(i), "background").unwrap() as u8;
            let background = [channel(0), channel(1), channel(2)];
            let composite = |image: &pixel::Rgba| pixel::composite(image, background);
            let reference = pixel::read_png(&file("png")).unwrap();
            let stats = pixel::compare(&composite(&reference), &composite(&ours), background)
                .unwrap()
                .stats;
            eprintln!(
                "{name}: {} differing, max channel {}",
                stats.mismatch, stats.max_channel
            );
            if text {
                assert_eq!(stats.mismatch, 0, "{name}");
                assert!(
                    stats.max_channel <= MAX_CHANNEL,
                    "{name}: {}",
                    stats.max_channel
                );
            } else {
                assert!(stats.mismatch_pct <= 0.2 && stats.max_blob <= 64, "{name}");
            }
        }
    }
}
