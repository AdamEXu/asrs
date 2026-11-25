use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use asrs_parity::json::Json;
use asrs_parity::pixel;
use asrs_parity::vectors as parity_vectors;

use crate::canvas::{platform, read_json, write};
use crate::oracle::{self, Paths};
use crate::pixel::antialias;
use crate::pixel::gates::TextInk;
use crate::{Result, cargo};

const USAGE: &str = "usage: cargo xtask text <command>

DOM text scenes for asrs-ui's Text element (oracle/text): every font size, weight and style of
www/global.css and www/hub.css, and the line-height, text-align, direction, white-space and
text-overflow cases those rules use, captured in Electron at 1x and 2x, light and dark.
References land in target/parity/text/<os>/.

commands:
  ref                capture the references (scenes/, ref/<scene>@<n>x.{png,json})
  boxes [--check]    write parity/vectors/text/boxes-<os>.jsonl.zst: every light-scene box with
                     its DOM border box and text client rects, at 1x and 2x
  run                render the scenes with asrs-ui (example text_scenes, parity feature) into
                     port/ and gate every capture: shrink-to-fit box sizes within 0.05 px
                     (1 px for the user agent control font), box
                     and glyph-run edges within 1 px, at most 6 % pixelmatch (YIQ 0.1) inside
                     the text regions (DOM text rects dilated 2 px), and each text rect LCD or
                     grayscale as the reference has it
  scroll             the 10k-row virtual list test in release: cache bounded while every row
                     scrolls past, draw p95 within one 60 Hz frame";

const SCALES: [&str; 2] = ["1", "2"];
const SIZE_TOLERANCE: f64 = 0.05;
/// User agent fonts outside global.css and hub.css (the 13.333px control font): its sizes are
/// held to the layout gate only.
const LOOSE_SIZE_TOLERANCE: f64 = 1.0;
const EDGE_TOLERANCE: f64 = 1.0;
const TEXT_MAX_PCT: f64 = 6.0;
const DILATE: f64 = 2.0;
const ELLIPSIS: &str = "\u{2026}";

pub fn run(args: &[String]) -> Result {
    let paths = Paths::new()?;
    let check = args.iter().any(|a| a == "--check");
    match args.first().map(String::as_str) {
        Some("ref") => reference(&paths),
        Some("boxes") => boxes(&paths, check),
        Some("run") => run_port(&paths),
        Some("scroll") => cargo(&[
            "test",
            "--release",
            "-p",
            "asrs-ui",
            "--features",
            "parity",
            "--test",
            "text_scroll",
        ]),
        _ => Err(USAGE.to_owned()),
    }
}

fn reference_dir(paths: &Paths) -> PathBuf {
    paths.root.join("target/parity/text").join(platform())
}

const STAMP: &str = "generator.txt";

fn generator_hash(paths: &Paths) -> Result<String> {
    oracle::hash_tree(&paths.root, &["oracle/text", "oracle/src/canvas/png.ts"])
}

/// The references' generator and, on Linux, the font profile they were drawn under.
fn stamp(paths: &Paths) -> Result<String> {
    let fonts = std::env::var("FONTCONFIG_FILE").unwrap_or_default();
    Ok(format!("{} {fonts}", generator_hash(paths)?))
}

fn ensure_reference(paths: &Paths) -> Result<PathBuf> {
    let dir = reference_dir(paths);
    let written = fs::read_to_string(dir.join(STAMP)).unwrap_or_default();
    if written.trim() != stamp(paths)?.trim() {
        reference(paths)?;
    }
    Ok(dir)
}

fn exec(command: &mut Command, what: &str) -> Result {
    let status = command
        .status()
        .map_err(|e| format!("running {what}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{what} failed ({status})"))
    }
}

fn reference(paths: &Paths) -> Result {
    exec(
        Command::new("node")
            .arg(paths.oracle_src.join("text/build.mjs"))
            .arg("--harness")
            .arg(&paths.harness)
            .arg("--out")
            .arg(&paths.dist),
        "oracle/text/build.mjs",
    )?;
    let out = reference_dir(paths);
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    for scale in SCALES {
        let mut command = Command::new(paths.electron()?);
        command
            .args([
                "--force-color-profile=srgb",
                "--lang=en-US",
                &format!("--force-device-scale-factor={scale}"),
            ])
            .arg(paths.dist.join("textMain.mjs"))
            .arg(&out)
            .arg(scale)
            .env_remove("ELECTRON_RUN_AS_NODE");
        oracle::pin_environment(&mut command, oracle::DEFAULT_ZONE);
        exec(&mut command, "the text oracle")?;
    }
    write(&out.join(STAMP), format!("{}\n", stamp(paths)?).as_bytes())?;
    eprintln!("text references in {}", out.display());
    Ok(())
}

