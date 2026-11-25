use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use asrs_parity::json::{self, Json};
use asrs_parity::pixel::{self, Stats};
use asrs_parity::vectors as parity_vectors;

use crate::oracle::{self, Paths};
use crate::{Result, cargo, host};

const USAGE: &str = "usage: cargo xtask canvas <command>

The canvas suite: display-list scenes replayed on Chromium's CPU canvas (the reference) and GPU
canvas, mask-gamma calibration, TextMetrics vectors and DOM text scenes, all generated on the
host it runs on. References land in target/parity/canvas/<os>/.

commands:
  ref [<scene>...]      Chromium references: scenes/*.json, ref/<scene>.{cpu,gpu}.png, and with no
                        scene named also measure.jsonl and dom/<scene>@{1,2}x.{png,json}
  lut [--lcd] [--check] calibrate parity/vectors/canvas/lut-<os>.bin from the calibration scenes and
                        the platform text backend (asrs-cli canvas lut), or with --lcd
                        lut-<os>-lcd.bin from the DOM calibration scenes, where DOM text is LCD:
                        Windows, and Linux under stock.conf
  dom [--check]         capture the DOM text scenes at device scale 1 and 2 into
                        parity/vectors/canvas/scenes-windows/ or scenes-linux-<font profile>/, with
                        each text box's antialiasing (`mode`: lcd or grayscale)
  measure [--check]     write parity/vectors/canvas/measure-<os>.jsonl.zst
  scenes                write parity/vectors/canvas/scenes-<os>/: the display lists and CPU
                        references of the text scenes `asrs-cli` tests against on that platform
  diff <port dir> [<scene>...]
                        gate <port dir>/<scene>.png (straight RGBA, like getImageData) against the
                        references; writes <port dir>/report.json and diff/<scene>.png
  run <program> [<arg>...] [-- <scene>...]
                        run `<program> <arg>... <scenes dir> <port dir> [<scene>...]`, then diff
  stable                regenerate the references and the LUTs twice from scratch and require
                        identical bytes, and the LUTs to equal the committed ones, as must the DOM
                        text scenes where there are some for this host
  heatmap               run the upstream Heatmap fixtures on Chromium's CPU canvas, replay them
                        with asrs-cli heatmap, and gate recorded fields and the shadow canvas
                        exactly and the output RGBA at a max channel delta of 1
  list                  scene names

Gates against the CPU canvas: pixel scenes at most 0.2 % pixelmatch (YIQ 0.1) and no blob over
64 px; text scenes 0 % and max channel delta 24. Every scene: pixelmatch distance to the GPU
canvas at most Chromium's own CPU-to-GPU distance. Off macOS, where that canvas is software
(gpuFeatureStatus of info.json) and equals the CPU canvas byte for byte, the distance is reported and
not enforced.";

const ELECTRON_FLAGS: &[&str] = &["--force-color-profile=srgb", "--lang=en-US"];
pub(crate) const DOM_SCALES: [&str; 2] = ["1", "2"];
const PIXEL_MAX_PCT: f64 = 0.2;
const PIXEL_MAX_BLOB: u64 = 64;
const TEXT_MAX_CHANNEL: u8 = 24;

pub fn run(args: &[String]) -> Result {
    let paths = Paths::new()?;
    let rest = args.get(1..).unwrap_or_default();
    let check = rest.iter().any(|a| a == "--check");
    match args.first().map(String::as_str) {
        Some("ref") => reference(&paths, &reference_dir(&paths), rest),
        Some("lut") if rest.iter().any(|a| a == "--lcd") => lcd_lut(&paths, check),
        Some("lut") => lut(&paths, check),
        Some("dom") => dom(&paths, check),
        Some("measure") => measure(&paths, check),
        Some("scenes") => pin_scenes(&paths),
        Some("diff") => match rest.split_first() {
            Some((port, scenes)) => diff(&reference_dir(&paths), Path::new(port), scenes),
            None => Err(USAGE.to_owned()),
        },
        Some("run") => run_port(&paths, rest),
        Some("stable") => stable(&paths),
        Some("heatmap") => heatmap(&paths),
        Some("list") => electron(&paths, &["list".to_owned()]),
        _ => Err(USAGE.to_owned()),
    }
}

pub(crate) fn platform() -> &'static str {
    std::env::consts::OS
}

pub(crate) fn reference_dir(paths: &Paths) -> PathBuf {
    paths.root.join("target/parity/canvas").join(platform())
}

fn vectors_dir(paths: &Paths) -> PathBuf {
    paths.root.join("parity/vectors/canvas")
}

fn electron(paths: &Paths, args: &[String]) -> Result {
    electron_with(paths, &[], args)
}

pub(crate) fn electron_with(paths: &Paths, flags: &[String], args: &[String]) -> Result {
    let main = paths.dist.join("canvasMain.mjs");
    if !main.exists() {
        return Err(format!(
            "{} is missing; run `cargo xtask oracle setup`",
            main.display()
        ));
    }
    let mut command = Command::new(paths.electron()?);
    command
        .args(ELECTRON_FLAGS)
        .args(flags)
        .arg(main)
        .args(args)
        .env_remove("ELECTRON_RUN_AS_NODE");
    oracle::pin_environment(&mut command, oracle::DEFAULT_ZONE);
    let status = command
        .status()
        .map_err(|e| format!("running Electron: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "canvas oracle {} failed ({status})",
            args.join(" ")
        ))
    }
}

fn path_arg(path: &Path) -> String {
    path.display().to_string()
}

const GENERATOR_STAMP: &str = "generator.txt";

fn generator_hash(paths: &Paths) -> Result<String> {
    oracle::hash_tree(
        &paths.root,
        &["oracle/src/canvas", "oracle/src/canvasMain.ts"],
    )
}

fn references_current(paths: &Paths, dir: &Path) -> Result<bool> {
    let stamp = fs::read_to_string(dir.join(GENERATOR_STAMP)).unwrap_or_default();
    Ok(stamp.trim() == generator_hash(paths)?)
}

pub(crate) fn reference(paths: &Paths, out: &Path, scenes: &[String]) -> Result {
    check_fonts(paths)?;
    oracle::build_harness(paths)?;
    let stamp = out.join(GENERATOR_STAMP);
    if scenes.is_empty() {
        for stale in [out.join("scenes"), out.join("ref"), out.join("dom")] {
            if stale.exists() {
                fs::remove_dir_all(&stale).map_err(|e| format!("{}: {e}", stale.display()))?;
            }
        }
    } else if stamp.exists() {
        fs::remove_file(&stamp).map_err(|e| format!("{}: {e}", stamp.display()))?;
    }
    let mut args = vec!["canvas".to_owned(), path_arg(out)];
    args.extend(scenes.iter().cloned());
    electron(paths, &args)?;
    if scenes.is_empty() {
        electron(paths, &["measure".to_owned(), path_arg(out)])?;
        dom_scenes(paths, &["dom"], out)?;
        write(&stamp, format!("{}\n", generator_hash(paths)?).as_bytes())?;
    }
    eprintln!("canvas references in {}", out.display());
    Ok(())
}

/// Runs a DOM capture mode of the oracle at each device scale.
pub(crate) fn dom_scenes(paths: &Paths, mode: &[&str], out: &Path) -> Result {
    for scale in DOM_SCALES {
        let mut args = vec![mode[0].to_owned(), path_arg(out)];
        args.extend(mode[1..].iter().map(|a| (*a).to_owned()));
        args.push(scale.to_owned());
        electron_with(
            paths,
            &[format!("--force-device-scale-factor={scale}")],
            &args,
        )?;
    }
    Ok(())
}

fn asrs_cli(paths: &Paths) -> Result<Command> {
    cargo(&["build", "--quiet", "--release", "-p", "asrs-cli"])?;
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| paths.root.join("target"));
    Ok(Command::new(
        target
            .join("release")
            .join(format!("asrs-cli{}", std::env::consts::EXE_SUFFIX)),
    ))
}

fn fit_lut(paths: &Paths, scratch: &Path) -> Result<Vec<u8>> {
    check_fonts(paths)?;
    oracle::build_harness(paths)?;
    electron(paths, &["canvas-lut".to_owned(), path_arg(scratch)])?;
    fit(paths, "lut", scratch, &scratch.join("lut.bin"))
}

fn fit(paths: &Paths, command: &str, reference: &Path, out: &Path) -> Result<Vec<u8>> {
    let status = asrs_cli(paths)?
        .args(["canvas", command])
        .arg(reference)
        .arg(out)
        .status()
        .map_err(|e| format!("running asrs-cli: {e}"))?;
    if !status.success() {
        return Err(format!("asrs-cli canvas {command} failed ({status})"));
    }
    oracle::read(out)
}

fn lcd_text() -> bool {
    match platform() {
        "windows" => true,
        "linux" => host::font_profile().as_deref() == Some("stock"),
        _ => false,
    }
}

fn fit_lcd_lut(paths: &Paths, scratch: &Path) -> Result<Vec<u8>> {
    if !lcd_text() {
        return Err(
            "DOM text is not LCD here (Windows, or Linux with FONTCONFIG_FILE=<dir>/stock.conf)"
                .to_owned(),
        );
    }
    check_fonts(paths)?;
    oracle::build_harness(paths)?;
    clear_dir(&scratch.join("lcd"))?;
    dom_scenes(paths, &["canvas-lut", "dom"], scratch)?;
    fit(paths, "lcd", &scratch.join("lcd"), &scratch.join("lcd.bin"))
}

fn clear_dir(dir: &Path) -> Result {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    Ok(())
}

/// Where a table or DOM scene was recorded, kept beside it: the Linux ones come from another
/// architecture than that platform's other references.
fn provenance(paths: &Paths, info: &Path) -> Result<Vec<(String, Json)>> {
    let info = read_json(info)?;
    let mut fields = Vec::new();
    for k in ["electron", "chrome", "arch", "osRelease"] {
        let value = info.get(k).ok_or_else(|| format!("info.json has no {k}"))?;
        fields.push((k.to_owned(), value.clone()));
    }
    if platform() == "linux" {
        let profile = host::font_profile().unwrap_or_default();
        fields.push(("profile".to_owned(), Json::string(&profile)));
    }
    let generator = oracle::hash_tree(
        &paths.root,
        &[
            "oracle/src/canvas",
            "oracle/src/canvasMain.ts",
            "crates/asrs-canvas/src/text",
        ],
    )?;
    fields.push(("generator".to_owned(), Json::string(&generator)));
    Ok(fields)
}

fn set_provenanced(
    paths: &Paths,
    info: &Path,
    key: &str,
    entry: impl IntoIterator<Item = (&'static str, Json)>,
) -> Result {
    let mut fields = provenance(paths, info)?;
    fields.extend(entry.into_iter().map(|(k, v)| (k.to_owned(), v)));
    let json = Json::object(fields.iter().map(|(k, v)| (k.as_str(), v.clone())));
    set_manifest_field(paths, platform(), key, json)
}

fn lcd_lut(paths: &Paths, check: bool) -> Result {
    let scratch = reference_dir(paths).join("lut");
    let bytes = fit_lcd_lut(paths, &scratch)?;
    let name = format!("lut-{}-lcd.bin", platform());
    let committed = vectors_dir(paths).join(&name);
    if check {
        return if oracle::read(&committed)? == bytes {
            eprintln!("{name} is up to date");
            Ok(())
        } else {
            Err(format!("regenerated {name} differs from the committed one"))
        };
    }
    write(&committed, &bytes)?;
    set_provenanced(
        paths,
        &scratch.join("info.json"),
        "lcdLut",
        [
            ("file", Json::string(&name)),
            ("content", Json::string(&oracle::hash_bytes(&bytes))),
        ],
    )?;
    eprintln!("wrote parity/vectors/canvas/{name}");
    Ok(())
}

/// The committed DOM text scenes of this host: their directory and manifest key.
fn dom_vectors(paths: &Paths) -> Result<(PathBuf, String)> {
    let (dir, key) = match (platform(), host::font_profile().as_deref()) {
        ("windows", _) => ("scenes-windows".to_owned(), "dom".to_owned()),
        ("linux", Some(p @ ("gray" | "stock"))) => {
            (format!("scenes-linux-{p}"), format!("dom-{p}"))
        }
        ("linux", _) => return Err("FONTCONFIG_FILE is not gray.conf or stock.conf".to_owned()),
        (os, _) => return Err(format!("no DOM text scenes are kept for {os}")),
    };
    Ok((vectors_dir(paths).join(dir), key))
}

/// The DOM captures under `dir` as they are committed: file name and bytes.
fn dom_files(dir: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    let mut files = Vec::new();
    for name in files_under(dir)? {
        let Some(scene) = name.strip_suffix(".json") else {
            continue;
        };
        let captured = read_json(&dir.join(&name))?;
        let scale = captured.get("scale");
        let scale = scale.ok_or_else(|| format!("{name}: no scale"))?;
        let rows = captured.get("rows").and_then(Json::as_array);
        let rows = rows
            .ok_or_else(|| format!("{name}: no rows"))?
            .iter()
            .map(|row| {
                let fields = ["id", "rect", "mode"].map(|key| {
                    let value = row.get(key).cloned();
                    Ok((
                        key,
                        value.ok_or_else(|| format!("{name}: a row has no {key}"))?,
                    ))
                });
                Ok(Json::object(
                    fields.into_iter().collect::<Result<Vec<_>>>()?,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let pinned = Json::object([
            ("scene", Json::string(scene)),
            ("scale", scale.clone()),
            ("rows", Json::Array(rows)),
        ]);
        let image = oracle::read(&dir.join(format!("{scene}.png")))?;
        files.push((name.clone(), format!("{}\n", pinned.to_pretty()).into()));
        files.push((format!("{scene}.png"), image));
    }
    Ok(files)
}

fn different_dom_files(committed: &Path, files: &[(String, Vec<u8>)]) -> Result<Vec<String>> {
    let mut differing = Vec::new();
    for (name, bytes) in files {
        if fs::read(committed.join(name)).ok().as_ref() != Some(bytes) {
            differing.push(format!("{name} (against the committed one)"));
        }
    }
    for name in files_under(committed)? {
        if name.starts_with("dom-") && !files.iter().any(|(n, _)| *n == name) {
            differing.push(format!("{name} (committed, not captured)"));
        }
    }
    Ok(differing)
}

fn dom(paths: &Paths, check: bool) -> Result {
    let (vectors, key) = dom_vectors(paths)?;
    check_fonts(paths)?;
    oracle::build_harness(paths)?;
    let scratch = reference_dir(paths).join(&key);
    clear_dir(&scratch)?;
    dom_scenes(paths, &["dom"], &scratch)?;
    let files = dom_files(&scratch.join("dom"))?;
    if check {
        let differing = different_dom_files(&vectors, &files)?;
        return if differing.is_empty() {
            eprintln!("{}: {} files identical", vectors.display(), files.len());
            Ok(())
        } else {
            Err(format!(
                "DOM text scenes differ:\n  {}",
                differing.join("\n  ")
            ))
        };
    }
    let mut entries = Vec::new();
    for (name, bytes) in &files {
        write(&vectors.join(name), bytes)?;
        entries.push(Json::object([
            ("file", Json::string(name)),
            ("content", Json::string(&oracle::hash_bytes(bytes))),
        ]));
    }
    set_provenanced(
        paths,
        &scratch.join("info.json"),
        &key,
        [("files", Json::Array(entries))],
    )?;
    eprintln!("wrote {} files to {}", files.len(), vectors.display());
    Ok(())
}

pub(crate) fn lut(paths: &Paths, check: bool) -> Result {
    let scratch = reference_dir(paths).join("lut");
    let bytes = fit_lut(paths, &scratch)?;
    let name = format!("lut-{}.bin", platform());
    let committed = vectors_dir(paths).join(&name);
    if check {
        return if oracle::read(&committed)? == bytes {
            eprintln!("{name} is up to date");
            Ok(())
        } else {
            Err(format!("regenerated {name} differs from the committed one"))
        };
    }
    write(&committed, &bytes)?;
    update_manifest(
        paths,
        &scratch.join("info.json"),
        "lut",
        Json::object([
            ("file", Json::string(&name)),
            ("content", Json::string(&oracle::hash_bytes(&bytes))),
        ]),
    )?;
    eprintln!("wrote parity/vectors/canvas/{name}");
    Ok(())
}

/// The scenes whose references are small enough to commit.
const PINNED_SCENES: &[&str] = &[
    "text-light-1x",
    "text-dark-1x",
    "text-light-2x",
    "textstyles-light-1x",
    "textstyles-dark-1x",
    "textstyles-light-2x",
    "textemoji-light-1x",
    "textemoji-dark-1x",
    "textemoji-light-2x",
    "gradienttext-light-1x",
    "transforms-1x",
    "transforms-2x",
    "floatmatrix-1x",
    "floatmatrix-2x",
    "quarterturns-1x",
    "quarterturns-2x",
    "gradientedges-light-1x",
    "gradientedges-light-2x",
];

fn pin_scenes(paths: &Paths) -> Result {
    check_fonts(paths)?;
    let reference = reference_dir(paths);
    if !references_current(paths, &reference)? {
        self::reference(paths, &reference, &[])?;
    }
    write_pinned(paths, &reference)
}

fn write_pinned(paths: &Paths, reference: &Path) -> Result {
    let dir = vectors_dir(paths).join(format!("scenes-{}", platform()));
    let mut entries = Vec::new();
    for name in PINNED_SCENES {
        let scene = oracle::read(&reference.join(format!("scenes/{name}.json")))?;
        let image = oracle::read(&reference.join(format!("ref/{name}.cpu.png")))?;
        write(&dir.join(format!("{name}.json")), &scene)?;
        write(&dir.join(format!("{name}.png")), &image)?;
        entries.push(Json::object([
            ("scene", Json::string(name)),
            ("content", Json::string(&oracle::hash_bytes(&image))),
        ]));
    }
    let count = entries.len();
    update_manifest(
        paths,
        &reference.join("info.json"),
        "scenes",
        Json::Array(entries),
    )?;
    eprintln!("wrote {count} scenes to {}", dir.display());
    Ok(())
}

fn recorded_fonts(paths: &Paths) -> Result<Vec<(String, Option<String>)>> {
    let name = format!("{}.json", platform());
    let manifest = read_json(&vectors_dir(paths).join(&name))?;
    let Some(Json::Object(fonts)) = manifest.get("host").and_then(|h| h.get("fonts")) else {
        return Err(format!("{name} records no host fonts"));
    };
    Ok(fonts
        .iter()
        .map(|(path, hash)| {
            let hash = hash.as_str().map(|h| h.to_string_lossy());
            (path.to_string_lossy(), hash)
        })
        .collect())
}

/// Refuses a Windows or Linux host whose fonts are not the files the manifest records for the
/// render host, wherever fontconfig finds them on Linux.
pub(crate) fn check_fonts(paths: &Paths) -> Result {
    match platform() {
        "linux" => check_fontconfig(paths),
        "windows" => {
            let mut problems = Vec::new();
            for (path, want) in recorded_fonts(paths)? {
                let found = fs::read(&path).ok().map(|b| crate::sha256_hex(&b));
                if found != want {
                    problems.push(format!("{path}: not the recorded file"));
                }
            }
            font_problems(&problems)
        }
        _ => Ok(()),
    }
}

fn font_problems(problems: &[String]) -> Result {
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the fonts differ from the render host's:\n  {}",
        problems.join("\n  ")
    ))
}

fn check_fontconfig(paths: &Paths) -> Result {
    let listed = Command::new("fc-list")
        .args(["-f", "%{file}\n"])
        .output()
        .map_err(|e| format!("fc-list: {e}"))?;
    let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
    let mut problems = Vec::new();
    for (path, want) in recorded_fonts(paths)? {
        let name = path.rsplit('/').next().unwrap_or_default();
        let found: Vec<&str> = listed
            .lines()
            .filter(|l| l.rsplit('/').next() == Some(name))
            .collect();
        if found.is_empty() {
            problems.push(format!("{name}: not installed"));
        }
        for file in found {
            if Some(crate::sha256_hex(&oracle::read(Path::new(file))?)) != want {
                problems.push(format!("{file}: not the recorded file"));
            }
        }
    }
    font_problems(&problems).map_err(|e| format!("{e}\n(set FONTCONFIG_FILE)"))
}

fn measure(paths: &Paths, check: bool) -> Result {
    check_fonts(paths)?;
    oracle::build_harness(paths)?;
    let scratch = reference_dir(paths).join("measure");
    electron(paths, &["measure".to_owned(), path_arg(&scratch)])?;
    let text = String::from_utf8(oracle::read(&scratch.join("measure.jsonl"))?)
        .map_err(|e| e.to_string())?;
    parity_vectors::parse_records(&text)?;
    let name = format!("measure-{}", platform());
    let dir = vectors_dir(paths);
    if check {
        let committed = parity_vectors::vector_path(&dir, &name)
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
        &dir.join(format!("{name}.jsonl.zst")),
        &parity_vectors::compress(&text),
    )?;
    update_manifest(
        paths,
        &scratch.join("info.json"),
        "measure",
        Json::object([
            ("file", Json::string(&format!("{name}.jsonl.zst"))),
            ("cases", Json::Number(text.lines().count() as f64)),
            (
                "content",
                Json::string(&oracle::hash_bytes(text.as_bytes())),
            ),
        ]),
    )?;
    eprintln!(
        "wrote parity/vectors/canvas/{name}.jsonl.zst ({} records)",
        text.lines().count()
    );
    Ok(())
}

pub(crate) fn write(path: &Path, bytes: &[u8]) -> Result {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

pub(crate) fn read_json(path: &Path) -> Result<Json> {
    let text = String::from_utf8(oracle::read(path)?).map_err(|e| e.to_string())?;
    json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The fonts `recorded` has that are missing from `now` or other files in it.
fn changed_fonts(recorded: &Json, now: &Json) -> Vec<String> {
    let Some(Json::Object(fonts)) = recorded.get("fonts") else {
        return Vec::new();
    };
    let now = now.get("fonts");
    fonts
        .iter()
        .filter(|(path, hash)| {
            now.and_then(|n| n.get(&path.to_string_lossy()).cloned())
                .as_ref()
                != Some(hash)
        })
        .map(|(path, _)| path.to_string_lossy())
        .collect()
}

/// Stores the host description, unless the host's fonts are not the ones the vectors were recorded
/// with: an operating system update must not move the reference. Deleting the description from
/// the manifest by hand allows it.
pub(crate) fn set_manifest_host(paths: &Paths, platform: &str, description: Json) -> Result {
    let name = format!("{platform}.json");
    let manifest = read_json(&vectors_dir(paths).join(&name)).unwrap_or(Json::Null);
    if let Some(recorded) = manifest.get("host") {
        let changed = changed_fonts(recorded, &description);
        if !changed.is_empty() {
            return Err(format!(
                "{name} records other files than this host's:\n  {}",
                changed.join("\n  ")
            ));
        }
    }
    set_manifest_field(paths, platform, "host", description)
}

fn set_manifest_field(paths: &Paths, platform: &str, key: &str, value: Json) -> Result {
    let path = vectors_dir(paths).join(format!("{platform}.json"));
    let mut fields: Vec<(String, Json)> = match read_json(&path) {
        Ok(Json::Object(pairs)) => pairs
            .into_iter()
            .map(|(k, v)| (k.to_string_lossy(), v))
            .collect(),
        _ => Vec::new(),
    };
    match fields.iter_mut().find(|(name, _)| name == key) {
        Some(slot) => slot.1 = value,
        None => fields.push((key.to_owned(), value)),
    }
    let json = Json::object(fields.iter().map(|(k, v)| (k.as_str(), v.clone())));
    write(&path, format!("{}\n", json.to_pretty()).as_bytes())
}

fn update_manifest(paths: &Paths, info: &Path, key: &str, entry: Json) -> Result {
    let path = vectors_dir(paths).join(format!("{}.json", platform()));
    let mut fields: Vec<(String, Json)> = match read_json(&path) {
        Ok(Json::Object(pairs)) => pairs
            .into_iter()
            .map(|(k, v)| (k.to_string_lossy(), v))
            .collect(),
        _ => Vec::new(),
    };
    let info = read_json(info)?;
    let mut set = |k: &str, v: Json| match fields.iter_mut().find(|(name, _)| name == k) {
        Some(slot) => slot.1 = v,
        None => fields.push((k.to_owned(), v)),
    };
    for k in ["electron", "chrome", "platform", "arch", "osRelease"] {
        if let Some(v) = info.get(k) {
            set(k, v.clone());
        }
    }
    set(
        "generator",
        Json::string(&oracle::hash_tree(
            &paths.root,
            &[
                "oracle/src/canvas",
                "oracle/src/canvasMain.ts",
                "crates/asrs-canvas/src/text",
            ],
        )?),
    );
    set(key, entry);
    let json = Json::object(fields.iter().map(|(k, v)| (k.as_str(), v.clone())));
    write(&path, format!("{}\n", json.to_pretty()).as_bytes())
}

struct SceneInfo {
    gate: String,
    background: [u8; 3],
}

fn scene_info(path: &Path) -> Result<SceneInfo> {
    let scene = read_json(path)?;
    let text = |k: &str| {
        scene
            .get(k)
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .ok_or_else(|| format!("{}: no {k}", path.display()))
    };
    let background = scene
        .get("background")
        .and_then(Json::as_array)
        .filter(|a| a.len() == 3)
        .and_then(|a| {
            let c = |i: usize| a[i].as_f64().map(|v| v as u8);
            Some([c(0)?, c(1)?, c(2)?])
        })
        .ok_or_else(|| format!("{}: bad background", path.display()))?;
    Ok(SceneInfo {
        gate: text("gate")?,
        background,
    })
}

fn scene_names(reference: &Path, wanted: &[String]) -> Result<Vec<String>> {
    let dir = reference.join("scenes");
    let mut names = Vec::new();
    for entry in fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}; run `cargo xtask canvas ref` first", dir.display()))?
    {
        let path = entry.map_err(|e| e.to_string())?.path();
        if let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".json"))
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    for w in wanted {
        if !names.contains(w) {
            return Err(format!("no reference scene {w}"));
        }
    }
    Ok(names
        .into_iter()
        .filter(|n| wanted.is_empty() || wanted.contains(n))
        .collect())
}

fn stats_json(s: &Stats) -> Json {
    Json::object([
        ("mismatchPct", Json::Number(s.mismatch_pct)),
        ("mismatch", Json::Number(s.mismatch as f64)),
        ("maxBlob", Json::Number(s.max_blob as f64)),
        ("maxChannel", Json::Number(f64::from(s.max_channel))),
        ("meanAbs", Json::Number(s.mean_abs)),
        ("exactMismatch", Json::Number(s.exact_mismatch as f64)),
        ("inked", Json::Number(s.inked as f64)),
    ])
}

/// Whether the GPU-distance rule binds on the host the references come from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GpuRule {
    Enforced,
    /// Chromium's GPU canvas is software and equals its CPU canvas, so the rule would turn the
    /// gate against the CPU canvas into exact equality.
    Reported,
}

/// `disabled_software` is what Chromium says where there is no GPU process (the Linux hosts) and
/// `unavailable_software` where it finds no usable adapter (the Windows hosts).
fn gpu_rule(platform: &str, status: &Json) -> Result<GpuRule> {
    let canvas = status.get("2d_canvas").and_then(Json::as_str);
    let canvas = canvas.ok_or("gpuFeatureStatus has no 2d_canvas")?;
    Ok(match canvas.to_string_lossy().as_str() {
        "disabled_software" | "unavailable_software" if platform != "darwin" => GpuRule::Reported,
        _ => GpuRule::Enforced,
    })
}

fn reference_gpu_rule(reference: &Path) -> Result<GpuRule> {
    let path = reference.join("info.json");
    let info = read_json(&path)?;
    let platform = info.get("platform").and_then(Json::as_str);
    let platform = platform.ok_or_else(|| format!("{}: no platform", path.display()))?;
    let status = info.get("gpuFeatureStatus").unwrap_or(&Json::Null);
    gpu_rule(&platform.to_string_lossy(), status).map_err(|e| format!("{}: {e}", path.display()))
}

fn gpu_distance(port_gpu: &Stats, cpu_gpu: &Stats) -> Option<String> {
    (port_gpu.mismatch > cpu_gpu.mismatch).then(|| {
        format!(
            "{:.3} % from the GPU canvas, Chromium CPU is {:.3} %",
            port_gpu.mismatch_pct, cpu_gpu.mismatch_pct
        )
    })
}

fn gate(
    scene: &SceneInfo,
    rule: GpuRule,
    port_cpu: &Stats,
    port_gpu: &Stats,
    cpu_gpu: &Stats,
) -> Vec<String> {
    let mut failures = Vec::new();
    match scene.gate.as_str() {
        "text" => {
            if port_cpu.mismatch > 0 {
                failures.push(format!("{} differing pixels", port_cpu.mismatch));
            }
            if port_cpu.max_channel > TEXT_MAX_CHANNEL {
                failures.push(format!("max channel delta {}", port_cpu.max_channel));
            }
        }
        _ => {
            if port_cpu.mismatch_pct > PIXEL_MAX_PCT {
                failures.push(format!("{:.3} % differing", port_cpu.mismatch_pct));
            }
            if port_cpu.max_blob > PIXEL_MAX_BLOB {
                failures.push(format!("blob of {} px", port_cpu.max_blob));
            }
        }
    }
    if rule == GpuRule::Enforced {
        failures.extend(gpu_distance(port_gpu, cpu_gpu));
    }
    failures
}

pub(crate) fn diff(reference: &Path, port: &Path, wanted: &[String]) -> Result {
    let names = scene_names(reference, wanted)?;
    let rule = reference_gpu_rule(reference)?;
    let diff_dir = port.join("diff");
    fs::create_dir_all(&diff_dir).map_err(|e| format!("{}: {e}", diff_dir.display()))?;
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    let mut reported = Vec::new();
    eprintln!(
        "{:<24} {:>9} {:>6} {:>4} | {:>9} {:>9} | result",
        "scene", "vs cpu %", "blob", "ch", "vs gpu %", "cpu-gpu %"
    );
    for name in names {
        let scene = scene_info(&reference.join(format!("scenes/{name}.json")))?;
        if scene.gate == "calibration" {
            continue;
        }
        let load = |p: PathBuf| pixel::read_png(&p).map(|i| pixel::composite(&i, scene.background));
        let cpu = load(reference.join(format!("ref/{name}.cpu.png")))?;
        let gpu = load(reference.join(format!("ref/{name}.gpu.png")))?;
        let cpu_gpu = pixel::compare(&cpu, &gpu, scene.background)
            .map_err(|e| format!("{name}: {e}"))?
            .stats;
        let port_png = port.join(format!("{name}.png"));
        let compared = if port_png.exists() {
            load(port_png).and_then(|ours| {
                Ok((
                    pixel::compare(&cpu, &ours, scene.background)?,
                    pixel::compare(&gpu, &ours, scene.background)?.stats,
                ))
            })
        } else {
            Err("no port render".to_owned())
        };
        let (port_cpu, port_gpu) = match compared {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{name:<24} {e} | FAIL");
                failed.push(name.clone());
                rows.push(Json::object([
                    ("scene", Json::string(&name)),
                    ("pass", Json::Bool(false)),
                    ("failures", Json::Array(vec![Json::string(&e)])),
                ]));
                continue;
            }
        };
        let rule = if cpu_gpu.exact_mismatch == 0 {
            rule
        } else {
            GpuRule::Enforced
        };
        let failures = gate(&scene, rule, &port_cpu.stats, &port_gpu, &cpu_gpu);
        let distance = gpu_distance(&port_gpu, &cpu_gpu).filter(|_| rule == GpuRule::Reported);
        if distance.is_some() {
            reported.push(name.clone());
        }
        if port_cpu.stats.exact_mismatch > 0 {
            pixel::write_png(
                &diff_dir.join(format!("{name}.png")),
                &pixel::diff_image(&cpu, &port_cpu.mask),
            )?;
        }
        let s = &port_cpu.stats;
        eprintln!(
            "{name:<24} {:>9.3} {:>6} {:>4} | {:>9.3} {:>9.3} | {}",
            s.mismatch_pct,
            s.max_blob,
            s.max_channel,
            port_gpu.mismatch_pct,
            cpu_gpu.mismatch_pct,
            match (failures.is_empty(), &distance) {
                (true, None) => "ok".to_owned(),
                (true, Some(d)) => format!("ok (not enforced: {d})"),
                (false, _) => format!("FAIL: {}", failures.join("; ")),
            }
        );
        if !failures.is_empty() {
            failed.push(name.clone());
        }
        rows.push(Json::object([
            ("scene", Json::string(&name)),
            ("gate", Json::string(&scene.gate)),
            ("pass", Json::Bool(failures.is_empty())),
            (
                "failures",
                Json::Array(failures.iter().map(|f| Json::string(f)).collect()),
            ),
            ("gpuRuleEnforced", Json::Bool(rule == GpuRule::Enforced)),
            (
                "gpuDistance",
                distance.map_or(Json::Null, |d| Json::string(&d)),
            ),
            ("portVsCpu", stats_json(&port_cpu.stats)),
            ("portVsGpu", stats_json(&port_gpu)),
            ("cpuVsGpu", stats_json(&cpu_gpu)),
        ]));
    }
    write(
        &port.join("report.json"),
        format!("{}\n", Json::Array(rows).to_pretty()).as_bytes(),
    )?;
    if rule == GpuRule::Reported {
        eprintln!(
            "canvas: the GPU canvas is software here, so the GPU-distance rule is reported only: {} scenes beyond it",
            reported.len()
        );
    }
    if failed.is_empty() {
        eprintln!("canvas: every scene within its gate");
        Ok(())
    } else {
        Err(format!(
            "canvas: {} scenes outside their gate: {}",
            failed.len(),
            failed.join(", ")
        ))
    }
}

fn run_port(paths: &Paths, args: &[String]) -> Result {
    let (program, scenes) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &[][..]),
    };
    let Some((exe, exe_args)) = program.split_first() else {
        return Err(USAGE.to_owned());
    };
    let ref_dir = reference_dir(paths);
    if !references_current(paths, &ref_dir)? {
        reference(paths, &ref_dir, &[])?;
    }
    let port = ref_dir.join("port");
    if port.exists() {
        fs::remove_dir_all(&port).map_err(|e| format!("{}: {e}", port.display()))?;
    }
    fs::create_dir_all(&port).map_err(|e| format!("{}: {e}", port.display()))?;
    let status = Command::new(exe)
        .args(exe_args)
        .arg(ref_dir.join("scenes"))
        .arg(&port)
        .args(scenes)
        .status()
        .map_err(|e| format!("running {exe}: {e}"))?;
    if !status.success() {
        return Err(format!("{exe} failed ({status})"));
    }
    diff(&ref_dir, &port, scenes)
}

const HEATMAP_MAX_CHANNEL: u8 = 1;
const HEATMAP_FIELDS: &[&str] = &[
    "created",
    "setData",
    "store",
    "throw",
    "canvas",
    "readError",
];

fn heatmap(paths: &Paths) -> Result {
    let dir = reference_dir(paths).join("heatmap");
    electron(paths, &["heatmap".to_owned(), path_arg(&dir)])?;
    let refs = dir.join("heatmap");
    let port = refs.join("port");
    if port.exists() {
        fs::remove_dir_all(&port).map_err(|e| format!("{}: {e}", port.display()))?;
    }
    cargo(&[
        "run",
        "-p",
        "asrs-cli",
        "--quiet",
        "--",
        "heatmap",
        &path_arg(&refs),
        &path_arg(&port),
    ])?;

    let mut expected_names = Vec::new();
    for entry in fs::read_dir(&refs).map_err(|e| format!("{}: {e}", refs.display()))? {
        let name = entry.map_err(|e| e.to_string())?.file_name();
        if let Some(name) = name.to_str().and_then(|n| n.strip_suffix(".expected.json")) {
            expected_names.push(name.to_owned());
        }
    }
    expected_names.sort_unstable();
    if expected_names.is_empty() {
        return Err(format!("no heatmap references under {}", refs.display()));
    }

    let mut failures = Vec::new();
    let mut max_channel = 0u8;
    let mut png_count = 0u64;
    for name in &expected_names {
        let expected = read_json(&refs.join(format!("{name}.expected.json")))?;
        let actual = read_json(&port.join(format!("{name}.data.json")))?;
        let updates = expected
            .get("updates")
            .and_then(Json::as_array)
            .map(<[Json]>::to_vec)
            .unwrap_or_default();
        let ours = actual
            .get("updates")
            .and_then(Json::as_array)
            .map(<[Json]>::to_vec)
            .unwrap_or_default();
        if updates.len() != ours.len() {
            failures.push(format!(
                "{name}: {} updates expected, {} replayed",
                updates.len(),
                ours.len()
            ));
            continue;
        }
        for (i, (expected_update, actual_update)) in updates.iter().zip(&ours).enumerate() {
            for key in HEATMAP_FIELDS {
                if expected_update.get(key) != actual_update.get(key) {
                    failures.push(format!("{name} update {i}: {key} differs"));
                }
            }
            let reference_shadow = refs.join(format!("{name}.{i}.shadow"));
            if reference_shadow.exists() {
                let port_shadow = port.join(format!("{name}.{i}.shadow"));
                match (fs::read(&reference_shadow), fs::read(&port_shadow)) {
                    (Ok(a), Ok(b)) if a == b => {}
                    (Ok(a), Ok(b)) => {
                        let differing = a.iter().zip(&b).filter(|(x, y)| x != y).count();
                        failures.push(format!(
                            "{name} update {i}: shadow canvas differs in {differing} bytes ({} vs {} bytes)",
                            a.len(),
                            b.len()
                        ));
                    }
                    (_, Err(e)) => failures.push(format!("{}: {e}", port_shadow.display())),
                    (Err(e), _) => failures.push(format!("{}: {e}", reference_shadow.display())),
                }
            }
            let reference_png = refs.join(format!("{name}.{i}.png"));
            if !reference_png.exists() {
                continue;
            }
            png_count += 1;
            let port_png = port.join(format!("{name}.{i}.png"));
            match heatmap_channel_delta(&reference_png, &port_png) {
                Ok(delta) => {
                    max_channel = max_channel.max(delta);
                    if delta > HEATMAP_MAX_CHANNEL {
                        failures.push(format!("{name} update {i}: max channel delta {delta}"));
                    }
                }
                Err(e) => failures.push(format!("{name} update {i}: {e}")),
            }
        }
    }
    eprintln!(
        "heatmap: {} fixtures, {png_count} renders, max channel delta {max_channel}",
        expected_names.len()
    );
    if failures.is_empty() {
        eprintln!("heatmap: every update matches the CPU canvas");
        Ok(())
    } else {
        for failure in failures.iter().take(10) {
            eprintln!("  {failure}");
        }
        Err(format!("heatmap: {} failures", failures.len()))
    }
}

fn heatmap_channel_delta(reference: &Path, port: &Path) -> Result<u8> {
    let a = pixel::read_png(reference)?;
    let b = pixel::read_png(port).map_err(|e| format!("{}: {e}", port.display()))?;
    if a.width != b.width || a.height != b.height {
        return Err(format!(
            "size differs: {}x{} vs {}x{}",
            a.width, a.height, b.width, b.height
        ));
    }
    Ok(a.data
        .iter()
        .zip(&b.data)
        .map(|(x, y)| x.abs_diff(*y))
        .max()
        .unwrap_or(0))
}

fn files_under(dir: &Path) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let rel = path.strip_prefix(dir).expect("under dir");
                out.push(rel.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    out.sort();
    Ok(out)
}

fn stable(paths: &Paths) -> Result {
    let base = paths.root.join("target/parity/canvas-stable");
    if base.exists() {
        fs::remove_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    }
    let runs = [base.join("a"), base.join("b")];
    let mut luts: Vec<(String, Vec<u8>)> = Vec::new();
    let mut dom = Vec::new();
    for dir in &runs {
        reference(paths, dir, &[])?;
        let lut = dir.join("lut");
        luts.push((format!("lut-{}.bin", platform()), fit_lut(paths, &lut)?));
        if lcd_text() {
            let bytes = fit_lcd_lut(paths, &lut)?;
            luts.push((format!("lut-{}-lcd.bin", platform()), bytes));
        }
        if let Ok((vectors, _)) = dom_vectors(paths) {
            dom.extend(different_dom_files(
                &vectors,
                &dom_files(&dir.join("dom"))?,
            )?);
        }
    }
    let (a, b) = (files_under(&runs[0])?, files_under(&runs[1])?);
    if a != b {
        return Err("the two runs wrote different file sets".to_owned());
    }
    let mut differing = Vec::new();
    for rel in &a {
        if oracle::read(&runs[0].join(rel))? != oracle::read(&runs[1].join(rel))? {
            differing.push(rel.clone());
        }
    }
    for (name, bytes) in &luts {
        if oracle::read(&vectors_dir(paths).join(name))? != *bytes {
            differing.push(format!("{name} (against the committed one)"));
        }
    }
    differing.extend(dom);
    differing.sort();
    differing.dedup();
    if differing.is_empty() {
        eprintln!(
            "canvas stable: {} files identical across two runs; {} equal the committed ones",
            a.len(),
            luts.iter()
                .map(|(name, _)| name.as_str())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(" and ")
        );
        Ok(())
    } else {
        Err(format!(
            "canvas stable: {} files differ:\n  {}",
            differing.len(),
            differing.join("\n  ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(canvas: &str) -> Json {
        Json::object([("2d_canvas", Json::string(canvas))])
    }

    #[test]
    fn the_gpu_rule_binds_on_hardware_canvases() {
        for hardware in ["enabled", "enabled_on", "enabled_force", "enabled_readback"] {
            let rule = gpu_rule("linux", &status(hardware)).unwrap();
            assert_eq!(rule, GpuRule::Enforced);
        }
        for software in ["disabled_software", "unavailable_software"] {
            let rule = |platform| gpu_rule(platform, &status(software)).unwrap();
            assert_eq!(rule("linux"), GpuRule::Reported);
            assert_eq!(rule("win32"), GpuRule::Reported);
            assert_eq!(rule("darwin"), GpuRule::Enforced);
        }
        let unknown = gpu_rule("linux", &status("anything")).unwrap();
        assert_eq!(unknown, GpuRule::Enforced);
        assert!(gpu_rule("linux", &Json::Null).is_err());
        let number = Json::object([("2d_canvas", Json::Bool(false))]);
        assert!(gpu_rule("linux", &number).is_err());
    }

    #[test]
    fn the_gpu_rule_of_each_recorded_host() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../parity/vectors/canvas");
        for (os, rule) in [
            ("macos", GpuRule::Enforced),
            ("linux", GpuRule::Reported),
            ("windows", GpuRule::Reported),
        ] {
            let manifest = read_json(&root.join(format!("{os}.json"))).unwrap();
            let host = manifest.get("host").unwrap();
            let status = host.get("chromiumRasterizer").unwrap().get("featureStatus");
            let platform = manifest.get("platform").unwrap().as_str().unwrap();
            let found = gpu_rule(&platform.to_string_lossy(), status.unwrap());
            assert_eq!(found.unwrap(), rule, "{os}");
        }
    }

    /// A scene whose GPU canvas is `gpu_off` pixels from its CPU canvas, and a port `off` pixels
    /// from the latter.
    fn diff_scene(name: &str, canvas: &str, gpu_off: usize, off: usize) -> (Result, Json) {
        let dir = std::env::temp_dir().join(format!("asrs-canvas-{name}-{}", std::process::id()));
        let (reference, port) = (dir.join("ref-dir"), dir.join("port"));
        let info = Json::object([
            ("platform", Json::string("linux")),
            ("gpuFeatureStatus", status(canvas)),
        ]);
        write(&reference.join("info.json"), info.to_compact().as_bytes()).unwrap();
        let scene = r#"{"gate":"pixel","background":[255,255,255]}"#;
        write(&reference.join("scenes/s.json"), scene.as_bytes()).unwrap();
        fs::create_dir_all(reference.join("ref")).unwrap();
        fs::create_dir_all(&port).unwrap();
        let mut image = pixel::Rgba {
            width: 100,
            height: 100,
            data: vec![255; 40000],
        };
        pixel::write_png(&reference.join("ref/s.cpu.png"), &image).unwrap();
        for i in 0..gpu_off {
            image.data[i * 8..i * 8 + 3].fill(0);
        }
        pixel::write_png(&reference.join("ref/s.gpu.png"), &image).unwrap();
        image.data.fill(255);
        for i in 0..off {
            image.data[i * 8..i * 8 + 3].fill(0);
        }
        pixel::write_png(&port.join("s.png"), &image).unwrap();
        let result = diff(&reference, &port, &[]);
        let report = read_json(&port.join("report.json")).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        (result, report)
    }

    #[test]
    fn a_port_further_from_the_gpu_canvas_fails_only_where_that_is_hardware() {
        let row = |report: &Json, key: &str| report.as_array().unwrap()[0].get(key).cloned();
        let (enforced, report) = diff_scene("hardware", "enabled", 0, 10);
        assert!(enforced.is_err());
        assert_eq!(row(&report, "gpuRuleEnforced"), Some(Json::Bool(true)));
        let failures = row(&report, "failures").unwrap();
        let failure = failures.as_array().unwrap()[0].as_str().unwrap();
        assert!(failure.to_string_lossy().contains("from the GPU canvas"));

        let (reported, report) = diff_scene("software", "unavailable_software", 0, 10);
        assert!(reported.is_ok());
        assert_eq!(row(&report, "gpuRuleEnforced"), Some(Json::Bool(false)));
        let distance = row(&report, "gpuDistance").unwrap();
        assert!(
            distance
                .as_str()
                .unwrap()
                .to_string_lossy()
                .contains("0.100 %")
        );

        let (cpu_gate, _) = diff_scene("software-cpu", "unavailable_software", 0, 30);
        assert!(cpu_gate.is_err());
        assert!(diff_scene("exact", "enabled", 0, 0).0.is_ok());
    }

    #[test]
    fn the_rule_binds_where_a_software_gpu_canvas_is_not_the_cpu_canvas() {
        let (result, report) = diff_scene("software-differs", "disabled_software", 1, 10);
        assert!(result.is_err());
        let row = &report.as_array().unwrap()[0];
        assert_eq!(row.get("gpuRuleEnforced"), Some(&Json::Bool(true)));
        assert!(
            diff_scene("software-within", "disabled_software", 10, 10)
                .0
                .is_ok()
        );
    }

    #[test]
    fn a_reference_without_a_feature_status_is_refused() {
        let dir = std::env::temp_dir().join(format!("asrs-canvas-none-{}", std::process::id()));
        let software = br#"{"gpuFeatureStatus":{"2d_canvas":"disabled_software"}}"#;
        for info in [&b"{}"[..], br#"{"platform":"linux"}"#, software] {
            write(&dir.join("info.json"), info).unwrap();
            assert!(reference_gpu_rule(&dir).is_err());
        }
        fs::remove_dir_all(&dir).unwrap();
        assert!(reference_gpu_rule(&dir).is_err());
    }

    /// Every text box of the committed DOM scenes says how Chromium antialiased it, and the pixels
    /// of the grey ones agree.
    #[test]
    fn committed_dom_text_boxes_carry_their_mode() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../parity/vectors/canvas");
        for (dir, lcd) in [
            ("scenes-windows", true),
            ("scenes-linux-stock", true),
            ("scenes-linux-gray", false),
        ] {
            let mut captures = 0;
            for file in files_under(&root.join(dir)).unwrap() {
                let Some(name) = file.strip_suffix(".json").filter(|f| f.starts_with("dom-"))
                else {
                    continue;
                };
                captures += 1;
                let scene = read_json(&root.join(dir).join(&file)).unwrap();
                let image = pixel::read_png(&root.join(dir).join(format!("{name}.png"))).unwrap();
                let modes = host::text_modes(&scene, &image)
                    .unwrap_or_else(|e| panic!("{dir}/{name}: {e}"));
                let scale = name.rsplit('@').next().unwrap().trim_end_matches('x');
                for (id, mode) in &modes {
                    if name.starts_with("dom-text-") {
                        assert_eq!(
                            *mode,
                            lcd && host::lcd_sized(id, scale),
                            "{dir}/{name} {id}"
                        );
                    }
                    assert!(lcd || !mode, "{dir}/{name} {id}");
                }
                let count = |of: bool| modes.iter().filter(|(_, m)| *m == of).count();
                let kind = name.split('-').nth(1).unwrap();
                let acrylic = dir == "scenes-windows" && kind == "hub";
                let boxes = match kind {
                    "text" => 65,
                    "lcd" => 38,
                    _ if acrylic => 27,
                    _ => 26,
                };
                assert_eq!(modes.len(), boxes, "{dir}/{name}");
                if acrylic {
                    assert_eq!(count(true), 0, "{dir}/{name}");
                } else if lcd && kind != "text" {
                    assert!(count(true) > 0 && count(false) > 0, "{dir}/{name}");
                }
            }
            assert_eq!(captures, 12, "{dir}");
        }
    }

    #[test]
    fn a_recorded_font_cannot_change() {
        let host = |fonts: &[(&str, &str)]| {
            let fonts = fonts.iter().map(|(k, v)| (*k, Json::string(v)));
            Json::object([("fonts", Json::object(fonts))])
        };
        let recorded = host(&[("a.ttf", "1"), ("b.ttf", "2")]);
        let more = host(&[("a.ttf", "1"), ("b.ttf", "2"), ("c.ttf", "3")]);
        assert!(changed_fonts(&recorded, &more).is_empty());
        assert_eq!(
            changed_fonts(&recorded, &host(&[("a.ttf", "1")])),
            ["b.ttf"]
        );
        let other = host(&[("a.ttf", "9"), ("b.ttf", "2")]);
        assert_eq!(changed_fonts(&recorded, &other), ["a.ttf"]);
        assert!(changed_fonts(&Json::Null, &other).is_empty());
    }
}
