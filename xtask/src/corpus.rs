use std::fs;
use std::path::{Path, PathBuf};

use asrs_parity::json::{self, Json};

use crate::Result;
use crate::oracle::{self, Paths};

const USAGE: &str = "usage: cargo xtask corpus <command>

commands:
  gen [<name>...] [--check]
            run each synthetic corpus generator (parity/corpus/synthetic/<name>.spec.json)
            into target/parity/corpus/synthetic/<name>/ and write
            parity/corpus/synthetic/<name>.manifest.json listing every case with file sizes
            and hashes; --check regenerates into a scratch directory and fails unless every
            file is byte-identical to the committed manifest
  list [<name>]
            print case ids and descriptions from the committed manifests
  fuzz [--seed <u32>] [--count <n>] [--check]
            mutate the generated synthetic corpora (run `corpus gen` first) into
            target/parity/corpus/fuzz/<seed>/ with list.jsonl naming each mutant's source and
            mutations; --check runs the fuzzer twice and fails unless both runs are identical
  fetch [<name>...] [--offline]
            download the public logs in parity/corpus/public.toml into
            $ASRS_CACHE/public-logs/, verifying size and sha256; files already present with the
            right hash are kept; --offline only verifies what is there";

const FUZZ_DEFAULT_SEED: &str = "1";
const FUZZ_DEFAULT_COUNT: &str = "1000";

const SPEC_SUFFIX: &str = ".spec.json";
const MANIFEST_SUFFIX: &str = ".manifest.json";

pub fn run(args: &[String]) -> Result {
    let paths = Paths::new()?;
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("gen") => generate(&paths, rest),
        Some("list") => list(&paths, rest),
        Some("fuzz") => fuzz(&paths, rest),
        Some("fetch") => crate::public::fetch(&paths, rest),
        _ => Err(USAGE.to_owned()),
    }
}

pub(crate) fn synthetic_dir(root: &Path) -> PathBuf {
    root.join("parity/corpus/synthetic")
}

pub(crate) fn output_dir(root: &Path, name: &str) -> PathBuf {
    root.join("target/parity/corpus/synthetic").join(name)
}

struct Spec {
    name: String,
    path: PathBuf,
    entry: String,
    seed: String,
}

fn discover(root: &Path) -> Result<Vec<Spec>> {
    let dir = synthetic_dir(root);
    let mut specs = Vec::new();
    for item in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = item.map_err(|e| e.to_string())?.path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(SPEC_SUFFIX))
            .map(str::to_owned)
        else {
            continue;
        };
        let text = String::from_utf8(oracle::read(&path)?).map_err(|e| e.to_string())?;
        let json = json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let field = |key: &str| {
            json.get(key)
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy())
                .ok_or_else(|| format!("{}: needs a string {key:?}", path.display()))
        };
        specs.push(Spec {
            entry: field("entry")?,
            seed: field("seed")?,
            name,
            path,
        });
    }
    specs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(specs)
}

fn generate(paths: &Paths, args: &[String]) -> Result {
    let check = args.iter().any(|a| a == "--check");
    if let Some(flag) = args.iter().find(|a| a.starts_with("--") && *a != "--check") {
        return Err(format!("corpus gen: unknown option {flag}"));
    }
    let wanted: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let specs = discover(&paths.root)?;
    for name in &wanted {
        if !specs.iter().any(|s| &s.name == *name) {
            return Err(format!(
                "corpus gen: no spec {name:?} under parity/corpus/synthetic"
            ));
        }
    }
    oracle::build_harness(paths)?;
    let oracle_hash = oracle::hash_tree(
        &paths.oracle_src,
        &["src", "build.mjs", "package-lock.json"],
    )?;
    let mut failures = Vec::new();
    for spec in specs
        .iter()
        .filter(|s| wanted.is_empty() || wanted.contains(&&s.name))
    {
        eprintln!("==> corpus {}", spec.name);
        let out = output_dir(&paths.root, &spec.name);
        let staging = out.with_file_name(format!(".{}.staging", spec.name));
        remove_dir(&staging)?;
        let stdout = oracle::pipe(
            oracle::oracle_command(paths, &spec.entry)?
                .arg("--seed")
                .arg(&spec.seed)
                .arg("--out")
                .arg(&staging),
            Vec::new(),
        )?;
        let generator = oracle::hash_bytes(
            format!(
                "{oracle_hash}:{}",
                oracle::hash_bytes(&oracle::read(&spec.path)?)
            )
            .as_bytes(),
        );
        let manifest = build_manifest(paths, spec, &generator, &stdout, &staging)?;
        let manifest_path =
            synthetic_dir(&paths.root).join(format!("{}{MANIFEST_SUFFIX}", spec.name));
        if check {
            let committed =
                String::from_utf8(oracle::read(&manifest_path)?).map_err(|e| e.to_string())?;
            remove_dir(&staging)?;
            match compare(&committed, &manifest) {
                Some(report) => failures.push(format!("{}: {report}", spec.name)),
                None => eprintln!("  {} cases, byte-identical", count_cases(&manifest)),
            }
            continue;
        }
        remove_dir(&out)?;
        fs::rename(&staging, &out).map_err(|e| format!("{}: {e}", out.display()))?;
        fs::write(&manifest_path, &manifest)
            .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        eprintln!(
            "  {} cases -> {}",
            count_cases(&manifest),
            out.strip_prefix(&paths.root).unwrap_or(&out).display()
        );
    }
    if failures.is_empty() {
        if check {
            eprintln!("corpus gen --check: every corpus regenerates byte-identically");
        }
        Ok(())
    } else {
        Err(failures.join("\n"))
    }
}

