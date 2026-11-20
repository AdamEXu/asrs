use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use asrs_parity::random::{Rng, random_value};
use asrs_parity::{Request, Response, diff, vectors};

use crate::oracle::{self, Paths};
use crate::{Result, cargo};

const USAGE: &str = "usage: cargo xtask parity <suite> [options]

Runs the oracle and asrs-cli on the same input and compares their output. On a mismatch
the structured differ prints the differing field path, sample index and value bits.

suites:
  cve1 [--count N] [--seed S] [--plant]
        CVE-1 round trip on both sides: every case of the committed cve1/torture vectors
        and N (default 2000) seeded random values go through `oracle:identity` in the
        oracle (TS decoder and encoder) and in asrs-cli (Rust decoder and encoder). The
        oracle must still match the committed vectors and the two sides must agree.
  fn <spec id or .jsonl path>... [--plant]
        stream fn requests through both sides and compare every response
  decode [<log file>...] [--timeout SECS]
        decode log files with both sides and compare the canonical dumps: every
        .dslog/.dsevents file under the corpus b tree, the public log cache and the
        fuzz corpora, or just the files listed. An oracle that does not finish in time
        and an asrs-cli that exits with code 3 mean 'never completes'; that verdict
        must agree, completed dumps must be byte-identical, and anything else fails.
  diff <expected> <actual>
        structured diff of two files: JSONL (.jsonl or .jsonl.zst) line by line,
        anything else byte by byte

--plant flips one bit in one Rust response before comparing, to show what a mismatch
report looks like; the run then fails on purpose.";

const SHOWN: usize = 5;

pub fn run(args: &[String]) -> Result {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("cve1") => cve1(rest),
        Some("fn") => fn_suite(rest),
        Some("decode") => decode_suite(rest),
        Some("diff") => match rest {
            [expected, actual] => diff_files(Path::new(expected), Path::new(actual)),
            _ => Err(USAGE.to_owned()),
        },
        Some(
            suite @ ("js" | "log" | "export" | "live" | "model" | "trace" | "files" | "pixel"),
        ) => Err(format!(
            "parity suite {suite:?} is not implemented yet; its harness lands with the units that port it"
        )),
        _ => Err(USAGE.to_owned()),
    }
}

struct Options {
    count: usize,
    seed: u64,
    plant: bool,
    positional: Vec<String>,
}

fn options(args: &[String]) -> Result<Options> {
    let mut out = Options {
        count: 2000,
        seed: 1,
        plant: false,
        positional: Vec::new(),
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let mut number = |name: &str| -> Result<u64> {
            iter.next()
                .and_then(|v| v.parse().ok())
                .ok_or_else(|| format!("{name} needs a number"))
        };
        match arg.as_str() {
            "--count" => out.count = number("--count")? as usize,
            "--seed" => out.seed = number("--seed")?,
            "--plant" => out.plant = true,
            flag if flag.starts_with("--") => {
                return Err(format!("unknown option {flag}\n\n{USAGE}"));
            }
            _ => out.positional.push(arg.clone()),
        }
    }
    Ok(out)
}

struct Case {
    label: String,
    request: String,
    oracle: Option<String>,
    rust: Option<String>,
}

