use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use asrs_parity::json::Json;
use asrs_parity::pixel;
use asrs_parity::vectors as parity_vectors;

use crate::canvas::{electron_with, platform, read_json, write};
use crate::oracle::{self, Paths};
use crate::{Result, cargo};

const USAGE: &str = "usage: cargo xtask ui <command>

UI theme scenes: icon buttons over every upstream symbol in each state, the filter-chain probes
and the hover and active backgrounds, captured from upstream global.css in Electron at 1x and 2x,
light and dark. References land in target/parity/ui/<os>/.

commands:
  ref                 capture the references (scenes/, ref/<scene>@<n>x.{png,json})
  tints [--check]     write parity/vectors/ui/tints-<os>.jsonl.zst from the sampled probes and
                      button backgrounds
  run                 render the scenes with asrs-ui (example ui_scenes, parity feature) into
                      port/ and gate every capture: at most 0.2 % pixelmatch, no blob over 64 px,
                      every opaque probe and background exactly equal, translucent
                      probes within one level";

const PIXEL_MAX_PCT: f64 = 0.2;
const PIXEL_MAX_BLOB: u64 = 64;
const SCALES: [&str; 2] = ["1", "2"];
const TRANSLUCENT_TOLERANCE: u8 = 1;

pub fn run(args: &[String]) -> Result {
    let paths = Paths::new()?;
    let check = args.iter().any(|a| a == "--check");
    match args.first().map(String::as_str) {
        Some("ref") => reference(&paths),
        Some("tints") => tints(&paths, check),
        Some("run") => run_port(&paths),
        _ => Err(USAGE.to_owned()),
    }
}

fn reference_dir(paths: &Paths) -> PathBuf {
    paths.root.join("target/parity/ui").join(platform())
}

const STAMP: &str = "generator.txt";

fn generator_hash(paths: &Paths) -> Result<String> {
    oracle::hash_tree(
        &paths.root,
        &["oracle/src/canvas/ui.ts", "oracle/src/canvasMain.ts"],
    )
}

fn ensure_reference(paths: &Paths) -> Result<PathBuf> {
    let dir = reference_dir(paths);
    let stamp = fs::read_to_string(dir.join(STAMP)).unwrap_or_default();
    if stamp.trim() != generator_hash(paths)? {
        reference(paths)?;
    }
    Ok(dir)
}

fn reference(paths: &Paths) -> Result {
    oracle::build_harness(paths)?;
    let out = reference_dir(paths);
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    for scale in SCALES {
        electron_with(
            paths,
            &[format!("--force-device-scale-factor={scale}")],
            &["ui".to_owned(), out.display().to_string(), scale.to_owned()],
        )?;
    }
    write(
        &out.join(STAMP),
        format!("{}\n", generator_hash(paths)?).as_bytes(),
    )?;
    eprintln!("ui references in {}", out.display());
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

fn samples(capture: &Json) -> Vec<Json> {
    let field = |k: &str| capture.get(k).and_then(Json::as_array).unwrap_or_default();
    let mut out = Vec::new();
    for p in field("probes") {
        out.push(Json::object(
            ["chain", "input", "opacity", "rgba"]
                .map(|k| (k, p.get(k).cloned().unwrap_or(Json::Null))),
        ));
    }
    for (i, rgba) in field("buttons").iter().enumerate() {
        out.push(Json::object([
            ("button", Json::Number(i as f64)),
            ("rgba", rgba.clone()),
        ]));
    }
    out
}

fn tint_records(dir: &Path) -> Result<String> {
    let mut text = String::new();
    for name in scene_names(dir)? {
        for scale in SCALES {
            let capture = read_json(&dir.join(format!("ref/{name}@{scale}x.json")))?;
            for sample in samples(&capture) {
                let Json::Object(pairs) = sample else {
                    unreachable!()
                };
                let mut record = vec![
                    ("scene".to_owned(), Json::string(&name)),
                    (
                        "scale".to_owned(),
                        Json::Number(scale.parse().expect("scale")),
                    ),
                ];
                record.extend(pairs.into_iter().map(|(k, v)| (k.to_string_lossy(), v)));
                let json = Json::object(record.iter().map(|(k, v)| (k.as_str(), v.clone())));
                text.push_str(&json.to_compact());
                text.push('\n');
            }
        }
    }
    Ok(text)
}

fn tints(paths: &Paths, check: bool) -> Result {
    let dir = ensure_reference(paths)?;
    let text = tint_records(&dir)?;
    parity_vectors::parse_records(&text)?;
    let vectors = paths.root.join("parity/vectors/ui");
    let name = format!("tints-{}", platform());
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
        "wrote parity/vectors/ui/{name}.jsonl.zst ({} records)",
        text.lines().count()
    );
    Ok(())
}

