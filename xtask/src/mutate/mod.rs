mod probe;
mod records;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use asrs_parity::json::{self, Json};

use crate::{Result, workspace_root};

/// Installed by this command into the cache directory, never from PATH.
const CARGO_MUTANTS_VERSION: &str = "27.1.0";

const USAGE: &str =
    "usage: cargo xtask mutate <source file>... --test <package>:<test>... [options]
       cargo xtask mutate --vector <id>... --test <package>:<test>... [options]

Mutation testing of a port, and of the replay harnesses that judge it.

With source files: a sample of cargo-mutants' mutants of those files, each built and run against
the named vector tests only, listed as caught or missed. A missed mutant is then probed: a build
of the original code records whether the tests reach the mutated place and whether the mutated
expression ever had another value there, so \"equivalent\" is not decided by reading:
  not reached  no fixture executes it; it says nothing about equivalence. Add a fixture.
  differs      it gave another value on a fixture's input and no comparison noticed. Compare the
               value's effect, or add a fixture where it has one. Not equivalent as code.
  agrees       reached, with the same value every time. Add a separating input, or show in the PR
               why none exists.
  reached      reached; this kind of mutant has no value to compare.
  unprobed     the probe does not compile there.

With --vector: the recorded vector itself is mutated instead of the port (one recorded value at a
time, a dropped record of each kind, a record of an unknown kind), and the tests must fail. A
missed one names a recorded field, or a kind, the replay does not compare. No rebuild, so a run
costs one run of the test per mutant. A dropped record goes unnoticed, and is listed without
counting as missed, where each record carries its own input; in a session vector it is a hole.

  --test <pkg>:<test>  an integration test that replays the unit's vectors (repeatable)
  --only <name>        pass this name filter to the test binaries
  --sample N           at most N mutants, or recorded fields per vector (default 40)
  --seed N             which sample (default 1); a reviewer runs another seed
  --jobs N             parallel mutants (default 2; 4 for --vector)
  --no-probe           do not probe missed mutants
  --out <file>         write the Markdown report there too

cargo-mutants is installed at the pinned version on first use. Exits with an error when anything
was missed. Runs of source mutants build a copy of the checkout per job under
$CARGO_TARGET_DIR/mutate (default target/mutate).";

struct Options {
    files: Vec<String>,
    vectors: Vec<String>,
    tests: Vec<(String, String)>,
    only: Option<String>,
    sample: usize,
    seed: u64,
    jobs: Option<usize>,
    probe: bool,
    out: Option<PathBuf>,
}

fn options(args: &[String]) -> Result<Options> {
    let usage = || USAGE.to_owned();
    let mut o = Options {
        files: Vec::new(),
        vectors: Vec::new(),
        tests: Vec::new(),
        only: None,
        sample: 40,
        seed: 1,
        jobs: None,
        probe: true,
        out: None,
    };
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--no-probe" {
            o.probe = false;
            continue;
        }
        if !arg.starts_with("--") {
            o.files.push(arg.clone());
            continue;
        }
        let value = args.next().cloned().ok_or_else(usage)?;
        match arg.as_str() {
            "--vector" => o.vectors.push(value),
            "--test" => {
                let (package, test) = value.split_once(':').ok_or_else(usage)?;
                o.tests.push((package.to_owned(), test.to_owned()));
            }
            "--only" => o.only = Some(value),
            "--sample" => o.sample = value.parse().map_err(|_| usage())?,
            "--seed" => o.seed = value.parse().map_err(|_| usage())?,
            "--jobs" => o.jobs = Some(value.parse().map_err(|_| usage())?),
            "--out" => o.out = Some(PathBuf::from(value)),
            _ => return Err(usage()),
        }
    }
    let sources = !o.files.is_empty();
    if o.tests.is_empty() || sources != o.vectors.is_empty() || o.sample == 0 || o.jobs == Some(0) {
        return Err(usage());
    }
    Ok(o)
}

