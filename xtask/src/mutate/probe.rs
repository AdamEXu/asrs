use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use syn::spanned::Spanned;
use syn::visit::{self, Visit};

use super::{Mutant, Options, cargo_binary, packages};
use crate::{Result, workspace_root};

const LOG_VARIABLE: &str = "ASRS_MUTATE_PROBE";
const MODULE: &str = "__asrs_mutate_probe";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Verdict {
    NotReached,
    Differs,
    Agrees,
    Reached,
    Unprobed,
}

impl Verdict {
    pub(super) fn name(self) -> &'static str {
        match self {
            Verdict::NotReached => "not reached",
            Verdict::Differs => "differs",
            Verdict::Agrees => "agrees",
            Verdict::Reached => "reached",
            Verdict::Unprobed => "unprobed",
        }
    }
}

/// How much a mutant's probe records: the value too, or only that the place is reached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Depth {
    Value,
    Reach,
    Nothing,
}

impl Depth {
    fn shallower(self) -> Depth {
        match self {
            Depth::Value => Depth::Reach,
            Depth::Reach | Depth::Nothing => Depth::Nothing,
        }
    }
}

/// Runs the tests once on a copy of the checkout whose mutated places report being reached, and
/// what the mutated expression would have given. The code's own results stay as they are.
pub(super) fn run(options: &Options, work: &Path, mutants: &[&Mutant]) -> Result<Vec<Verdict>> {
    let root = workspace_root();
    let copy = work.join("probe-src");
    sync(&root, &copy)?;
    let log = work.join("probe.log");
    let mut depths = vec![Depth::Value; mutants.len()];
    for _ in 0..=2 * mutants.len() {
        let mut probes: BTreeMap<&str, Vec<(usize, &Mutant, Depth)>> = BTreeMap::new();
        for (i, m) in mutants.iter().enumerate() {
            probes.entry(&m.file).or_default().push((i, m, depths[i]));
        }
        let mut lines: BTreeMap<String, Vec<(usize, usize, usize)>> = BTreeMap::new();
        for (file, list) in &probes {
            let text = fs::read_to_string(root.join(file)).map_err(|e| format!("{file}: {e}"))?;
            let (probed, placed) = instrument(&text, list).map_err(|e| format!("{file}: {e}"))?;
            for (i, _, _) in list {
                match placed.iter().find(|(id, _, _)| id == i) {
                    Some(place) => lines.entry((*file).to_owned()).or_default().push(*place),
                    None => depths[*i] = Depth::Nothing,
                }
            }
            write_changed(&copy.join(file), probed.as_bytes())?;
        }
        eprintln!("==> probe build");
        let build = test_command(options, &copy, work)
            .args(["--no-run", "--message-format=short"])
            .output()
            .map_err(|e| format!("cargo test: {e}"))?;
        if build.status.success() {
            let _ = fs::remove_file(&log);
            eprintln!("==> probe run");
            let test = test_command(options, &copy, work)
                .env(LOG_VARIABLE, &log)
                .output()
                .map_err(|e| format!("cargo test: {e}"))?;
            if !test.status.success() {
                return Err(format!(
                    "mutate: the probed build fails the tests the original passes:\n{}",
                    tail(&test.stdout)
                ));
            }
            let seen = fs::read_to_string(&log).unwrap_or_default();
            return Ok((0..mutants.len())
                .map(|i| verdict(i, depths[i], &seen))
                .collect());
        }
        // A probe that does not compile is made shallower. Probes add no lines, so an error's
        // line finds it.
        let stderr = String::from_utf8_lossy(&build.stderr);
        let mut changed = false;
        for (file, line) in error_lines(&stderr) {
            for (id, from, to) in lines.get(&file).into_iter().flatten() {
                if (*from..=*to).contains(&line) && depths[*id] != Depth::Nothing {
                    depths[*id] = depths[*id].shallower();
                    changed = true;
                }
            }
        }
        if !changed {
            return Err(format!(
                "mutate: the probed build fails:\n{}",
                tail(&build.stderr)
            ));
        }
    }
    Err("mutate: no probed build compiles".to_owned())
}

