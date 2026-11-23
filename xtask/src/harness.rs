use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

use crate::{Result, workspace_root};

const ALLOW: &str = "xtask/harness-allow.toml";
const USAGE: &str = "usage: cargo xtask harness [--list] [--root <checkout>]

Strictness lint over replay harnesses (files of crates/*/tests that use asrs_parity, and
crates/asrs-parity/src/live), part of `cargo xtask check`. Rules:
  default-slot   unwrap_or_default, or an empty value given another way (unwrap_or(\"\"),
                 unwrap_or_else(Vec::new), None => Default::default()): an absent recorded value or
                 port result becomes one that can compare equal. Name the absence: ok_or_else,
                 expect, or an Option comparison. A string matched on afterwards is fine if the
                 catch-all arm fails. Named values (0.0, false, Value::Undefined) do not count.
  catch-all      a match on recorded strings or Json values whose catch-all arm does nothing (or
                 only prints or counts), so an unknown record kind, field or operation passes.
                 Return an error or panic. An arm that gives a value is not seen.
  skipped-kind   a match arm naming a recorded kind and doing nothing, so its records are skipped.
                 Compare it, or assert where another harness does.
  silent-return  an early return, continue or break out of a replay because a recorded field is
                 absent (let-else, a None arm, or an if whose whole body leaves). An `if let` on
                 an optional field, filter_map and `?` are not seen; read through tally::Fields.
  open-chain     an if/else-if chain over recorded fields, or comparing with strings, that has no
                 final else, so a record that matches none passes.
  no-tally       a file that loads vectors itself and never checks how many records it compared
                 (an assertion on a length or a compared/recorded count, or two lengths compared).
                 Use asrs_parity::tally::Tally, or check_records and check_fn_vectors.
Findings in macro arguments (assertion messages) do not count. xtask/harness-allow.toml lists the
ones that stay, by file, function and rule, each with a reason. --list prints every finding,
allowed ones too, in the allow file's format.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Rule {
    DefaultSlot,
    CatchAll,
    SkippedKind,
    SilentReturn,
    OpenChain,
    NoTally,
}

impl Rule {
    const ALL: [Rule; 6] = [
        Rule::DefaultSlot,
        Rule::CatchAll,
        Rule::SkippedKind,
        Rule::SilentReturn,
        Rule::OpenChain,
        Rule::NoTally,
    ];

    fn name(self) -> &'static str {
        match self {
            Rule::DefaultSlot => "default-slot",
            Rule::CatchAll => "catch-all",
            Rule::SkippedKind => "skipped-kind",
            Rule::SilentReturn => "silent-return",
            Rule::OpenChain => "open-chain",
            Rule::NoTally => "no-tally",
        }
    }

    fn parse(name: &str) -> Option<Rule> {
        Rule::ALL.into_iter().find(|r| r.name() == name)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Finding {
    rule: Rule,
    function: String,
    line: usize,
    text: String,
}

/// A whole file's entry has the function `*`.
const ANY_FUNCTION: &str = "*";

#[derive(Debug)]
struct Allow {
    file: String,
    function: String,
    rule: Rule,
    count: usize,
}

pub fn run(args: &[String]) -> Result {
    let mut list = false;
    let mut root = workspace_root();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => list = true,
            "--root" => root = PathBuf::from(args.next().ok_or_else(|| USAGE.to_owned())?),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let allow_path = root.join(ALLOW);
    let allow_path = match allow_path.exists() {
        true => allow_path,
        false => workspace_root().join(ALLOW),
    };
    let allow = read_allow(&allow_path)?;
    let mut findings: BTreeMap<String, Vec<Finding>> = BTreeMap::new();
    for path in harness_files(&root)? {
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file = path
            .strip_prefix(&root)
            .expect("under the root")
            .to_string_lossy()
            .replace('\\', "/");
        if !text.contains("asrs_parity") && !file.starts_with("crates/asrs-parity/") {
            continue;
        }
        let found = lint(&text).map_err(|e| format!("{file}: {e}"))?;
        if !found.is_empty() {
            findings.insert(file, found);
        }
    }
    if list {
        print!("{}", listing(&findings));
        return Ok(());
    }
    let (mut problems, stale) = judge(&findings, &allow);
    for (file, note) in stale {
        // An entry for a file of a branch that has not landed is only reported.
        match root.join(&file).exists() {
            true => problems.push(note),
            false => eprintln!("harness lint: {note}"),
        }
    }
    if problems.is_empty() {
        let allowed: usize = findings.values().map(Vec::len).sum();
        eprintln!("harness ok ({allowed} allowed findings)");
        return Ok(());
    }
    Err(format!(
        "harness lint failed (see `cargo xtask harness --help`; a finding that must stay goes in {ALLOW} with a reason):\n  {}",
        problems.join("\n  ")
    ))
}

fn harness_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let crates = root.join("crates");
    for entry in fs::read_dir(&crates).map_err(|e| format!("{}: {e}", crates.display()))? {
        let tests = entry.map_err(|e| e.to_string())?.path().join("tests");
        if tests.is_dir() {
            crate::layout::collect_rs(&tests, &mut files)?;
        }
    }
    crate::layout::collect_rs(&crates.join("asrs-parity/src/live"), &mut files)?;
    files.sort();
    Ok(files)
}

fn read_allow(path: &Path) -> Result<Vec<Allow>> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse_allow(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse_allow(text: &str) -> Result<Vec<Allow>> {
    let table: toml::Table = text.parse().map_err(|e| format!("{e}"))?;
    let entries = match table.get("allow") {
        Some(toml::Value::Array(entries)) => entries.as_slice(),
        Some(_) => return Err("\"allow\" must be a list of tables".to_owned()),
        None => &[],
    };
    let mut out: Vec<Allow> = Vec::new();
    for entry in entries {
        let string = |key: &str| {
            entry
                .get(key)
                .and_then(toml::Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("{entry}: needs a string {key:?}"))
        };
        let file = string("file")?;
        let function = string("function")?;
        let rule = string("rule")?;
        let rule = Rule::parse(&rule).ok_or_else(|| format!("{file}: unknown rule {rule:?}"))?;
        if string("reason")?.trim().len() < 12 {
            return Err(format!("{file} {function}: the reason says nothing"));
        }
        let count = match entry.get("count") {
            None => 1,
            Some(n) => n
                .as_integer()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| *n > 0)
                .ok_or_else(|| format!("{file} {function}: count must be a positive integer"))?,
        };
        if out
            .iter()
            .any(|a| a.file == file && a.function == function && a.rule == rule)
        {
            return Err(format!("{file} {function} {}: listed twice", rule.name()));
        }
        out.push(Allow {
            file,
            function,
            rule,
            count,
        });
    }
    Ok(out)
}

type Groups<'a> = BTreeMap<(&'a str, &'a str, Rule), Vec<&'a Finding>>;

fn group(findings: &BTreeMap<String, Vec<Finding>>) -> Groups<'_> {
    let mut groups: Groups<'_> = BTreeMap::new();
    for (file, found) in findings {
        for f in found {
            groups
                .entry((file, &f.function, f.rule))
                .or_default()
                .push(f);
        }
    }
    groups
}

/// The findings beyond the allowed counts, and the entries (with their file) that allow more than
/// there is.
fn judge(
    findings: &BTreeMap<String, Vec<Finding>>,
    allow: &[Allow],
) -> (Vec<String>, Vec<(String, String)>) {
    let groups = group(findings);
    let mut problems = Vec::new();
    for ((file, function, rule), found) in &groups {
        let allowed = allow
            .iter()
            .find(|a| a.file == *file && a.function == *function && a.rule == *rule)
            .map_or(0, |a| a.count);
        if found.len() <= allowed {
            continue;
        }
        for f in found {
            problems.push(format!(
                "{file}:{}: {} in {function}: {}{}",
                f.line,
                rule.name(),
                f.text,
                match allowed {
                    0 => String::new(),
                    n => format!(" ({n} of {} allowed here)", found.len()),
                }
            ));
        }
    }
    let stale = allow
        .iter()
        .filter_map(|a| {
            let n = groups
                .get(&(a.file.as_str(), a.function.as_str(), a.rule))
                .map_or(0, Vec::len);
            (n < a.count).then(|| {
                let note = format!(
                    "{ALLOW} allows {} {} in {} {}, and there are {n}",
                    a.count,
                    a.rule.name(),
                    a.file,
                    a.function
                );
                (a.file.clone(), note)
            })
        })
        .collect();
    (problems, stale)
}

fn listing(findings: &BTreeMap<String, Vec<Finding>>) -> String {
    let mut out = String::new();
    for ((file, function, rule), found) in group(findings) {
        for f in &found {
            out.push_str(&format!("# {}: {}\n", f.line, f.text));
        }
        out.push_str(&format!(
            "[[allow]]\nfile = {file:?}\nfunction = {function:?}\nrule = {:?}\ncount = {}\nreason = \"\"\n\n",
            rule.name(),
            found.len()
        ));
    }
    out
}

fn lint(text: &str) -> Result<Vec<Finding>> {
    let file = syn::parse_file(text).map_err(|e| format!("line {}: {e}", e.span().start().line))?;
    let mut lint = Lint {
        findings: Vec::new(),
        path: Vec::new(),
        loads: None,
        tallies: false,
        matched: Vec::new(),
    };
    lint.visit_file(&file);
    if let (Some((line, text)), false) = (lint.loads.take(), lint.tallies) {
        lint.findings.push(Finding {
            rule: Rule::NoTally,
            function: ANY_FUNCTION.to_owned(),
            line,
            text,
        });
    }
    lint.findings.sort_by_key(|f| f.line);
    Ok(lint.findings)
}

struct Lint {
    findings: Vec<Finding>,
    path: Vec<String>,
    loads: Option<(usize, String)>,
    tallies: bool,
    /// The defaulting calls whose value a strict match rejects.
    matched: Vec<Place>,
}

impl Lint {
    fn report(&mut self, rule: Rule, at: &impl ToTokens, line: usize) {
        self.report_text(rule, at.to_token_stream().to_string(), line);
    }

    fn report_text(&mut self, rule: Rule, mut text: String, line: usize) {
        if text.len() > 110 {
            let mut end = 110;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str(" ...");
        }
        self.findings.push(Finding {
            rule,
            function: self
                .path
                .last()
                .cloned()
                .unwrap_or_else(|| ANY_FUNCTION.to_owned()),
            line,
            text: text.replace('\n', " "),
        });
    }

    fn macro_tokens(&mut self, mac: &syn::Macro) {
        let Some(name) = mac.path.segments.last() else {
            return;
        };
        if matches!(
            name.ident.to_string().as_str(),
            "assert" | "assert_eq" | "assert_ne"
        ) {
            let conditions = if name.ident == "assert" { 1 } else { 2 };
            let args = mac.parse_body_with(
                syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
            );
            for arg in args.iter().flatten().take(conditions) {
                self.tallies |= counts_records(&arg.to_token_stream().to_string());
            }
        }
        // `fn_vectors!` and `record_vectors!` hold their replay closure in macro tokens.
        if let Ok(args) = mac.parse_body_with(
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
        ) && args.iter().any(|a| matches!(a, syn::Expr::Closure(_)))
        {
            for arg in &args {
                self.visit_expr(arg);
            }
        }
    }
}

/// Whether an assertion's tokens compare a number of records: a length or a counter named for
/// what was compared.
fn counts_records(tokens: &str) -> bool {
    tokens.contains(". len ()")
        || tokens
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .any(|word| {
                ["compared", "recorded", "tally", "checked", "replayed"]
                    .iter()
                    .any(|w| word.contains(w))
            })
}

/// Whether an expression reads a vector record or its JSON.
fn reads_record(expr: &impl ToTokens) -> bool {
    let tokens = expr.to_token_stream().to_string();
    [". field (", ". string (", ". value (\"", ". get (\""]
        .iter()
        .any(|read| tokens.contains(read))
}

/// Whether an expression reads a record or compares with a string literal.
fn tests_record(expr: &syn::Expr) -> bool {
    struct Compares(bool);
    impl<'ast> Visit<'ast> for Compares {
        fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
            self.0 |= matches!(node.op, syn::BinOp::Eq(_))
                && (is_string_literal(&node.left) || is_string_literal(&node.right));
            visit::visit_expr_binary(self, node);
        }
    }
    let mut compares = Compares(false);
    compares.visit_expr(expr);
    compares.0 || reads_record(expr)
}

fn is_string_literal(expr: &syn::Expr) -> bool {
    matches!(expr, syn::Expr::Lit(l) if matches!(l.lit, syn::Lit::Str(_)))
}

fn is_ident(expr: &syn::Expr, name: &str) -> bool {
    matches!(expr, syn::Expr::Path(p) if p.path.is_ident(name))
}

/// A value that stands for nothing: what `Default` gives, by any spelling.
fn is_empty_value(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Lit(l) => matches!(&l.lit, syn::Lit::Str(s) if s.value().is_empty()),
        syn::Expr::Array(a) => a.elems.is_empty(),
        syn::Expr::Reference(r) => is_empty_value(&r.expr),
        syn::Expr::Index(i) => is_empty_value(&i.expr),
        syn::Expr::Paren(p) => is_empty_value(&p.expr),
        syn::Expr::Block(b) => {
            matches!(b.block.stmts.as_slice(), [syn::Stmt::Expr(e, None)] if is_empty_value(e))
        }
        syn::Expr::Macro(m) => m.mac.path.is_ident("vec") && m.mac.tokens.is_empty(),
        syn::Expr::MethodCall(m) => {
            ["to_owned", "to_string", "into"]
                .iter()
                .any(|name| m.method == name)
                && is_empty_value(&m.receiver)
        }
        syn::Expr::Call(c) => {
            is_default_path(&c.func)
                && c.args
                    .iter()
                    .all(|a| is_empty_value(a) || is_ident(a, "None"))
        }
        _ => false,
    }
}

/// `Default::default`, `Vec::new` and the like.
fn is_default_path(expr: &syn::Expr) -> bool {
    matches!(expr, syn::Expr::Path(p) if p.path.segments.len() > 1
        && p.path.segments.last().is_some_and(|s| s.ident == "default" || s.ident == "new"))
}

/// A function that gives an empty value.
fn gives_empty(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Closure(c) => is_empty_value(&c.body),
        _ => is_default_path(expr),
    }
}

/// Whether a branch for an absent `from` gives an empty value. An empty list or string written
/// out for an absent port value is how the page shows it; only a `default()` is suspect there.
fn branch_defaults(from: &impl ToTokens, body: &syn::Expr) -> bool {
    is_empty_value(body)
        && (reads_record(from) || body.to_token_stream().to_string().contains("default ()"))
}

fn defaults(call: &syn::ExprMethodCall) -> bool {
    let first = call.args.first();
    match call.method.to_string().as_str() {
        "unwrap_or_default" => true,
        "unwrap_or" => first.is_some_and(is_empty_value),
        "unwrap_or_else" => first.is_some_and(gives_empty),
        "map_or" => first.is_some_and(is_empty_value) && reads_record(&call.receiver),
        "map_or_else" => first.is_some_and(gives_empty) && reads_record(&call.receiver),
        _ => false,
    }
}

/// A `None` or `Err(_)` pattern.
fn is_absence(pat: &syn::Pat) -> bool {
    match pat {
        syn::Pat::Ident(i) => i.ident == "None",
        syn::Pat::Path(p) => p.path.is_ident("None"),
        syn::Pat::TupleStruct(t) => t.path.is_ident("Err") && t.elems.iter().all(catches_all),
        _ => false,
    }
}

/// The arms a match on an optional takes when it is absent: `None`, `Err(_)`, or a wildcard
/// beside a `Some`.
fn absence_arms(node: &syn::ExprMatch) -> impl Iterator<Item = &syn::Arm> {
    let optional = node.arms.iter().any(|a| {
        matches!(&a.pat, syn::Pat::TupleStruct(t) if t.path.is_ident("Some") || t.path.is_ident("Ok"))
    });
    node.arms
        .iter()
        .filter(move |a| is_absence(&a.pat) || optional && matches!(a.pat, syn::Pat::Wild(_)))
}

/// An expression that leaves a replay as if nothing were wrong.
fn does_nothing(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Tuple(t) => t.elems.is_empty(),
        syn::Expr::Block(b) => b.block.stmts.iter().all(statement_does_nothing),
        syn::Expr::Macro(m) => prints(&m.mac),
        syn::Expr::Call(_) => is_ok_unit(expr),
        _ => leaves(expr) || counts_up(expr),
    }
}

fn statement_does_nothing(stmt: &syn::Stmt) -> bool {
    match stmt {
        syn::Stmt::Expr(syn::Expr::Verbatim(v), _) => v.is_empty(),
        syn::Stmt::Expr(e, _) => does_nothing(e),
        syn::Stmt::Local(l) => {
            matches!(l.pat, syn::Pat::Wild(_))
                && l.init.as_ref().is_some_and(|i| {
                    matches!(*i.expr, syn::Expr::Path(_) | syn::Expr::Reference(_))
                })
        }
        syn::Stmt::Macro(m) => prints(&m.mac),
        syn::Stmt::Item(_) => true,
    }
}

fn prints(mac: &syn::Macro) -> bool {
    ["print", "println", "eprint", "eprintln"]
        .iter()
        .any(|name| mac.path.is_ident(name))
}

fn counts_up(expr: &syn::Expr) -> bool {
    matches!(expr, syn::Expr::Binary(b) if matches!(b.op, syn::BinOp::AddAssign(_))
        && matches!(*b.left, syn::Expr::Path(_))
        && matches!(*b.right, syn::Expr::Lit(_)))
}

fn is_ok(expr: &syn::Expr, value: impl Fn(&syn::Expr) -> bool) -> bool {
    matches!(expr, syn::Expr::Call(c) if matches!(&*c.func, syn::Expr::Path(p)
            if p.path.segments.last().is_some_and(|s| s.ident == "Ok"))
        && c.args.len() == 1
        && value(&c.args[0]))
}

fn is_ok_unit(expr: &syn::Expr) -> bool {
    is_ok(
        expr,
        |e| matches!(e, syn::Expr::Tuple(t) if t.elems.is_empty()),
    )
}

fn leaves(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Continue(_) => true,
        syn::Expr::Break(b) => b.expr.is_none(),
        syn::Expr::Return(r) => match &r.expr {
            None => true,
            Some(value) => {
                let nothing = |e: &syn::Expr| {
                    is_empty_value(e)
                        || is_ident(e, "None")
                        || matches!(e, syn::Expr::Tuple(t) if t.elems.is_empty())
                        || matches!(e, syn::Expr::Lit(l) if matches!(&l.lit, syn::Lit::Bool(b) if b.value))
                };
                nothing(value) || is_ok(value, nothing)
            }
        },
        _ => false,
    }
}

/// A block that ends by leaving and does nothing before it.
fn block_leaves(block: &syn::Block) -> bool {
    matches!(block.stmts.split_last(), Some((syn::Stmt::Expr(e, _), before))
        if leaves(e) && before.iter().all(statement_does_nothing))
}

fn catches_all(pat: &syn::Pat) -> bool {
    match pat {
        syn::Pat::Wild(_) => true,
        syn::Pat::Ident(i) => match &i.subpat {
            None => i
                .ident
                .to_string()
                .starts_with(|c: char| c.is_lowercase() || c == '_'),
            Some((_, inner)) => catches_all(inner),
        },
        syn::Pat::Tuple(t) => t.elems.iter().all(catches_all),
        syn::Pat::Or(o) => o.cases.iter().any(catches_all),
        syn::Pat::Paren(p) => catches_all(&p.pat),
        syn::Pat::Reference(r) => catches_all(&r.pat),
        syn::Pat::TupleStruct(t) => {
            (t.path.is_ident("Some") || t.path.is_ident("Ok")) && t.elems.iter().all(catches_all)
        }
        _ => false,
    }
}

fn is_string(pat: &syn::Pat) -> bool {
    match pat {
        syn::Pat::Lit(l) => matches!(l.lit, syn::Lit::Str(_)),
        syn::Pat::Or(o) => o.cases.iter().all(is_string),
        syn::Pat::Paren(p) => is_string(&p.pat),
        syn::Pat::TupleStruct(t) => t.path.is_ident("Some") && t.elems.iter().all(is_string),
        _ => false,
    }
}

/// Finds a string literal, a constant or a `Json`/`Value` variant in a pattern: what records are
/// made of.
struct RecordPattern(bool);

impl<'ast> Visit<'ast> for RecordPattern {
    fn visit_lit_str(&mut self, _: &'ast syn::LitStr) {
        self.0 = true;
    }

    fn visit_pat_ident(&mut self, node: &'ast syn::PatIdent) {
        let name = node.ident.to_string();
        self.0 |= name.len() > 1 && !name.contains(|c: char| c.is_lowercase());
        visit::visit_pat_ident(self, node);
    }

    fn visit_path(&mut self, path: &'ast syn::Path) {
        if path.segments.len() > 1
            && path
                .segments
                .iter()
                .any(|s| s.ident == "Json" || s.ident == "Value")
        {
            self.0 = true;
        }
    }
}

/// Whether an unknown string ends in an arm that does something, such as failing.
fn rejects_unknown(node: &syn::ExprMatch) -> bool {
    let names_empty = |arm: &syn::Arm| arm.pat.to_token_stream().to_string().contains("\"\"");
    node.arms.iter().any(|arm| is_string(&arm.pat))
        && !node.arms.iter().any(names_empty)
        && node
            .arms
            .iter()
            .any(|arm| arm.guard.is_none() && catches_all(&arm.pat) && !does_nothing(&arm.body))
}

/// Where the defaulting calls are that an expression ends in, under `as_str`, a reference or the
/// arms of a match.
fn defaulted(expr: &syn::Expr) -> Vec<Place> {
    match expr {
        syn::Expr::MethodCall(call) if defaults(call) => vec![place(&call.method)],
        syn::Expr::MethodCall(call) if call.method == "as_str" => defaulted(&call.receiver),
        syn::Expr::Reference(r) => defaulted(&r.expr),
        syn::Expr::Paren(p) => defaulted(&p.expr),
        syn::Expr::Match(m) => m.arms.iter().flat_map(|a| defaulted(&a.body)).collect(),
        _ => Vec::new(),
    }
}

type Place = (usize, usize);

fn place(node: &impl Spanned) -> Place {
    let at = node.span().start();
    (at.line, at.column)
}

/// Finds an `if`, let-else or match that leaves when the variable `name` is absent.
struct LeavesOnAbsence<'a> {
    name: &'a syn::Ident,
    found: Vec<(usize, String)>,
}

impl LeavesOnAbsence<'_> {
    fn is_it(&self, expr: &syn::Expr) -> bool {
        match expr {
            syn::Expr::Reference(r) => self.is_it(&r.expr),
            _ => is_ident(expr, &self.name.to_string()),
        }
    }
}

impl<'ast> Visit<'ast> for LeavesOnAbsence<'_> {
    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        let cond = node.cond.to_token_stream().to_string();
        if node.else_branch.is_none()
            && block_leaves(&node.then_branch)
            && (cond == format!("{} . is_none ()", self.name)
                || cond == format!("! {} . is_some ()", self.name))
        {
            self.found
                .push((node.span().start().line, node.to_token_stream().to_string()));
        }
        visit::visit_expr_if(self, node);
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let Some(init) = &node.init
            && self.is_it(&init.expr)
            && let Some((_, diverge)) = &init.diverge
            && matches!(&**diverge, syn::Expr::Block(b) if block_leaves(&b.block))
        {
            self.found
                .push((node.span().start().line, node.to_token_stream().to_string()));
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        if self.is_it(&node.expr) {
            for arm in absence_arms(node) {
                if does_nothing(&arm.body) {
                    self.found
                        .push((arm.span().start().line, arm.to_token_stream().to_string()));
                }
            }
        }
        visit::visit_expr_match(self, node);
    }
}

/// Finds a strict match on the variable `name`.
struct StrictMatch<'a> {
    name: &'a syn::Ident,
    found: bool,
}

impl<'ast> Visit<'ast> for StrictMatch<'_> {
    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        let scrutinee = node.expr.to_token_stream().to_string();
        let name = self.name.to_string();
        if scrutinee == name
            || scrutinee == format!("{name} . as_str ()")
            || scrutinee == format!("& * {name}")
            || scrutinee.starts_with(&format!("({name} . as_str () ,"))
        {
            self.found |= rejects_unknown(node);
        }
        visit::visit_expr_match(self, node);
    }
}

/// Finds where the variable `name` is used: in the message of a panic or a print, or elsewhere.
struct Uses<'a> {
    name: &'a syn::Ident,
    messages: bool,
    others: bool,
}

impl<'ast> Visit<'ast> for Uses<'_> {
    fn visit_path(&mut self, node: &'ast syn::Path) {
        self.others |= node.is_ident(self.name);
        visit::visit_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if !node.tokens.to_string().contains(&self.name.to_string()) {
            return;
        }
        match prints(node) || node.path.is_ident("panic") || node.path.is_ident("unreachable") {
            true => self.messages = true,
            false => self.others = true,
        }
    }
}

fn on_records(node: &syn::ExprMatch) -> bool {
    let mut found = RecordPattern(false);
    for arm in &node.arms {
        found.visit_pat(&arm.pat);
    }
    found.0
}

impl<'ast> Visit<'ast> for Lint {
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.path.push(node.sig.ident.to_string());
        visit::visit_item_fn(self, node);
        self.path.pop();
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.path.push(node.sig.ident.to_string());
        visit::visit_impl_item_fn(self, node);
        self.path.pop();
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        if let Some(last) = node.segments.last() {
            let name = last.ident.to_string();
            match name.as_str() {
                "load" | "load_local" | "parse_records"
                    if node.segments.len() > 1
                        && node.segments[node.segments.len() - 2].ident == "vectors" =>
                {
                    self.loads.get_or_insert_with(|| {
                        (node.span().start().line, node.to_token_stream().to_string())
                    });
                }
                _ => self.tallies |= node.segments.iter().any(|s| s.ident == "Tally"),
            }
        }
        visit::visit_path(self, node);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.macro_tokens(node);
        visit::visit_macro(self, node);
    }

    fn visit_block(&mut self, node: &'ast syn::Block) {
        for (i, stmt) in node.stmts.iter().enumerate() {
            let syn::Stmt::Local(local) = stmt else {
                continue;
            };
            if let syn::Pat::Ident(name) = &local.pat
                && let Some(init) = &local.init
            {
                if init.diverge.is_none() && reads_record(&init.expr) {
                    let mut absence = LeavesOnAbsence {
                        name: &name.ident,
                        found: Vec::new(),
                    };
                    for rest in &node.stmts[i + 1..] {
                        absence.visit_stmt(rest);
                    }
                    for (line, text) in absence.found {
                        self.report_text(Rule::SilentReturn, text, line);
                    }
                }
                let places = defaulted(&init.expr);
                if places.is_empty() {
                    continue;
                }
                let mut strict = StrictMatch {
                    name: &name.ident,
                    found: false,
                };
                for rest in &node.stmts[i + 1..] {
                    strict.visit_stmt(rest);
                }
                let mut uses = Uses {
                    name: &name.ident,
                    messages: false,
                    others: false,
                };
                for rest in &node.stmts[i + 1..] {
                    uses.visit_stmt(rest);
                }
                if strict.found || uses.messages && !uses.others {
                    self.matched.extend(places);
                }
            }
        }
        visit::visit_block(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let at = place(&node.method);
        if defaults(node) && !self.matched.contains(&at) {
            self.report(Rule::DefaultSlot, node, at.0);
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_expr_binary(&mut self, node: &'ast syn::ExprBinary) {
        let length = |e: &syn::Expr| e.to_token_stream().to_string().ends_with(". len ()");
        if matches!(node.op, syn::BinOp::Eq(_) | syn::BinOp::Ne(_))
            && length(&node.left)
            && length(&node.right)
        {
            self.tallies = true;
        }
        visit::visit_expr_binary(self, node);
    }

    fn visit_expr_match(&mut self, node: &'ast syn::ExprMatch) {
        if rejects_unknown(node) {
            self.matched.extend(defaulted(&node.expr));
        }
        let records = on_records(node);
        for arm in &node.arms {
            if arm.guard.is_some() || !does_nothing(&arm.body) {
                continue;
            }
            let line = arm.span().start().line;
            if records && catches_all(&arm.pat) {
                self.report(Rule::CatchAll, arm, line);
            } else if is_string(&arm.pat) {
                self.report(Rule::SkippedKind, arm, line);
            } else if reads_record(&node.expr) && absence_arms(node).any(|a| std::ptr::eq(a, arm)) {
                self.report(Rule::SilentReturn, arm, line);
            }
        }
        for arm in node.arms.iter().filter(|a| is_absence(&a.pat)) {
            if branch_defaults(&node.expr, &arm.body) {
                self.report(Rule::DefaultSlot, arm, arm.span().start().line);
            }
        }
        visit::visit_expr_match(self, node);
    }

    fn visit_local(&mut self, node: &'ast syn::Local) {
        if let Some(init) = &node.init
            && let Some((_, diverge)) = &init.diverge
            && reads_record(&init.expr)
            && (leaves(diverge)
                || matches!(&**diverge, syn::Expr::Block(b) if block_leaves(&b.block)))
        {
            self.report(Rule::SilentReturn, node, node.span().start().line);
        }
        visit::visit_local(self, node);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if node.path.segments.len() > 1
            && let Some(last) = node.path.segments.last()
            && last.ident == "unwrap_or_default"
        {
            self.report(Rule::DefaultSlot, node, last.ident.span().start().line);
        }
        visit::visit_expr_path(self, node);
    }

    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        // `load` imported by itself is called without the `vectors::` before it.
        let tokens = node.tree.to_token_stream().to_string();
        let mut words = tokens.split(|c: char| !c.is_alphanumeric() && c != '_');
        if tokens.contains("vectors ::")
            && words.any(|w| ["load", "load_local", "parse_records"].contains(&w))
        {
            self.loads
                .get_or_insert_with(|| (node.span().start().line, tokens));
        }
        visit::visit_item_use(self, node);
    }

    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
        if matches!(&*node.cond, syn::Expr::Let(_))
            && let Some((_, other)) = &node.else_branch
            && branch_defaults(&node.cond, other)
        {
            self.report(Rule::DefaultSlot, node, node.span().start().line);
        }
        if node.else_branch.is_none() && reads_record(&node.cond) && block_leaves(&node.then_branch)
        {
            self.report(Rule::SilentReturn, node, node.span().start().line);
        }
        let mut branches = 0;
        let mut record_branches = 0;
        let mut link = Some(node);
        let mut open = false;
        while let Some(branch) = link {
            branches += 1;
            record_branches += usize::from(tests_record(&branch.cond));
            link = match &branch.else_branch {
                None => {
                    open = true;
                    None
                }
                Some((_, next)) => match &**next {
                    syn::Expr::If(next) => Some(next),
                    _ => None,
                },
            };
        }
        if open && branches > 1 && record_branches > 1 {
            self.report(Rule::OpenChain, node, node.span().start().line);
        }
        // The rest of the chain is part of this finding, not a chain of its own.
        self.visit_expr(&node.cond);
        self.visit_block(&node.then_branch);
        let mut rest = &node.else_branch;
        while let Some((_, branch)) = rest {
            match &**branch {
                syn::Expr::If(next) => {
                    self.visit_expr(&next.cond);
                    self.visit_block(&next.then_branch);
                    rest = &next.else_branch;
                }
                other => {
                    self.visit_expr(other);
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(text: &str) -> Vec<Rule> {
        lint(text).unwrap().into_iter().map(|f| f.rule).collect()
    }

    /// dom_tabs.rs before the C05 review (aefbd615).
    #[test]
    fn flags_the_defaulted_command_slots() {
        let found = lint(
            r#"impl Runner {
    fn capture(&mut self) -> Result<(), String> {
        for uuid in self.uuids() {
            let slot = self
                .commands
                .borrow()
                .get(&uuid)
                .cloned()
                .unwrap_or_default();
            let Some(command) = slot.borrow_mut().take() else {
                continue;
            };
            self.emitted.push(command);
        }
        Ok(())
    }
}"#,
        )
        .unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::DefaultSlot);
        assert_eq!(found[0].function, "capture");
        assert_eq!(found[0].line, 9);
    }

    /// dom_tabs.rs before the C02 review (d83536a8), cut to two arms.
    #[test]
    fn flags_the_catch_all_record_match() {
        let found = lint(
            r#"impl Runner {
    fn dispatch_event(&mut self, target: &str, etype: &str) {
        match (target, etype) {
            (t, "click") if t.contains("highlight") => {
                self.console_highlight_click();
            }
            (t, "scroll") => {
                self.scroll_event(t);
            }
            _ => {}
        }
    }
}"#,
        )
        .unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].rule, Rule::CatchAll);
        assert_eq!(found[0].function, "dispatch_event");
        assert_eq!(found[0].line, 10);
    }

    #[test]
    fn a_catch_all_must_do_something() {
        for body in [
            "Ok(())",
            "{}",
            "()",
            "continue",
            "return",
            "return Ok(())",
            "{ return None; }",
        ] {
            for pattern in ["_", "other", "(_, _) | (_, \"x\")"] {
                let text = format!(
                    "fn f() {{ loop {{ match kind {{ \"a\" => check(), {pattern} => {body}, }} }} }}"
                );
                assert_eq!(rules(&text), [Rule::CatchAll], "{text}");
            }
        }
        for arm in [
            "_ => return Err(unknown())",
            "other => panic!(\"{other}\")",
            "_ => { failures.push(1); }",
            "_ if known(kind) => {}",
        ] {
            let text = format!("fn f() {{ match kind {{ \"a\" => check(), {arm} }} }}");
            assert_eq!(rules(&text), [], "{text}");
        }
        assert_eq!(
            rules("fn f() { match json { Json::String(s) => check(s), _ => {} } }"),
            [Rule::CatchAll]
        );
        assert_eq!(
            rules("fn f() { match state { State::Open => check(), _ => {} } }"),
            []
        );
    }

    #[test]
    fn a_named_kind_that_is_skipped_is_reported() {
        assert_eq!(
            rules(
                "fn f() { match kind { \"dom\" | \"canvas\" => {} other => panic!(\"{other}\") } }"
            ),
            [Rule::SkippedKind]
        );
    }

    #[test]
    fn silent_exits_on_absent_fields() {
        for text in [
            "fn f(r: &Record) { let Some(x) = r.field(\"x\") else { return; }; }",
            "fn f() { for r in rs { let Ok(x) = r.value(\"x\") else { continue }; } }",
            "fn f() -> R { if record.field(\"x\").is_none() { return Ok(()); } Ok(()) }",
        ] {
            assert_eq!(rules(text), [Rule::SilentReturn], "{text}");
        }
        for text in [
            "fn f() -> R { let Some(x) = r.field(\"x\") else { return Err(()); }; Ok(()) }",
            "fn f() { let Some(tab) = self.tabs.tab(i) else { continue; }; }",
            "fn f() -> R { if r.field(\"x\").is_none() { return Err(()); } Ok(()) }",
        ] {
            assert_eq!(rules(text), [], "{text}");
        }
    }

    #[test]
    fn a_chain_over_fields_needs_an_else() {
        let chain = |tail: &str| {
            format!(
                "fn f() {{ if let Some(a) = step.get(\"a\") {{ x(a); }} else if let Some(b) = step.get(\"b\") {{ y(b); }} {tail} }}"
            )
        };
        assert_eq!(rules(&chain("")), [Rule::OpenChain]);
        assert_eq!(rules(&chain("else { panic!(); }")), []);
    }

    #[test]
    fn a_default_only_matched_strictly_is_allowed() {
        let replay = |arm: &str| {
            format!(
                "fn f() {{ let op = record.string(\"op\").unwrap_or_default(); match op.as_str() {{ \"a\" => x(), {arm} }} }}"
            )
        };
        assert_eq!(rules(&replay("other => panic!(\"{other}\")")), []);
        assert_eq!(
            rules(&replay("_ => {}")),
            [Rule::DefaultSlot, Rule::CatchAll]
        );
        assert_eq!(
            rules("fn f() { assert!(ok, \"{}\", record.string(\"case\").unwrap_or_default()); }"),
            []
        );
    }

    #[test]
    fn a_default_by_any_spelling_is_reported() {
        for default in [
            "unwrap_or(Default::default())",
            "unwrap_or(Slot::default())",
            "unwrap_or_else(Default::default)",
            "unwrap_or_else(|| Rc::new(RefCell::new(None)))",
            "unwrap_or(String::new())",
            "unwrap_or_else(String::new)",
            "unwrap_or(\"\")",
            "unwrap_or_else(|| \"\".to_owned())",
            "unwrap_or(&[])",
            "unwrap_or(vec![])",
            "map(Option::unwrap_or_default)",
        ] {
            let text = format!("fn f() {{ let slot = self.commands.get(&uuid).{default}; }}");
            assert_eq!(rules(&text), [Rule::DefaultSlot], "{text}");
        }
        for text in [
            "fn f() { let s = match slots.get(&id) { Some(s) => s, None => Default::default() }; }",
            "fn f() { let s = match r.string(\"x\") { Some(s) => s, None => String::new() }; }",
            "fn f() { let s = if let Some(s) = r.string(\"x\") { s } else { String::new() }; }",
        ] {
            assert_eq!(rules(text), [Rule::DefaultSlot], "{text}");
        }
        for text in [
            "fn f() { let x = args.get(i).cloned().unwrap_or(Value::Undefined); }",
            "fn f() { let x = init.get(\"x\").and_then(Json::as_f64).unwrap_or(0.0); }",
            "fn f() { let x = match self.show { None => \"\", Some(_) => \"1\" }; }",
            "fn f() { *counts.entry(kind).or_default() += 1; }",
            "fn f() { let log = read(path).unwrap_or_default(); panic!(\"failed: {log}\"); }",
        ] {
            assert_eq!(rules(text), [], "{text}");
        }
        assert_eq!(
            rules(
                "fn f() { let x = r.string(\"x\").unwrap_or_default(); match x.as_str() { \"\" => y(), _ => panic!() } }"
            ),
            [Rule::DefaultSlot]
        );
    }

    #[test]
    fn a_catch_all_is_found_under_other_patterns_and_bodies() {
        for arms in [
            "Some(\"a\") => check(), Some(_) => {} None => panic!()",
            "Some(\"a\") => check(), Some(_) | None => {}",
            "\"a\" => check(), &_ => {}",
            "CLICK => check(), SCROLL => check(), _ => {}",
            "\"a\" => check(), other => { let _ = other; }",
            "\"a\" => check(), other => eprintln!(\"{other}\")",
            "\"a\" => check(), _ => { skipped += 1; continue; }",
            "\"a\" => check(), _ => Ok::<(), String>(())",
            "\"a\" => check(), _ => return Ok(Vec::new())",
            "\"a\" => check(), _ => return true",
        ] {
            let text = format!("fn f() {{ loop {{ match kind {{ {arms} }} }} }}");
            assert_eq!(rules(&text), [Rule::CatchAll], "{text}");
        }
        for arms in [
            "Some(\"a\") => check(), Some(other) => panic!(\"{other}\"), None => panic!()",
            "\"a\" => check(), _ => return Err(unknown())",
            "\"a\" => check(), _ => return false",
        ] {
            let text = format!("fn f() {{ match kind {{ {arms} }} }}");
            assert_eq!(rules(&text), [], "{text}");
        }
        assert_eq!(
            rules("fn f() { if kind == \"click\" { a() } else if kind == \"scroll\" { b() } }"),
            [Rule::OpenChain]
        );
    }

    #[test]
    fn silent_exits_are_found_through_matches_and_variables() {
        for text in [
            "fn f() -> R { match r.field(\"x\") { None => return Ok(()), Some(x) => check(x) } }",
            "fn f() -> R { match r.field(\"x\") { None => Ok(()), Some(x) => check(x) } }",
            "fn f() -> R { match r.field(\"x\") { Some(x) => check(x), _ => Ok(()) } }",
            "fn f() { let x = r.field(\"x\"); if x.is_none() { return; } check(x); }",
            "fn f() { let x = r.field(\"x\"); let Some(x) = x else { return; }; }",
            "fn f() -> R { let Some(x) = r.field(\"x\") else { return Ok(Vec::new()); }; }",
            "fn f() { let Some(x) = r.field(\"x\") else { skipped += 1; return; }; }",
        ] {
            assert_eq!(rules(text), [Rule::SilentReturn], "{text}");
        }
        for text in [
            "fn f() { let id = r.value(\"sl\").unwrap(); match id { 0 => a(), _ => {} } }",
            "fn f() -> R { match r.field(\"x\") { None => Err(()), Some(x) => check(x) } }",
            "fn f() { let x = r.field(\"x\"); if x.is_none() { panic!(); } }",
        ] {
            assert_eq!(rules(text), [], "{text}");
        }
    }

    #[test]
    fn macro_closures_are_linted() {
        assert_eq!(
            rules(
                "record_vectors!(t, \"id\", |r: &Record| { if r.field(\"x\").is_none() { return Ok(()); } check(r) });"
            ),
            [Rule::SilentReturn]
        );
    }

    #[test]
    fn loading_vectors_needs_a_tally() {
        let load = "fn f() { for r in vectors::load(ID).unwrap() { check(&r); } ";
        assert_eq!(rules(&format!("{load} }}")), [Rule::NoTally]);
        for counted in [
            "assert_eq!(compared, records.len());",
            "assert_eq!(n, recorded);",
            "let t = Tally::by_first_key(ID, &records);",
            "if emitted.len() != records.len() { panic!(); }",
        ] {
            assert_eq!(rules(&format!("{load} {counted} }}")), [], "{counted}");
        }
        let message = "assert!(failures.is_empty(), \"{} differ\", failures.len());";
        assert_eq!(rules(&format!("{load} {message} }}")), [Rule::NoTally]);
        assert_eq!(
            rules("use asrs_parity::vectors::{Record, load}; fn f() { for r in load(ID) {} }"),
            [Rule::NoTally]
        );
    }

    #[test]
    fn the_allow_list_holds_counts_with_reasons() {
        let allow = parse_allow(
            "[[allow]]\nfile = \"a.rs\"\nfunction = \"f\"\nrule = \"catch-all\"\nreason = \"the first match fails on it\"\n",
        )
        .unwrap();
        let finding = |line| Finding {
            rule: Rule::CatchAll,
            function: "f".to_owned(),
            line,
            text: "_ => {}".to_owned(),
        };
        let one = BTreeMap::from([("a.rs".to_owned(), vec![finding(3)])]);
        assert_eq!(judge(&one, &allow), (Vec::new(), Vec::new()));
        let two = BTreeMap::from([("a.rs".to_owned(), vec![finding(3), finding(9)])]);
        assert_eq!(judge(&two, &allow).0.len(), 2);
        let none = BTreeMap::new();
        assert_eq!(judge(&none, &allow).1.len(), 1);
        assert_eq!(judge(&two, &[]).0.len(), 2);
        for bad in [
            "[[allow]]\nfile = \"a.rs\"\nfunction = \"f\"\nrule = \"catch-all\"\nreason = \"ok\"\n",
            "[[allow]]\nfile = \"a.rs\"\nfunction = \"f\"\nrule = \"nope\"\nreason = \"a long enough reason\"\n",
            "[[allow]]\nfile = \"a.rs\"\nfunction = \"f\"\nrule = \"catch-all\"\ncount = 0\nreason = \"a long enough reason\"\n",
        ] {
            assert!(parse_allow(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn every_harness_passes() {
        run(&[]).unwrap();
    }
}
