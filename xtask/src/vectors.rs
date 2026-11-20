use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use asrs_parity::json::{self, Json};
use asrs_parity::{Response, diff, vectors};

use crate::Result;
use crate::oracle::{self, Paths};

const MANIFEST: &str = "MANIFEST.json";
const FN_SPEC: &str = ".jsonl";
const ENTRY_SPEC: &str = ".entry.json";

pub(crate) fn specs_dir(root: &Path) -> PathBuf {
    root.join("parity/specs")
}

enum Spec {
    Fn(PathBuf),
    Entry {
        path: PathBuf,
        entry: String,
        args: Vec<String>,
    },
}

impl Spec {
    fn path(&self) -> &Path {
        match self {
            Spec::Fn(path) | Spec::Entry { path, .. } => path,
        }
    }
}

pub(crate) fn generate(paths: &Paths, args: &[String]) -> Result {
    let check = args.iter().any(|a| a == "--check");
    if let Some(flag) = args.iter().find(|a| a.starts_with("--") && *a != "--check") {
        return Err(format!("oracle gen: unknown option {flag}"));
    }
    let wanted: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let specs = discover(&specs_dir(&paths.root))?;
    for id in &wanted {
        if !specs.iter().any(|(sid, _)| sid == *id) {
            return Err(format!("oracle gen: no spec {id:?} under parity/specs"));
        }
    }
    let selected: Vec<&(String, Spec)> = specs
        .iter()
        .filter(|(id, _)| wanted.is_empty() || wanted.contains(&id))
        .collect();
    if selected.is_empty() {
        return Err("oracle gen: no specs under parity/specs".to_owned());
    }

    oracle::build_harness(paths)?;
    let vectors_dir = paths.root.join("parity/vectors");
    let oracle_hash = oracle::hash_tree(
        &paths.oracle_src,
        &["src", "build.mjs", "package-lock.json"],
    )?;
    let mut manifest = read_manifest(&vectors_dir)?;
    let mut failures = Vec::new();

    for (id, spec) in selected {
        eprintln!("==> {id}");
        let text = run_spec(paths, spec)?;
        let cases = text.lines().count();
        let content = oracle::hash_bytes(text.as_bytes());
        if check {
            let committed = vectors::vector_path(&vectors_dir, id)
                .ok_or_else(|| format!("{id}: no committed vector"))
                .and_then(|p| vectors::read_text(&p));
            match committed {
                Ok(committed) => {
                    if let Some(report) =
                        diff::report_jsonl(&committed, &text, 3, diff::report_lines)
                    {
                        failures.push(format!(
                            "{id}: regenerated vector differs from the committed one\n{report}"
                        ));
                    } else if manifest_field(&manifest, id, "content").as_deref()
                        != Some(content.as_str())
                    {
                        failures.push(format!("{id}: MANIFEST.json content hash is stale"));
                    } else {
                        eprintln!("  {cases} cases, identical");
                    }
                }
                Err(e) => failures.push(e),
            }
            continue;
        }
        let out = vectors_dir.join(format!("{id}.jsonl.zst"));
        fs::create_dir_all(out.parent().expect("vector path has a parent"))
            .map_err(|e| e.to_string())?;
        fs::write(&out, vectors::compress(&text)).map_err(|e| format!("{}: {e}", out.display()))?;
        let plain = vectors_dir.join(format!("{id}.jsonl"));
        if plain.exists() {
            fs::remove_file(&plain).map_err(|e| format!("{}: {e}", plain.display()))?;
        }
        let spec_rel = spec
            .path()
            .strip_prefix(&paths.root)
            .unwrap_or(spec.path())
            .to_string_lossy()
            .replace('\\', "/");
        let generator = oracle::hash_bytes(
            format!(
                "{oracle_hash}:{}",
                oracle::hash_bytes(&oracle::read(spec.path())?)
            )
            .as_bytes(),
        );
        set_entry(
            &mut manifest,
            id,
            Json::object([
                ("cases", Json::Number(cases as f64)),
                ("content", Json::string(&content)),
                ("generator", Json::string(&generator)),
                ("spec", Json::string(&spec_rel)),
            ]),
        );
        eprintln!(
            "  {cases} cases -> {}",
            out.strip_prefix(&paths.root).unwrap_or(&out).display()
        );
    }

    if check {
        return if failures.is_empty() {
            eprintln!("oracle gen --check: vectors are up to date");
            Ok(())
        } else {
            Err(failures.join("\n\n"))
        };
    }
    write_manifest(paths, &vectors_dir, manifest)
}

fn discover(dir: &Path) -> Result<Vec<(String, Spec)>> {
    let mut found = Vec::new();
    if dir.is_dir() {
        visit(dir, dir, &mut found)?;
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

fn visit(base: &Path, dir: &Path, found: &mut Vec<(String, Spec)>) -> Result {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            visit(base, &path, found)?;
            continue;
        }
        let rel = path
            .strip_prefix(base)
            .expect("under base")
            .to_string_lossy()
            .replace('\\', "/");
        if let Some(id) = rel.strip_suffix(ENTRY_SPEC) {
            let text = String::from_utf8(oracle::read(&path)?).map_err(|e| e.to_string())?;
            let json = json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            let entry = json
                .get("entry")
                .and_then(Json::as_str)
                .ok_or_else(|| format!("{}: needs a string \"entry\"", path.display()))?
                .to_string_lossy();
            let args = json
                .get("args")
                .and_then(Json::as_array)
                .unwrap_or_default()
                .iter()
                .map(|a| a.as_str().map(|s| s.to_string_lossy()))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| format!("{}: \"args\" must be strings", path.display()))?;
            found.push((id.to_owned(), Spec::Entry { path, entry, args }));
        } else if let Some(id) = rel.strip_suffix(FN_SPEC) {
            found.push((id.to_owned(), Spec::Fn(path)));
        }
    }
    Ok(())
}