fn scene_names(dir: &Path) -> Result<Vec<String>> {
    let scenes = dir.join("scenes");
    let mut names = Vec::new();
    for entry in fs::read_dir(&scenes).map_err(|e| format!("{}: {e}", scenes.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

fn list<'a>(v: &'a Json, k: &str) -> &'a [Json] {
    v.get(k).and_then(Json::as_array).unwrap_or_default()
}

fn string(v: &Json, k: &str) -> String {
    v.get(k)
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .unwrap_or_default()
}

fn rect(v: &Json) -> Option<[f64; 4]> {
    let a = v.as_array().filter(|a| a.len() == 4)?;
    Some([
        a[0].as_f64()?,
        a[1].as_f64()?,
        a[2].as_f64()?,
        a[3].as_f64()?,
    ])
}

fn by_id<'a>(boxes: &'a [Json], id: &str) -> Option<&'a Json> {
    boxes.iter().find(|b| string(b, "id") == id)
}

fn box_records(dir: &Path) -> Result<String> {
    let mut text = String::new();
    for name in scene_names(dir)?.iter().filter(|n| n.ends_with("-light")) {
        let scene = read_json(&dir.join(format!("scenes/{name}.json")))?;
        for scale in SCALES {
            let captured = read_json(&dir.join(format!("ref/{name}@{scale}x.json")))?;
            for spec in list(&scene, "boxes") {
                let id = string(spec, "id");
                let dom = by_id(list(&captured, "boxes"), &id)
                    .ok_or_else(|| format!("{name}@{scale}x: no captured box {id}"))?;
                let Json::Object(pairs) = spec.clone() else {
                    return Err(format!("{name}: box {id} is not an object"));
                };
                let mut record = vec![
                    ("scene".to_owned(), Json::string(name)),
                    (
                        "scale".to_owned(),
                        Json::Number(scale.parse().expect("scale")),
                    ),
                ];
                record.extend(pairs.into_iter().map(|(k, v)| (k.to_string_lossy(), v)));
                for k in ["rect", "lines"] {
                    record.push((k.to_owned(), dom.get(k).cloned().unwrap_or(Json::Null)));
                }
                let json = Json::object(record.iter().map(|(k, v)| (k.as_str(), v.clone())));
                text.push_str(&json.to_compact());
                text.push('\n');
            }
        }
    }
    Ok(text)
}

fn boxes(paths: &Paths, check: bool) -> Result {
    let dir = ensure_reference(paths)?;
    let text = box_records(&dir)?;
    parity_vectors::parse_records(&text)?;
    let vectors = paths.root.join("parity/vectors/text");
    let name = format!("boxes-{}", platform());
    if check {
        let committed = parity_vectors::vector_path(&vectors, &name)
            .ok_or_else(|| format!("no committed {name} vector"))
            .and_then(|p| parity_vectors::read_text(&p))?;
        return if committed == text {
            eprintln!("{name}: {} records, identical", text.lines().count());
            Ok(())
        } else {
            Err(format!("regenerated {name} differs from the committed one"))
        };
    }
    write(
        &vectors.join(format!("{name}.jsonl.zst")),
        &parity_vectors::compress(&text),
    )?;
    let info = read_json(&dir.join("info.json"))?;
    let mut fields: Vec<(&str, Json)> = Vec::new();
    for k in ["electron", "chrome", "platform", "arch", "osRelease"] {
        if let Some(v) = info.get(k) {
            fields.push((k, v.clone()));
        }
    }
    fields.push(("generator", Json::string(&generator_hash(paths)?)));
    fields.push(("file", Json::string(&format!("{name}.jsonl.zst"))));
    fields.push(("cases", Json::Number(text.lines().count() as f64)));
    fields.push((
        "content",
        Json::string(&oracle::hash_bytes(text.as_bytes())),
    ));
    write(
        &vectors.join(format!("{}.json", platform())),
        format!("{}\n", Json::object(fields).to_pretty()).as_bytes(),
    )?;
    eprintln!(
        "wrote parity/vectors/text/{name}.jsonl.zst ({} records)",
        text.lines().count()
    );
    Ok(())
}

fn edges_within(a: [f64; 4], b: [f64; 4]) -> bool {
    let edges = |r: [f64; 4]| [r[0], r[1], r[0] + r[2], r[1] + r[3]];
    edges(a)
        .iter()
        .zip(edges(b))
        .all(|(p, q)| (p - q).abs() <= EDGE_TOLERANCE)
}

#[derive(Default)]
struct Gates {
    failures: Vec<String>,
    max_size_error: f64,
    max_loose_error: f64,
}

