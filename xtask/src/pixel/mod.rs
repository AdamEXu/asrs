mod gates;
mod plant;
mod report;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use asrs_parity::json::{self, Json};
use asrs_parity::pixel::{self, Rgba};

use crate::{Result, app, cargo, workspace_root};
use gates::{Capture, Dump, GlDump, Outcome, Patch, PlatformMask, Rect, RenderMode};

const USAGE: &str = r"usage: cargo xtask pixel <command>

The pixel gates of PLAN section 2.5 between an upstream run (`cargo xtask app run`, under
target/parity/app/<label>) and a port run (`asrs --parity-scenario`, under
target/parity/port/<label>). Both sides write the same layout: <scenario>/<scale>x-<theme>/
with run.json and captures/<name>.{png,json} plus canvas and 3D dumps.

  compare <upstream> <port> [--report <dir>]
                  every capture of every variant: ICC to sRGB normalization of upstream
                  captures recorded as display.icc, masks (both records plus parity/masks.json
                  for the run's platform), gate 1 (probed boxes within 1 px), gate 2 (0.2 %
                  and no blob over 64 px outside text, window and canvas dumps), gate 3 (text
                  boxes dilated 2 px: at most 6 %, glyph runs within 1 px), gate 4 (3D dumps
                  through asrs_parity::pixel3d) and the colour-patch rule. Writes report.html
                  (default target/parity/pixel/report)
  port [<scenario>...] [--scale 1|2]... [--theme light|dark]... [--label <label>]
                  build asrs with the parity feature and run each scenario headless into
                  target/parity/port/<label> (default `run`)
  check           the port's exit criteria: colour-patch and hub-empty at 1x and 2x, every
                  capture the scenario's window size times the scale, every patch exact
  selftest [<upstream variant dir>...]
                  identical inputs pass and each planted fault fails: a 1 px shift, a
                  missing icon, a P3 capture left unnormalized, an interior 3D shading change
                  of 3 levels; synthetic captures, then each given real upstream variant";

pub fn run(args: &[String]) -> Result {
    match args.split_first() {
        Some((cmd, rest)) if cmd == "compare" => compare_cmd(rest),
        Some((cmd, rest)) if cmd == "port" => {
            let options = PortOptions::parse(rest)?;
            run_port(&options).map(|_| ())
        }
        Some((cmd, [])) if cmd == "check" => check(),
        Some((cmd, rest)) if cmd == "selftest" => plant::selftest(rest),
        _ => Err(USAGE.to_owned()),
    }
}

fn parity_dir() -> PathBuf {
    workspace_root().join("target/parity")
}

fn resolve(arg: &str, kind: &str) -> PathBuf {
    let dir = PathBuf::from(arg);
    if dir.is_dir() {
        dir
    } else {
        parity_dir().join(kind).join(arg)
    }
}

fn compare_cmd(args: &[String]) -> Result {
    let mut positional = Vec::new();
    let mut report_dir = parity_dir().join("pixel/report");
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--report" {
            report_dir = PathBuf::from(iter.next().ok_or("--report needs a directory")?);
        } else if arg.starts_with("--") {
            return Err(format!("unknown option {arg}\n\n{USAGE}"));
        } else {
            positional.push(arg.as_str());
        }
    }
    let [reference, port] = positional[..] else {
        return Err(USAGE.to_owned());
    };
    let results = compare_roots(&resolve(reference, "app"), &resolve(port, "port"))?;
    let html = report::write(&report_dir, &results)?;
    summarize(&results, &html)
}

