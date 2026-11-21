use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use asrs_parity::json::{self, Json};

use crate::Result;
use crate::corpus;
use crate::oracle::{self, Paths};

pub(crate) const USAGE: &str =
    "usage: cargo xtask oracle historical [<corpus>...] [--check] [--jobs N]

Runs the `decode` dump and the export matrix of parity/corpus/historical.spec.json over every
log file of each synthetic corpus (default: all), using the real HistoricalDataSource and
worker bundles. Outputs go to target/parity/oracle/<corpus>/<input>/ (decode.jsonl and
export/<format>.<sampling>.<generated|original>.p<prefix index>.<ext>), with every output's
hash in index.jsonl there. parity/corpus/synthetic/<corpus>.historical.json records one hash
per input for the decode dump and for the export set.

  --check   rerun into target/parity/oracle/<corpus>.check and fail unless every output is
            byte-identical to the committed manifest
  --jobs N  oracle processes to run in parallel (default: available cores, at most 8)

The corpus must already be generated (`cargo xtask corpus gen`).";

pub(crate) const SPEC: &str = "parity/corpus/historical.spec.json";
pub(crate) const MANIFEST_SUFFIX: &str = ".historical.json";
pub(crate) const LOG_EXTENSIONS: &[&str] =
    &[".wpilog", ".rlog", ".csv", ".dslog", ".log", ".dsevents"];

struct Options {
    check: bool,
    jobs: usize,
    names: Vec<String>,
}

fn options(args: &[String]) -> Result<Options> {
    let mut out = Options {
        check: false,
        jobs: std::thread::available_parallelism().map_or(4, |n| n.get().min(8)),
        names: Vec::new(),
    };
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--check" => out.check = true,
            "--jobs" => {
                out.jobs = iter
                    .next()
                    .and_then(|v| v.parse().ok())
                    .filter(|&n: &usize| n > 0)
                    .ok_or("--jobs needs a positive number")?;
            }
            flag if flag.starts_with("--") => {
                return Err(format!("unknown option {flag}\n\n{USAGE}"));
            }
            name => out.names.push(name.to_owned()),
        }
    }
    Ok(out)
}