/// Box size, box edges and glyph-run edges of one box against the DOM.
fn gate_box(spec: &Json, dom: &Json, port: &Json, gates: &mut Gates) {
    let id = string(spec, "id");
    let (Some(dom_rect), Some(bounds)) = (
        dom.get("rect").and_then(rect),
        port.get("bounds").and_then(rect),
    ) else {
        gates.failures.push(format!("{id}: missing rects"));
        return;
    };
    let exact = !matches!(spec.get("exact"), Some(Json::Bool(false)));
    for (k, i) in [("width", 2), ("height", 3)] {
        if matches!(spec.get(k), Some(Json::Null) | None) {
            let ours = port.get(k).and_then(Json::as_f64).unwrap_or(f64::NAN);
            let err = (ours - dom_rect[i]).abs();
            let (max, tolerance) = if exact {
                (&mut gates.max_size_error, SIZE_TOLERANCE)
            } else {
                (&mut gates.max_loose_error, LOOSE_SIZE_TOLERANCE)
            };
            *max = max.max(err);
            if err.is_nan() || err > tolerance {
                gates
                    .failures
                    .push(format!("{id}: {k} {ours} vs DOM {}", dom_rect[i]));
            }
        }
    }
    if !edges_within(bounds, dom_rect) {
        gates
            .failures
            .push(format!("{id}: box {bounds:?} vs DOM {dom_rect:?}"));
    }
    let dom_runs: Vec<[f64; 4]> = list(dom, "lines")
        .iter()
        .filter_map(rect)
        .filter(|r| r[2] > 0.0)
        .collect();
    let ours: Vec<(String, [f64; 4])> = list(port, "fragments")
        .iter()
        .filter_map(|f| Some((string(f, "text"), f.get("rect").and_then(rect)?)))
        .filter(|(t, r)| t != ELLIPSIS && r[2] > 0.0)
        .collect();
    for (t, r) in &ours {
        if !dom_runs.iter().any(|d| edges_within(*r, *d)) {
            gates
                .failures
                .push(format!("{id}: run {t:?} at {r:?} matches no DOM text rect"));
        }
    }
    let inside = |r: &[f64; 4]| {
        r[0] >= dom_rect[0] - EDGE_TOLERANCE
            && r[0] + r[2] <= dom_rect[0] + dom_rect[2] + EDGE_TOLERANCE
    };
    for d in dom_runs.iter().filter(|d| inside(d)) {
        if !ours.iter().any(|(_, r)| edges_within(*r, *d)) {
            gates
                .failures
                .push(format!("{id}: DOM text rect {d:?} has no run"));
        }
    }
}

/// The DOM text rects of every box, clipped to boxes that hide overflow, dilated, in device
/// pixels.
fn text_mask(scene: &Json, dom: &Json, scale: f64, width: usize, height: usize) -> Vec<bool> {
    let mut mask = vec![false; width * height];
    for spec in list(scene, "boxes") {
        let Some(b) = by_id(list(dom, "boxes"), &string(spec, "id")) else {
            continue;
        };
        let Some(r) = b.get("rect").and_then(rect) else {
            continue;
        };
        let clip = string(spec, "overflow") != "visible";
        for t in list(b, "lines").iter().filter_map(rect) {
            if t[2] <= 0.0 {
                continue;
            }
            let (mut x0, mut y0, mut x1, mut y1) = (t[0], t[1], t[0] + t[2], t[1] + t[3]);
            if clip {
                x0 = x0.max(r[0]);
                y0 = y0.max(r[1]);
                x1 = x1.min(r[0] + r[2]);
                y1 = y1.min(r[1] + r[3]);
            }
            let to = |v: f64, max: usize| ((v * scale).round().max(0.0) as usize).min(max);
            let (x0, x1) = (to(x0 - DILATE, width), to(x1 + DILATE, width));
            let (y0, y1) = (to(y0 - DILATE, height), to(y1 + DILATE, height));
            for y in y0..y1 {
                mask[y * width + x0..y * width + x1].fill(true);
            }
        }
    }
    mask
}

fn rgb3(v: &Json) -> Option<[u8; 3]> {
    let a = v.as_array().filter(|a| a.len() == 3)?;
    let c = |i: usize| a[i].as_f64().map(|x| x as u8);
    Some([c(0)?, c(1)?, c(2)?])
}

fn hex_color(css: &str) -> Option<[u8; 3]> {
    let rgb = u32::from_str_radix(css.strip_prefix('#')?, 16).ok()?;
    (css.len() == 7).then_some([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8])
}

