use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use asrs_parity::json::{self, Json};
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors;

use super::{Draw, Options, cargo_binary, packages, test_names};
use crate::Result;

/// Occurrences of one recorded field that must all go unnoticed before it is reported.
const TRIES: usize = 3;
/// String fields that name a record's kind, beside its first key.
const KIND_FIELDS: &[&str] = &["op", "kind", "fn", "ev", "t", "main", "exception"];
const UNKNOWN: &str = "{\"asrsUnknownRecordKind\":true}";

#[derive(Clone, Debug, PartialEq)]
enum Step {
    Key(Utf16String),
    Index(usize),
}

/// A recorded value: which record, and where in it.
type Place = (usize, Vec<Step>);

#[derive(Clone, Debug)]
enum Change {
    /// One recorded value, with other occurrences of the same field to try.
    Value(Vec<Place>),
    Drop(usize),
    Unknown(usize),
}

#[derive(Debug)]
struct Case {
    name: String,
    change: Change,
}

struct TestBinary {
    executable: PathBuf,
    directory: PathBuf,
}

pub(super) fn run(options: &Options, work: &Path) -> Result<(String, usize)> {
    let binaries = build(options)?;
    let source = vectors::vectors_dir();
    let jobs = options.jobs.unwrap_or(4);
    let trees: Vec<PathBuf> = (0..jobs)
        .map(|k| work.join(format!("vectors-{k}")))
        .collect();
    for tree in &trees {
        if tree.exists() {
            fs::remove_dir_all(tree).map_err(|e| format!("{}: {e}", tree.display()))?;
        }
        link_tree(&source, tree)?;
    }
    let mut report = String::new();
    let mut missed_total = 0;
    for id in &options.vectors {
        let path = vectors::vector_path(&source, id)
            .ok_or_else(|| format!("mutate: no committed vector {id:?}"))?;
        let lines: Vec<String> = vectors::read_text(&path)?
            .lines()
            .map(str::to_owned)
            .collect();
        let records = lines
            .iter()
            .enumerate()
            .map(|(i, l)| json::parse(l).map_err(|e| format!("{id} line {}: {e}", i + 1)))
            .collect::<Result<Vec<Json>>>()?;
        let (cases, fields) = cases(&records, options);
        for tree in &trees {
            remove(tree, id)?;
        }
        eprintln!("==> {id}: unmutated");
        let started = Instant::now();
        write(&trees[0], id, &lines)?;
        if !passes(&binaries, &trees[0], options, Duration::from_secs(1800))? {
            return Err(format!(
                "mutate: {} fail on the unmutated {id}",
                test_names(options)
            ));
        }
        let timeout = started.elapsed() * 5 + Duration::from_secs(30);
        let queue: Mutex<Vec<(usize, &Case)>> =
            Mutex::new(cases.iter().enumerate().rev().collect());
        let results: Mutex<Vec<Option<bool>>> = Mutex::new(vec![None; cases.len()]);
        let errors = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for tree in &trees {
                let (queue, results, errors) = (&queue, &results, &errors);
                let (binaries, records, lines) = (&binaries, &records, &lines);
                scope.spawn(move || {
                    loop {
                        let next = queue.lock().expect("no panics under the lock").pop();
                        let Some((k, case)) = next else { break };
                        match caught(case, id, tree, lines, records, binaries, options, timeout) {
                            Ok(caught) => {
                                eprintln!(
                                    "{} {}",
                                    if caught { "caught" } else { "MISSED" },
                                    case.name
                                );
                                results.lock().expect("no panics under the lock")[k] = Some(caught);
                            }
                            Err(e) => errors.lock().expect("no panics under the lock").push(e),
                        }
                    }
                });
            }
        });
        if let Some(e) = errors
            .into_inner()
            .expect("no panics under the lock")
            .first()
        {
            return Err(e.clone());
        }
        let results = results.into_inner().expect("no panics under the lock");
        let of = |wanted: bool, dropped: bool| -> Vec<&Case> {
            cases
                .iter()
                .zip(&results)
                .filter(|(c, r)| {
                    **r == Some(wanted)
                        && dropped == (!wanted && matches!(c.change, Change::Drop(_)))
                })
                .map(|(c, _)| c)
                .collect()
        };
        let (caught, missed, dropped) = (of(true, false), of(false, false), of(false, true));
        missed_total += missed.len();
        report.push_str(&format!(
            "# Mutants of vector {id}\n\n{} records with {fields} recorded fields; {} mutants (seed {}), against {}: {} caught, {} missed, {} drops unnoticed.\n",
            records.len(),
            cases.len(),
            options.seed,
            test_names(options),
            caught.len(),
            missed.len(),
            dropped.len()
        ));
        for (title, list) in [
            ("Missed", missed),
            ("Dropped records unnoticed", dropped),
            ("Caught", caught),
        ] {
            if !list.is_empty() {
                report.push_str(&format!("\n## {title}\n\n"));
                for c in list {
                    report.push_str(&format!("- {}\n", c.name));
                }
            }
        }
        report.push('\n');
        for tree in &trees {
            remove(tree, id)?;
            link(&source, tree, &path)?;
        }
    }
    for tree in &trees {
        let _ = fs::remove_dir_all(tree);
    }
    Ok((report, missed_total))
}