pub(crate) fn run(paths: &Paths, args: &[String]) -> Result {
    let opts = options(args)?;
    let corpora = corpus::names(&paths.root)?;
    for name in &opts.names {
        if !corpora.contains(name) {
            return Err(format!("oracle historical: no corpus {name:?}"));
        }
    }
    oracle::build_harness(paths)?;
    let spec_path = paths.root.join(SPEC);
    let generator = oracle::hash_bytes(
        format!(
            "{}:{}",
            oracle::hash_tree(
                &paths.oracle_src,
                &["src", "build.mjs", "package-lock.json"]
            )?,
            oracle::hash_bytes(&oracle::read(&spec_path)?)
        )
        .as_bytes(),
    );
    let mut failures = Vec::new();
    for name in corpora
        .iter()
        .filter(|n| opts.names.is_empty() || opts.names.contains(n))
    {
        eprintln!("==> historical {name}");
        let inputs = corpus_inputs(paths, name)?;
        let out_root = paths.root.join("target/parity/oracle");
        let staging = out_root.join(format!(".{name}.staging"));
        corpus::remove_dir(&staging)?;
        fs::create_dir_all(&staging).map_err(|e| format!("{}: {e}", staging.display()))?;
        let records = run_shards(paths, name, &inputs, &staging, &spec_path, opts.jobs)?;
        let (manifest, index) = build_manifest(paths, name, &generator, &records, &staging)?;
        fs::write(staging.join("index.jsonl"), index).map_err(|e| e.to_string())?;
        let manifest_path =
            corpus::synthetic_dir(&paths.root).join(format!("{name}{MANIFEST_SUFFIX}"));
        let dest = if opts.check {
            out_root.join(format!("{name}.check"))
        } else {
            out_root.join(name)
        };
        corpus::remove_dir(&dest)?;
        fs::rename(&staging, &dest).map_err(|e| format!("{}: {e}", dest.display()))?;
        let rel = dest
            .strip_prefix(&paths.root)
            .unwrap_or(&dest)
            .display()
            .to_string();
        if opts.check {
            let committed =
                String::from_utf8(oracle::read(&manifest_path)?).map_err(|e| e.to_string())?;
            match corpus::compare(&committed, &manifest) {
                Some(report) => failures.push(format!("{name}: {report}\n  outputs kept in {rel}")),
                None => eprintln!("  {} inputs, byte-identical ({rel})", records.len()),
            }
        } else {
            fs::write(&manifest_path, &manifest)
                .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
            eprintln!("  {} inputs -> {rel}", records.len());
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

/// Log files of a generated corpus (the extensions HistoricalDataSource has a worker for), checked against its committed manifest so the oracle never
/// runs on stale inputs.
pub(crate) fn corpus_inputs(paths: &Paths, name: &str) -> Result<Vec<String>> {
    let manifest_path = corpus::synthetic_dir(&paths.root).join(format!("{name}.manifest.json"));
    let text = String::from_utf8(oracle::read(&manifest_path)?).map_err(|e| e.to_string())?;
    let manifest = json::parse(&text).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let dir = corpus::output_dir(&paths.root, name);
    let mut files = Vec::new();
    for case in manifest
        .get("list")
        .and_then(Json::as_array)
        .unwrap_or_default()
    {
        for file in case
            .get("files")
            .and_then(Json::as_array)
            .unwrap_or_default()
        {
            let field = |key: &str| {
                file.get(key)
                    .and_then(Json::as_str)
                    .map(|s| s.to_string_lossy())
                    .ok_or_else(|| format!("{}: file without {key}", manifest_path.display()))
            };
            let rel = field("path")?;
            let path = dir.join(&rel);
            let stale = || {
                format!(
                    "{} is missing or differs from the committed manifest; run `cargo xtask corpus gen {name}`",
                    path.display()
                )
            };
            let bytes = fs::read(&path).map_err(|_| stale())?;
            if oracle::hash_bytes(&bytes) != field("hash")? {
                return Err(stale());
            }
            files.push(rel);
        }
    }
    // Opening a .dsevents file opens its .dslog sibling, so the pair counts once.
    let inputs = files
        .iter()
        .filter(|rel| LOG_EXTENSIONS.iter().any(|ext| rel.ends_with(ext)))
        .filter(|rel| {
            rel.strip_suffix(".dsevents")
                .is_none_or(|base| !files.contains(&format!("{base}.dslog")))
        })
        .cloned()
        .collect();
    Ok(inputs)
}

fn run_shards(
    paths: &Paths,
    name: &str,
    inputs: &[String],
    staging: &Path,
    spec: &Path,
    jobs: usize,
) -> Result<Vec<Json>> {
    let root = corpus::output_dir(&paths.root, name);
    let jobs = jobs.min(inputs.len()).max(1);
    let shards: Vec<Vec<String>> = (0..jobs)
        .map(|j| inputs.iter().skip(j).step_by(jobs).cloned().collect())
        .collect();
    let results: Vec<Result<String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = shards
            .iter()
            .enumerate()
            .map(|(j, shard)| {
                let root = &root;
                scope.spawn(move || run_shard(paths, root, staging, spec, j, shard))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("shard thread panicked".to_owned()))
            })
            .collect()
    });
    let mut records = Vec::new();
    for result in results {
        for line in result?.lines() {
            let record = json::parse(line).map_err(|e| format!("historicalRun output: {e}"))?;
            if let Some(error) = record.get("error").and_then(Json::as_str) {
                let input = record
                    .get("input")
                    .and_then(Json::as_str)
                    .map(|s| s.to_string_lossy());
                return Err(format!(
                    "{input:?}: oracle error: {}",
                    error.to_string_lossy()
                ));
            }
            records.push(record);
        }
    }
    let input_of = |r: &Json| {
        r.get("input")
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .unwrap_or_default()
    };
    records.sort_by_key(input_of);
    if records.len() != inputs.len() {
        return Err(format!(
            "historicalRun reported {} of {} inputs; see {}/oracle-*.log",
            records.len(),
            inputs.len(),
            staging.display()
        ));
    }
    Ok(records)
}