pub fn run(args: &[String]) -> Result {
    let options = options(args)?;
    let started = Instant::now();
    let work = std::env::var_os("CARGO_TARGET_DIR")
        .map_or_else(|| workspace_root().join("target"), PathBuf::from)
        .join("mutate");
    fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let (mut report, missed) = match options.files.is_empty() {
        true => records::run(&options, &work)?,
        false => sources(&options, &work)?,
    };
    report.push_str(&format!(
        "\n{} in {:.1} minutes.\n",
        std::env::consts::OS,
        started.elapsed().as_secs_f64() / 60.0
    ));
    print!("{report}");
    if let Some(out) = &options.out {
        fs::write(out, &report).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    match missed {
        0 => Ok(()),
        n => Err(format!("mutate: {n} missed")),
    }
}

/// A seeded order (splitmix64), so the same seed draws the same sample everywhere.
struct Draw(u64);

impl Draw {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    /// `n` of `items`, in their original order.
    fn sample<T>(&mut self, mut items: Vec<T>, n: usize) -> Vec<T> {
        let mut order: Vec<usize> = (0..items.len()).collect();
        for i in (1..order.len()).rev() {
            order.swap(i, self.below(i + 1));
        }
        let mut kept = order;
        kept.truncate(n);
        kept.sort_unstable();
        let mut slots: Vec<Option<T>> = items.drain(..).map(Some).collect();
        kept.into_iter()
            .map(|i| slots[i].take().expect("each index once"))
            .collect()
    }
}

#[derive(Clone, Debug)]
struct Mutant {
    name: String,
    file: String,
    genre: String,
    replacement: String,
    /// 1-based line and column of the mutated text.
    start: (usize, usize),
}

impl Mutant {
    fn parse(json: &Json) -> Option<Mutant> {
        let text = |key: &str| Some(json.get(key)?.as_str()?.to_string_lossy());
        let at = |key: &str| {
            let p = json.get("span")?.get(key)?;
            Some((
                p.get("line")?.as_f64()? as usize,
                p.get("column")?.as_f64()? as usize,
            ))
        };
        Some(Mutant {
            name: text("name")?,
            file: text("file")?,
            genre: text("genre")?,
            replacement: text("replacement")?,
            start: at("start")?,
        })
    }
}

fn sources(options: &Options, work: &Path) -> Result<(String, usize)> {
    let root = workspace_root();
    for file in &options.files {
        if !root.join(file).is_file() {
            return Err(format!("mutate: {file} is not a file of the checkout"));
        }
    }
    let tool = cargo_mutants()?;
    let mut list = mutants_command(&tool, &root);
    list.args(["--list", "--json"]);
    for file in &options.files {
        list.arg("--file").arg(file);
    }
    let output = list
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| format!("cargo mutants: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo mutants --list exited with {}",
            output.status
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let listed = json::parse(&text).map_err(|e| format!("cargo mutants --list: {e}"))?;
    let all: Vec<Mutant> = listed
        .as_array()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            Mutant::parse(m).ok_or_else(|| format!("cargo mutants --list: {}", m.to_compact()))
        })
        .collect::<Result<_>>()?;
    if all.is_empty() {
        return Err("mutate: cargo-mutants has no mutant of these files".to_owned());
    }
    let total = all.len();
    let sample = Draw(options.seed).sample(all, options.sample);

    let out = work.join("mutants.out");
    if out.exists() {
        fs::remove_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    let tmp = work.join("tmp");
    fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let mut command = mutants_command(&tool, &root);
    let packages: Vec<&str> = packages(options);
    command
        .args([
            "--no-shuffle",
            "--colors",
            "never",
            "--caught",
            "--unviable",
        ])
        .args(["--cap-lints", "true"])
        .arg("--jobs")
        .arg(options.jobs.unwrap_or(2).to_string())
        .arg("--output")
        .arg(work)
        .arg("--test-package")
        .arg(packages.join(","))
        .env("TMPDIR", &tmp)
        .stdout(Stdio::from(std::io::stderr()));
    for file in &options.files {
        command.arg("--file").arg(file);
    }
    for mutant in &sample {
        command
            .arg("--re")
            .arg(format!("^{}$", escape(&mutant.name)));
    }
    command.arg("--");
    for (_, test) in &options.tests {
        command.arg("--test").arg(test);
    }
    if let Some(only) = &options.only {
        command.arg(only);
    }
    let status = command
        .status()
        .map_err(|e| format!("cargo mutants: {e}"))?;
    // 2: mutants were missed; 3: some timed out.
    if !matches!(status.code(), Some(0 | 2 | 3)) {
        return Err(format!(
            "cargo mutants failed ({status}); the unmutated tests must pass (logs in {})",
            out.join("log").display()
        ));
    }
    let outcomes = fs::read_to_string(out.join("outcomes.json"))
        .map_err(|e| format!("mutants.out/outcomes.json: {e}"))?;
    let outcomes = parse_outcomes(&outcomes)?;
    let of = |summary: &str| -> Vec<&Mutant> {
        sample
            .iter()
            .filter(|m| outcomes.get(&m.name).is_some_and(|s| s == summary))
            .collect()
    };
    let (caught, missed) = (of("CaughtMutant"), of("MissedMutant"));
    let (timed_out, unviable) = (of("Timeout"), of("Unviable"));
    let counted = caught.len() + missed.len() + timed_out.len() + unviable.len();
    if counted != sample.len() {
        return Err(format!(
            "mutate: cargo-mutants ran {counted} of the {} sampled mutants",
            sample.len()
        ));
    }
    let verdicts = match options.probe && !missed.is_empty() {
        true => Some(probe::run(options, work, &missed)?),
        false => None,
    };

    let mut report = format!(
        "# Mutants of {}\n\n{} of {total} (seed {}), against {}: {} caught, {} missed, {} timed out, {} did not build.\n",
        options.files.join(", "),
        sample.len(),
        options.seed,
        test_names(options),
        caught.len(),
        missed.len(),
        timed_out.len(),
        unviable.len(),
    );
    if !missed.is_empty() {
        report.push_str("\n## Missed\n\n");
        for (i, m) in missed.iter().enumerate() {
            match &verdicts {
                Some(v) => report.push_str(&format!("- {}: **{}**\n", m.name, v[i].name())),
                None => report.push_str(&format!("- {}\n", m.name)),
            }
        }
    }
    for (title, list) in [
        ("Timed out (counts as caught)", &timed_out),
        ("Did not build", &unviable),
        ("Caught", &caught),
    ] {
        if !list.is_empty() {
            report.push_str(&format!("\n## {title}\n\n"));
            for m in list {
                report.push_str(&format!("- {}\n", m.name));
            }
        }
    }
    Ok((report, missed.len()))
}

fn packages(options: &Options) -> Vec<&str> {
    let mut packages: Vec<&str> = options.tests.iter().map(|(p, _)| p.as_str()).collect();
    packages.sort_unstable();
    packages.dedup();
    packages
}

fn test_names(options: &Options) -> String {
    let list: Vec<String> = options
        .tests
        .iter()
        .map(|(p, t)| format!("`{p}:{t}`"))
        .collect();
    list.join(", ")
}

/// Mutant name to summary, from cargo-mutants' outcomes.json.
fn parse_outcomes(text: &str) -> Result<BTreeMap<String, String>> {
    let json = json::parse(text).map_err(|e| format!("outcomes.json: {e}"))?;
    let outcomes = json
        .get("outcomes")
        .and_then(Json::as_array)
        .ok_or("outcomes.json has no outcomes")?;
    let mut out = BTreeMap::new();
    for outcome in outcomes {
        let Some(mutant) = outcome.get("scenario").and_then(|s| s.get("Mutant")) else {
            continue;
        };
        let name = mutant.get("name").and_then(Json::as_str);
        let summary = outcome.get("summary").and_then(Json::as_str);
        let (Some(name), Some(summary)) = (name, summary) else {
            return Err(format!("outcomes.json: {}", outcome.to_compact()));
        };
        out.insert(name.to_string_lossy(), summary.to_string_lossy());
    }
    Ok(out)
}

fn escape(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if "\\.+*?()|[]{}^$#&-~".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn mutants_command(tool: &Path, root: &Path) -> Command {
    let mut command = Command::new(tool);
    command
        .args(["mutants", "--no-config"])
        .current_dir(root)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR");
    command
}

fn cargo_mutants() -> Result<PathBuf> {
    let (cache, _) = asrs_parity::cache::cache_dir();
    let root = cache.join(format!("cargo-mutants-{CARGO_MUTANTS_VERSION}"));
    let tool = root
        .join("bin")
        .join(format!("cargo-mutants{}", std::env::consts::EXE_SUFFIX));
    if !tool.is_file() {
        eprintln!("==> cargo install cargo-mutants {CARGO_MUTANTS_VERSION}");
        let status = Command::new(cargo_binary())
            .args(["install", "cargo-mutants", "--locked", "--version"])
            .arg(format!("={CARGO_MUTANTS_VERSION}"))
            .arg("--root")
            .arg(&root)
            .env_remove("CARGO_TARGET_DIR")
            .status()
            .map_err(|e| format!("cargo install: {e}"))?;
        if !status.success() {
            return Err(format!("cargo install cargo-mutants failed ({status})"));
        }
    }
    let output = Command::new(&tool)
        .args(["mutants", "--version"])
        .output()
        .map_err(|e| format!("{}: {e}", tool.display()))?;
    let version = String::from_utf8_lossy(&output.stdout);
    if version.trim() != format!("cargo-mutants {CARGO_MUTANTS_VERSION}") {
        return Err(format!("{} reports {version:?}", tool.display()));
    }
    Ok(tool)
}

fn cargo_binary() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn files_and_vectors_are_two_modes() {
        let o = options(&strings(&[
            "a.rs",
            "--test",
            "asrs-hub:dom_tabs",
            "--seed",
            "7",
        ]))
        .unwrap();
        assert_eq!(o.tests, [("asrs-hub".to_owned(), "dom_tabs".to_owned())]);
        assert_eq!((o.seed, o.sample), (7, 40));
        for bad in [
            &["a.rs"][..],
            &["--test", "p:t"],
            &["a.rs", "--vector", "v", "--test", "p:t"],
            &["a.rs", "--test", "dom_tabs"],
            &["a.rs", "--test", "p:t", "--sample", "0"],
            &["a.rs", "--test", "p:t", "--jobs", "0"],
        ] {
            assert!(options(&strings(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn samples_are_seeded_and_bounded() {
        let draw = |seed, n| Draw(seed).sample((0..100).collect::<Vec<_>>(), n);
        assert_eq!(draw(1, 40), draw(1, 40));
        assert_ne!(draw(1, 40), draw(2, 40));
        assert_eq!(draw(1, 40).len(), 40);
        assert!(draw(3, 40).is_sorted());
        assert_eq!(draw(1, 1000), (0..100).collect::<Vec<_>>());
    }

    #[test]
    fn outcomes_skip_the_baseline() {
        let text = r#"{"outcomes":[{"scenario":"Baseline","summary":"Success"},
            {"scenario":{"Mutant":{"name":"a.rs:1:2: x"}},"summary":"MissedMutant"}]}"#;
        let outcomes = parse_outcomes(text).unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes["a.rs:1:2: x"], "MissedMutant");
    }

    #[test]
    fn names_are_matched_literally() {
        assert_eq!(
            escape("a.rs:96:13: replace && with || in X::y -> Vec<T>"),
            "a\\.rs:96:13: replace \\&\\& with \\|\\| in X::y \\-> Vec<T>"
        );
    }
}