/// The record-level mutants, then a sample of the recorded fields.
fn cases(records: &[Json], options: &Options) -> (Vec<Case>, usize) {
    let mut draw = Draw(options.seed);
    let mut kinds: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut fields: BTreeMap<String, Vec<Place>> = BTreeMap::new();
    for (i, record) in records.iter().enumerate() {
        let kind = kind(record);
        kinds.entry(kind.clone()).or_default().push(i);
        let mut leaves = Vec::new();
        visit(record, &mut Vec::new(), &mut String::new(), &mut leaves);
        for (shape, path) in leaves {
            fields
                .entry(format!("{kind}: {shape}"))
                .or_default()
                .push((i, path));
        }
    }
    let mut cases = vec![Case {
        name: "a record of an unknown kind is added".to_owned(),
        change: Change::Unknown(draw.below(records.len().max(1))),
    }];
    for (kind, at) in &kinds {
        cases.push(Case {
            name: format!("a {kind} record is dropped (1 of {})", at.len()),
            change: Change::Drop(at[draw.below(at.len())]),
        });
    }
    let total = fields.len();
    let sampled = draw.sample(fields.into_iter().collect::<Vec<_>>(), options.sample);
    for (name, places) in sampled {
        let n = places.len();
        cases.push(Case {
            name: format!("{name} ({n} recorded)"),
            change: Change::Value(draw.sample(places, TRIES)),
        });
    }
    (cases, total)
}

fn kind(record: &Json) -> String {
    let Json::Object(pairs) = record else {
        return "(not an object)".to_owned();
    };
    let Some((first, _)) = pairs.first() else {
        return "(empty)".to_owned();
    };
    let named = pairs.iter().find_map(|(k, v)| {
        let key = k.to_string_lossy();
        let value = v.as_str()?.to_string_lossy();
        (KIND_FIELDS.contains(&key.as_str()) && value.len() <= 40).then(|| format!("{key}={value}"))
    });
    named.unwrap_or_else(|| first.to_string_lossy())
}

/// Every leaf under `json`: the field it is (indices left out) and the path to it. A CVE-1
/// object (`{"o":[[key, value], ...]}`) reads as an object, and `{"n": bits}` as one number.
fn visit(
    json: &Json,
    path: &mut Vec<Step>,
    shape: &mut String,
    out: &mut Vec<(String, Vec<Step>)>,
) {
    let length = shape.len();
    match json {
        Json::Object(pairs) => {
            let single = match pairs.as_slice() {
                [(k, v)] => Some((k, v)),
                _ => None,
            };
            if let Some((k, v)) = single
                && k.eq_str("n")
                && v.as_str().is_some()
            {
                path.push(Step::Key(k.clone()));
                out.push((shape.clone(), path.clone()));
                path.pop();
                return;
            }
            if let Some((k, Json::Array(entries))) = single
                && k.eq_str("o")
                && entries
                    .iter()
                    .all(|e| matches!(e, Json::Array(p) if p.len() == 2 && p[0].as_str().is_some()))
            {
                path.push(Step::Key(k.clone()));
                for (i, entry) in entries.iter().enumerate() {
                    let Json::Array(pair) = entry else { continue };
                    let key = pair[0].as_str().expect("checked above").to_string_lossy();
                    path.extend([Step::Index(i), Step::Index(1)]);
                    shape.push('.');
                    shape.push_str(&key);
                    visit(&pair[1], path, shape, out);
                    shape.truncate(length);
                    path.truncate(path.len() - 2);
                }
                path.pop();
                return;
            }
            for (k, v) in pairs {
                path.push(Step::Key(k.clone()));
                if !shape.is_empty() {
                    shape.push('.');
                }
                shape.push_str(&k.to_string_lossy());
                visit(v, path, shape, out);
                shape.truncate(length);
                path.pop();
            }
        }
        Json::Array(items) if !items.is_empty() => {
            for (i, v) in items.iter().enumerate() {
                path.push(Step::Index(i));
                shape.push_str("[]");
                visit(v, path, shape, out);
                shape.truncate(length);
                path.pop();
            }
        }
        _ => out.push((shape.clone(), path.clone())),
    }
}

