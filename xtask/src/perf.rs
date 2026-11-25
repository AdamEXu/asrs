use std::path::{Path, PathBuf};
use std::process::Command;

use asrs_parity::json::{self, Json};

use crate::oracle::{self, Paths};
use crate::{Result, cargo, public};

const USAGE: &str = "usage: cargo xtask perf <command>

commands:
  decode [<log file>...] [--runs N]
            decode time, CPU and memory of every public log (parity/corpus/public.toml), or of
            the files listed: the release asrs-cli against upstream's decode in the oracle, N
            runs (default 5) of each side in turn, as `decode-bench` and `oracle decodeBench`
            time them
  frames [--runs N] [--scene <name>]... [--log <file>]
            frame time and memory of the busiest scene of every view in the headless hub
            (crates/asrs-app/examples/frame_bench.rs, release), N runs (default 5)
Both print Markdown tables with the load average of the machine before and after. Wall time is
only meaningful on a quiet machine; instructions and CPU time (macOS only, from
`/usr/bin/time -l`, minus the process's own startup) tolerate load better. Wall, CPU and
instruction figures are medians over the runs; p95 over 5 runs is their maximum.";

const FRAME_BUDGET_MS: f64 = 16.6;

/// From upstream's asset release `default-assets-v2`, in the asset cache.
const ASSETS: [&str; 4] = [
    "Field2d_2026FRCFieldV2",
    "Field3d_2026FRCFieldV2",
    "Robot_2026FRCKitBotV2",
    "Joystick_XboxControllerBlueV3",
];

pub fn run(args: &[String]) -> Result {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("decode") => decode(rest),
        Some("frames") => frames(rest),
        _ => Err(USAGE.to_owned()),
    }
}

struct Options {
    runs: usize,
    positional: Vec<String>,
    forwarded: Vec<String>,
}

fn options(args: &[String], forwarded: &[&str]) -> Result<Options> {
    let mut out = Options {
        runs: 5,
        positional: Vec::new(),
        forwarded: Vec::new(),
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut value = || iter.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--runs" => {
                out.runs = value()?
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or("--runs needs a number")?;
            }
            flag if forwarded.contains(&flag) => {
                let value = value()?.clone();
                out.forwarded.extend([arg.clone(), value]);
            }
            flag if flag.starts_with("--") => {
                return Err(format!("unknown option {flag}\n\n{USAGE}"));
            }
            _ => out.positional.push(arg.clone()),
        }
    }
    Ok(out)
}

pub(crate) fn release_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::workspace_root().join("target"))
        .join("release")
}

fn load_average() -> String {
    Command::new("uptime")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split("load average").nth(1).map(str::to_owned))
        .map_or_else(
            || "unknown".to_owned(),
            |s| s.trim_start_matches(['s', ':', ' ']).trim().to_owned(),
        )
}

fn sorted(values: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut values: Vec<f64> = values.collect();
    values.sort_by(f64::total_cmp);
    values
}

fn median(values: impl Iterator<Item = f64>) -> f64 {
    let values = sorted(values);
    match values.len() {
        0 => f64::NAN,
        n if n % 2 == 1 => values[n / 2],
        n => (values[n / 2 - 1] + values[n / 2]) / 2.0,
    }
}

fn p95(values: impl Iterator<Item = f64>) -> f64 {
    let values = sorted(values);
    match values.len() {
        0 => f64::NAN,
        n => values[((n as f64 * 0.95).ceil() as usize).clamp(1, n) - 1],
    }
}

/// What `/usr/bin/time -l` says of one process and its threads.
#[derive(Clone, Copy, Default)]
struct Usage {
    cpu_ms: f64,
    instructions: f64,
    peak_bytes: f64,
}