fn verdict(id: usize, depth: Depth, log: &str) -> Verdict {
    let has = |what: &str| log.lines().any(|l| l == format!("{id} {what}"));
    match (depth, has("differs"), has("agrees"), has("reached")) {
        (Depth::Nothing, ..) => Verdict::Unprobed,
        (_, true, ..) => Verdict::Differs,
        (_, _, true, _) => Verdict::Agrees,
        (_, _, _, true) => Verdict::Reached,
        _ => Verdict::NotReached,
    }
}

fn test_command(options: &Options, copy: &Path, work: &Path) -> Command {
    let mut command = Command::new(cargo_binary());
    command
        .arg("test")
        .current_dir(copy)
        .env("CARGO_TARGET_DIR", work.join("probe-target"))
        .env("RUSTFLAGS", "--cap-lints=warn");
    for package in packages(options) {
        command.args(["-p", package]);
    }
    for (_, test) in &options.tests {
        command.args(["--test", test]);
    }
    if let Some(only) = &options.only {
        command.arg(only);
    }
    command
}

fn tail(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(40)..].join("\n")
}

/// `path:line:col: error...` lines of rustc's short format.
fn error_lines(stderr: &str) -> Vec<(String, usize)> {
    stderr
        .lines()
        .filter_map(|l| {
            let mut parts = l.splitn(4, ':');
            let file = parts.next()?;
            let line = parts.next()?.parse().ok()?;
            parts.next()?;
            parts
                .next()?
                .trim_start()
                .starts_with("error")
                .then(|| (file.replace('\\', "/"), line))
        })
        .collect()
}

/// Copies every tracked file that changed since the last copy, keeping the times of the rest so
/// the probe build is incremental.
fn sync(root: &Path, copy: &Path) -> Result {
    let output = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("git ls-files: {e}"))?;
    if !output.status.success() {
        return Err("mutate: git ls-files failed; the probe copies the tracked files".to_owned());
    }
    for name in String::from_utf8_lossy(&output.stdout).split('\0') {
        let from = root.join(name);
        if name.is_empty() || !from.is_file() {
            continue;
        }
        let bytes = fs::read(&from).map_err(|e| format!("{name}: {e}"))?;
        write_changed(&copy.join(name), &bytes)?;
    }
    Ok(())
}

fn write_changed(path: &PathBuf, bytes: &[u8]) -> Result {
    if fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(());
    }
    let dir = path.parent().expect("a file has a directory");
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Text inserted at a byte offset, or replacing `len` bytes there. Insertions that open a probe
/// go outermost first and ones that close it innermost first, so nested probes nest.
#[derive(Debug)]
struct Edit {
    at: usize,
    len: usize,
    closes: bool,
    size: usize,
    text: String,
}

type Placed = Vec<(usize, usize, usize)>;

fn instrument(text: &str, probes: &[(usize, &Mutant, Depth)]) -> Result<(String, Placed)> {
    let file = syn::parse_file(text).map_err(|e| e.to_string())?;
    let mut finder = Finder {
        text,
        starts: line_starts(text),
        probes,
        edits: Vec::new(),
        placed: Vec::new(),
    };
    finder.visit_file(&file);
    let Finder {
        mut edits, placed, ..
    } = finder;
    edits.sort_by(|a, b| {
        let order = |e: &Edit| match (e.len > 0, e.closes) {
            (true, _) => 2,
            (false, true) => 0,
            (false, false) => 1,
        };
        a.at.cmp(&b.at)
            .then(order(a).cmp(&order(b)))
            .then(match a.closes {
                true => a.size.cmp(&b.size),
                false => b.size.cmp(&a.size),
            })
    });
    let mut out = String::new();
    let mut from = 0;
    for edit in &edits {
        out.push_str(&text[from..edit.at]);
        out.push_str(&edit.text);
        from = edit.at + edit.len;
    }
    out.push_str(&text[from..]);
    if !placed.is_empty() {
        out.push_str(&runtime(
            probes.iter().map(|(id, ..)| *id).max().unwrap_or(0) + 1,
        ));
    }
    Ok((out, placed))
}