fn change(json: &mut Json, path: &[Step]) {
    if let [step, rest @ ..] = path {
        let inner = match (step, json) {
            (Step::Key(key), Json::Object(pairs)) => {
                pairs.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
            }
            (Step::Index(i), Json::Array(items)) => items.get_mut(*i),
            _ => None,
        };
        change(inner.expect("the path was found in this record"), rest);
        return;
    }
    *json = match &*json {
        Json::Null => Json::Bool(true),
        Json::Bool(b) => Json::Bool(!b),
        Json::Number(n) => Json::Number(n + 1.0),
        Json::String(s) => Json::string(&other_text(&s.to_string_lossy())),
        Json::Array(_) => Json::Array(vec![Json::Null]),
        Json::Object(_) => Json::object([("asrsMutated", Json::Bool(true))]),
    };
}

/// Another string of the same form: hex stays hex, so CVE-1 bits and bytes still decode.
fn other_text(text: &str) -> String {
    let hex = !text.is_empty()
        && text.len().is_multiple_of(2)
        && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !hex {
        return format!("{text}~");
    }
    // Not the last digit of a number: x86_64 and a platform's pow may differ there.
    let at = text.len() / 2;
    let digit = u8::from_str_radix(&text[at..=at], 16).expect("hex") ^ 1;
    format!("{}{digit:x}{}", &text[..at], &text[at + 1..])
}

