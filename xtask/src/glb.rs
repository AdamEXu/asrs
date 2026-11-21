use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use asrs_parity::diff;

use crate::oracle::{self, Paths};
use crate::{Result, cargo, workspace_root};

pub fn run(args: &[String]) -> Result {
    let mut zips = None;
    let mut jobs = 4usize;
    let mut plant = false;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--zips" => {
                zips = Some(PathBuf::from(
                    iter.next().ok_or("--zips needs a directory")?,
                ))
            }
            "--jobs" => {
                jobs = iter
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--jobs needs a number")?
            }
            "--plant" => plant = true,
            other => {
                return Err(format!(
                    "unknown argument {other}; usage: cargo xtask parity glb [--zips <dir>] [--jobs N] [--plant]"
                ));
            }
        }
    }
    let paths = Paths::new()?;
    let zips = zips.unwrap_or_else(|| paths.cache.join("asset-zips"));
    let assets = workspace_root().join("target/parity/glb-release");
    let count = extract(&zips, &assets)?;
    eprintln!("{count} asset folders in {}", assets.display());

    let dir = assets.to_string_lossy().into_owned();
    let jobs_text = jobs.to_string();
    let started = Instant::now();
    let expected = capture(
        oracle::oracle_command(&paths, "genGlb")?.args(["release", &dir, "--jobs", &jobs_text]),
        "oracle",
    )?;
    eprintln!("oracle: {:.1} s", started.elapsed().as_secs_f64());
    cargo(&["build", "--quiet", "--release", "-p", "asrs-cli"])?;
    let started = Instant::now();
    let cli = target_dir()
        .join("release")
        .join(format!("asrs-cli{}", std::env::consts::EXE_SUFFIX));
    let actual = capture(
        Command::new(cli).args(["glb", "release", &dir, "--jobs", &jobs_text]),
        "asrs-cli",
    )?;
    eprintln!("asrs-cli: {:.1} s", started.elapsed().as_secs_f64());

    let mut actual_lines: Vec<String> = actual.lines().map(str::to_owned).collect();
    if plant && let Some(line) = actual_lines.iter_mut().find(|l| l.contains(r#"{"n":""#)) {
        let at = line.find(r#"{"n":""#).expect("found above") + 6 + 15;
        let flipped = u8::from_str_radix(&line[at..at + 1], 16).map_err(|e| e.to_string())? ^ 1;
        line.replace_range(at..at + 1, &format!("{flipped:x}"));
    }
    let expected_lines: Vec<&str> = expected.lines().collect();
    let mut failures = Vec::new();
    let mut never = 0;
    for i in 0..expected_lines.len().max(actual_lines.len()) {
        let (e, a) = (
            expected_lines.get(i).copied(),
            actual_lines.get(i).map(String::as_str),
        );
        if e.is_some_and(|e| e.contains(r#""out":{"never":true}"#)) {
            never += 1;
        }
        match (e, a) {
            (Some(e), Some(a)) => {
                if let Some(report) = diff::report_lines(e, a) {
                    failures.push(format!(
                        "line {}: {}\n  {}",
                        i + 1,
                        case_of(e),
                        report.replace('\n', "\n  ")
                    ));
                }
            }
            (Some(e), None) => failures.push(format!(
                "line {}: {} missing from asrs-cli",
                i + 1,
                case_of(e)
            )),
            (None, Some(a)) => failures.push(format!(
                "line {}: {} missing from the oracle",
                i + 1,
                case_of(a)
            )),
            (None, None) => {}
        }
    }
    let total = expected_lines.len();
    for failure in failures.iter().take(5) {
        eprintln!("\n{failure}");
    }
    if failures.is_empty() {
        eprintln!("parity glb: {total}/{total} cases match ({never} where upstream never answers)");
        Ok(())
    } else {
        Err(format!(
            "\nparity glb: {} of {total} cases differ",
            failures.len()
        ))
    }
}

fn case_of(line: &str) -> String {
    line.split(",\"out\":")
        .next()
        .unwrap_or(line)
        .chars()
        .take(160)
        .collect()
}

fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace_root().join("target"))
}

fn capture(command: &mut Command, side: &str) -> Result<String> {
    oracle::pin_environment(command, oracle::DEFAULT_ZONE);
    let output = command
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("running {side}: {e}"))?;
    if !output.status.success() {
        return Err(format!("{side} failed ({})", output.status));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("{side} output: {e}"))
}

/// Extracts each distinct `Field3d_*`/`Robot_*` zip under `zips` into `<out>/<zip stem>`.
fn extract(zips: &Path, out: &Path) -> Result<usize> {
    let mut found = BTreeMap::new();
    collect(zips, &mut found)?;
    if found.is_empty() {
        return Err(format!("no Field3d_/Robot_ zips under {}", zips.display()));
    }
    fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    for (stem, zip) in &found {
        let target = out.join(stem);
        if target.join("config.json").is_file() {
            continue;
        }
        let partial = out.join(format!(".{stem}.partial"));
        if partial.exists() {
            fs::remove_dir_all(&partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        }
        let status = Command::new("unzip")
            .args(["-q", "-o"])
            .arg(zip)
            .arg("-d")
            .arg(&partial)
            .status()
            .map_err(|e| format!("running unzip: {e}"))?;
        if !status.success() {
            return Err(format!("unzip {} failed ({status})", zip.display()));
        }
        fs::rename(&partial, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    Ok(found.len())
}

fn collect(dir: &Path, found: &mut BTreeMap<String, PathBuf>) -> Result {
    let mut entries: Vec<_> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, found)?;
            continue;
        }
        let Some(stem) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".zip"))
        else {
            continue;
        };
        if (stem.starts_with("Field3d_") || stem.starts_with("Robot_")) && !found.contains_key(stem)
        {
            found.insert(stem.to_owned(), path);
        }
    }
    Ok(())
}