fn summarize(results: &[CaptureResult], html: &Path) -> Result {
    let mut failed = 0;
    for r in results {
        if r.pass() {
            eprintln!("  pass {}", r.label);
            continue;
        }
        failed += 1;
        eprintln!("FAIL {}", r.label);
        if let Some(e) = &r.error {
            eprintln!("    {e}");
        }
        for c in r.outcome.failures() {
            eprintln!("    gate {} {}: {}", c.gate, c.what, c.detail);
        }
    }
    eprintln!("report: {}", html.display());
    if results.is_empty() {
        Err("nothing compared".to_owned())
    } else if failed == 0 {
        eprintln!("pixel: {} captures pass", results.len());
        Ok(())
    } else {
        Err(format!(
            "pixel: {failed} of {} captures fail",
            results.len()
        ))
    }
}

pub(crate) struct CaptureResult {
    pub label: String,
    pub outcome: Outcome,
    pub error: Option<String>,
    pub reference: Option<Rgba>,
    pub port: Option<Rgba>,
}

impl CaptureResult {
    pub fn pass(&self) -> bool {
        self.error.is_none() && self.outcome.pass()
    }

    fn failed(label: String, error: String) -> Self {
        Self {
            label,
            outcome: Outcome::default(),
            error: Some(error),
            reference: None,
            port: None,
        }
    }
}

fn read_json(path: &Path) -> Result<Json> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn string(value: &Json, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Json::as_str)
        .map(ToString::to_string)
}

fn rect(value: &Json) -> Option<Rect> {
    let a = value.as_array().filter(|a| a.len() == 4)?;
    Some([
        a[0].as_f64()?,
        a[1].as_f64()?,
        a[2].as_f64()?,
        a[3].as_f64()?,
    ])
}

fn list<'a>(value: &'a Json, key: &str) -> &'a [Json] {
    value.get(key).and_then(Json::as_array).unwrap_or(&[])
}

fn field_rect(value: &Json, key: &str, context: &str) -> Result<Rect> {
    value
        .get(key)
        .and_then(rect)
        .ok_or_else(|| format!("{context}: no {key} rect"))
}

pub(crate) fn variant_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    if root.join("run.json").exists() {
        return Ok(vec![root.to_path_buf()]);
    }
    let mut dirs = Vec::new();
    for scenario in sorted_entries(root)? {
        if scenario.is_dir() {
            for variant in sorted_entries(&scenario)? {
                if variant.join("run.json").exists() {
                    dirs.push(variant);
                }
            }
        }
    }
    Ok(dirs)
}

fn sorted_entries(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    Ok(entries)
}

pub(crate) fn capture_names(variant: &Path) -> Result<Vec<String>> {
    Ok(sorted_entries(&variant.join("captures"))?
        .iter()
        .filter_map(|e| {
            e.file_name()?
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_owned)
        })
        .collect())
}

fn image_of(variant: &Path, record: &Json, name: &str, port: bool) -> Result<Rgba> {
    let captures = variant.join("captures");
    let file = string(record, "image").unwrap_or_else(|| format!("{name}.png"));
    let path = captures.join(&file);
    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let image = pixel::decode_png(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let embedded = pixel::png_icc_profile(&bytes)?;
    let space = string(record, "colourSpace").unwrap_or_default();
    let image = match (space.as_str(), port) {
        ("srgb", _) if embedded.is_some() => {
            return Err(format!(
                "{}: the PNG carries an ICC profile but the record says srgb",
                path.display()
            ));
        }
        ("srgb", _) => image,
        ("display.icc", false) => {
            let icc_path = variant.join("display.icc");
            let icc = fs::read(&icc_path).map_err(|e| {
                format!(
                    "{name}: recorded in the display's colour space, but {}: {e}",
                    icc_path.display()
                )
            })?;
            app::normalize(&image, Some(&icc))?
        }
        (other, true) => {
            return Err(format!(
                "{name}: the port must capture sRGB, its record says {other:?}"
            ));
        }
        (other, false) => return Err(format!("{name}: unknown colour space {other:?}")),
    };
    let size = (
        record.get("width").and_then(Json::as_f64),
        record.get("height").and_then(Json::as_f64),
    );
    if size != (Some(f64::from(image.width)), Some(f64::from(image.height))) {
        return Err(format!(
            "{name}: image is {}x{}, record says {size:?}",
            image.width, image.height
        ));
    }
    Ok(image)
}

fn render_mode(value: Option<String>) -> Result<RenderMode> {
    match value.as_deref() {
        None | Some("standard") => Ok(RenderMode::Standard),
        Some("low-power") => Ok(RenderMode::LowPower),
        Some("cinematic") => Ok(RenderMode::Cinematic),
        Some(other) => Err(format!("unknown 3D render mode {other:?}")),
    }
}

pub(crate) fn edges_from_png(image: &Rgba) -> Vec<bool> {
    image
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| p[0] > 127)
        .collect()
}