/// The manifest keeps one case per line so diffs stay readable.
fn build_manifest(
    paths: &Paths,
    spec: &Spec,
    generator: &str,
    stdout: &str,
    dir: &Path,
) -> Result<String> {
    let mut listed = Vec::new();
    let mut lines = Vec::new();
    let mut content = String::new();
    let mut total = 0u64;
    let mut previous: Option<String> = None;
    for (i, line) in stdout.lines().enumerate() {
        let record =
            json::parse(line).map_err(|e| format!("{} output line {}: {e}", spec.entry, i + 1))?;
        let id = str_field(&record, "id")?;
        if previous.as_ref().is_some_and(|p| p >= &id) {
            return Err(format!(
                "{}: case ids are not strictly ascending at {id:?}",
                spec.entry
            ));
        }
        previous = Some(id.clone());
        let mut files = Vec::new();
        for file in record
            .get("files")
            .and_then(Json::as_array)
            .unwrap_or_default()
        {
            let rel = file
                .as_str()
                .map(|s| s.to_string_lossy())
                .ok_or_else(|| format!("{id}: file paths must be strings"))?;
            let bytes = oracle::read(&dir.join(&rel))?;
            let hash = oracle::hash_bytes(&bytes);
            total += bytes.len() as u64;
            content.push_str(&format!("{rel}\0{hash}\n"));
            files.push(Json::object([
                ("path", Json::string(&rel)),
                ("size", Json::Number(bytes.len() as f64)),
                ("hash", Json::string(&hash)),
            ]));
            listed.push(rel);
        }
        lines.push(
            Json::object([
                ("id", Json::string(&id)),
                ("about", Json::string(&str_field(&record, "about")?)),
                ("files", Json::Array(files)),
            ])
            .to_compact(),
        );
    }
    let mut on_disk = Vec::new();
    collect_files(dir, dir, &mut on_disk)?;
    listed.sort();
    on_disk.sort();
    if listed != on_disk {
        let extra: Vec<_> = on_disk
            .iter()
            .filter(|p| !listed.contains(p))
            .take(5)
            .collect();
        let missing: Vec<_> = listed
            .iter()
            .filter(|p| !on_disk.contains(p))
            .take(5)
            .collect();
        return Err(format!(
            "{}: files on disk do not match the case list (unlisted {extra:?}, missing {missing:?})",
            spec.entry
        ));
    }
    let header = Json::object([
        ("corpus", Json::string(&spec.name)),
        ("entry", Json::string(&spec.entry)),
        ("seed", Json::string(&spec.seed)),
        (
            "upstreamCommit",
            Json::string(&oracle::installed(paths)?.upstream_commit),
        ),
        ("electron", Json::string(oracle::ELECTRON_VERSION)),
        ("generator", Json::string(generator)),
        ("cases", Json::Number(lines.len() as f64)),
        ("files", Json::Number(listed.len() as f64)),
        ("bytes", Json::Number(total as f64)),
        (
            "content",
            Json::string(&oracle::hash_bytes(content.as_bytes())),
        ),
    ])
    .to_pretty();
    let header = header.strip_suffix("\n}").ok_or("manifest header shape")?;
    Ok(format!(
        "{header},\n  \"list\": [\n    {}\n  ]\n}}\n",
        lines.join(",\n    ")
    ))
}

fn str_field(record: &Json, key: &str) -> Result<String> {
    record
        .get(key)
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| format!("generator record needs a string {key:?}"))
}

fn count_cases(manifest: &str) -> usize {
    manifest
        .lines()
        .filter(|l| l.trim_start().starts_with("{\"id\":"))
        .count()
}

