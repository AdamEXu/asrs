mod gates;
mod plant;
mod report;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use asrs_parity::json::{self, Json};
use asrs_parity::pixel::{self, Rgba};

use crate::{Result, app, cargo, workspace_root};
pub(crate) use gates::TIMELINE_TOLERANCE;
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
                  through asrs_parity::pixel3d) and the colour-patch rule. Every upstream
                  variant also needs a passing colour-patch capture from the same run at the
                  same platform, scale and colour settings, and a variant on one side only
                  fails. Writes report.html (default target/parity/pixel/report)
  port [<scenario>...] [--scale 1|2]... [--theme light|dark]... [--label <label>]
       [--upstream <label>]
                  build asrs with the parity feature and run each scenario headless into
                  target/parity/port/<label> (default `run`); logs a scenario opens as
                  `app:<name>` come from the inputs/ the Electron driver wrote for that
                  scenario under target/parity/app/<upstream label> (default: the port label)
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
    for child in sorted_entries(root)? {
        if child.join("run.json").exists() {
            dirs.push(child);
        } else if child.is_dir() {
            for variant in sorted_entries(&child)? {
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

/// Capture names are `[a-z0-9-]`, so a dotted stem (`<name>.hub-state.json`) is a capture's
/// side file.
pub(crate) fn capture_names(variant: &Path) -> Result<Vec<String>> {
    Ok(sorted_entries(&variant.join("captures"))?
        .iter()
        .filter_map(|e| {
            e.file_name()?
                .to_str()?
                .strip_suffix(".json")
                .filter(|stem| !stem.contains('.'))
                .map(str::to_owned)
        })
        .collect())
}

/// Whether an upstream run kept the display's colour space (`--colour display`), from the
/// profile source the Electron driver records.
fn display_colour_run(variant: &Path) -> Result<bool> {
    let run = read_json(&variant.join("run.json"))?;
    let source = run
        .get("displayProfile")
        .and_then(|p| string(p, "source"))
        .ok_or_else(|| format!("{}: run.json records no display profile", variant.display()))?;
    Ok(!matches!(source.as_str(), "none" | "forced-srgb"))
}

fn image_of(variant: &Path, record: &Json, name: &str, port: bool) -> Result<(Rgba, bool)> {
    let captures = variant.join("captures");
    let file = string(record, "image").unwrap_or_else(|| format!("{name}.png"));
    let path = captures.join(&file);
    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let image = pixel::decode_png(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    let embedded = pixel::png_icc_profile(&bytes)?;
    let space = string(record, "colourSpace").unwrap_or_default();
    let normalized = space == "display.icc";
    let image = match (space.as_str(), port) {
        ("srgb", _) if embedded.is_some() => {
            return Err(format!(
                "{}: the PNG carries an ICC profile but the record says srgb",
                path.display()
            ));
        }
        ("srgb", false) if display_colour_run(variant)? => {
            return Err(format!(
                "{name}: the run captured in the display's colour space but the record says srgb"
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
    Ok((image, normalized))
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
    let (image, normalized) = image_of(variant, &record, name, port)?;
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
    let hub_state = match string(&record, "hubState") {
        Some(file) => Some(
            fs::read(captures.join(&file))
                .map_err(|e| format!("{}: {e}", captures.join(&file).display()))?,
        ),
        None => None,
    };
    let timeline_range = match record.get("timelineRange") {
        None | Some(Json::Null) => None,
        Some(range) => Some(
            range
                .as_array()
                .filter(|a| a.len() == 2)
                .and_then(|a| Some([a[0].as_f64()?, a[1].as_f64()?]))
                .ok_or_else(|| format!("{name}: timelineRange is not two numbers"))?,
        ),
    };
    Ok(Capture {
        scale,
        image,
        normalized,
        probes,
        texts,
        masks,
        canvases,
        gl,
        patches,
        hub_state,
        timeline_range,
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
    let mut results = vec![compare_files(label, reference, port)?];
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

/// `run.json`'s `files`: (path under files/, changed from its seed).
fn written_files(variant: &Path) -> Result<Vec<(String, bool)>> {
    let run = read_json(&variant.join("run.json"))?;
    Ok(list(&run, "files")
        .iter()
        .filter_map(|f| {
            let changed = f.get("changed") != Some(&Json::Bool(false));
            Some((string(f, "file")?, changed))
        })
        .collect())
}

/// Every file the port wrote equals the file upstream wrote at the same path, and every file
/// upstream wrote the port wrote too.
fn compare_files(label: &str, reference: &Path, port: &Path) -> Result<CaptureResult> {
    let ours = written_files(port)?;
    let theirs = written_files(reference)?;
    let mut outcome = Outcome::default();
    for (file, _) in ours.iter().filter(|(_, changed)| *changed) {
        let read = |side: &Path| fs::read(side.join("files").join(file)).ok();
        let upstream_wrote = theirs.iter().any(|(f, changed)| f == file && *changed);
        let (pass, detail) = match (upstream_wrote, read(reference), read(port)) {
            (true, Some(expected), Some(actual)) if actual == expected => {
                (true, format!("{} bytes equal", actual.len()))
            }
            (true, Some(expected), Some(actual)) => (
                false,
                format!(
                    "{} bytes upstream, {} on the port, contents differ",
                    expected.len(),
                    actual.len()
                ),
            ),
            (true, _, _) => (false, "listed but not copied into files/".to_owned()),
            (false, _, _) => (false, "written by the port only".to_owned()),
        };
        outcome.push("files", file, pass, detail);
    }
    let unwritten = theirs
        .iter()
        .filter(|(f, changed)| *changed && !ours.iter().any(|(o, c)| o == f && *c))
        .map(|(f, _)| f.as_str());
    for file in unwritten {
        outcome.push(
            "files",
            file,
            false,
            "written by upstream, not by the port".into(),
        );
    }
    if outcome.checks.is_empty() {
        outcome.push(
            "files",
            "written files",
            true,
            "neither side wrote a file".into(),
        );
    }
    Ok(CaptureResult {
        label: format!("{label} files"),
        outcome,
        error: None,
        reference: None,
        port: None,
    })
}

fn colour_key(variant: &Path) -> Result<String> {
    let run = read_json(&variant.join("run.json"))?;
    let field = |v: Option<&Json>| match v {
        Some(Json::String(s)) => s.to_string(),
        Some(Json::Number(n)) => n.to_string(),
        _ => "?".to_owned(),
    };
    Ok(format!(
        "{} {}x colour {} profile {}",
        field(run.get("platform")),
        field(run.get("scale")),
        field(run.get("colour")),
        field(run.get("displayProfile").and_then(|p| p.get("source")))
    ))
}

/// Upstream colour-patch results per colour key. A grey UI hides a capture that is still in the
/// display's colour space (Display P3 leaves greys alone), so no capture counts unless the same
/// run passed the colour-patch rule at the same settings.
fn colour_patch_hosts(variants: &[PathBuf]) -> Result<Vec<(String, bool, String)>> {
    let mut hosts = Vec::new();
    for variant in variants {
        for name in capture_names(variant)? {
            let record = read_json(&variant.join("captures").join(format!("{name}.json")))?;
            if !matches!(record.get("patches"), Some(Json::Array(_))) {
                continue;
            }
            let key = colour_key(variant)?;
            let (pass, detail) = match load_capture(variant, &name, false) {
                Ok(capture) => {
                    let patches = capture.patches.clone().unwrap_or_default();
                    let check = gates::patch_check(
                        "upstream",
                        &capture,
                        &patches,
                        gates::reference_patch_limit(&capture),
                    );
                    (check.pass, check.detail)
                }
                Err(e) => (false, e),
            };
            hosts.push((key, pass, format!("{} {name}: {detail}", variant.display())));
        }
    }
    Ok(hosts)
}

pub(crate) fn compare_roots(reference: &Path, port: &Path) -> Result<Vec<CaptureResult>> {
    let mut results = Vec::new();
    let variants = variant_dirs(reference)?;
    if variants.is_empty() {
        return Err(format!("{}: no upstream variants", reference.display()));
    }
    let hosts = colour_patch_hosts(&variants)?;
    let label_of = |root: &Path, variant: &Path| {
        let rel = variant.strip_prefix(root).expect("variant under its root");
        if rel.as_os_str().is_empty() {
            variant
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            rel.display().to_string()
        }
    };
    let mut rels = Vec::new();
    for variant in &variants {
        let rel = variant
            .strip_prefix(reference)
            .expect("variant under its root");
        rels.push(rel.to_path_buf());
        let label = label_of(reference, variant);
        let key = colour_key(variant)?;
        let ours: Vec<&(String, bool, String)> = hosts.iter().filter(|h| h.0 == key).collect();
        if ours.is_empty() {
            results.push(CaptureResult::failed(
                format!("{label} colour patch"),
                format!("the upstream run has no colour-patch capture at {key}, so its colours are unverified"),
            ));
        } else if let Some(bad) = ours.iter().find(|h| !h.1) {
            results.push(CaptureResult::failed(
                format!("{label} colour patch"),
                format!("the upstream colour patch at {key} fails: {}", bad.2),
            ));
        }
        let theirs = port.join(rel);
        if !theirs.join("run.json").exists() {
            results.push(CaptureResult::failed(
                label,
                format!("no port run at {}", theirs.display()),
            ));
            continue;
        }
        results.extend(compare_variant(&label, variant, &theirs)?);
    }
    if port.is_dir() {
        for variant in variant_dirs(port)? {
            let rel = variant.strip_prefix(port).expect("variant under its root");
            if !rels.iter().any(|r| r == rel) {
                results.push(CaptureResult::failed(
                    label_of(port, &variant),
                    "run by the port only".into(),
                ));
            }
        }
    }
    Ok(results)
}

#[derive(Default)]
struct PortOptions {
    scenarios: Vec<String>,
    flags: Vec<String>,
    label: String,
    upstream: Option<String>,
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
                "--upstream" => {
                    options.upstream = Some(iter.next().ok_or("--upstream needs a label")?.clone());
                }
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
        .join("debug")
        .join(format!("asrs{}", std::env::consts::EXE_SUFFIX)))
}

/// The work-area origin of the display at the global origin (Electron's primary) in an upstream
/// run, where the driver placed its window.
fn primary_work_area(run_json: &Path) -> Option<(f64, f64)> {
    let run = read_json(run_json).ok()?;
    list(&run, "displays").iter().find_map(|display| {
        let bounds = display.get("bounds")?;
        let origin = (bounds.get("x")?.as_f64()?, bounds.get("y")?.as_f64()?);
        let area = display.get("workArea")?;
        (origin == (0.0, 0.0)).then_some((area.get("x")?.as_f64()?, area.get("y")?.as_f64()?))
    })
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
        let upstream = parity_dir()
            .join("app")
            .join(options.upstream.as_ref().unwrap_or(&options.label))
            .join(&name);
        let variants = match upstream.is_dir() {
            true => variant_dirs(&upstream)?,
            false => Vec::new(),
        };
        let inputs = variants
            .iter()
            .map(|v| v.join("inputs"))
            .find(|i| i.is_dir());
        let work_area = variants
            .iter()
            .find_map(|v| primary_work_area(&v.join("run.json")));
        let status = Command::new(&exe)
            .arg("--parity-scenario")
            .arg(scenarios.join(format!("{name}.json")))
            .arg("--out")
            .arg(&out)
            .args(
                inputs
                    .iter()
                    .flat_map(|i| [std::ffi::OsStr::new("--inputs"), i.as_os_str()]),
            )
            .arg("--bundled-assets")
            .arg(workspace_root().join("reference/AdvantageScope/bundledAssets"))
            .args(
                work_area
                    .iter()
                    .flat_map(|(x, y)| ["--work-area".to_owned(), format!("{x},{y}")]),
            )
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
                for c in outcome
                    .checks
                    .iter()
                    .filter(|c| c.gate == "colour patch" && c.what != "upstream")
                {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn variant(root: &Path, side: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = root.join(side);
        let listed: Vec<String> = files
            .iter()
            .map(|(file, bytes)| {
                let path = dir.join("files").join(file);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, bytes).unwrap();
                format!(
                    r#"{{"file":"{file}","bytes":{},"changed":true}}"#,
                    bytes.len()
                )
            })
            .collect();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("run.json"),
            format!(r#"{{"files":[{}]}}"#, listed.join(",")),
        )
        .unwrap();
        dir
    }

    #[test]
    fn every_upstream_file_must_be_written_and_equal() {
        let root = std::env::temp_dir().join(format!("asrs-compare-files-{}", std::process::id()));
        let readme = ("userAssets/README.txt", "readme");
        let upstream = variant(
            &root,
            "upstream",
            &[readme, ("prefs.json", "{}"), ("state-dev.json", "{}")],
        );
        let port = variant(
            &root,
            "port",
            &[readme, ("prefs.json", "{}"), ("state-dev.json", "{}")],
        );
        let outcome = compare_files("v", &upstream, &port).unwrap().outcome;
        assert!(outcome.pass(), "{:?}", outcome.checks);

        let port = variant(&root, "partial", &[readme, ("prefs.json", "{ }")]);
        let outcome = compare_files("v", &upstream, &port).unwrap().outcome;
        let failed: Vec<&str> = outcome.failures().map(|c| c.what.as_str()).collect();
        assert_eq!(failed, ["prefs.json", "state-dev.json"]);
        let _ = fs::remove_dir_all(&root);
    }
}