pub(crate) fn load_capture(variant: &Path, name: &str, port: bool) -> Result<Capture> {
    let captures = variant.join("captures");
    let record = read_json(&captures.join(format!("{name}.json")))?;
    let scale = record
        .get("scale")
        .and_then(Json::as_f64)
        .ok_or_else(|| format!("{name}: no scale"))?;
    let image = image_of(variant, &record, name, port)?;
    let probes = match record.get("probes") {
        Some(Json::Object(pairs)) => pairs
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    v.as_array()
                        .unwrap_or(&[])
                        .iter()
                        .filter_map(rect)
                        .collect(),
                )
            })
            .collect(),
        _ => Vec::new(),
    };
    let texts = list(&record, "texts")
        .iter()
        .flat_map(|t| list(t, "rects").iter().filter_map(rect))
        .collect();
    let masks = list(&record, "masks")
        .iter()
        .map(|m| {
            Ok((
                string(m, "reason").unwrap_or_default(),
                field_rect(m, "rect", name)?,
            ))
        })
        .collect::<Result<_>>()?;
    let read = |file: &str| pixel::read_png(&captures.join(file));
    let canvases = list(&record, "canvases")
        .iter()
        .map(|c| {
            let file = string(c, "file").ok_or_else(|| format!("{name}: canvas without a file"))?;
            Ok(Dump {
                rect: field_rect(c, "rect", &file)?,
                image: read(&file)?,
                file,
            })
        })
        .collect::<Result<_>>()?;
    let gl = list(&record, "gl")
        .iter()
        .map(|g| {
            let file =
                string(g, "file").ok_or_else(|| format!("{name}: 3D dump without a file"))?;
            let edges = match string(g, "edges") {
                Some(edges) => Some(edges_from_png(&read(&edges)?)),
                None => None,
            };
            Ok(GlDump {
                rect: field_rect(g, "rect", &file)?,
                image: read(&file)?,
                edges,
                mode: render_mode(string(g, "mode"))?,
                file,
            })
        })
        .collect::<Result<_>>()?;
    let patches = match record.get("patches") {
        Some(Json::Array(items)) => Some(
            items
                .iter()
                .map(|p| {
                    let rgb = p
                        .get("rgb")
                        .and_then(Json::as_array)
                        .filter(|a| a.len() == 3)
                        .and_then(|a| {
                            let c = |i: usize| a[i].as_f64().map(|v| v as u8);
                            Some([c(0)?, c(1)?, c(2)?])
                        })
                        .ok_or_else(|| format!("{name}: patch without an rgb triple"))?;
                    Ok(Patch {
                        rgb,
                        rect: field_rect(p, "rect", name)?,
                    })
                })
                .collect::<Result<_>>()?,
        ),
        _ => None,
    };
    Ok(Capture {
        scale,
        image,
        probes,
        texts,
        masks,
        canvases,
        gl,
        patches,
    })
}