/// Ignores the generator hash, so a change to the oracle that leaves the output alone still passes.
pub(crate) fn compare(committed: &str, regenerated: &str) -> Option<String> {
    let strip = |text: &str| -> Vec<String> {
        text.lines()
            .filter(|l| !l.trim_start().starts_with("\"generator\":"))
            .map(str::to_owned)
            .collect()
    };
    let (a, b) = (strip(committed), strip(regenerated));
    if a == b {
        return None;
    }
    let mut report = String::from("regenerated corpus differs from the committed manifest");
    let mut shown = 0;
    for line in b.iter().filter(|l| !a.contains(l)).take(5) {
        report.push_str(&format!("\n  new:  {line}"));
        shown += 1;
    }
    for line in a.iter().filter(|l| !b.contains(l)).take(5) {
        report.push_str(&format!("\n  gone: {line}"));
        shown += 1;
    }
    if shown == 0 {
        report.push_str("\n  (same lines, different order)");
    }
    Some(report)
}

pub(crate) fn names(root: &Path) -> Result<Vec<String>> {
    Ok(discover(root)?.into_iter().map(|s| s.name).collect())
}

fn list(paths: &Paths, args: &[String]) -> Result {
    for spec in discover(&paths.root)? {
        if args.first().is_some_and(|n| n != &spec.name) {
            continue;
        }
        let path = synthetic_dir(&paths.root).join(format!("{}{MANIFEST_SUFFIX}", spec.name));
        let text = String::from_utf8(oracle::read(&path)?).map_err(|e| e.to_string())?;
        let manifest = json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        for case in manifest
            .get("list")
            .and_then(Json::as_array)
            .unwrap_or_default()
        {
            println!(
                "{}/{}\t{}",
                spec.name,
                str_field(case, "id")?,
                str_field(case, "about")?
            );
        }
    }
    Ok(())
}

fn fuzz(paths: &Paths, args: &[String]) -> Result {
    let mut seed = FUZZ_DEFAULT_SEED.to_owned();
    let mut count = FUZZ_DEFAULT_COUNT.to_owned();
    let mut check = false;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--seed" => {
                seed = rest
                    .next()
                    .ok_or("corpus fuzz: --seed needs a value")?
                    .clone()
            }
            "--count" => {
                count = rest
                    .next()
                    .ok_or("corpus fuzz: --count needs a value")?
                    .clone()
            }
            "--check" => check = true,
            other => return Err(format!("corpus fuzz: unknown argument {other}")),
        }
    }
    let inputs: Vec<PathBuf> = discover(&paths.root)?
        .iter()
        .map(|spec| output_dir(&paths.root, &spec.name))
        .collect();
    if let Some(missing) = inputs.iter().find(|dir| !dir.is_dir()) {
        return Err(format!(
            "corpus fuzz: {} is missing; run `cargo xtask corpus gen` first",
            missing.display()
        ));
    }
    oracle::build_harness(paths)?;
    let out = paths.root.join("target/parity/corpus/fuzz").join(&seed);
    let run = |dir: &Path| -> Result<String> {
        remove_dir(dir)?;
        let mut command = oracle::oracle_command(paths, "corpusFuzz")?;
        command
            .args(["--seed", &seed, "--count", &count, "--out"])
            .arg(dir);
        for input in &inputs {
            command.arg("--in").arg(input);
        }
        let list = oracle::pipe(&mut command, Vec::new())?;
        let mut digest = String::new();
        for line in list.lines() {
            let record = json::parse(line).map_err(|e| format!("corpusFuzz output: {e}"))?;
            for file in record
                .get("files")
                .and_then(Json::as_array)
                .unwrap_or_default()
            {
                let rel = file
                    .as_str()
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                digest.push_str(&format!(
                    "{rel}\0{}\n",
                    oracle::hash_bytes(&oracle::read(&dir.join(&rel))?)
                ));
            }
        }
        fs::write(dir.join("list.jsonl"), &list).map_err(|e| format!("{}: {e}", dir.display()))?;
        Ok(format!(
            "{}{}",
            oracle::hash_bytes(list.as_bytes()),
            oracle::hash_bytes(digest.as_bytes())
        ))
    };
    eprintln!("==> corpus fuzz seed {seed}, {count} mutants");
    let first = run(&out)?;
    if check {
        let again = out.with_file_name(format!(".{seed}.check"));
        let second = run(&again)?;
        remove_dir(&again)?;
        if first != second {
            return Err(format!(
                "corpus fuzz --check: seed {seed} produced different output on a rerun"
            ));
        }
        eprintln!("corpus fuzz --check: seed {seed} reproduces byte-identically");
    }
    eprintln!(
        "  -> {}",
        out.strip_prefix(&paths.root).unwrap_or(&out).display()
    );
    Ok(())
}

pub(crate) fn remove_dir(dir: &Path) -> Result {
    match fs::remove_dir_all(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(format!("{}: {e}", dir.display()))
        }
        _ => Ok(()),
    }
}

fn collect_files(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result {
    for item in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = item.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect_files(base, &path, out)?;
        } else {
            let rel = path.strip_prefix(base).expect("under base");
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}
