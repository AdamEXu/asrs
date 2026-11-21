//! `cargo xtask parity historical`: scripted hub sessions run through the real
//! HistoricalDataSource (`oracle historicalSession`) and through the port
//! (`asrs-cli historical-session`), compared trace for trace.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use asrs_parity::diff;
use asrs_parity::json::{self, Json};

use crate::oracle::{self, Paths};
use crate::parity::{SHOWN, decode_cases, finish, plant};
use crate::{Result, cargo, public};

pub(crate) const USAGE: &str =
    "usage: cargo xtask parity historical [<log file>...] [--jobs N] [--shard N] [--plant]

For every log file of the decode suite (corpus a and b, the public log cache, the fuzz corpora)
or just the files listed, runs two scripted hub sessions through both sides and compares the
traces: every step, status, progress and refresh callback, main message, worker creation,
worker request and termination, page throw and Log dump.
  main:<file>   open it, request fields over time, merge a second log (its public merge pair,
                else the next file) under /Log1, load everything, stop
  stops:<file>  stop a source while main reads, then another while fields are decoding
plus one session with a missing file and a file no worker reads.
A session the port does not model yet ends with an `unmodeled` line; its trace up to there
must be a prefix of the oracle's.
  --jobs N    shards run in parallel (default 4)
  --shard N   sessions per oracle process (default 16)
  --plant     flip one bit in the first Rust trace, so the run fails on purpose";

const SHARD_TIMEOUT_SECS: u64 = 1800;

fn step(op: &str, extra: &[(&str, Json)]) -> Json {
    let mut pairs = vec![("op", Json::string(op))];
    pairs.extend(extra.iter().cloned());
    Json::object(pairs)
}

fn no_settle() -> (&'static str, Json) {
    ("settle", Json::Bool(false))
}

fn numbers(values: &[f64]) -> Json {
    Json::Array(values.iter().map(|&v| Json::Number(v)).collect())
}

fn open(path: &str, prefix: &str, settle: bool) -> Json {
    let mut extra = vec![
        ("path", Json::string(path)),
        ("prefix", Json::string(prefix)),
    ];
    if !settle {
        extra.push(no_settle());
    }
    step("open", &extra)
}

fn pick(indices: &[f64], parents: &[f64]) -> Json {
    step(
        "pick",
        &[("indices", numbers(indices)), ("parents", numbers(parents))],
    )
}

fn frame(ms: f64, settle: bool) -> Json {
    let mut extra = vec![("ms", Json::Number(ms))];
    if !settle {
        extra.push(no_settle());
    }
    step("frame", &extra)
}

fn dump(phase: &str) -> Json {
    step("dump", &[("phase", Json::string(phase))])
}

fn main_script(a: &str, b: &str) -> Vec<Json> {
    vec![
        open(a, "", false),
        frame(16.0, false),
        step("settle", &[]),
        dump("initial"),
        frame(40.0, true),
        pick(&[0.0, 2.0, -1.0], &[1.0]),
        step("tick", &[]),
        dump("picked"),
        step("tick", &[]),
        open(b, "/Log1", false),
        frame(16.0, false),
        step("settle", &[]),
        pick(&[-1.0, -3.0, 1.0], &[-2.0]),
        step("tick", &[]),
        dump("merged"),
        step("loadEverything", &[]),
        dump("full"),
        step("stop", &[]),
        frame(16.0, true),
        step("tick", &[]),
        dump("stopped"),
    ]
}

fn stops_script(a: &str) -> Vec<Json> {
    vec![
        open(a, "", false),
        frame(16.0, false),
        step("stop", &[("source", Json::Number(0.0)), no_settle()]),
        step("settle", &[]),
        dump("stopped-reading"),
        open(a, "/Log1", true),
        pick(&[0.0, 1.0, -1.0], &[2.0]),
        step("tick", &[no_settle()]),
        step("stop", &[("source", Json::Number(1.0)), no_settle()]),
        step("settle", &[]),
        frame(16.0, true),
        step("tick", &[]),
        dump("stopped-decoding"),
    ]
}

fn session(name: String, steps: Vec<Json>) -> (String, String) {
    let line = Json::object([("name", Json::string(&name)), ("steps", Json::Array(steps))]);
    (name, line.to_compact())
}

/// The partner merged into each file's main session: its public merge pair, else the next file.
fn partners(paths: &Paths, files: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let groups = public::merge_groups(paths)?;
    let group_of = |file: &Path| groups.iter().find(|(p, _)| p == file).map(|(_, g)| g);
    Ok(files
        .iter()
        .enumerate()
        .map(|(i, file)| {
            group_of(file)
                .and_then(|group| {
                    groups
                        .iter()
                        .find(|(p, g)| g == group && p != file && files.contains(p))
                        .map(|(p, _)| p.clone())
                })
                .unwrap_or_else(|| files[(i + 1) % files.len()].clone())
        })
        .collect())
}