fn cve1(args: &[String]) -> Result {
    let opts = options(args)?;
    let records = vectors::load("cve1/torture")?;
    let mut cases = Vec::new();
    let mut skipped = 0;
    for record in &records {
        let input = record
            .string("input")
            .ok_or("torture record without input")?;
        if input.trim().is_empty() {
            skipped += 1;
            continue;
        }
        let expect = |text: Option<String>| {
            Some(match text {
                Some(t) => format!(r#"{{"ok":{t}}}"#),
                None => r#"{"error":""}"#.to_owned(),
            })
        };
        let output = record.string("output");
        cases.push(Case {
            label: format!("torture {:?}", record.string("case").unwrap_or_default()),
            request: format!(r#"{{"fn":"oracle:identity","args":[{input}]}}"#),
            oracle: expect(output.clone()),
            rust: expect(record.string("rust").or(output)),
        });
    }
    let torture = cases.len();
    let mut rng = Rng::new(opts.seed);
    for i in 0..opts.count {
        let value = random_value(&mut rng, 4);
        let expected = Response::Ok(value.clone()).encode();
        cases.push(Case {
            label: format!("random #{i} (seed {})", opts.seed),
            request: Request::new("oracle:identity", vec![value]).encode(),
            oracle: Some(expected.clone()),
            rust: Some(expected),
        });
    }
    let plant_at = opts.plant.then(|| {
        cases
            .iter()
            .position(|c| c.label.contains("5000 numbers"))
            .unwrap_or(0)
    });
    let failures = compare(&cases, plant_at)?;
    eprintln!(
        "cve1: {torture} torture cases ({skipped} with an empty input skipped) and {} random values (seed {})",
        opts.count, opts.seed
    );
    finish("cve1", cases.len(), failures)
}

fn fn_suite(args: &[String]) -> Result {
    let opts = options(args)?;
    if opts.positional.is_empty() {
        return Err(USAGE.to_owned());
    }
    let root = crate::workspace_root();
    let mut cases = Vec::new();
    for spec in &opts.positional {
        let path = resolve_spec(&root, spec)?;
        let text = String::from_utf8(oracle::read(&path)?).map_err(|e| e.to_string())?;
        for (i, line) in text.lines().enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.trim().is_empty() {
                continue;
            }
            cases.push(Case {
                label: format!("{spec}:{}", i + 1),
                request: line.to_owned(),
                oracle: None,
                rust: None,
            });
        }
    }
    let plant_at = opts.plant.then_some(0);
    let failures = compare(&cases, plant_at)?;
    finish("fn", cases.len(), failures)
}

fn resolve_spec(root: &Path, spec: &str) -> Result<PathBuf> {
    let direct = PathBuf::from(spec);
    if direct.is_file() {
        return Ok(direct);
    }
    let in_specs = crate::vectors::specs_dir(root).join(format!("{spec}.jsonl"));
    if in_specs.is_file() {
        return Ok(in_specs);
    }
    Err(format!(
        "no fn spec {spec:?} (neither a file nor parity/specs/{spec}.jsonl)"
    ))
}

/// `cargo xtask parity decode`: `oracle decode` and `asrs-cli decode` on the same file must
/// produce the same verdict (a dump or "never completes") and the same dump bytes.
fn decode_suite(args: &[String]) -> Result {
    let mut timeout = 60u64;
    let mut positional = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--timeout" => {
                timeout = iter
                    .next()
                    .and_then(|v| v.parse().ok())
                    .ok_or("--timeout needs a number")?;
            }
            flag if flag.starts_with("--") => {
                return Err(format!("unknown option {flag}\n\n{USAGE}"));
            }
            _ => positional.push(arg.clone()),
        }
    }
    let paths = Paths::new()?;
    let cases = if positional.is_empty() {
        decode_cases(&paths)?
    } else {
        positional.iter().map(PathBuf::from).collect()
    };
    if cases.is_empty() {
        return Err("no decode cases found".to_owned());
    }
    cargo(&["build", "--quiet", "-p", "asrs-cli"])?;
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::workspace_root().join("target"))
        .join("debug")
        .join(format!("asrs-cli{}", std::env::consts::EXE_SUFFIX));
    let timeout = Duration::from_secs(timeout);
    let workers = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(4)
        .min(8);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<std::sync::Mutex<Option<(DecodeVerdict, DecodeVerdict)>>> = (0..cases.len())
        .map(|_| std::sync::Mutex::new(None))
        .collect();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= cases.len() {
                        return;
                    }
                    let path = &cases[i];
                    let oracle = DecodeVerdict::oracle(run_decode(
                        oracle::oracle_command(&paths, "decode")
                            .expect("oracle decode command")
                            .arg(path),
                        timeout,
                    ));
                    let rust = DecodeVerdict::port(run_decode(
                        Command::new(&target).arg("decode").arg(path),
                        timeout,
                    ));
                    *results[i].lock().expect("decode result poisoned") = Some((oracle, rust));
                }
            });
        }
    });
    let mut failures = Vec::new();
    let mut never = 0;
    for (i, path) in cases.iter().enumerate() {
        let (oracle, rust) = results[i]
            .lock()
            .expect("decode result poisoned")
            .take()
            .expect("every case ran");
        if matches!(oracle, DecodeVerdict::NeverCompletes) {
            never += 1;
        }
        match (oracle, rust) {
            (DecodeVerdict::Dump(want), DecodeVerdict::Dump(got)) => {
                if let Some(report) = diff::report_bytes(&want, &got) {
                    failures.push(format!("{}: {report}", path.display()));
                }
            }
            (DecodeVerdict::NeverCompletes, DecodeVerdict::NeverCompletes) => {}
            (o, r) => failures.push(format!(
                "{}: oracle {} but asrs-cli {}",
                path.display(),
                o.describe(),
                r.describe()
            )),
        }
    }
    eprintln!(
        "decode: {} cases, {never} where upstream never completes",
        cases.len()
    );
    finish("decode", cases.len(), failures)
}