fn runtime(count: usize) -> String {
    format!(
        r#"
#[allow(dead_code, clippy::all, clippy::disallowed_methods, clippy::disallowed_types)]
mod {MODULE} {{
    use std::fmt::Debug;
    use std::io::Write;
    use std::sync::atomic::{{AtomicBool, Ordering}};
    static SEEN: [[AtomicBool; 3]; {count}] = [const {{ [const {{ AtomicBool::new(false) }}; 3] }}; {count}];
    fn note(id: usize, kind: usize, what: &str) {{
        if SEEN[id][kind].swap(true, Ordering::SeqCst) {{ return; }}
        if let Some(path) = std::env::var_os("{LOG_VARIABLE}") {{
            let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).expect("probe log");
            file.write_all(format!("{{id}} {{what}}\n").as_bytes()).expect("probe log");
        }}
    }}
    pub fn reach(id: usize) {{ note(id, 0, "reached"); }}
    fn same(id: usize, same: bool) {{
        if same {{ note(id, 1, "agrees"); }} else {{ note(id, 2, "differs"); }}
    }}
    pub fn value<T: Debug>(id: usize, own: T, mutant: impl FnOnce() -> T) -> T {{
        let mutant = std::panic::catch_unwind(std::panic::AssertUnwindSafe(mutant));
        same(id, mutant.is_ok_and(|m| format!("{{m:?}}") == format!("{{own:?}}")));
        own
    }}
    pub fn and(id: usize, l: bool, r: impl FnOnce() -> bool) -> bool {{
        reach(id);
        if l {{ let r = r(); same(id, r); r }} else {{ false }}
    }}
    pub fn or(id: usize, l: bool, r: impl FnOnce() -> bool) -> bool {{
        reach(id);
        if l {{ true }} else {{ let r = r(); same(id, !r); r }}
    }}
}}
"#
    )
}

fn line_starts(text: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect()
}

struct Finder<'a> {
    text: &'a str,
    starts: Vec<usize>,
    probes: &'a [(usize, &'a Mutant, Depth)],
    edits: Vec<Edit>,
    placed: Placed,
}

impl Finder<'_> {
    fn offset(&self, at: proc_macro2::LineColumn) -> usize {
        let start = self.starts[at.line - 1];
        self.text[start..]
            .char_indices()
            .nth(at.column)
            .map_or(self.text.len(), |(i, _)| start + i)
    }

    /// The probes of `genre` at the place a span starts.
    fn at(&self, genre: &str, span: proc_macro2::Span) -> Vec<(usize, String, Depth)> {
        let at = span.start();
        self.probes
            .iter()
            .filter(|(_, m, depth)| {
                m.genre == genre && m.start == (at.line, at.column + 1) && *depth != Depth::Nothing
            })
            .map(|(id, m, depth)| (*id, m.replacement.clone(), *depth))
            .collect()
    }

    fn place(&mut self, id: usize, node: proc_macro2::Span) {
        self.placed.push((id, node.start().line, node.end().line));
    }

    fn insert(&mut self, at: usize, closes: bool, size: usize, text: String) {
        self.edits.push(Edit {
            at,
            len: 0,
            closes,
            size,
            text,
        });
    }

    fn wrap(&mut self, node: proc_macro2::Span, before: String, after: String) {
        let (from, to) = (self.offset(node.start()), self.offset(node.end()));
        self.insert(from, false, to - from, before);
        self.insert(to, true, to - from, after);
    }

    /// `{ reach(id); <expr> }`.
    fn reach(&mut self, ids: &[usize], node: proc_macro2::Span) {
        let calls: String = ids
            .iter()
            .map(|id| format!("{MODULE}::reach({id}); "))
            .collect();
        self.wrap(node, format!("{{ {calls}"), " }".to_owned());
        for id in ids {
            self.place(*id, node);
        }
    }
}

