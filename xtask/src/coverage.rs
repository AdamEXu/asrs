use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use asrs_parity::json::Json;

use crate::Result;
use crate::oracle::{self, Paths};
use crate::vectors::{self, SpecRun};

const USAGE: &str = "usage: cargo xtask oracle coverage <spec>... [options]

Runs each spec's oracle under V8 precise coverage and reports, per upstream TypeScript file and
function, the lines the specs execute. A <spec> is a spec or vector id, an id prefix
(dom/field-2d) or the path of a spec file or directory. Covers the Node entries (fn, decode, the
main-process and model generators), their worker threads, the upstream worker bundles, code they
bundle at run time, and the pages and main processes of the Electron windows they open (Blink
`dom`). Module evaluation is left out, and so is, per run, what the same harness executes on an
empty fixture or request list.

  --file <path>       an upstream file or directory under src/ the unit ports (repeatable); the
                      report covers only these, function by function. Without it, one line per
                      upstream file reached.
  --require <t>       a place every run must reach: <file>:<function> or <file>:<line>[-<line>]
                      (repeatable)
  --out <file>        write the Markdown report there instead of stdout
  --jobs N            parallel runs (default 3)

Fails when a run reaches none of the listed files, a run misses a --require, no run loads a listed
file, or a run prints anything but its committed vector.";

struct Options {
    specs: Vec<String>,
    files: Vec<String>,
    require: Vec<String>,
    out: Option<PathBuf>,
    jobs: usize,
}

fn options(args: &[String]) -> Result<Options> {
    let mut o = Options {
        specs: Vec::new(),
        files: Vec::new(),
        require: Vec::new(),
        out: None,
        jobs: 3,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if !arg.starts_with("--") {
            o.specs.push(arg.clone());
            continue;
        }
        let value = args.next().cloned().ok_or_else(|| USAGE.to_owned())?;
        match arg.as_str() {
            "--file" => o.files.push(upstream_path(&value)),
            "--require" => o.require.push(upstream_path(&value)),
            "--out" => o.out = Some(PathBuf::from(value)),
            "--jobs" => {
                o.jobs = value
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| USAGE.to_owned())?;
            }
            _ => return Err(USAGE.to_owned()),
        }
    }
    if o.specs.is_empty() {
        return Err(USAGE.to_owned());
    }
    Ok(o)
}

/// PLAN rows write ported files as `src/...`; the report's paths are relative to `src/`.
fn upstream_path(path: &str) -> String {
    path.strip_prefix("src/").unwrap_or(path).to_owned()
}

pub(crate) fn run(paths: &Paths, args: &[String]) -> Result {
    let options = options(args)?;
    oracle::installed(paths)?;
    let coverage = paths.out.join("coverage");
    let dist = coverage.join("dist");
    let bundles = coverage.join("bundles");
    build(paths, &dist)?;
    let mut covered = paths.clone();
    covered.dist = dist.clone();
    let runs = vectors::spec_runs(&covered, &options.specs)?;
    upstream_bundles(paths, &bundles)?;

    let raw = coverage.join(format!("raw-{}", std::process::id()));
    let result = measure(paths, &covered, &options, &raw, runs, &bundles);
    let _ = fs::remove_dir_all(&raw);
    result
}

