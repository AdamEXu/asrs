use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use asrs_parity::json::{self, Json};
use asrs_parity::pixel::{self, Stats};
use asrs_parity::vectors as parity_vectors;

use crate::oracle::{self, Paths};
use crate::{Result, cargo};

const USAGE: &str = "usage: cargo xtask canvas <command>

The canvas suite: display-list scenes replayed on Chromium's CPU canvas (the reference) and GPU
canvas, mask-gamma calibration, TextMetrics vectors and DOM text scenes, all generated on the
host it runs on. References land in target/parity/canvas/<os>/.

commands:
  ref [<scene>...]      Chromium references: scenes/*.json, ref/<scene>.{cpu,gpu}.png, and with no
                        scene named also measure.jsonl and dom/<scene>@{1,2}x.{png,json}
  lut [--check]         calibrate parity/vectors/canvas/lut-<os>.bin from the calibration scenes and
                        the platform text backend (asrs-cli canvas lut)
  measure [--check]     write parity/vectors/canvas/measure-<os>.jsonl.zst
  diff <port dir> [<scene>...]
                        gate <port dir>/<scene>.png (straight RGBA, like getImageData) against the
                        references; writes <port dir>/report.json and diff/<scene>.png
  run <program> [<arg>...] [-- <scene>...]
                        run `<program> <arg>... <scenes dir> <port dir> [<scene>...]`, then diff
  stable                regenerate the references and the LUT twice from scratch and require
                        identical bytes, and the LUT to equal the committed one
  heatmap               run the upstream Heatmap fixtures on Chromium's CPU canvas, replay them
                        with asrs-cli heatmap, and gate recorded fields exactly and RGBA at a
                        max channel delta of 1
  list                  scene names

Gates against the CPU canvas: pixel scenes at most 0.2 % pixelmatch (YIQ 0.1) and no blob over
64 px; text scenes 0 % and max channel delta 24. Every scene: pixelmatch distance to the GPU
canvas at most Chromium's own CPU-to-GPU distance.";

const ELECTRON_FLAGS: &[&str] = &["--force-color-profile=srgb", "--lang=en-US"];
const PIXEL_MAX_PCT: f64 = 0.2;
const PIXEL_MAX_BLOB: u64 = 64;
const TEXT_MAX_CHANNEL: u8 = 24;

pub fn run(args: &[String]) -> Result {
    let paths = Paths::new()?;
    let rest = args.get(1..).unwrap_or_default();
    let check = rest.iter().any(|a| a == "--check");
    match args.first().map(String::as_str) {
        Some("ref") => reference(&paths, &reference_dir(&paths), rest),
        Some("lut") => lut(&paths, check),
        Some("measure") => measure(&paths, check),
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
        for scale in ["1", "2"] {
            electron_with(
                paths,
                &[format!("--force-device-scale-factor={scale}")],
                &["dom".to_owned(), path_arg(out), scale.to_owned()],
            )?;
        }
        write(&stamp, format!("{}\n", generator_hash(paths)?).as_bytes())?;
    }
    eprintln!("canvas references in {}", out.display());
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
    oracle::build_harness(paths)?;
    electron(paths, &["canvas-lut".to_owned(), path_arg(scratch)])?;
    let out = scratch.join("lut.bin");
    let status = asrs_cli(paths)?
        .args(["canvas", "lut"])
        .arg(scratch)
        .arg(&out)
        .status()
        .map_err(|e| format!("running asrs-cli: {e}"))?;
    if !status.success() {
        return Err(format!("asrs-cli canvas lut failed ({status})"));
    }
    oracle::read(&out)
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

fn measure(paths: &Paths, check: bool) -> Result {
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

pub(crate) fn set_manifest_field(paths: &Paths, platform: &str, key: &str, value: Json) -> Result {
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

fn gate(scene: &SceneInfo, port_cpu: &Stats, port_gpu: &Stats, cpu_gpu: &Stats) -> Vec<String> {
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
    if port_gpu.mismatch > cpu_gpu.mismatch {
        failures.push(format!(
            "{:.3} % from the GPU canvas, Chromium CPU is {:.3} %",
            port_gpu.mismatch_pct, cpu_gpu.mismatch_pct
        ));
    }
    failures
}

pub(crate) fn diff(reference: &Path, port: &Path, wanted: &[String]) -> Result {
    let names = scene_names(reference, wanted)?;
    let diff_dir = port.join("diff");
    fs::create_dir_all(&diff_dir).map_err(|e| format!("{}: {e}", diff_dir.display()))?;
    let mut rows = Vec::new();
    let mut failed = Vec::new();
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
        let failures = gate(&scene, &port_cpu.stats, &port_gpu, &cpu_gpu);
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
            if failures.is_empty() {
                "ok".to_owned()
            } else {
                format!("FAIL: {}", failures.join("; "))
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
            ("portVsCpu", stats_json(&port_cpu.stats)),
            ("portVsGpu", stats_json(&port_gpu)),
            ("cpuVsGpu", stats_json(&cpu_gpu)),
        ]));
    }
    write(
        &port.join("report.json"),
        format!("{}\n", Json::Array(rows).to_pretty()).as_bytes(),
    )?;
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
    let mut luts = Vec::new();
    for dir in &runs {
        reference(paths, dir, &[])?;
        luts.push(fit_lut(paths, &dir.join("lut"))?);
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
    let committed = oracle::read(&vectors_dir(paths).join(format!("lut-{}.bin", platform())))?;
    if luts.iter().any(|l| *l != committed) {
        differing.push(format!(
            "lut-{}.bin (against the committed one)",
            platform()
        ));
    }
    if differing.is_empty() {
        eprintln!(
            "canvas stable: {} files identical across two runs; LUT equals the committed one",
            a.len()
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