impl<'ast> Visit<'ast> for Finder<'_> {
    fn visit_block(&mut self, node: &'ast syn::Block) {
        if let Some(first) = node.stmts.first() {
            let ids: Vec<usize> = self
                .at("FnValue", first.span())
                .into_iter()
                .map(|(id, ..)| id)
                .collect();
            if !ids.is_empty() {
                let calls: String = ids
                    .iter()
                    .map(|id| format!("{MODULE}::reach({id}); "))
                    .collect();
                let at = self.offset(first.span().start());
                self.insert(at, false, usize::MAX, calls);
                for id in ids {
                    self.place(id, first.span());
                }
            }
        }
        visit::visit_block(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        let found = self.at("BinaryOperator", node.op.span());
        let values: Vec<&(usize, String, Depth)> =
            found.iter().filter(|f| f.2 == Depth::Value).collect();
        let assigns = node.op.span().source_text().is_some_and(|op| {
            op.ends_with('=') && !["==", "!=", "<=", ">="].contains(&op.as_str())
        });
        if let Some((id, _, _)) = values.first()
            && !assigns
        {
            let span = node.span();
            let (from, to) = (self.offset(span.start()), self.offset(span.end()));
            let op_at = self.offset(node.op.span().start());
            let op_len = self.offset(node.op.span().end()) - op_at;
            let own = self.text[op_at..op_at + op_len].to_owned();
            let (before, between, after) = match own.as_str() {
                "&&" => (
                    format!("{MODULE}::and({id}, "),
                    ", || ".to_owned(),
                    ")".to_owned(),
                ),
                "||" => (
                    format!("{MODULE}::or({id}, "),
                    ", || ".to_owned(),
                    ")".to_owned(),
                ),
                _ => {
                    let mut own = format!("l {own} r");
                    for (id, mutant, _) in &values {
                        own = format!("{MODULE}::value({id}, {own}, || l {mutant} r)");
                    }
                    (
                        "(match (&(".to_owned(),
                        "), &(".to_owned(),
                        format!(")) {{ (l, r) => {own} }})"),
                    )
                }
            };
            self.insert(from, false, to - from, before);
            self.edits.push(Edit {
                at: op_at,
                len: op_len,
                closes: false,
                size: 0,
                text: between,
            });
            self.insert(to, true, to - from, after);
            for (id, _, _) in &values {
                self.place(*id, span);
            }
        }
        let reach: Vec<usize> = found
            .iter()
            .filter(|f| f.2 == Depth::Reach || assigns)
            .map(|f| f.0)
            .collect();
        if !reach.is_empty() && (assigns || values.is_empty()) {
            self.reach(&reach, node.span());
        }
        visit::visit_expr_binary(self, node);
    }

    fn visit_expr_unary(&mut self, node: &'ast syn::ExprUnary) {
        for (id, _, depth) in self.at("UnaryOperator", node.op.span()) {
            let span = node.span();
            match depth {
                Depth::Value => {
                    let op = match node.op {
                        syn::UnOp::Not(_) => "!",
                        _ => "-",
                    };
                    self.wrap(
                        span,
                        "(match ".to_owned(),
                        format!(" {{ v => {MODULE}::value({id}, v, || {op}v) }})"),
                    );
                    self.place(id, span);
                }
                _ => self.reach(&[id], span),
            }
        }
        visit::visit_expr_unary(self, node);
    }

    fn visit_arm(&mut self, node: &'ast syn::Arm) {
        for (id, ..) in self.at("MatchArm", node.span()) {
            self.reach(&[id], node.body.span());
        }
        if let Some((_, guard)) = &node.guard {
            for (id, replacement, depth) in self.at("MatchArmGuard", guard.span()) {
                match depth {
                    Depth::Value => {
                        self.wrap(
                            guard.span(),
                            format!("{MODULE}::value({id}, "),
                            format!(", || {replacement})"),
                        );
                        self.place(id, guard.span());
                    }
                    _ => self.reach(&[id], guard.span()),
                }
            }
        }
        visit::visit_arm(self, node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mutant(genre: &str, replacement: &str, line: usize, column: usize) -> Mutant {
        Mutant {
            name: String::new(),
            file: "a.rs".to_owned(),
            genre: genre.to_owned(),
            replacement: replacement.to_owned(),
            start: (line, column),
        }
    }

    fn body(text: &str, mutants: &[Mutant], depth: Depth) -> String {
        let probes: Vec<(usize, &Mutant, Depth)> = mutants
            .iter()
            .enumerate()
            .map(|(i, m)| (i, m, depth))
            .collect();
        let (out, placed) = instrument(text, &probes).unwrap();
        assert_eq!(placed.len(), mutants.len(), "{out}");
        syn::parse_file(&out).unwrap_or_else(|e| panic!("{e}: {out}"));
        out.lines().next().unwrap().to_owned()
    }

    #[test]
    fn comparisons_evaluate_each_side_once() {
        assert_eq!(
            body(
                "fn f() -> bool { a() < b }",
                &[mutant("BinaryOperator", "<=", 1, 22)],
                Depth::Value
            ),
            "fn f() -> bool { (match (&(a() ), &( b)) { (l, r) => __asrs_mutate_probe::value(0, l < r, || l <= r) }) }"
        );
    }

    #[test]
    fn several_mutants_of_one_operator_share_a_probe() {
        let out = body(
            "fn f() -> f64 { a + b }",
            &[
                mutant("BinaryOperator", "-", 1, 19),
                mutant("BinaryOperator", "*", 1, 19),
            ],
            Depth::Value,
        );
        assert!(
            out.contains("value(1, __asrs_mutate_probe::value(0, l + r, || l - r), || l * r)"),
            "{out}"
        );
    }

    #[test]
    fn nested_probes_nest() {
        let out = body(
            "fn f() -> bool { a < b && c < d }",
            &[
                mutant("BinaryOperator", "||", 1, 24),
                mutant("BinaryOperator", ">", 1, 20),
                mutant("BinaryOperator", ">", 1, 29),
            ],
            Depth::Value,
        );
        assert!(
            out.starts_with("fn f() -> bool { __asrs_mutate_probe::and(0, (match (&(a "),
            "{out}"
        );
        assert!(out.ends_with("|| l > r) })) }"), "{out}");
    }

    #[test]
    fn other_genres_and_depths() {
        assert_eq!(
            body(
                "fn f() -> bool { self.x }",
                &[mutant("FnValue", "true", 1, 18)],
                Depth::Value
            ),
            "fn f() -> bool { __asrs_mutate_probe::reach(0); self.x }"
        );
        assert_eq!(
            body(
                "fn f() { match x { A => g(), _ => h() } }",
                &[mutant("MatchArm", "", 1, 20)],
                Depth::Value
            ),
            "fn f() { match x { A => { __asrs_mutate_probe::reach(0); g() }, _ => h() } }"
        );
        assert_eq!(
            body(
                "fn f() { x += 1; }",
                &[mutant("BinaryOperator", "-=", 1, 12)],
                Depth::Value
            ),
            "fn f() { { __asrs_mutate_probe::reach(0); x += 1 }; }"
        );
        assert_eq!(
            body(
                "fn f() -> bool { !x }",
                &[mutant("UnaryOperator", "", 1, 18)],
                Depth::Value
            ),
            "fn f() -> bool { (match !x { v => __asrs_mutate_probe::value(0, v, || !v) }) }"
        );
        assert_eq!(
            body(
                "fn f() -> bool { a < b }",
                &[mutant("BinaryOperator", "<=", 1, 20)],
                Depth::Reach
            ),
            "fn f() -> bool { { __asrs_mutate_probe::reach(0); a < b } }"
        );
    }

    #[test]
    fn verdicts_rank_differing_first() {
        let log = "0 reached\n1 reached\n1 agrees\n2 agrees\n2 differs\n";
        let of = |id| verdict(id, Depth::Value, log);
        assert_eq!(of(0), Verdict::Reached);
        assert_eq!(of(1), Verdict::Agrees);
        assert_eq!(of(2), Verdict::Differs);
        assert_eq!(of(3), Verdict::NotReached);
        assert_eq!(verdict(0, Depth::Nothing, log), Verdict::Unprobed);
    }

    #[test]
    fn short_errors_name_their_line() {
        assert_eq!(
            error_lines(
                "crates/a/src/b.rs:12:5: error[E0308]: mismatched types\ncrates/a/src/b.rs:13:5: warning: x\n"
            ),
            [("crates/a/src/b.rs".to_owned(), 12)]
        );
    }
}