fn run_spec(paths: &Paths, spec: &Spec) -> Result<String> {
    match spec {
        Spec::Entry { entry, args, .. } => {
            let text = oracle::pipe(oracle::oracle_command(paths, entry)?.args(args), Vec::new())?;
            for (i, line) in text.lines().enumerate() {
                json::parse(line).map_err(|e| format!("{entry} output line {}: {e}", i + 1))?;
            }
            Ok(text)
        }
        Spec::Fn(path) => {
            let requests = oracle::read(path)?;
            let request_text = String::from_utf8(requests.clone()).map_err(|e| e.to_string())?;
            let responses = oracle::pipe(&mut oracle::oracle_command(paths, "fn")?, requests)?;
            merge_fn_records(&request_text, &responses)
                .map_err(|e| format!("{}: {e}", path.display()))
        }
    }
}

/// Each vector line is the request object with the oracle's response key appended.
fn merge_fn_records(requests: &str, responses: &str) -> Result<String> {
    let requests: Vec<&str> = requests
        .lines()
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .filter(|l| !l.trim().is_empty())
        .collect();
    let responses: Vec<&str> = responses.lines().collect();
    if requests.len() != responses.len() {
        return Err(format!(
            "{} requests but {} responses",
            requests.len(),
            responses.len()
        ));
    }
    let mut out = String::new();
    for (i, (request, response)) in requests.iter().zip(&responses).enumerate() {
        let line = i + 1;
        if let Ok(Response::Error(message)) = Response::parse(response) {
            return Err(format!("request {line} failed in the oracle: {message}"));
        }
        let Json::Object(mut pairs) =
            json::parse(request).map_err(|e| format!("request {line}: {e}"))?
        else {
            return Err(format!("request {line} is not an object"));
        };
        let Json::Object(response) =
            json::parse(response).map_err(|e| format!("response {line}: {e}"))?
        else {
            return Err(format!("response {line} is not an object"));
        };
        pairs.retain(|(k, _)| !["ok", "throw", "error"].iter().any(|r| k.eq_str(r)));
        pairs.extend(response);
        out.push_str(&Json::Object(pairs).to_compact());
        out.push('\n');
    }
    Ok(out)
}

fn read_manifest(dir: &Path) -> Result<Json> {
    let path = dir.join(MANIFEST);
    if !path.exists() {
        return Ok(Json::Object(Vec::new()));
    }
    let text = String::from_utf8(oracle::read(&path)?).map_err(|e| e.to_string())?;
    json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn manifest_field(manifest: &Json, id: &str, field: &str) -> Option<String> {
    manifest
        .get("vectors")?
        .get(id)?
        .get(field)?
        .as_str()
        .map(|s| s.to_string_lossy())
}

fn set_entry(manifest: &mut Json, id: &str, entry: Json) {
    let mut vectors = match manifest.get("vectors") {
        Some(Json::Object(pairs)) => pairs.clone(),
        _ => Vec::new(),
    };
    vectors.retain(|(k, _)| !k.eq_str(id));
    vectors.push((id.into(), entry));
    vectors.sort_by(|a, b| a.0.cmp(&b.0));
    set_field(manifest, "vectors", Json::Object(vectors));
}

fn set_field(object: &mut Json, key: &str, value: Json) {
    if let Json::Object(pairs) = object {
        match pairs.iter_mut().find(|(k, _)| k.eq_str(key)) {
            Some(slot) => slot.1 = value,
            None => pairs.push((key.into(), value)),
        }
    }
}

fn write_manifest(paths: &Paths, dir: &Path, mut manifest: Json) -> Result {
    let commit = Command::new("git")
        .args(["-C"])
        .arg(&paths.reference)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    let package = String::from_utf8(oracle::read(&paths.reference.join("package.json"))?)
        .map_err(|e| e.to_string())?;
    let version = json::parse(&package)
        .ok()
        .and_then(|p| {
            p.get("version")
                .and_then(Json::as_str)
                .map(|v| v.to_string_lossy())
        })
        .unwrap_or_default();
    let vectors = manifest
        .get("vectors")
        .cloned()
        .unwrap_or(Json::Object(Vec::new()));
    let mut ordered = Json::object([
        ("format", Json::string("CVE-1")),
        ("upstreamCommit", Json::string(&commit)),
        ("upstreamVersion", Json::string(&version)),
        ("electron", Json::string(oracle::ELECTRON_VERSION)),
    ]);
    set_field(&mut ordered, "vectors", vectors);
    manifest = ordered;
    let path = dir.join(MANIFEST);
    fs::write(&path, manifest.to_pretty() + "\n").map_err(|e| format!("{}: {e}", path.display()))
}
