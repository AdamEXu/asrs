use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use asrs_parity::json::{self, Json};

use crate::Result;
use crate::oracle::{self, Paths};
use crate::vectors::{self, Backend, DOM_ENTRY};

pub(crate) const DOM_USAGE: &str = "usage: cargo xtask oracle dom [--backend jsdom|blink] <spec or fixture file>

Runs one DOM fixture and prints its records. The backend defaults to the spec's \"backend\"
(Blink when it names none); a bare fixture file runs on Blink. Like `oracle run`, it uses the
built harness: run `cargo xtask oracle build` after editing oracle/src.";

pub(crate) const AB_USAGE: &str = "usage: cargo xtask oracle dom-ab [<spec id prefix>...] [--jobs N] [--out <file>]

Runs every committed dom/* and form/* spec (or those whose id starts with a prefix) on jsdom and
on Blink with scripted geometry, and writes a Markdown report: per family, which vectors are
byte-identical and, for the others, how many records each backend has that the other lacks, by
record kind, with the first differing line. Specs with real geometry run on Blink only. The
committed vector is compared with the run of the backend MANIFEST.json says generated it.";

pub(crate) fn run(paths: &Paths, args: &[String]) -> Result {
    let (backend, rest) = match args {
        [flag, name, rest @ ..] if flag == "--backend" => (
            Some(Backend::parse(name).ok_or_else(|| DOM_USAGE.to_owned())?),
            rest,
        ),
        _ => (None, args),
    };
    let [file] = rest else {
        return Err(DOM_USAGE.to_owned());
    };
    let backend = match backend {
        Some(backend) => backend,
        None => spec_backend(Path::new(file))?.unwrap_or(Backend::Blink),
    };
    let text = run_backend(paths, backend, Path::new(file))?;
    print!("{text}");
    Ok(())
}

fn spec_backend(file: &Path) -> Result<Option<Backend>> {
    let text = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let spec = json::parse(&text).map_err(|e| format!("{}: {e}", file.display()))?;
    match spec.get("entry").and_then(Json::as_str) {
        Some(entry) => vectors::dom_backend(file, &spec, &entry.to_string_lossy()),
        None => Ok(None),
    }
}

fn run_backend(paths: &Paths, backend: Backend, file: &Path) -> Result<String> {
    oracle::pipe(
        oracle::oracle_command_in(paths, backend.entry(), oracle::DEFAULT_ZONE)?
            .arg(file)
            .current_dir(&paths.root),
        Vec::new(),
    )
}

struct Case {
    id: String,
    path: PathBuf,
    backend: Backend,
    real: bool,
}

fn cases(paths: &Paths, prefixes: &[&String]) -> Result<Vec<Case>> {
    let specs = vectors::specs_dir(&paths.root);
    let mut out = Vec::new();
    for family in ["dom", "form"] {
        collect(&specs, &specs.join(family), &mut out)?;
    }
    out.retain(|c| prefixes.is_empty() || prefixes.iter().any(|p| c.id.starts_with(p.as_str())));
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

fn collect(base: &Path, dir: &Path, out: &mut Vec<Case>) -> Result {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect(base, &path, out)?;
            continue;
        }
        let rel = path
            .strip_prefix(base)
            .expect("under base")
            .to_string_lossy()
            .replace('\\', "/");
        let Some(id) = rel.strip_suffix(".entry.json") else {
            continue;
        };
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let spec = json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if spec.get("entry").and_then(Json::as_str).map(|e| e.to_string_lossy()).as_deref()
            != Some(DOM_ENTRY)
        {
            continue;
        }
        let backend = vectors::dom_backend(&path, &spec, DOM_ENTRY)?.expect("a dom spec");
        let real = spec
            .get("fixture")
            .and_then(|f| f.get("geometry"))
            .and_then(Json::as_str)
            .is_some_and(|g| g.to_string_lossy() == "real");
        out.push(Case {
            id: id.to_owned(),
            path,
            backend,
            real,
        });
    }
    Ok(())
}

enum Outcome {
    Identical,
    RealOnly,
    Differs(Difference),
    Failed(String),
}

struct Difference {
    jsdom_lines: usize,
    blink_lines: usize,
    only_jsdom: BTreeMap<String, usize>,
    only_blink: BTreeMap<String, usize>,
    first: Option<(usize, String, String)>,
}

struct Result1 {
    outcome: Outcome,
    committed: Option<String>,
}

pub(crate) fn ab(paths: &Paths, args: &[String]) -> Result {
    let mut jobs = 3usize;
    let mut out_file = None;
    let mut prefixes = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--jobs" => {
                jobs = args
                    .get(i + 1)
                    .and_then(|n| n.parse().ok())
                    .filter(|n| *n > 0)
                    .ok_or_else(|| AB_USAGE.to_owned())?;
                i += 1;
            }
            "--out" => {
                out_file = Some(PathBuf::from(
                    args.get(i + 1).ok_or_else(|| AB_USAGE.to_owned())?,
                ));
                i += 1;
            }
            flag if flag.starts_with("--") => return Err(AB_USAGE.to_owned()),
            _ => prefixes.push(&args[i]),
        }
        i += 1;
    }
    let cases = cases(paths, &prefixes)?;
    if cases.is_empty() {
        return Err("oracle dom-ab: no dom specs match".to_owned());
    }
    oracle::build_harness(paths)?;
    let vectors_dir = paths.root.join("parity/vectors");
    let manifest_text = fs::read_to_string(vectors_dir.join("MANIFEST.json"))
        .map_err(|e| format!("parity/vectors/MANIFEST.json: {e}"))?;
    let manifest =
        json::parse(&manifest_text).map_err(|e| format!("parity/vectors/MANIFEST.json: {e}"))?;
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<Result1>>> = Mutex::new((0..cases.len()).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..jobs.min(cases.len()) {
            scope.spawn(|| {
                loop {
                    let k = next.fetch_add(1, Ordering::SeqCst);
                    let Some(case) = cases.get(k) else { break };
                    eprintln!("==> {}", case.id);
                    let result = compare(paths, &vectors_dir, &manifest, case);
                    results.lock().expect("no panics while holding the lock")[k] = Some(result);
                }
            });
        }
    });
    let results: Vec<Result1> = results
        .into_inner()
        .expect("no panics while holding the lock")
        .into_iter()
        .map(|r| r.expect("every case ran"))
        .collect();
    let report = report(&cases, &results);
    match out_file {
        Some(path) => fs::write(&path, &report).map_err(|e| format!("{}: {e}", path.display()))?,
        None => print!("{report}"),
    }
    let failed = results
        .iter()
        .filter(|r| matches!(r.outcome, Outcome::Failed(_)))
        .count();
    if failed > 0 {
        return Err(format!("oracle dom-ab: {failed} runs failed"));
    }
    Ok(())
}

/// The backend that generated the committed vector; entries from before the Blink backend
/// have no `backend` and were generated by jsdom.
fn generated_by(manifest: &Json, id: &str) -> Backend {
    manifest
        .get("vectors")
        .and_then(|v| v.get(id))
        .and_then(|e| e.get("backend"))
        .and_then(Json::as_str)
        .and_then(|b| Backend::parse(&b.to_string_lossy()))
        .unwrap_or(Backend::Jsdom)
}

fn compare(paths: &Paths, vectors_dir: &Path, manifest: &Json, case: &Case) -> Result1 {
    let committed = asrs_parity::vectors::vector_path(vectors_dir, &case.id)
        .and_then(|p| asrs_parity::vectors::read_text(&p).ok());
    let generator = generated_by(manifest, &case.id);
    let blink = match run_backend(paths, Backend::Blink, &case.path) {
        Ok(text) => text,
        Err(e) => {
            return Result1 {
                outcome: Outcome::Failed(format!("blink: {e}")),
                committed: None,
            };
        }
    };
    if case.real {
        return Result1 {
            committed: committed_note(committed.as_deref(), generator, None, &blink),
            outcome: Outcome::RealOnly,
        };
    }
    let jsdom = match run_backend(paths, Backend::Jsdom, &case.path) {
        Ok(text) => text,
        Err(e) => {
            return Result1 {
                outcome: Outcome::Failed(format!("jsdom: {e}")),
                committed: None,
            };
        }
    };
    let outcome = if jsdom == blink {
        Outcome::Identical
    } else {
        Outcome::Differs(difference(&jsdom, &blink))
    };
    Result1 {
        committed: committed_note(committed.as_deref(), generator, Some(&jsdom), &blink),
        outcome,
    }
}

fn committed_note(committed: Option<&str>, backend: Backend, jsdom: Option<&str>, blink: &str) -> Option<String> {
    let committed = committed?;
    let run = match backend {
        Backend::Jsdom => jsdom?,
        Backend::Blink => blink,
    };
    (committed != run).then(|| {
        format!(
            "the committed vector differs from this {} run (stale or unstable)",
            backend.name()
        )
    })
}

fn kind(line: &str) -> String {
    match json::parse(line) {
        Ok(Json::Object(pairs)) => pairs
            .first()
            .map_or_else(|| "{}".to_owned(), |(k, _)| k.to_string_lossy()),
        _ => "?".to_owned(),
    }
}

fn difference(jsdom: &str, blink: &str) -> Difference {
    let a: Vec<&str> = jsdom.lines().collect();
    let b: Vec<&str> = blink.lines().collect();
    let mut counts: BTreeMap<&str, isize> = BTreeMap::new();
    for line in &a {
        *counts.entry(line).or_default() += 1;
    }
    for line in &b {
        *counts.entry(line).or_default() -= 1;
    }
    let mut only_jsdom = BTreeMap::new();
    let mut only_blink = BTreeMap::new();
    for (line, n) in counts {
        if n > 0 {
            *only_jsdom.entry(kind(line)).or_default() += n as usize;
        } else if n < 0 {
            *only_blink.entry(kind(line)).or_default() += (-n) as usize;
        }
    }
    let first = (0..a.len().max(b.len()))
        .find(|&k| a.get(k) != b.get(k))
        .map(|k| {
            (
                k + 1,
                a.get(k).copied().unwrap_or("(end)").to_owned(),
                b.get(k).copied().unwrap_or("(end)").to_owned(),
            )
        });
    Difference {
        jsdom_lines: a.len(),
        blink_lines: b.len(),
        only_jsdom,
        only_blink,
        first,
    }
}

/// The report groups vectors by family: the spec id without its last segment, except under
/// `dom/tabs/`, where each tab case is its own family up to its first `-`.
fn family(id: &str) -> String {
    if let Some(case) = id.strip_prefix("dom/tabs/") {
        let tab = case.split('-').next().unwrap_or(case);
        let tab = match tab {
            "field" => case.splitn(3, '-').take(2).collect::<Vec<_>>().join("-"),
            _ => tab.to_owned(),
        };
        return format!("dom/tabs/{tab}");
    }
    id.rsplit_once('/').map_or(id, |(dir, _)| dir).to_owned()
}

fn clip(line: &str) -> String {
    const MAX: usize = 240;
    let mut out: String = line.chars().take(MAX).collect();
    if line.chars().count() > MAX {
        out.push_str(" ...");
    }
    out.replace('`', "'")
}

fn kinds(map: &BTreeMap<String, usize>) -> String {
    if map.is_empty() {
        return "none".to_owned();
    }
    map.iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn report(cases: &[Case], results: &[Result1]) -> String {
    let mut families: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (k, case) in cases.iter().enumerate() {
        families.entry(family(&case.id)).or_default().push(k);
    }
    let count = |f: &dyn Fn(&Outcome) -> bool| results.iter().filter(|r| f(&r.outcome)).count();
    let mut out = String::new();
    out.push_str("# DOM A/B: jsdom against Blink with scripted geometry\n\n");
    out.push_str(&format!(
        "{} specs in {} families: {} identical, {} differ, {} real geometry (Blink only), {} failed.\n\n",
        cases.len(),
        families.len(),
        count(&|o| matches!(o, Outcome::Identical)),
        count(&|o| matches!(o, Outcome::Differs(_))),
        count(&|o| matches!(o, Outcome::RealOnly)),
        count(&|o| matches!(o, Outcome::Failed(_))),
    ));
    out.push_str("| Family | Specs | Identical | Differ | Spec backend |\n|---|---|---|---|---|\n");
    for (name, members) in &families {
        let identical = members
            .iter()
            .filter(|&&k| matches!(results[k].outcome, Outcome::Identical))
            .count();
        let differ = members
            .iter()
            .filter(|&&k| matches!(results[k].outcome, Outcome::Differs(_)))
            .count();
        let mut backends: Vec<&str> = members.iter().map(|&k| cases[k].backend.name()).collect();
        backends.sort();
        backends.dedup();
        out.push_str(&format!(
            "| `{name}` | {} | {identical} | {differ} | {} |\n",
            members.len(),
            backends.join(", ")
        ));
    }
    for (name, members) in &families {
        let notes: Vec<String> = members
            .iter()
            .filter_map(|&k| {
                let case = &cases[k];
                let result = &results[k];
                let mut note = match &result.outcome {
                    Outcome::Identical if result.committed.is_none() => return None,
                    Outcome::Identical => format!("- `{}`: identical", case.id),
                    Outcome::RealOnly => format!("- `{}`: real geometry, Blink only", case.id),
                    Outcome::Failed(e) => format!("- `{}`: FAILED: {}", case.id, clip(e)),
                    Outcome::Differs(d) => {
                        let mut s = format!(
                            "- `{}`: {} jsdom lines, {} Blink lines; only in jsdom: {}; only in Blink: {}",
                            case.id,
                            d.jsdom_lines,
                            d.blink_lines,
                            kinds(&d.only_jsdom),
                            kinds(&d.only_blink)
                        );
                        if let Some((line, a, b)) = &d.first {
                            s.push_str(&format!(
                                "\n  - first difference at line {line}\n    - jsdom: `{}`\n    - Blink: `{}`",
                                clip(a),
                                clip(b)
                            ));
                        }
                        s
                    }
                };
                if let Some(c) = &result.committed {
                    note.push_str(&format!("\n  - note: {c}"));
                }
                Some(note)
            })
            .collect();
        if notes.is_empty() {
            continue;
        }
        out.push_str(&format!("\n## `{name}`\n\n{}\n", notes.join("\n")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn families_split_tab_cases() {
        assert_eq!(family("dom/tabs/video-multi"), "dom/tabs/video");
        assert_eq!(family("dom/tabs/swerve-c05"), "dom/tabs/swerve");
        assert_eq!(family("dom/tabs/field-3d-live"), "dom/tabs/field-3d");
        assert_eq!(family("dom/tabs/field-2d"), "dom/tabs/field-2d");
        assert_eq!(family("dom/timeline/hidden"), "dom/timeline");
        assert_eq!(family("form/select"), "form");
    }

    #[test]
    fn difference_counts_records_by_kind() {
        let d = difference(
            "{\"a\":1}\n{\"dom\":1}\n{\"dom\":2}\n",
            "{\"a\":1}\n{\"dom\":3}\n{\"exception\":\"x\"}\n{\"dom\":2}\n",
        );
        assert_eq!(d.only_jsdom.get("dom"), Some(&1));
        assert_eq!(d.only_blink.get("dom"), Some(&1));
        assert_eq!(d.only_blink.get("exception"), Some(&1));
        assert_eq!(d.first.as_ref().map(|f| f.0), Some(2));
    }
}