fn run_port(paths: &Paths) -> Result {
    let dir = ensure_reference(paths)?;
    let port = dir.join("port");
    if port.exists() {
        fs::remove_dir_all(&port).map_err(|e| format!("{}: {e}", port.display()))?;
    }
    fs::create_dir_all(&port).map_err(|e| format!("{}: {e}", port.display()))?;
    cargo(&[
        "build",
        "--quiet",
        "--release",
        "-p",
        "asrs-ui",
        "--features",
        "parity",
        "--example",
        "text_scenes",
    ])?;
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.root.join("target"));
    let exe = target.join("release/examples/text_scenes");
    exec(
        Command::new(&exe).arg(dir.join("scenes")).arg(&port),
        "text_scenes",
    )?;
    let mut failed = Vec::new();
    eprintln!(
        "{:<24} {:>8} {:>8} {:>9} {:>9} {:>6} {:>5} {:>5} {:>5} | result",
        "capture", "size err", "UA err", "text %", "whole %", "blob", "lcd", "gray", "none"
    );
    for name in scene_names(&dir)? {
        let scene = read_json(&dir.join(format!("scenes/{name}.json")))?;
        let background = scene
            .get("background")
            .and_then(rgb3)
            .ok_or_else(|| format!("{name}: bad background"))?;
        for scale in SCALES {
            let capture = format!("{name}@{scale}x");
            let dom = read_json(&dir.join(format!("ref/{capture}.json")))?;
            let ours = read_json(&port.join(format!("{capture}.json")))?;
            let mut gates = Gates::default();
            for spec in list(&scene, "boxes") {
                let id = string(spec, "id");
                match (
                    by_id(list(&dom, "boxes"), &id),
                    by_id(list(&ours, "boxes"), &id),
                ) {
                    (Some(d), Some(p)) => gate_box(spec, d, p, &mut gates),
                    _ => gates.failures.push(format!("{id}: not captured")),
                }
            }
            let reference = pixel::composite(
                &pixel::read_png(&dir.join(format!("ref/{capture}.png")))?,
                background,
            );
            let mine = pixel::composite(
                &pixel::read_png(&port.join(format!("{capture}.png")))?,
                background,
            );
            let inks: Vec<TextInk> = list(&scene, "boxes")
                .iter()
                .flat_map(|spec| {
                    let color = hex_color(&string(spec, "color"));
                    let dom = by_id(list(&dom, "boxes"), &string(spec, "id"));
                    let lines = dom.map_or(&[][..], |dom| list(dom, "lines"));
                    let rects = lines.iter().filter_map(rect);
                    rects.filter_map(move |rect| {
                        Some(TextInk {
                            rect,
                            color: color?,
                        })
                    })
                })
                .collect();
            let device = scale.parse().expect("scale");
            let modes = antialias::compare(inks.iter(), &reference, &mine, device);
            for (rect, expected, actual) in &modes.differing {
                gates.failures.push(format!(
                    "{rect:?} is {expected:?} upstream and {actual:?} here"
                ));
            }
            let cmp = pixel::compare(&reference, &mine, background)?;
            let (w, h) = (reference.width as usize, reference.height as usize);
            let mask = text_mask(&scene, &dom, device, w, h);
            let region = mask.iter().filter(|&&m| m).count().max(1);
            let differing = mask
                .iter()
                .zip(&cmp.mask)
                .filter(|(m, d)| **m && **d)
                .count();
            let text_pct = 100.0 * differing as f64 / region as f64;
            if text_pct > TEXT_MAX_PCT {
                gates
                    .failures
                    .push(format!("{text_pct:.3} % of text-region pixels differ"));
            }
            if cmp.stats.exact_mismatch > 0 {
                fs::create_dir_all(port.join("diff")).map_err(|e| e.to_string())?;
                pixel::write_png(
                    &port.join(format!("diff/{capture}.png")),
                    &pixel::diff_image(&reference, &cmp.mask),
                )?;
            }
            eprintln!(
                "{capture:<24} {:>8.4} {:>8.4} {:>9.3} {:>9.3} {:>6} {:>5} {:>5} {:>5} | {}",
                gates.max_size_error,
                gates.max_loose_error,
                text_pct,
                cmp.stats.mismatch_pct,
                cmp.stats.max_blob,
                modes.lcd,
                modes.grayscale,
                modes.unclear,
                if gates.failures.is_empty() {
                    "ok".to_owned()
                } else {
                    format!("FAIL: {}", gates.failures.join("; "))
                }
            );
            if !gates.failures.is_empty() {
                failed.push(capture);
            }
        }
    }
    if failed.is_empty() {
        eprintln!("text: every capture within its gates");
        Ok(())
    } else {
        Err(format!(
            "text: {} captures outside their gates: {}",
            failed.len(),
            failed.join(", ")
        ))
    }
}