pub(crate) fn platform_masks(platform: &str) -> Result<Vec<PlatformMask>> {
    let path = workspace_root().join("parity/masks.json");
    let all = read_json(&path)?;
    let Some(entries) = all.get(platform).and_then(Json::as_array) else {
        return Err(format!(
            "{}: no masks for platform {platform:?}",
            path.display()
        ));
    };
    entries
        .iter()
        .map(|m| {
            let reason = string(m, "reason").unwrap_or_default();
            let index = |key: &str| m.get(key).and_then(Json::as_f64).map(|v| v as i64);
            if let Some(r) = m.get("rect").and_then(rect) {
                Ok(PlatformMask::Rect(reason, r))
            } else if m.get("transparent") == Some(&Json::Bool(true)) {
                Ok(PlatformMask::Transparent(reason))
            } else if let Some(row) = index("deviceRow") {
                Ok(PlatformMask::DeviceRow(reason, row))
            } else if let Some(column) = index("deviceColumn") {
                Ok(PlatformMask::DeviceColumn(reason, column))
            } else {
                Err(format!("{}: mask {reason:?} has no shape", path.display()))
            }
        })
        .collect()
}

fn run_platform(variant: &Path) -> Result<String> {
    string(&read_json(&variant.join("run.json"))?, "platform")
        .ok_or_else(|| format!("{}: run.json records no platform", variant.display()))
}

pub(crate) fn compare_variant(
    label: &str,
    reference: &Path,
    port: &Path,
) -> Result<Vec<CaptureResult>> {
    let platform = run_platform(reference)?;
    let port_platform = run_platform(port)?;
    if platform != port_platform {
        return Ok(vec![CaptureResult::failed(
            label.to_owned(),
            format!("upstream ran on {platform}, the port on {port_platform}"),
        )]);
    }
    let masks = platform_masks(&platform)?;
    let names = capture_names(reference)?;
    let mut results = Vec::new();
    for extra in capture_names(port)?
        .into_iter()
        .filter(|n| !names.contains(n))
    {
        results.push(CaptureResult::failed(
            format!("{label} {extra}"),
            "captured by the port only".into(),
        ));
    }
    if names.is_empty() {
        results.push(CaptureResult::failed(
            label.to_owned(),
            "no upstream captures".into(),
        ));
    }
    for name in names {
        let label = format!("{label} {name}");
        let loaded = load_capture(reference, &name, false)
            .and_then(|r| Ok((r, load_capture(port, &name, true)?)));
        results.push(match loaded {
            Ok((r, p)) => CaptureResult {
                label,
                outcome: gates::compare(&r, &p, &masks),
                error: None,
                reference: Some(r.image),
                port: Some(p.image),
            },
            Err(e) => CaptureResult::failed(label, e),
        });
    }
    Ok(results)
}

pub(crate) fn compare_roots(reference: &Path, port: &Path) -> Result<Vec<CaptureResult>> {
    let mut results = Vec::new();
    let variants = variant_dirs(reference)?;
    if variants.is_empty() {
        return Err(format!("{}: no upstream variants", reference.display()));
    }
    for variant in variants {
        let rel = variant
            .strip_prefix(reference)
            .expect("variant under its root");
        let label = if rel.as_os_str().is_empty() {
            variant
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            rel.display().to_string()
        };
        let theirs = port.join(rel);
        if !theirs.join("run.json").exists() {
            results.push(CaptureResult::failed(
                label,
                format!("no port run at {}", theirs.display()),
            ));
            continue;
        }
        results.extend(compare_variant(&label, &variant, &theirs)?);
    }
    Ok(results)
}

#[derive(Default)]
struct PortOptions {
    scenarios: Vec<String>,
    flags: Vec<String>,
    label: String,
}

impl PortOptions {
    fn parse(args: &[String]) -> Result<Self> {
        let mut options = PortOptions {
            label: "run".to_owned(),
            ..Default::default()
        };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--scale" | "--theme" => {
                    let value = iter.next().ok_or(format!("{arg} needs a value"))?;
                    options.flags.extend([arg.clone(), value.clone()]);
                }
                "--label" => options.label = iter.next().ok_or("--label needs a value")?.clone(),
                flag if flag.starts_with("--") => {
                    return Err(format!("unknown option {flag}\n\n{USAGE}"));
                }
                name => options
                    .scenarios
                    .push(name.trim_end_matches(".json").to_owned()),
            }
        }
        Ok(options)
    }
}

