use std::fs;
use std::path::Path;

use asrs_canvas::text::lut;
use asrs_parity::json::{self, Json};
use asrs_parity::pixel::{self, Rgb};

use crate::dwrite::{DWrite, Variant};
use crate::scenes::parse_css;

const FAMILY: &str = "ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";
const SAMPLES: [&str; 4] = [
    "Robot/Drive/LeftVelocity",
    "Teleoperated 0123456789 +4.37s",
    "AVWAY Tij fi Wg",
    "NT:/SmartDashboard/Auto Chooser",
];

fn style(id: &str) -> Option<String> {
    let (size, rest) = id.split_once("px").unwrap_or((id, ""));
    let size: f64 = size.parse().ok()?;
    let css = match rest {
        "" => format!("{size}px {FAMILY}"),
        "-bold" => format!("bold {size}px {FAMILY}"),
        "-mono" | "-mono-bold" => return None,
        w => format!("{} {size}px {FAMILY}", w.trim_start_matches('-')),
    };
    Some(css)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum XMode {
    Quarter,
    Integer,
    Exact,
}

struct Row {
    css: String,
    text: &'static str,
    rect: [f64; 4],
    chars: Vec<f64>,
}

struct Placed {
    box_: [i32; 4],
    raw: Vec<[u8; 3]>,
}

fn rows(scene: &Json) -> Vec<Row> {
    let mut out = Vec::new();
    for r in scene.get("rows").and_then(Json::as_array).unwrap_or_default() {
        let id = r.get("id").and_then(Json::as_str).map(|s| s.to_string_lossy()).unwrap_or_default();
        let Some((sid, j)) = id.split_once('/') else { continue };
        let Some(css) = style(sid) else { continue };
        let j: usize = j.parse().unwrap_or(9);
        if j >= 4 {
            continue;
        }
        let num = |v: &Json| v.as_f64().unwrap_or(0.0);
        let rect = r.get("rect").and_then(Json::as_array).unwrap_or_default();
        let chars = r.get("chars").and_then(Json::as_array).unwrap_or_default();
        out.push(Row {
            css,
            text: SAMPLES[j],
            rect: [num(&rect[0]), num(&rect[1]), num(&rect[2]), num(&rect[3])],
            chars: chars
                .iter()
                .map(|c| c.as_array().map(|c| num(&c[1])).unwrap_or(0.0))
                .collect(),
        });
    }
    out
}

fn place(dw: &mut DWrite, row: &Row, scale: f64, mode: XMode, baseline: i32) -> Placed {
    let css = parse_css(&row.css);
    let face = dw.face(&css.key);
    let run = crate::shape::shape(&face.data, face.face_index(), face.upem, css.size, row.text);
    let box_ = [
        (row.rect[0] * scale).floor() as i32 - 4,
        (row.rect[1] * scale).floor() as i32 - 4,
        ((row.rect[0] + row.rect[2]) * scale).ceil() as i32 + 4,
        ((row.rect[1] + row.rect[3]) * scale).ceil() as i32 + 4,
    ];
    let w = (box_[2] - box_[0]) as usize;
    let h = (box_[3] - box_[1]) as usize;
    let mut plane = vec![[0f64; 3]; w * h];
    let face = dw.face(&css.key);
    let rb = rustybuzz::Face::from_slice(&face.data, face.face_index()).expect("face");
    let mut buffer = rustybuzz::UnicodeBuffer::new();
    buffer.push_str(row.text);
    let shaped = rustybuzz::shape(&rb, &[], buffer);
    for (k, info) in shaped.glyph_infos().iter().enumerate() {
        let cluster = info.cluster as usize;
        let cluster_pen = run
            .glyphs
            .iter()
            .zip(shaped.glyph_infos())
            .find(|(_, i)| i.cluster as usize == cluster)
            .map_or(0.0, |(g, _)| g.1);
        let css_x = row.chars.get(cluster).copied().unwrap_or(row.rect[0]) + run.glyphs[k].1
            - cluster_pen;
        let dx = css_x * scale;
        let (ix, sub) = match mode {
            XMode::Quarter => {
                let q = (dx * 4.0 + 0.5).floor();
                let ix = (q / 4.0).floor();
                (ix as i32, (q / 4.0 - ix) as f32)
            }
            XMode::Integer => ((dx + 0.5).floor() as i32, 0.0),
            XMode::Exact => (dx.floor() as i32, (dx - dx.floor()) as f32),
        };
        let Some((mut mask, _)) = dw.mask(
            &css.key,
            info.glyph_id as u16,
            (css.size * scale) as f32,
            [1.0, 0.0, 0.0, 1.0],
            (sub, 0.0),
        ) else {
            continue;
        };
        // Skia's SK_MAX_SIZE_FOR_LCDTEXT: above 48 device px LCD becomes A8 generated from LCD.
        if css.size * scale > 48.0 {
            for p in mask.data.chunks_exact_mut(3) {
                let a = ((u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2])) / 3) as u8;
                p.fill(a);
            }
        }
        for my in 0..mask.height as i32 {
            for mx in 0..mask.width as i32 {
                let px = ix + mask.left + mx - box_[0];
                let py = baseline + mask.top + my - box_[1];
                if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                    continue;
                }
                let src = &mask.data[((my * mask.width as i32 + mx) * 3) as usize..][..3];
                let dst = &mut plane[py as usize * w + px as usize];
                for c in 0..3 {
                    let cov = f64::from(src[c]);
                    dst[c] = cov + dst[c] * (1.0 - cov / 255.0);
                }
            }
        }
    }
    Placed {
        box_,
        raw: plane
            .iter()
            .map(|p| p.map(|v| v.round().clamp(0.0, 255.0) as u8))
            .collect(),
    }
}