fn rgba_of(v: &Json) -> Option<[u8; 4]> {
    let a = v.as_array().filter(|a| a.len() == 4)?;
    let c = |i: usize| a[i].as_f64().map(|x| x as u8);
    Some([c(0)?, c(1)?, c(2)?, c(3)?])
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
        "ui_scenes",
    ])?;
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.root.join("target"));
    let exe = target.join("release/examples/ui_scenes");
    let status = Command::new(&exe)
        .arg(dir.join("scenes"))
        .arg(&port)
        .status()
        .map_err(|e| format!("running {}: {e}", exe.display()))?;
    if !status.success() {
        return Err(format!("ui_scenes failed ({status})"));
    }
    let mut failed = Vec::new();
    eprintln!(
        "{:<22} {:>9} {:>6} {:>4} {:>8} | result",
        "capture", "diff %", "blob", "ch", "samples"
    );
    for name in scene_names(&dir)? {
        let scene = read_json(&dir.join(format!("scenes/{name}.json")))?;
        let background = scene
            .get("background")
            .and_then(rgba3)
            .ok_or_else(|| format!("{name}: bad background"))?;
        for scale in SCALES {
            let capture = format!("{name}@{scale}x");
            let reference = pixel::composite(
                &pixel::read_png(&dir.join(format!("ref/{capture}.png")))?,
                background,
            );
            let ours_rgba = pixel::read_png(&port.join(format!("{capture}.png")))?;
            let ours = pixel::composite(&ours_rgba, background);
            let cmp = pixel::compare(&reference, &ours, background)?;
            let mut failures = Vec::new();
            if cmp.stats.mismatch_pct > PIXEL_MAX_PCT {
                failures.push(format!("{:.3} % differing", cmp.stats.mismatch_pct));
            }
            if cmp.stats.max_blob > PIXEL_MAX_BLOB {
                failures.push(format!("blob of {} px", cmp.stats.max_blob));
            }
            let ref_json = read_json(&dir.join(format!("ref/{capture}.json")))?;
            let port_json = read_json(&port.join(format!("{capture}.json")))?;
            let (a, b) = (samples(&ref_json), samples(&port_json));
            let mut wrong = 0;
            if a.len() != b.len() {
                failures.push(format!("{} samples, port has {}", a.len(), b.len()));
            }
            for (x, y) in a.iter().zip(&b) {
                let translucent = x
                    .get("opacity")
                    .and_then(Json::as_f64)
                    .is_some_and(|o| o < 1.0);
                let tolerance = if translucent {
                    TRANSLUCENT_TOLERANCE
                } else {
                    0
                };
                let (Some(p), Some(q)) = (
                    x.get("rgba").and_then(rgba_of),
                    y.get("rgba").and_then(rgba_of),
                ) else {
                    wrong += 1;
                    continue;
                };
                if (0..4).any(|c| p[c].abs_diff(q[c]) > tolerance) {
                    wrong += 1;
                    if wrong <= 3 {
                        eprintln!("  {capture}: {} vs port {}", x.to_compact(), y.to_compact());
                    }
                }
            }
            if wrong > 0 {
                failures.push(format!("{wrong} samples differ"));
            }
            if cmp.stats.exact_mismatch > 0 {
                fs::create_dir_all(port.join("diff")).map_err(|e| e.to_string())?;
                pixel::write_png(
                    &port.join(format!("diff/{capture}.png")),
                    &pixel::diff_image(&reference, &cmp.mask),
                )?;
            }
            let s = &cmp.stats;
            eprintln!(
                "{capture:<22} {:>9.3} {:>6} {:>4} {:>8} | {}",
                s.mismatch_pct,
                s.max_blob,
                s.max_channel,
                a.len() - wrong,
                if failures.is_empty() {
                    "ok".to_owned()
                } else {
                    format!("FAIL: {}", failures.join("; "))
                }
            );
            if !failures.is_empty() {
                failed.push(capture);
            }
        }
    }
    if failed.is_empty() {
        eprintln!("ui: every capture within its gate");
        Ok(())
    } else {
        Err(format!(
            "ui: {} captures outside their gate: {}",
            failed.len(),
            failed.join(", ")
        ))
    }
}

fn rgba3(v: &Json) -> Option<[u8; 3]> {
    let a = v.as_array().filter(|a| a.len() == 3)?;
    let c = |i: usize| a[i].as_f64().map(|x| x as u8);
    Some([c(0)?, c(1)?, c(2)?])
}