fn port_binary() -> Result<PathBuf> {
    cargo(&[
        "build",
        "--release",
        "--locked",
        "-p",
        "asrs-app",
        "--features",
        "parity",
        "--bin",
        "asrs",
    ])?;
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"));
    Ok(target
        .join("release")
        .join(format!("asrs{}", std::env::consts::EXE_SUFFIX)))
}

fn run_port(options: &PortOptions) -> Result<PathBuf> {
    let exe = port_binary()?;
    let scenarios = workspace_root().join("parity/scenarios");
    let mut names: Vec<String> = sorted_entries(&scenarios)?
        .iter()
        .filter_map(|p| {
            p.file_name()?
                .to_str()?
                .strip_suffix(".json")
                .map(str::to_owned)
        })
        .collect();
    if let Some(missing) = options.scenarios.iter().find(|n| !names.contains(n)) {
        return Err(format!("no scenario {missing} in {}", scenarios.display()));
    }
    if !options.scenarios.is_empty() {
        names.retain(|n| options.scenarios.contains(n));
    }
    let root = parity_dir().join("port").join(&options.label);
    let mut failures = Vec::new();
    for name in names {
        let out = root.join(&name);
        if out.exists() {
            fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        }
        let status = Command::new(&exe)
            .arg("--parity-scenario")
            .arg(scenarios.join(format!("{name}.json")))
            .arg("--out")
            .arg(&out)
            .args(&options.flags)
            .current_dir(workspace_root())
            .status()
            .map_err(|e| format!("running {}: {e}", exe.display()))?;
        if !status.success() {
            failures.push(format!("{name} ({status})"));
        }
    }
    if failures.is_empty() {
        Ok(root)
    } else {
        Err(format!("port scenarios failed: {}", failures.join(", ")))
    }
}

fn check() -> Result {
    let root = run_port(&PortOptions {
        scenarios: vec!["colour-patch".to_owned(), "hub-empty".to_owned()],
        label: "check".to_owned(),
        ..Default::default()
    })?;
    let mut failures = Vec::new();
    let mut scales_seen = Vec::new();
    for variant in variant_dirs(&root)? {
        let label = variant
            .strip_prefix(&root)
            .expect("under root")
            .display()
            .to_string();
        let run = read_json(&variant.join("run.json"))?;
        let window = run.get("window").ok_or("run.json has no window")?;
        let dim = |key: &str| window.get(key).and_then(Json::as_f64).unwrap_or(f64::NAN);
        for name in capture_names(&variant)? {
            let capture = load_capture(&variant, &name, true)?;
            let expected = (dim("width") * capture.scale, dim("height") * capture.scale);
            let actual = (
                f64::from(capture.image.width),
                f64::from(capture.image.height),
            );
            eprintln!(
                "  {label} {name}: {}x{} at {}x",
                actual.0, actual.1, capture.scale
            );
            if actual != expected {
                failures.push(format!("{label} {name}: {actual:?}, expected {expected:?}"));
            }
            scales_seen.push(capture.scale);
            if let Some(patches) = &capture.patches {
                let outcome = gates::compare(&capture, &capture, &[]);
                for c in outcome.checks.iter().filter(|c| c.gate == "colour patch") {
                    eprintln!("    {} {}: {}", c.gate, c.what, c.detail);
                }
                if patches.len() != 512 || !outcome.pass() {
                    failures.push(format!(
                        "{label} {name}: {} patches, not all exact",
                        patches.len()
                    ));
                }
            }
        }
    }
    for scale in [1.0, 2.0] {
        if !scales_seen.contains(&scale) {
            failures.push(format!("no capture at {scale}x"));
        }
    }
    if failures.is_empty() {
        eprintln!("pixel check: passed");
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}