fn parse_usage(text: &str) -> Option<Usage> {
    let mut usage = Usage::default();
    let mut lines = text.lines();
    let times: Vec<&str> = lines.next()?.split_whitespace().collect();
    let seconds = |name: &str| {
        let at = times.iter().position(|t| *t == name)?;
        times.get(at.checked_sub(1)?)?.parse::<f64>().ok()
    };
    usage.cpu_ms = (seconds("user")? + seconds("sys")?) * 1e3;
    for line in lines {
        let Some((value, name)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(value) = value.parse() else { continue };
        match name.trim() {
            "instructions retired" => usage.instructions = value,
            "peak memory footprint" => usage.peak_bytes = value,
            _ => {}
        }
    }
    Some(usage)
}

fn measured(command: &Command, usage_file: &Path) -> Result<(Vec<u8>, Option<Usage>)> {
    let counted = cfg!(target_os = "macos");
    let mut timed = if counted {
        let mut timed = Command::new("/usr/bin/time");
        timed.arg("-l").arg("-o").arg(usage_file);
        timed.arg(command.get_program());
        timed
    } else {
        Command::new(command.get_program())
    };
    timed.args(command.get_args());
    for (name, value) in command.get_envs() {
        match value {
            Some(value) => timed.env(name, value),
            None => timed.env_remove(name),
        };
    }
    let output = timed
        .output()
        .map_err(|e| format!("running {command:?}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{command:?} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let usage = if counted {
        let text = String::from_utf8(oracle::read(usage_file)?).map_err(|e| e.to_string())?;
        Some(parse_usage(&text).ok_or_else(|| format!("unreadable time output: {text}"))?)
    } else {
        None
    };
    Ok((output.stdout, usage))
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Upstream,
    Port,
}

impl Side {
    fn command(self, paths: &Paths) -> Result<Command> {
        match self {
            Side::Upstream => oracle::oracle_command(paths, "decodeBench"),
            Side::Port => {
                let exe = release_dir().join(format!("asrs-cli{}", std::env::consts::EXE_SUFFIX));
                let mut command = Command::new(exe);
                command.arg("decode-bench");
                Ok(command)
            }
        }
    }
}

struct Sample {
    initial_ms: f64,
    total_ms: f64,
    usage: Usage,
}

struct Series {
    startup: Usage,
    samples: Vec<Sample>,
    loaded: Option<(f64, f64)>,
}

fn number(json: &Json, key: &str) -> Result<f64> {
    json.get(key)
        .and_then(Json::as_f64)
        .ok_or_else(|| format!("no number {key:?} in {}", json.to_compact()))
}

fn decode(args: &[String]) -> Result {
    let opts = options(args, &[])?;
    let paths = Paths::new()?;
    let files = if opts.positional.is_empty() {
        public::files(&paths)?
    } else {
        opts.positional.iter().map(PathBuf::from).collect()
    };
    cargo(&["build", "--quiet", "--release", "-p", "asrs-cli"])?;
    oracle::build_harness(&paths)?;
    let scratch = paths.root.join("target/perf");
    std::fs::create_dir_all(&scratch).map_err(|e| format!("{}: {e}", scratch.display()))?;
    let usage_file = scratch.join("usage.txt");
    let before = load_average();
    let mut rows = Vec::new();
    for file in &files {
        let mut sides = [Side::Upstream, Side::Port].map(|side| {
            (
                side,
                Series {
                    startup: Usage::default(),
                    samples: Vec::new(),
                    loaded: None,
                },
            )
        });
        for (side, series) in &mut sides {
            let mut command = side.command(&paths)?;
            command.arg("--startup");
            let mut startups = Vec::new();
            for _ in 0..opts.runs {
                startups.extend(measured(&command, &usage_file)?.1);
            }
            series.startup = Usage {
                cpu_ms: median(startups.iter().map(|u| u.cpu_ms)),
                instructions: median(startups.iter().map(|u| u.instructions)),
                peak_bytes: median(startups.iter().map(|u| u.peak_bytes)),
            };
        }
        for run in 0..opts.runs {
            let order = if run % 2 == 0 { [0, 1] } else { [1, 0] };
            for i in order {
                let (side, series) = &mut sides[i];
                let mut command = side.command(&paths)?;
                command.arg(file);
                let (out, usage) = measured(&command, &usage_file)?;
                let text = String::from_utf8(out).map_err(|e| e.to_string())?;
                let json = json::parse(text.trim()).map_err(|e| e.to_string())?;
                let loaded = (number(&json, "fields")?, number(&json, "samples")?);
                if series.loaded.is_some_and(|l| l != loaded) {
                    return Err(format!("{}: runs loaded different data", file.display()));
                }
                series.loaded = Some(loaded);
                let initial_ms = number(&json, "initialMs")?;
                series.samples.push(Sample {
                    initial_ms,
                    total_ms: initial_ms + number(&json, "fieldsMs")?,
                    usage: usage.unwrap_or_default(),
                });
            }
        }
        let [(_, upstream), (_, port)] = sides;
        if upstream.loaded != port.loaded {
            return Err(format!(
                "{}: upstream loaded {:?} (fields, samples), the port {:?}",
                file.display(),
                upstream.loaded,
                port.loaded
            ));
        }
        rows.push(row(file, &upstream, &port));
        eprintln!("{}", rows.last().expect("a row"));
    }
    println!("load average before: {before}; after: {}", load_average());
    println!(
        "{} runs a side, milliseconds as upstream / port (port ÷ upstream)\n",
        opts.runs
    );
    println!(
        "| log | fields, samples | initial load, median | whole decode, median | whole decode, p95 | CPU | instructions (M) | peak memory (MB) |\n|---|---|---|---|---|---|---|---|"
    );
    for row in rows {
        println!("{row}");
    }
    Ok(())
}

fn pair(upstream: f64, port: f64) -> String {
    format!("{upstream:.1} / {port:.1} ({:.2})", port / upstream)
}

fn row(file: &Path, upstream: &Series, port: &Series) -> String {
    let both = |of: &dyn Fn(&Series) -> f64| pair(of(upstream), of(port));
    let (fields, samples) = upstream.loaded.unwrap_or_default();
    let cells = [
        file.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        format!("{fields}, {samples}"),
        both(&|s| median(s.samples.iter().map(|x| x.initial_ms))),
        both(&|s| median(s.samples.iter().map(|x| x.total_ms))),
        both(&|s| p95(s.samples.iter().map(|x| x.total_ms))),
        both(&|s| median(s.samples.iter().map(|x| x.usage.cpu_ms)) - s.startup.cpu_ms),
        both(&|s| {
            (median(s.samples.iter().map(|x| x.usage.instructions)) - s.startup.instructions) / 1e6
        }),
        both(&|s| median(s.samples.iter().map(|x| x.usage.peak_bytes)) / 1e6),
    ];
    format!("| {} |", cells.join(" | "))
}

/// The user data folder whose `userAssets` holds the release assets the scenes show.
fn extract_assets(paths: &Paths) -> Result<PathBuf> {
    let user_data = paths.root.join("target/perf/userData");
    for name in ASSETS {
        let dir = user_data.join("userAssets").join(name);
        if dir.join("config.json").is_file() {
            continue;
        }
        let zip = paths
            .cache
            .join(format!("asset-zips/default-assets-v2/{name}.zip"));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        oracle::exec(
            Command::new("unzip")
                .args(["-o", "-q"])
                .arg(&zip)
                .arg("-d")
                .arg(&dir),
        )?;
    }
    Ok(user_data)
}

fn frames(args: &[String]) -> Result {
    let opts = options(args, &["--scene", "--log"])?;
    if !opts.positional.is_empty() {
        return Err(USAGE.to_owned());
    }
    cargo(&[
        "build",
        "--quiet",
        "--release",
        "-p",
        "asrs-app",
        "--features",
        "parity",
        "--example",
        "frame_bench",
    ])?;
    let exe = release_dir().join("examples/frame_bench");
    let paths = Paths::new()?;
    let assets = extract_assets(&paths)?;
    let before = load_average();
    let mut scenes: Vec<(String, Vec<Json>)> = Vec::new();
    for _ in 0..opts.runs {
        let mut command = Command::new(&exe);
        command
            .args(&opts.forwarded)
            .arg("--cache")
            .arg(&paths.cache)
            .arg("--assets")
            .arg(&assets);
        let output = command
            .stderr(std::process::Stdio::inherit())
            .output()
            .map_err(|e| format!("running {}: {e}", exe.display()))?;
        if !output.status.success() {
            return Err(format!("frame_bench failed ({})", output.status));
        }
        let text = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
        for line in text.lines() {
            let json = json::parse(line).map_err(|e| e.to_string())?;
            let name = json
                .get("scene")
                .and_then(Json::as_str)
                .ok_or_else(|| format!("no scene in {line}"))?
                .to_string();
            match scenes.iter_mut().find(|(n, _)| *n == name) {
                Some((_, runs)) => runs.push(json),
                None => scenes.push((name, vec![json])),
            }
        }
    }
    println!("load average before: {before}; after: {}", load_average());
    println!(
        "{} runs, medians over the runs, milliseconds\n\n| scene | frame p50 | frame p95 | frame max | frame CPU (mean) | frame and image p95 | budget {FRAME_BUDGET_MS} | memory (MB) |\n|---|---|---|---|---|---|---|---|",
        opts.runs
    );
    let mut over = Vec::new();
    for (name, runs) in &scenes {
        let value = |key: &str| -> Result<f64> {
            let values: Result<Vec<f64>> = runs.iter().map(|r| number(r, key)).collect();
            Ok(median(values?.into_iter()))
        };
        let frame_p95 = value("p95")?;
        let verdict = if frame_p95 < FRAME_BUDGET_MS {
            "met"
        } else {
            over.push(name.clone());
            "over"
        };
        println!(
            "| {name} | {:.2} | {frame_p95:.2} | {:.2} | {:.2} | {:.2} | {verdict} | {:.0} |",
            value("p50")?,
            value("max")?,
            value("cpu")?,
            value("imageP95")?,
            value("memoryMb")?,
        );
    }
    if over.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "p95 not under {FRAME_BUDGET_MS} ms: {}",
            over.join(", ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{median, p95, parse_usage};

    #[test]
    fn reads_time_output() {
        let text = "        0.26 real         0.20 user         0.05 sys\n           158089216  maximum resident set size\n          4077519066  instructions retired\n          1010518361  cycles elapsed\n           137577600  peak memory footprint\n";
        let usage = parse_usage(text).expect("parsed");
        assert_eq!(usage.cpu_ms, 250.0);
        assert_eq!(usage.instructions, 4_077_519_066.0);
        assert_eq!(usage.peak_bytes, 137_577_600.0);
    }

    #[test]
    fn percentiles() {
        let values = || [5.0, 1.0, 4.0, 2.0, 3.0].into_iter();
        assert_eq!(median(values()), 3.0);
        assert_eq!(median([1.0, 2.0].into_iter()), 1.5);
        assert_eq!(p95(values()), 5.0);
        assert_eq!(p95((1..=100).map(f64::from)), 95.0);
    }
}