/// The `.dslog`/`.dsevents` files the suite decodes: corpus b, the public log cache and the
/// generated fuzz corpora.
fn decode_cases(paths: &Paths) -> Result<Vec<PathBuf>> {
    let mut cases = Vec::new();
    let is_ds = |p: &Path| {
        p.extension()
            .is_some_and(|e| e == "dslog" || e == "dsevents")
    };
    collect_paths(
        &crate::workspace_root().join("target/parity/corpus/synthetic/b"),
        &is_ds,
        &mut cases,
    );
    collect_paths(&paths.cache.join("public-logs"), &is_ds, &mut cases);
    collect_paths(
        &crate::workspace_root().join("target/parity/corpus/fuzz"),
        &is_ds,
        &mut cases,
    );
    cases.sort();
    Ok(cases)
}

fn collect_paths(dir: &Path, keep: &dyn Fn(&Path) -> bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_paths(&path, keep, out);
        } else if keep(&path) {
            out.push(path);
        }
    }
}

const NEVER_COMPLETES: i32 = 3;

enum DecodeRun {
    Exited(Option<i32>, Vec<u8>),
    TimedOut,
}

enum DecodeVerdict {
    Dump(Vec<u8>),
    NeverCompletes,
    Broken(String),
}

impl DecodeVerdict {
    /// The oracle never completes when its worker loops, so a timeout is that verdict there.
    fn oracle(run: DecodeRun) -> Self {
        match run {
            DecodeRun::Exited(Some(0), out) => DecodeVerdict::Dump(out),
            DecodeRun::Exited(code, _) => DecodeVerdict::Broken(format!("exited with {code:?}")),
            DecodeRun::TimedOut => DecodeVerdict::NeverCompletes,
        }
    }

    /// asrs-cli detects the loop and says so with its own exit code; it must never time out.
    fn port(run: DecodeRun) -> Self {
        match run {
            DecodeRun::Exited(Some(0), out) => DecodeVerdict::Dump(out),
            DecodeRun::Exited(Some(NEVER_COMPLETES), _) => DecodeVerdict::NeverCompletes,
            DecodeRun::Exited(code, _) => DecodeVerdict::Broken(format!("exited with {code:?}")),
            DecodeRun::TimedOut => DecodeVerdict::Broken("timed out".to_owned()),
        }
    }

    fn describe(&self) -> String {
        match self {
            DecodeVerdict::Dump(bytes) => format!("dumped {} bytes", bytes.len()),
            DecodeVerdict::NeverCompletes => "never completes".to_owned(),
            DecodeVerdict::Broken(why) => why.clone(),
        }
    }
}

fn run_decode(command: &mut Command, timeout: Duration) -> DecodeRun {
    use std::io::Read as _;
    let mut child = match command.stdout(Stdio::piped()).stderr(Stdio::null()).spawn() {
        Ok(child) => child,
        Err(_) => return DecodeRun::Exited(None, Vec::new()),
    };
    let mut pipe = child.stdout.take().expect("stdout is piped");
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = pipe.read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => break None,
        }
    };
    if status.is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let out = reader.join().unwrap_or_default();
    match status {
        Some(status) => DecodeRun::Exited(status.code(), out),
        None => DecodeRun::TimedOut,
    }
}