fn sessions(paths: &Paths, files: &[PathBuf]) -> Result<Vec<(String, String)>> {
    let text = |p: &Path| p.to_string_lossy().into_owned();
    let mut out = Vec::new();
    for (file, partner) in files.iter().zip(partners(paths, files)?) {
        let rel = file
            .strip_prefix(&paths.root)
            .or_else(|_| file.strip_prefix(&paths.cache))
            .unwrap_or(file)
            .display()
            .to_string();
        out.push(session(
            format!("main:{rel}"),
            main_script(&text(file), &text(&partner)),
        ));
        out.push(session(format!("stops:{rel}"), stops_script(&text(file))));
    }
    let missing = paths.root.join("target/parity/corpus/missing.wpilog");
    let unreadable = paths.root.join("parity/corpus/historical.spec.json");
    out.push(session(
        "errors".to_owned(),
        vec![
            open(&text(&missing), "", true),
            open(&text(&unreadable), "/Log1", true),
            dump("errors"),
        ],
    ));
    Ok(out)
}

fn run_capture(command: &mut Command) -> std::result::Result<Vec<u8>, String> {
    use std::io::Read as _;
    use std::time::{Duration, Instant};
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let mut stderr = child.stderr.take().expect("stderr is piped");
    let out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + Duration::from_secs(SHARD_TIMEOUT_SECS);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break None,
        }
    };
    if status.is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let out = out.join().unwrap_or_default();
    let err = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    match status {
        Some(status) if status.success() => Ok(out),
        Some(status) => Err(format!("exited with {status}: {}", err.trim())),
        None => Err(format!("did not finish in {SHARD_TIMEOUT_SECS} s")),
    }
}

/// The name in a `{"session": name}` trace line, unescaped.
fn session_name(line: &str) -> Option<String> {
    let record = json::parse(line.trim_end()).ok()?;
    let [entry] = record.get("o")?.as_array()? else {
        return None;
    };
    let [tag, name] = entry.as_array()? else {
        return None;
    };
    tag.as_str()?
        .eq_str("session")
        .then(|| name.as_str().map(ToString::to_string))
        .flatten()
}