#[allow(clippy::too_many_arguments)]
fn caught(
    case: &Case,
    id: &str,
    tree: &Path,
    lines: &[String],
    records: &[Json],
    binaries: &[TestBinary],
    options: &Options,
    timeout: Duration,
) -> Result<bool> {
    let mut attempts = Vec::new();
    match &case.change {
        Change::Value(places) => {
            for (record, path) in places {
                let mut json = records[*record].clone();
                change(&mut json, path);
                let mut lines = lines.to_vec();
                lines[*record] = json.to_compact();
                attempts.push(lines);
            }
        }
        Change::Drop(at) => {
            let mut lines = lines.to_vec();
            lines.remove(*at);
            attempts.push(lines);
        }
        Change::Unknown(at) => {
            let mut lines = lines.to_vec();
            lines.insert(*at + 1, UNKNOWN.to_owned());
            attempts.push(lines);
        }
    }
    for lines in attempts {
        write(tree, id, &lines)?;
        if !passes(binaries, tree, options, timeout)? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether the tests pass on the vectors in `tree`.
fn passes(
    binaries: &[TestBinary],
    tree: &Path,
    options: &Options,
    timeout: Duration,
) -> Result<bool> {
    for binary in binaries {
        let mut command = Command::new(&binary.executable);
        command
            .current_dir(&binary.directory)
            .env("CARGO_MANIFEST_DIR", &binary.directory)
            .env(vectors::VECTORS_ENV, tree)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(only) = &options.only {
            command.arg(only);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("{}: {e}", binary.executable.display()))?;
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                break Some(status);
            }
            if started.elapsed() > timeout {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        if !status.is_some_and(|s| s.success()) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn build(options: &Options) -> Result<Vec<TestBinary>> {
    let mut command = Command::new(cargo_binary());
    command
        .args([
            "test",
            "--no-run",
            "--locked",
            "--message-format=json-render-diagnostics",
        ])
        .current_dir(crate::workspace_root())
        .stderr(Stdio::inherit());
    for package in packages(options) {
        command.args(["-p", package]);
    }
    for (_, test) in &options.tests {
        command.args(["--test", test]);
    }
    let output = command.output().map_err(|e| format!("cargo test: {e}"))?;
    if !output.status.success() {
        return Err(format!("cargo test --no-run failed ({})", output.status));
    }
    let mut found = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Ok(message) = json::parse(line) else {
            continue;
        };
        let text = |j: Option<&Json>| j.and_then(Json::as_str).map(|s| s.to_string_lossy());
        let (Some(executable), Some(manifest)) = (
            text(message.get("executable")),
            text(message.get("manifest_path")),
        ) else {
            continue;
        };
        let name = text(message.get("target").and_then(|t| t.get("name"))).unwrap_or_default();
        let directory = PathBuf::from(manifest)
            .parent()
            .expect("a manifest has a directory")
            .to_path_buf();
        let package = directory
            .file_name()
            .map(|n| n.to_string_lossy().into_owned());
        if options
            .tests
            .iter()
            .any(|(p, t)| *t == name && Some(p) == package.as_ref())
        {
            found.push(TestBinary {
                executable: PathBuf::from(executable),
                directory,
            });
        }
    }
    if found.len() != options.tests.len() {
        return Err(format!(
            "mutate: cargo built {} of {}",
            found.len(),
            test_names(options)
        ));
    }
    Ok(found)
}

/// A copy of the vectors made of hard links, so each job can replace one file.
fn link_tree(source: &Path, tree: &Path) -> Result {
    for entry in fs::read_dir(source).map_err(|e| format!("{}: {e}", source.display()))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        let to = tree.join(path.file_name().expect("a directory entry has a name"));
        if path.is_dir() {
            link_tree(&path, &to)?;
        } else {
            link_file(&path, &to)?;
        }
    }
    Ok(())
}

fn link_file(from: &Path, to: &Path) -> Result {
    let dir = to.parent().expect("a file has a directory");
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if fs::hard_link(from, to).is_err() {
        fs::copy(from, to).map_err(|e| format!("{}: {e}", from.display()))?;
    }
    Ok(())
}

fn link(source: &Path, tree: &Path, path: &Path) -> Result {
    let rel = path.strip_prefix(source).expect("under the vectors");
    link_file(path, &tree.join(rel))
}

fn remove(tree: &Path, id: &str) -> Result {
    while let Some(path) = vectors::vector_path(tree, id) {
        fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

fn write(tree: &Path, id: &str, lines: &[String]) -> Result {
    let path = tree.join(format!("{id}.jsonl"));
    let text: String = lines.iter().flat_map(|l| [l.as_str(), "\n"]).collect();
    fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaves(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        visit(
            &json::parse(text).unwrap(),
            &mut Vec::new(),
            &mut String::new(),
            &mut out,
        );
        out.into_iter().map(|(shape, _)| shape).collect()
    }

    #[test]
    fn fields_are_found_through_cve1_objects() {
        assert_eq!(
            leaves(
                r#"{"command":{"o":[["x",{"n":"3ff0000000000000"}],["list",[1,{"u":null}]]]},"s":"a","e":[]}"#
            ),
            ["command.x", "command.list[]", "command.list[].u", "s", "e"]
        );
    }

    #[test]
    fn a_change_touches_one_value() {
        let mut json =
            json::parse(r#"{"a":{"o":[["x",{"n":"3ff0000000000000"}]]},"b":[true,"t"]}"#).unwrap();
        let mut out = Vec::new();
        visit(&json, &mut Vec::new(), &mut String::new(), &mut out);
        for (_, path) in &out {
            change(&mut json, path);
        }
        assert_eq!(
            json.to_compact(),
            r#"{"a":{"o":[["x",{"n":"3ff0000010000000"}]]},"b":[false,"t~"]}"#
        );
    }

    #[test]
    fn kinds_come_from_an_op_or_the_first_key() {
        let of = |t: &str| kind(&json::parse(t).unwrap());
        assert_eq!(of(r#"{"dom":1,"at":2}"#), "dom");
        assert_eq!(of(r#"{"case":"x","op":"start"}"#), "op=start");
        assert_eq!(of("[1]"), "(not an object)");
    }
}