fn reference_coverage(img: &Rgb, x: i32, y: i32, fg: u8, bg: u8) -> Option<[u8; 3]> {
    if x < 0 || y < 0 || x >= img.width as i32 || y >= img.height as i32 {
        return None;
    }
    let p = img.data[y as usize * img.width as usize + x as usize];
    let span = f64::from(bg) - f64::from(fg);
    Some(p.map(|v| ((f64::from(bg) - f64::from(v)) / span * 255.0).round().clamp(0.0, 255.0) as u8))
}

fn error(img: &Rgb, placed: &Placed, fg: u8, bg: u8) -> u64 {
    let w = (placed.box_[2] - placed.box_[0]) as usize;
    let mut err = 0u64;
    for (i, raw) in placed.raw.iter().enumerate() {
        let x = placed.box_[0] + (i % w) as i32;
        let y = placed.box_[1] + (i / w) as i32;
        if let Some(r) = reference_coverage(img, x, y, fg, bg) {
            for c in 0..3 {
                err += u64::from(raw[c].abs_diff(r[c]));
            }
        }
    }
    err
}

pub fn run(dir: &Path, out: &Path) -> Result<(), String> {
    let mut dw = DWrite::new(Variant {
        grid_fit: true,
        cleartype: true,
    });
    fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let mut samples: Vec<Vec<(u8, u8)>> = vec![Vec::new(); lut::ROWS];
    let mut fits = Vec::new();
    for theme in ["light", "dark"] {
        let (fg, bg) = if theme == "light" { (0x22, 0xff) } else { (0xee, 0x22) };
        for scale in [1u32, 2] {
            let name = format!("dom-text-{theme}@{scale}x");
            let img = pixel::composite(&pixel::read_png(&dir.join(format!("{name}.png")))?, [bg; 3]);
            let scene = json::parse(&fs::read_to_string(dir.join(format!("{name}.json"))).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let mut modes = [0usize; 3];
            let mut placed_rows = Vec::new();
            for row in rows(&scene) {
                let s = f64::from(scale);
                let mut best: Option<(u64, XMode, i32, Placed)> = None;
                for mode in [XMode::Quarter, XMode::Integer, XMode::Exact] {
                    let top = (row.rect[1] * s).floor() as i32;
                    for b in 0..=((row.rect[3] * s).ceil() as i32 + 2) {
                        let placed = place(&mut dw, &row, s, mode, top + b);
                        let e = error(&img, &placed, fg, bg);
                        if best.as_ref().is_none_or(|(be, ..)| e < *be) {
                            best = Some((e, mode, b, placed));
                        }
                    }
                }
                let (_, mode, b, placed) = best.expect("a placement");
                modes[mode as usize] += 1;
                fits.push(format!("{name} {} {:?}: {mode:?} baseline +{b}", row.css, row.text));
                placed_rows.push(placed);
            }
            let bucket = lut::luminance_bucket([fg; 3]);
            for placed in &placed_rows {
                let w = (placed.box_[2] - placed.box_[0]) as usize;
                for (i, raw) in placed.raw.iter().enumerate() {
                    let x = placed.box_[0] + (i % w) as i32;
                    let y = placed.box_[1] + (i / w) as i32;
                    if let Some(r) = reference_coverage(&img, x, y, fg, bg) {
                        for c in 0..3 {
                            if raw[c] != 0 || r[c] != 0 {
                                samples[bucket].push((raw[c], r[c]));
                            }
                        }
                    }
                }
            }
            eprintln!("{name}: x placement quarter/integer/exact = {modes:?}");
            fs::write(out.join(format!("{name}.rows.bin")), encode_rows(&placed_rows)).map_err(|e| e.to_string())?;
        }
    }
    for line in &fits {
        println!("{line}");
    }
    let mut table = [[0u8; 256]; lut::ROWS];
    for (k, s) in samples.iter().enumerate() {
        table[k] = if s.is_empty() { lut::MaskGammaLut::identity().row(k).to_owned() } else { lut::fit_row(s) };
    }
    fs::write(out.join("lut-lcd.bin"), table.iter().flatten().copied().collect::<Vec<u8>>()).map_err(|e| e.to_string())?;
    for theme in ["light", "dark"] {
        let (fg, bg) = if theme == "light" { (0x22u8, 0xffu8) } else { (0xee, 0x22) };
        let row = table[lut::luminance_bucket([fg; 3])];
        for scale in [1u32, 2] {
            let name = format!("dom-text-{theme}@{scale}x");
            let reference = pixel::composite(&pixel::read_png(&dir.join(format!("{name}.png")))?, [bg; 3]);
            let placed_rows = decode_rows(&fs::read(out.join(format!("{name}.rows.bin"))).map_err(|e| e.to_string())?);
            let mut port = reference.clone();
            let mut in_box = vec![false; port.data.len()];
            for placed in &placed_rows {
                let w = (placed.box_[2] - placed.box_[0]) as usize;
                for (i, raw) in placed.raw.iter().enumerate() {
                    let x = placed.box_[0] + (i % w) as i32;
                    let y = placed.box_[1] + (i / w) as i32;
                    if x < 0 || y < 0 || x >= port.width as i32 || y >= port.height as i32 {
                        continue;
                    }
                    let idx = y as usize * port.width as usize + x as usize;
                    in_box[idx] = true;
                    port.data[idx] = raw.map(|r| {
                        let a = f64::from(row[r as usize]) / 255.0;
                        (f64::from(bg) + (f64::from(fg) - f64::from(bg)) * a).round() as u8
                    });
                }
            }
            let cmp = pixel::compare(&port, &reference, [bg; 3])?;
            let boxed = in_box.iter().filter(|&&b| b).count();
            let bad = cmp.mask.iter().zip(&in_box).filter(|(m, b)| **m && **b).count();
            let max_channel = port
                .data
                .iter()
                .zip(&reference.data)
                .zip(&in_box)
                .filter(|(_, b)| **b)
                .map(|((a, b), _)| (0..3).map(|c| a[c].abs_diff(b[c])).max().unwrap_or(0))
                .max()
                .unwrap_or(0);
            let mut grey = 0;
            for p in &port.data {
                if p[0] != p[1] || p[1] != p[2] {
                    grey += 1;
                }
            }
            println!(
                "{name}: text boxes {boxed} px, pixelmatch differing {bad} ({:.3} %), max channel {max_channel}, coloured port px {grey}",
                100.0 * bad as f64 / boxed.max(1) as f64
            );
            pixel::write_png(
                &out.join(format!("{name}.port.png")),
                &pixel::Rgba {
                    width: port.width,
                    height: port.height,
                    data: port.data.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
                },
            )?;
        }
    }
    Ok(())
}

fn encode_rows(rows: &[Placed]) -> Vec<u8> {
    let mut out = Vec::new();
    for r in rows {
        for v in r.box_ {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend(r.raw.iter().flatten());
    }
    out
}

fn decode_rows(mut bytes: &[u8]) -> Vec<Placed> {
    let mut rows = Vec::new();
    while bytes.len() >= 16 {
        let mut box_ = [0i32; 4];
        for (k, v) in box_.iter_mut().enumerate() {
            *v = i32::from_le_bytes(bytes[k * 4..k * 4 + 4].try_into().expect("4 bytes"));
        }
        bytes = &bytes[16..];
        let n = ((box_[2] - box_[0]) * (box_[3] - box_[1])) as usize;
        let raw = bytes[..n * 3].chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        bytes = &bytes[n * 3..];
        rows.push(Placed { box_, raw });
    }
    rows
}