/// Runs every request through both sides. A case with recorded expectations is checked
/// against them on each side; every case is also checked oracle against Rust, except where
/// the recorded expectations say the two sides differ on purpose.
fn compare(cases: &[Case], plant_at: Option<usize>) -> Result<Vec<String>> {
    let input: String = cases.iter().map(|c| format!("{}\n", c.request)).collect();
    let paths = Paths::new()?;
    let oracle_out = oracle::pipe(
        &mut oracle::oracle_command(&paths, "fn")?,
        input.clone().into_bytes(),
    )?;
    let mut rust_out: Vec<String> = oracle::pipe(&mut asrs_cli()?, input.into_bytes())?
        .lines()
        .map(str::to_owned)
        .collect();
    let oracle_out: Vec<&str> = oracle_out.lines().collect();
    if oracle_out.len() != cases.len() || rust_out.len() != cases.len() {
        return Err(format!(
            "{} requests, but the oracle answered {} and asrs-cli {}",
            cases.len(),
            oracle_out.len(),
            rust_out.len()
        ));
    }
    if let Some(i) = plant_at {
        rust_out[i] =
            plant(&rust_out[i]).ok_or("--plant: the chosen response has no number to flip")?;
        eprintln!(
            "planted a one-bit change in the Rust response for {}",
            cases[i].label
        );
    }
    let mut failures = Vec::new();
    for (i, case) in cases.iter().enumerate() {
        let (got_oracle, got_rust) = (oracle_out[i], rust_out[i].as_str());
        let mut problems = Vec::new();
        if let Some(want) = &case.oracle
            && let Some(report) = diff::report_responses(want, got_oracle)
        {
            problems.push(format!("oracle vs committed vector: {report}"));
        }
        if let Some(want) = &case.rust
            && let Some(report) = diff::report_responses(want, got_rust)
        {
            problems.push(format!("asrs-cli vs expected: {report}"));
        }
        if case.oracle == case.rust
            && let Some(report) = diff::report_responses(got_oracle, got_rust)
        {
            problems.push(format!("asrs-cli vs oracle: {report}"));
        }
        if !problems.is_empty() {
            failures.push(format!(
                "{} (line {}): {}\n  {}",
                case.label,
                i + 1,
                truncate(&case.request, 160),
                problems.join("\n").replace('\n', "\n  ")
            ));
        }
    }
    Ok(failures)
}

/// Flips the lowest bit of the middle number in a response line.
fn plant(line: &str) -> Option<String> {
    let starts: Vec<usize> = line
        .match_indices(r#"{"n":""#)
        .map(|(i, _)| i + 6)
        .collect();
    let start = *starts.get(starts.len() / 2)?;
    let digit = start + 15;
    let flipped = u8::from_str_radix(line.get(digit..digit + 1)?, 16).ok()? ^ 1;
    Some(format!(
        "{}{flipped:x}{}",
        &line[..digit],
        &line[digit + 1..]
    ))
}

fn finish(suite: &str, total: usize, failures: Vec<String>) -> Result {
    if failures.is_empty() {
        eprintln!("parity {suite}: {total}/{total} cases match");
        return Ok(());
    }
    for failure in failures.iter().take(SHOWN) {
        eprintln!("\n{failure}");
    }
    if failures.len() > SHOWN {
        eprintln!("\n... and {} more", failures.len() - SHOWN);
    }
    Err(format!(
        "\nparity {suite}: {} of {total} cases differ",
        failures.len()
    ))
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!("{}...", text.chars().take(max).collect::<String>())
    }
}

fn asrs_cli() -> Result<Command> {
    cargo(&["build", "--quiet", "-p", "asrs-cli"])?;
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::workspace_root().join("target"));
    let mut command = Command::new(
        target
            .join("debug")
            .join(format!("asrs-cli{}", std::env::consts::EXE_SUFFIX)),
    );
    oracle::pin_environment(&mut command, oracle::DEFAULT_ZONE);
    command.arg("fn");
    Ok(command)
}

fn diff_files(expected: &Path, actual: &Path) -> Result {
    let is_jsonl = |p: &Path| {
        let name = p.to_string_lossy();
        name.ends_with(".jsonl") || name.ends_with(".jsonl.zst")
    };
    let report = if is_jsonl(expected) && is_jsonl(actual) {
        let (e, a) = (vectors::read_text(expected)?, vectors::read_text(actual)?);
        diff::report_jsonl(&e, &a, SHOWN, diff::report_responses)
    } else {
        diff::report_bytes(&oracle::read(expected)?, &oracle::read(actual)?)
    };
    match report {
        None => {
            eprintln!("identical");
            Ok(())
        }
        Some(report) => Err(report),
    }
}
