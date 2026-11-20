use std::fs;
use std::path::{Path, PathBuf};

use asrs_parity::json::{self, Json};
use asrs_parity::{Response, diff, vectors};

use crate::Result;
use crate::oracle::{self, Paths};

const MANIFEST: &str = "MANIFEST.json";
const FN_SPEC: &str = ".jsonl";
const FN_ZONED_SPEC: &str = ".fn.json";
const REQUESTS: &str = ".requests.jsonl";
const ENTRY_SPEC: &str = ".entry.json";
const REGENERATE: &str = "regenerate with `cargo xtask oracle gen` and commit the result";

pub(crate) fn specs_dir(root: &Path) -> PathBuf {
    root.join("parity/specs")
}

enum Kind {
    Fn { requests: PathBuf },
    Entry { entry: String, args: Vec<String> },
}

struct Spec {
    id: String,
    path: PathBuf,
    kind: Kind,
    zones: Option<Vec<String>>,
}

impl Spec {
    fn inputs(&self) -> Vec<&Path> {
        match &self.kind {
            Kind::Fn { requests } if requests != &self.path => vec![&self.path, requests],
            _ => vec![&self.path],
        }
    }

    /// One vector per zone; a spec without `zones` makes one UTC vector named after the spec.
    fn vectors(&self) -> Vec<Vector<'_>> {
        match &self.zones {
            None => vec![Vector {
                id: self.id.clone(),
                zone: oracle::DEFAULT_ZONE.to_owned(),
                spec: self,
            }],
            Some(zones) => zones
                .iter()
                .map(|zone| Vector {
                    id: format!("{}/{zone}", self.id),
                    zone: zone.clone(),
                    spec: self,
                })
                .collect(),
        }
    }
}

struct Vector<'a> {
    id: String,
    zone: String,
    spec: &'a Spec,
}

/// What the manifest must say about a vector for it to count as current.
#[derive(Debug)]
struct Expected {
    id: String,
    generator: String,
    spec: String,
    zone: String,
    committed_content: Option<String>,
}

struct Current {
    commit: String,
    electron: String,
    environment: Json,
}