/// Splits a run's output into one trace per session, keyed by name.
fn split(out: &[u8]) -> HashMap<String, Vec<u8>> {
    let mut traces = HashMap::new();
    let mut current: Option<(String, Vec<u8>)> = None;
    for line in out.split_inclusive(|&b| b == b'\n') {
        let text = String::from_utf8_lossy(line);
        if text.starts_with(r#"{"o":[["session","#)
            && let Some(name) = session_name(&text)
        {
            current = Some((name, line.to_vec()));
            continue;
        }
        if let Some((_, trace)) = &mut current {
            trace.extend_from_slice(line);
        }
        if text.starts_with(r#"{"o":[["end","#)
            && let Some((name, trace)) = current.take()
        {
            traces.insert(name, trace);
        }
    }
    traces
}

const UNMODELED_LINE: &str = r#"{"o":[["unmodeled","#;

pub(crate) fn run(args: &[String]) -> Result {
    let mut jobs = 4usize;
    let mut shard = 16usize;
    let mut plant_first = false;
    let mut positional = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut count = |name: &str| {
            iter.next()
                .and_then(|v| v.parse().ok())
                .filter(|&n: &usize| n > 0)
                .ok_or(format!("{name} needs a positive number"))
        };
        match arg.as_str() {
            "--jobs" => jobs = count("--jobs")?,
            "--shard" => shard = count("--shard")?,
            "--plant" => plant_first = true,
            "--help" => return Err(USAGE.to_owned()),
            flag if flag.starts_with("--") => {
                return Err(format!("unknown option {flag}\n\n{USAGE}"));
            }
            _ => positional.push(PathBuf::from(arg)),
        }
    }
    let paths = Paths::new()?;
    let files = if positional.is_empty() {
        decode_cases(&paths)?
    } else {
        positional
            .iter()
            .map(|p| fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display())))
            .collect::<Result<_>>()?
    };
    if files.is_empty() {
        return Err("no log files found".to_owned());
    }
    cargo(&["build", "--quiet", "-p", "asrs-cli"])?;
    oracle::build_harness(&paths)?;
    let cli = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::workspace_root().join("target"))
        .join("debug")
        .join(format!("asrs-cli{}", std::env::consts::EXE_SUFFIX));

    let all = sessions(&paths, &files)?;
    let work = paths.root.join("target/parity/historical");
    fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let shards: Vec<&[(String, String)]> = all.chunks(shard).collect();
    for (i, chunk) in shards.iter().enumerate() {
        let text: String = chunk.iter().map(|(_, line)| format!("{line}\n")).collect();
        fs::write(work.join(format!("shard-{i}.jsonl")), text).map_err(|e| e.to_string())?;
    }
    eprintln!(
        "historical: {} files, {} sessions in {} shards, {jobs} at a time",
        files.len(),
        all.len(),
        shards.len()
    );

    type Outputs = (
        std::result::Result<Vec<u8>, String>,
        std::result::Result<Vec<u8>, String>,
    );
    let results: Vec<Mutex<Option<Outputs>>> = shards.iter().map(|_| Mutex::new(None)).collect();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..jobs {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= shards.len() {
                        return;
                    }
                    let file = work.join(format!("shard-{i}.jsonl"));
                    let oracle_out = oracle::oracle_command(&paths, "historicalSession")
                        .and_then(|mut c| run_capture(c.arg(&file)));
                    let mut rust = Command::new(&cli);
                    oracle::pin_environment(&mut rust, oracle::DEFAULT_ZONE);
                    let rust_out = run_capture(rust.arg("historical-session").arg(&file));
                    *results[i].lock().expect("shard result poisoned") =
                        Some((oracle_out, rust_out));
                    let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                    if n.is_multiple_of(20) || n == shards.len() {
                        eprintln!("  {n}/{} shards", shards.len());
                    }
                }
            });
        }
    });

    let mut failures = Vec::new();
    let mut unmodeled = Vec::new();
    let mut planted = !plant_first;
    for (i, chunk) in shards.iter().enumerate() {
        let (oracle_out, rust_out) = results[i]
            .lock()
            .expect("shard result poisoned")
            .take()
            .expect("every shard ran");
        let (oracle_out, rust_out) = match (oracle_out, rust_out) {
            (Ok(o), Ok(r)) => (o, r),
            (o, r) => {
                for (side, result) in [("oracle", o), ("asrs-cli", r)] {
                    if let Err(why) = result {
                        failures.push(format!("shard {i}: {side} {why}"));
                    }
                }
                continue;
            }
        };
        let (want, mut got) = (split(&oracle_out), split(&rust_out));
        for (name, _) in chunk.iter() {
            let (Some(want), Some(got)) = (want.get(name), got.get_mut(name)) else {
                failures.push(format!(
                    "{name}: missing from the {} output",
                    if want.contains_key(name) {
                        "asrs-cli"
                    } else {
                        "oracle"
                    }
                ));
                continue;
            };
            if !planted {
                let mut lines: Vec<String> = String::from_utf8_lossy(got)
                    .lines()
                    .map(str::to_owned)
                    .collect();
                if let Some(at) = lines.iter().position(|l| l.contains(r#"{"n":""#))
                    && let Some(flipped) = plant(&lines[at])
                {
                    lines[at] = flipped;
                    *got = lines
                        .iter()
                        .map(|l| format!("{l}\n"))
                        .collect::<String>()
                        .into_bytes();
                    planted = true;
                    eprintln!("planted a one-bit change in the Rust trace of {name}");
                }
            }
            if String::from_utf8_lossy(want).contains(r#"{"o":[["error","#) {
                failures.push(format!("{name}: the oracle reported a harness error"));
                continue;
            }
            let text = String::from_utf8_lossy(got);
            if let Some(at) = text.find(UNMODELED_LINE) {
                let prefix = &got[..at];
                if want.starts_with(prefix) {
                    unmodeled.push(name.clone());
                } else if let Some(report) =
                    diff::report_bytes(&want[..prefix.len().min(want.len())], prefix)
                {
                    failures.push(format!("{name}: before the unmodeled step, {report}"));
                } else {
                    failures.push(format!("{name}: asrs-cli printed more than the oracle"));
                }
                continue;
            }
            if let Some(report) = diff::report_bytes(want, got) {
                failures.push(format!("{name}: {report}"));
            }
        }
    }
    eprintln!(
        "historical: {} sessions compared, {} the port does not model yet",
        all.len(),
        unmodeled.len()
    );
    for name in unmodeled.iter().take(SHOWN) {
        eprintln!("  not modeled: {name}");
    }
    finish("historical", all.len() - unmodeled.len(), failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_keys_traces_by_the_unescaped_session_name() {
        let out = concat!(
            r#"{"o":[["session","main:C:\\logs\\a \"b\".wpilog"]]}"#,
            "\n",
            r#"{"o":[["step",{"n":"0000000000000000"}]]}"#,
            "\n",
            r#"{"o":[["end","main:C:\\logs\\a \"b\".wpilog"]]}"#,
            "\n",
        );
        let traces = split(out.as_bytes());
        assert_eq!(
            traces.get(r#"main:C:\logs\a "b".wpilog"#).map(Vec::len),
            Some(out.len())
        );
    }
}