fn run_shard(
    paths: &Paths,
    root: &Path,
    staging: &Path,
    spec: &Path,
    index: usize,
    shard: &[String],
) -> Result<String> {
    let log_path = staging.join(format!("oracle-{index}.log"));
    let log = fs::File::create(&log_path).map_err(|e| format!("{}: {e}", log_path.display()))?;
    let mut child = oracle::oracle_command(paths, "historicalRun")?
        .arg("--root")
        .arg(root)
        .arg("--out")
        .arg(staging)
        .arg("--spec")
        .arg(spec)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::from(log))
        .spawn()
        .map_err(|e| format!("starting the oracle: {e}"))?;
    let input: String = shard.iter().map(|s| format!("{s}\n")).collect();
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "historicalRun shard {index} exited with {}; see {}",
            output.status,
            log_path.display()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| e.to_string())
}

/// One manifest line per input keeps diffs readable; index.jsonl beside the outputs has the
/// hash of every single output.
fn build_manifest(
    paths: &Paths,
    name: &str,
    generator: &str,
    records: &[Json],
    dir: &Path,
) -> Result<(String, String)> {
    let mut lines = Vec::new();
    let mut index = String::new();
    let mut content = String::new();
    let mut total_ok = 0usize;
    for record in records {
        let input = str_field(record, "input")?;
        let mut decode = String::new();
        let mut export_listing = String::new();
        let (mut ok, mut failed, mut skipped) = (0usize, 0usize, 0usize);
        for output in record
            .get("outputs")
            .and_then(Json::as_array)
            .unwrap_or_default()
        {
            let output_name = str_field(output, "name")?;
            let outcome = str_field(output, "outcome")?;
            let (hash, size) = match outcome.as_str() {
                "ok" => {
                    let bytes = oracle::read(&dir.join(&input).join(str_field(output, "file")?))?;
                    (oracle::hash_bytes(&bytes), bytes.len())
                }
                "failed" | "skipped" => (String::new(), 0),
                other => return Err(format!("{input}: unknown outcome {other:?}")),
            };
            index.push_str(
                &Json::object([
                    ("input", Json::string(&input)),
                    ("output", Json::string(&output_name)),
                    ("outcome", Json::string(&outcome)),
                    ("size", Json::Number(size as f64)),
                    ("hash", Json::string(&hash)),
                ])
                .to_compact(),
            );
            index.push('\n');
            if output_name == "decode" {
                decode = hash;
                continue;
            }
            match outcome.as_str() {
                "ok" => ok += 1,
                "failed" => failed += 1,
                _ => skipped += 1,
            }
            export_listing.push_str(&format!("{output_name}\0{outcome}\0{hash}\n"));
        }
        total_ok += ok + 1;
        let line = Json::object([
            ("input", Json::string(&input)),
            ("decode", Json::string(&decode)),
            (
                "exports",
                Json::string(&oracle::hash_bytes(export_listing.as_bytes())),
            ),
            ("ok", Json::Number(ok as f64)),
            ("failed", Json::Number(failed as f64)),
            ("skipped", Json::Number(skipped as f64)),
        ])
        .to_compact();
        content.push_str(&line);
        content.push('\n');
        lines.push(line);
    }
    let header = Json::object([
        ("corpus", Json::string(name)),
        ("spec", Json::string(SPEC)),
        (
            "upstreamCommit",
            Json::string(&oracle::installed(paths)?.upstream_commit),
        ),
        ("electron", Json::string(oracle::ELECTRON_VERSION)),
        ("generator", Json::string(generator)),
        ("inputs", Json::Number(lines.len() as f64)),
        ("files", Json::Number(total_ok as f64)),
        (
            "content",
            Json::string(&oracle::hash_bytes(content.as_bytes())),
        ),
    ])
    .to_pretty();
    let header = header.strip_suffix("\n}").ok_or("manifest header shape")?;
    let manifest = format!(
        "{header},\n  \"list\": [\n    {}\n  ]\n}}\n",
        lines.join(",\n    ")
    );
    Ok((manifest, index))
}

fn str_field(record: &Json, key: &str) -> Result<String> {
    record
        .get(key)
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| format!("historicalRun record needs a string {key:?}"))
}