pub(crate) fn generate(paths: &Paths, args: &[String]) -> Result {
    let check = args.iter().any(|a| a == "--check");
    if let Some(flag) = args.iter().find(|a| a.starts_with("--") && *a != "--check") {
        return Err(format!("oracle gen: unknown option {flag}"));
    }
    let wanted: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let specs = discover(&specs_dir(&paths.root))?;
    let all: Vec<Vector<'_>> = specs.iter().flat_map(Spec::vectors).collect();
    for (i, vector) in all.iter().enumerate() {
        if all[..i].iter().any(|v| v.id == vector.id) {
            return Err(format!(
                "oracle gen: two specs produce vector {:?}",
                vector.id
            ));
        }
    }
    for id in &wanted {
        if !all.iter().any(|v| v.id == **id || v.spec.id == **id) {
            return Err(format!(
                "oracle gen: no spec or vector {id:?} under parity/specs"
            ));
        }
    }
    let full = wanted.is_empty();
    let selected: Vec<&Vector<'_>> = all
        .iter()
        .filter(|v| full || wanted.iter().any(|w| **w == v.id || **w == v.spec.id))
        .collect();
    if selected.is_empty() {
        return Err("oracle gen: no specs under parity/specs".to_owned());
    }

    let install = oracle::installed(paths)?;
    let current = Current {
        commit: install.upstream_commit.clone(),
        electron: oracle::ELECTRON_VERSION.to_owned(),
        environment: oracle::environment_policy(),
    };
    let vectors_dir = paths.root.join("parity/vectors");
    let oracle_hash = oracle::hash_text_tree(
        &paths.oracle_src,
        &["src", "build.mjs", "package-lock.json"],
    )?;
    let mut manifest = read_manifest(&vectors_dir)?;
    let mut generators = Vec::new();
    for vector in &selected {
        generators.push(generator_hash(&oracle_hash, vector)?);
    }

    if check {
        let mut expected = Vec::new();
        for (vector, generator) in selected.iter().zip(&generators) {
            let committed_content = vectors::vector_path(&vectors_dir, &vector.id)
                .map(|p| vectors::read_text(&p))
                .transpose()?
                .map(|text| oracle::hash_bytes(text.as_bytes()));
            expected.push(Expected {
                id: vector.id.clone(),
                generator: generator.clone(),
                spec: spec_rel(paths, vector.spec),
                zone: vector.zone.clone(),
                committed_content,
            });
        }
        let stale = stale_entries(&manifest, &current, &expected, full);
        if !stale.is_empty() {
            return Err(format!(
                "oracle gen --check: committed vectors are stale ({} problems, oracle not run):\n  {}\n{REGENERATE}",
                stale.len(),
                stale.join("\n  ")
            ));
        }
        eprintln!(
            "  manifest current for {} vectors; regenerating to compare content",
            selected.len()
        );
    } else if !full {
        let mismatched = header_mismatches(&manifest, &current);
        if !mismatched.is_empty() {
            return Err(format!(
                "oracle gen: the manifest was generated under a different {}; regenerate every vector with `cargo xtask oracle gen` (no ids)",
                mismatched.join(", ")
            ));
        }
    }

    oracle::build_harness(paths)?;
    let mut failures = Vec::new();
    for (vector, generator) in selected.iter().zip(&generators) {
        eprintln!("==> {} ({})", vector.id, vector.zone);
        let text = run_vector(paths, vector)?;
        let cases = text.lines().count();
        if check {
            let committed = vectors::vector_path(&vectors_dir, &vector.id)
                .ok_or_else(|| format!("{}: no committed vector", vector.id))
                .and_then(|p| vectors::read_text(&p))?;
            match diff::report_jsonl(&committed, &text, 3, diff::report_lines) {
                Some(report) => failures.push(format!(
                    "{}: regenerated vector differs from the committed one\n{report}",
                    vector.id
                )),
                None => eprintln!("  {cases} cases, identical"),
            }
            continue;
        }
        let out = vectors_dir.join(format!("{}.jsonl.zst", vector.id));
        fs::create_dir_all(out.parent().expect("vector path has a parent"))
            .map_err(|e| e.to_string())?;
        fs::write(&out, vectors::compress(&text)).map_err(|e| format!("{}: {e}", out.display()))?;
        let plain = vectors_dir.join(format!("{}.jsonl", vector.id));
        if plain.exists() {
            fs::remove_file(&plain).map_err(|e| format!("{}: {e}", plain.display()))?;
        }
        set_entry(
            &mut manifest,
            &vector.id,
            Json::object([
                ("cases", Json::Number(cases as f64)),
                (
                    "content",
                    Json::string(&oracle::hash_bytes(text.as_bytes())),
                ),
                ("generator", Json::string(generator)),
                ("spec", Json::string(&spec_rel(paths, vector.spec))),
                ("zone", Json::string(&vector.zone)),
                ("generatedOn", Json::string(&oracle::host_tag())),
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
    if full {
        remove_orphans(&mut manifest, &vectors_dir, &all)?;
    }
    write_manifest(&vectors_dir, &install, manifest)
}

fn spec_rel(paths: &Paths, spec: &Spec) -> String {
    spec.path
        .strip_prefix(&paths.root)
        .unwrap_or(&spec.path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn generator_hash(oracle_hash: &str, vector: &Vector<'_>) -> Result<String> {
    let mut parts = vec![oracle_hash.to_owned()];
    for input in vector.spec.inputs() {
        parts.push(oracle::hash_text(&oracle::read(input)?));
    }
    parts.push(vector.zone.clone());
    Ok(oracle::hash_bytes(parts.join(":").as_bytes()))
}

fn header_mismatches(manifest: &Json, current: &Current) -> Vec<String> {
    let has_vectors = matches!(manifest.get("vectors"), Some(Json::Object(v)) if !v.is_empty());
    if !has_vectors {
        return Vec::new();
    }
    let mut out = Vec::new();
    let recorded = |key: &str| string_field(manifest, key);
    if recorded("upstreamCommit").as_deref() != Some(current.commit.as_str()) {
        out.push(format!(
            "upstream commit (manifest {:?}, oracle {:?})",
            recorded("upstreamCommit").unwrap_or_default(),
            current.commit
        ));
    }
    if recorded("electron").as_deref() != Some(current.electron.as_str()) {
        out.push(format!(
            "Electron version (manifest {:?}, oracle {:?})",
            recorded("electron").unwrap_or_default(),
            current.electron
        ));
    }
    if manifest.get("environment") != Some(&current.environment) {
        out.push(format!(
            "environment policy (manifest {}, oracle {})",
            manifest
                .get("environment")
                .map_or_else(|| "none".to_owned(), Json::to_compact),
            current.environment.to_compact()
        ));
    }
    out
}

/// Every reason the committed manifest does not describe what the current oracle sources,
/// specs and policy would generate. Content is not compared here; that needs the oracle.
fn stale_entries(
    manifest: &Json,
    current: &Current,
    expected: &[Expected],
    full: bool,
) -> Vec<String> {
    let mut out = header_mismatches(manifest, current);
    for want in expected {
        let id = &want.id;
        let Some(entry) = manifest.get("vectors").and_then(|v| v.get(id)) else {
            out.push(format!("{id}: no entry in MANIFEST.json"));
            continue;
        };
        let field = |key: &str| string_field(entry, key).unwrap_or_default();
        if field("generator") != want.generator {
            out.push(format!(
                "{id}: generator hash {} is not the current {} (the spec or the oracle sources changed since it was generated)",
                field("generator"),
                want.generator
            ));
        }
        if field("spec") != want.spec {
            out.push(format!(
                "{id}: recorded spec {:?} is not {:?}",
                field("spec"),
                want.spec
            ));
        }
        if field("zone") != want.zone {
            out.push(format!(
                "{id}: recorded zone {:?} is not {:?}",
                field("zone"),
                want.zone
            ));
        }
        match &want.committed_content {
            None => out.push(format!("{id}: no committed vector file")),
            Some(hash) if *hash != field("content") => out.push(format!(
                "{id}: committed file hashes to {hash}, MANIFEST.json says {}",
                field("content")
            )),
            Some(_) => {}
        }
    }
    if full && let Some(Json::Object(entries)) = manifest.get("vectors") {
        for (key, _) in entries {
            let key = key.to_string_lossy();
            if !expected.iter().any(|e| e.id == key) {
                out.push(format!("{key}: in MANIFEST.json but no spec produces it"));
            }
        }
    }
    out
}

fn remove_orphans(manifest: &mut Json, dir: &Path, all: &[Vector<'_>]) -> Result {
    let Some(Json::Object(entries)) = manifest.get("vectors").cloned() else {
        return Ok(());
    };
    let mut kept = Vec::new();
    for (key, value) in entries {
        let id = key.to_string_lossy();
        if all.iter().any(|v| v.id == id) {
            kept.push((key, value));
            continue;
        }
        eprintln!("  removing {id}: no spec produces it");
        if let Some(path) = vectors::vector_path(dir, &id) {
            fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    set_field(manifest, "vectors", Json::Object(kept));
    Ok(())
}

fn discover(dir: &Path) -> Result<Vec<Spec>> {
    let mut found = Vec::new();
    if dir.is_dir() {
        visit(dir, dir, &mut found)?;
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(found)
}

fn visit(base: &Path, dir: &Path, found: &mut Vec<Spec>) -> Result {
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
        if rel.ends_with(REQUESTS) {
            continue;
        }
        if let Some(id) = rel.strip_suffix(ENTRY_SPEC) {
            let json = read_json(&path)?;
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
            found.push(Spec {
                id: id.to_owned(),
                zones: zones(&path, &json)?,
                kind: Kind::Entry { entry, args },
                path,
            });
        } else if let Some(id) = rel.strip_suffix(FN_ZONED_SPEC) {
            let json = read_json(&path)?;
            let requests = json
                .get("requests")
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy())
                .filter(|r| r.ends_with(REQUESTS))
                .ok_or_else(|| {
                    format!(
                        "{}: needs \"requests\", a path ending in {REQUESTS} relative to the spec",
                        path.display()
                    )
                })?;
            found.push(Spec {
                id: id.to_owned(),
                zones: zones(&path, &json)?,
                kind: Kind::Fn {
                    requests: path.parent().expect("spec has a directory").join(requests),
                },
                path,
            });
        } else if let Some(id) = rel.strip_suffix(FN_SPEC) {
            found.push(Spec {
                id: id.to_owned(),
                zones: None,
                kind: Kind::Fn {
                    requests: path.clone(),
                },
                path,
            });
        }
    }
    Ok(())
}

fn read_json(path: &Path) -> Result<Json> {
    let text = String::from_utf8(oracle::read(path)?).map_err(|e| e.to_string())?;
    json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn zones(path: &Path, spec: &Json) -> Result<Option<Vec<String>>> {
    let Some(list) = spec.get("zones") else {
        return Ok(None);
    };
    let zones = list
        .as_array()
        .filter(|l| !l.is_empty())
        .and_then(|l| {
            l.iter()
                .map(|z| z.as_str().map(|s| s.to_string_lossy()))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| {
            format!(
                "{}: \"zones\" must be a non-empty list of strings",
                path.display()
            )
        })?;
    for (i, zone) in zones.iter().enumerate() {
        oracle::check_zone(zone).map_err(|e| format!("{}: {e}", path.display()))?;
        if zones[..i].contains(zone) {
            return Err(format!("{}: zone {zone:?} is listed twice", path.display()));
        }
    }
    Ok(Some(zones))
}

fn run_vector(paths: &Paths, vector: &Vector<'_>) -> Result<String> {
    let zone = &vector.zone;
    match &vector.spec.kind {
        Kind::Entry { entry, args } => {
            let text = oracle::pipe(
                oracle::oracle_command_in(paths, entry, zone)?.args(args),
                Vec::new(),
            )?;
            for (i, line) in text.lines().enumerate() {
                json::parse(line).map_err(|e| format!("{entry} output line {}: {e}", i + 1))?;
            }
            Ok(text)
        }
        Kind::Fn { requests } => {
            let bytes = oracle::read(requests)?;
            let request_text = String::from_utf8(bytes.clone()).map_err(|e| e.to_string())?;
            let responses =
                oracle::pipe(&mut oracle::oracle_command_in(paths, "fn", zone)?, bytes)?;
            merge_fn_records(&request_text, &responses)
                .map_err(|e| format!("{}: {e}", requests.display()))
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
    read_json(&path)
}

fn string_field(object: &Json, key: &str) -> Option<String> {
    object.get(key)?.as_str().map(|s| s.to_string_lossy())
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

fn write_manifest(dir: &Path, install: &oracle::Install, manifest: Json) -> Result {
    let vectors = manifest
        .get("vectors")
        .cloned()
        .unwrap_or(Json::Object(Vec::new()));
    let ordered = Json::object([
        ("format", Json::string("CVE-1")),
        ("upstreamCommit", Json::string(&install.upstream_commit)),
        ("upstreamVersion", Json::string(&install.upstream_version)),
        ("electron", Json::string(oracle::ELECTRON_VERSION)),
        ("environment", oracle::environment_policy()),
        ("vectors", vectors),
    ]);
    let path = dir.join(MANIFEST);
    fs::write(&path, ordered.to_pretty() + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current() -> Current {
        Current {
            commit: "c0".to_owned(),
            electron: "39.2.7".to_owned(),
            environment: oracle::environment_policy(),
        }
    }

    fn manifest(environment: Json, generator: &str) -> Json {
        Json::object([
            ("upstreamCommit", Json::string("c0")),
            ("electron", Json::string("39.2.7")),
            ("environment", environment),
            (
                "vectors",
                Json::object([(
                    "a/b",
                    Json::object([
                        ("content", Json::string("h1")),
                        ("generator", Json::string(generator)),
                        ("spec", Json::string("parity/specs/a/b.jsonl")),
                        ("zone", Json::string("UTC")),
                    ]),
                )]),
            ),
        ])
    }

    fn expected(generator: &str) -> Expected {
        Expected {
            id: "a/b".to_owned(),
            generator: generator.to_owned(),
            spec: "parity/specs/a/b.jsonl".to_owned(),
            zone: "UTC".to_owned(),
            committed_content: Some("h1".to_owned()),
        }
    }

    #[test]
    fn current_manifest_is_not_stale() {
        let m = manifest(oracle::environment_policy(), "g1");
        assert!(stale_entries(&m, &current(), &[expected("g1")], true).is_empty());
    }

    #[test]
    fn generator_change_is_stale() {
        let m = manifest(oracle::environment_policy(), "g1");
        let stale = stale_entries(&m, &current(), &[expected("g2")], true);
        assert_eq!(stale.len(), 1, "{stale:?}");
        assert!(stale[0].contains("generator hash"));
    }

    #[test]
    fn header_changes_are_stale() {
        let m = manifest(Json::object([("TZ", Json::string("UTC"))]), "g1");
        let mut now = current();
        now.commit = "c1".to_owned();
        now.electron = "40.0.0".to_owned();
        let stale = stale_entries(&m, &now, &[expected("g1")], true);
        assert_eq!(stale.len(), 3, "{stale:?}");
    }

    #[test]
    fn content_zone_and_orphans_are_stale() {
        let m = manifest(oracle::environment_policy(), "g1");
        let mut want = expected("g1");
        want.committed_content = Some("h2".to_owned());
        want.zone = "Asia/Kolkata".to_owned();
        assert_eq!(stale_entries(&m, &current(), &[want], true).len(), 2);
        let mut other = expected("g1");
        other.id = "c".to_owned();
        let stale = stale_entries(&m, &current(), &[other], true);
        assert!(stale.iter().any(|s| s.contains("no entry")));
        assert!(stale.iter().any(|s| s.contains("no spec produces it")));
        let mut other = expected("g1");
        other.id = "c".to_owned();
        assert_eq!(stale_entries(&m, &current(), &[other], false).len(), 1);
    }
}