fn measure(
    paths: &Paths,
    covered: &Paths,
    options: &Options,
    raw: &Path,
    mut runs: Vec<SpecRun>,
    bundles: &Path,
) -> Result {
    let mut baselines: Vec<SpecRun> = Vec::new();
    for run in &runs {
        if !baselines.iter().any(|b| b.entry == run.entry)
            && let Some(baseline) = baseline(covered, raw, run)?
        {
            baselines.push(baseline);
        }
    }
    let first = baselines.len();
    let ids: Vec<(String, String)> = runs
        .iter()
        .map(|r| {
            let base = baselines.iter().find(|b| b.entry == r.entry);
            (r.id.clone(), base.map(|b| b.id.clone()).unwrap_or_default())
        })
        .collect();
    let dir_of = |k: usize| raw.join(format!("run-{k}"));
    let mut jobs: Vec<(usize, SpecRun)> = baselines
        .drain(..)
        .chain(runs.drain(..))
        .enumerate()
        .collect();
    let base_ids: Vec<String> = jobs[..first].iter().map(|(_, r)| r.id.clone()).collect();
    jobs.reverse();
    let queue = Mutex::new(jobs);
    let failures = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..options.jobs {
            scope.spawn(|| {
                loop {
                    let next = queue.lock().expect("no panics under the lock").pop();
                    let Some((k, run)) = next else { break };
                    eprintln!("==> {}", run.id);
                    if let Err(e) = execute(paths, &dir_of(k), run) {
                        failures.lock().expect("no panics under the lock").push(e);
                    }
                }
            });
        }
    });
    let failures = failures.into_inner().expect("no panics under the lock");
    if !failures.is_empty() {
        return Err(format!("oracle coverage: {}", failures.join("\n")));
    }

    let path = |p: &Path| Json::string(&p.to_string_lossy());
    let strings = |list: &[String]| Json::Array(list.iter().map(|s| Json::string(s)).collect());
    let request = Json::object([
        ("upstream", path(&paths.upstream)),
        ("dist", path(&covered.dist)),
        ("bundles", path(bundles)),
        (
            "runs",
            Json::Array(
                ids.iter()
                    .enumerate()
                    .map(|(i, (id, base))| {
                        Json::object([
                            ("id", Json::string(id)),
                            ("dir", path(&dir_of(first + i))),
                            ("baseline", Json::string(base)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "baselines",
            Json::Array(
                base_ids
                    .iter()
                    .enumerate()
                    .map(|(k, id)| {
                        Json::object([("id", Json::string(id)), ("dir", path(&dir_of(k)))])
                    })
                    .collect(),
            ),
        ),
        ("files", strings(&options.files)),
        ("require", strings(&options.require)),
    ]);
    let request_path = raw.join("request.json");
    fs::write(&request_path, request.to_compact()).map_err(|e| e.to_string())?;
    let output = oracle::node()
        .arg(paths.oracle_src.join("coverage/analyze.mjs"))
        .arg(&request_path)
        .stderr(std::process::Stdio::inherit())
        .output()
        .map_err(|e| format!("running node: {e}"))?;
    let report = String::from_utf8_lossy(&output.stdout);
    match &options.out {
        Some(file) => {
            fs::write(file, report.as_bytes()).map_err(|e| format!("{}: {e}", file.display()))?;
        }
        None => print!("{report}"),
    }
    if output.status.success() {
        return Ok(());
    }
    let problems = report
        .split_once("## Problems")
        .map(|(_, p)| p.trim())
        .unwrap_or("the report could not be made");
    Err(format!("oracle coverage failed:\n{problems}"))
}

/// The same harness on nothing, for the entries that have an empty input.
fn baseline(covered: &Paths, raw: &Path, run: &SpecRun) -> Result<Option<SpecRun>> {
    let mut command = oracle::oracle_command_in(covered, &run.entry, &run.zone)?;
    match run.entry.as_str() {
        "fn" => {}
        "dom" | "domBlink" | "hubSession" => {
            fs::create_dir_all(raw).map_err(|e| format!("{}: {e}", raw.display()))?;
            let fixture = raw.join("empty.json");
            fs::write(&fixture, "{\"steps\":[]}").map_err(|e| e.to_string())?;
            command.arg(fixture).current_dir(&covered.root);
        }
        _ => return Ok(None),
    }
    Ok(Some(SpecRun {
        id: format!("empty {}", run.entry),
        entry: run.entry.clone(),
        zone: run.zone.clone(),
        command,
        input: Vec::new(),
        committed: None,
    }))
}

fn execute(paths: &Paths, dir: &Path, mut run: SpecRun) -> Result {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    run.command
        .env("NODE_OPTIONS", require_option(paths, "preload.cjs"))
        .env("ASRS_COVERAGE_OUT", dir);
    let input = run.input.clone();
    let stdout = oracle::pipe(&mut run.command, input).map_err(|e| format!("{}: {e}", run.id))?;
    if let Some(committed) = &run.committed
        && *committed != vectors::printed_vector(&run, &stdout)?
    {
        return Err(format!(
            "{}: the run under coverage differs from the committed vector (stale, or the coverage hooks changed it)",
            run.id
        ));
    }
    Ok(())
}

fn require_option(paths: &Paths, file: &str) -> String {
    let path = paths.oracle_src.join("coverage").join(file);
    format!(
        "--require={}",
        path.to_string_lossy()
            .replace('\\', "/")
            .replace(' ', "\\ ")
    )
}

/// The harness bundles with source maps, apart from the ones `oracle gen` runs.
fn build(paths: &Paths, dist: &Path) -> Result {
    let mut build = paths.clone();
    build.dist = dist.to_path_buf();
    oracle::build_harness_with(&build, |command| {
        command.env("NODE_OPTIONS", require_option(paths, "buildPreload.cjs"));
    })
}

/// Upstream's own bundles (the worker scripts run from them) once more, with source maps.
fn upstream_bundles(paths: &Paths, bundles: &Path) -> Result {
    let config = paths.oracle_src.join("coverage/rolldown.config.mjs");
    let inputs = oracle::hash_tree(
        &paths.upstream,
        &[
            "src",
            "rolldown.config.mjs",
            "package-lock.json",
            "tsconfig.json",
        ],
    )? + &oracle::hash_text(&oracle::read(&config)?);
    let missing = !bundles.join("hub.js.map").exists();
    oracle::stamped(paths, "coverage-bundles", &inputs, missing, || {
        oracle::step("rolldown -c (upstream's bundles with source maps)");
        oracle::exec(
            std::process::Command::new(paths.upstream.join("node_modules/.bin/rolldown"))
                .arg("-c")
                .arg(&config)
                .env("NO_COLOR", "1")
                .env("ASRS_COVERAGE_BUNDLES", bundles)
                .stdout(std::io::stderr())
                .current_dir(&paths.upstream),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_take_repeated_files_and_requirements() {
        let args = [
            "dom/field-2d",
            "--file",
            "src/shared/renderers/Field2dRenderer.ts",
            "--require",
            "shared/renderers/Field2dRenderer.ts:270-290",
            "--jobs",
            "2",
        ]
        .map(str::to_owned);
        let o = options(&args).unwrap();
        assert_eq!(o.files, ["shared/renderers/Field2dRenderer.ts"]);
        assert_eq!(o.require, ["shared/renderers/Field2dRenderer.ts:270-290"]);
        assert_eq!(o.jobs, 2);
        assert!(options(&["--file".to_owned(), "x".to_owned()]).is_err());
        assert!(options(&["spec".to_owned(), "--bogus".to_owned(), "x".to_owned()]).is_err());
    }
}
