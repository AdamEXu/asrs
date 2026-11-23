use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use quote::ToTokens;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

use crate::{Result, workspace_root};

const ALLOW: &str = "xtask/harness-allow.toml";
const USAGE: &str = "usage: cargo xtask harness [--list] [--root <checkout>]

Strictness lint over replay harnesses (crates/*/tests and crates/asrs-parity/src/live), part of
`cargo xtask check`. Rules:
  default-slot   unwrap_or_default: an absent recorded value or port result becomes one that can
                 compare equal. Name the absence: ok_or_else, expect, or an Option comparison.
                 A string only matched on afterwards is fine if the catch-all arm fails.
  catch-all      a match on recorded strings or Json values whose catch-all arm does nothing, so
                 an unknown record kind, field or operation passes. Return an error or panic.
  skipped-kind   a match arm naming a recorded kind and doing nothing, so its records are skipped.
                 Compare it, or assert where another harness does.
  silent-return  an early return, continue or break out of a replay because a recorded field is
                 absent (let-else, or an if whose whole body leaves).
  open-chain     an if/else-if chain over recorded fields with no final else, so a record that
                 matches none passes.
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
        let found = lint(&text).map_err(|e| format!("{file}: {e}"))?;
        if !found.is_empty() {
            findings.insert(file, found);
        }
    }
    if list {
        print!("{}", listing(&findings));
        return Ok(());
    }
    let (problems, stale) = judge(&findings, &allow);
    for note in &stale {
        eprintln!("harness lint: {note}");
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

/// The findings beyond the allowed counts, and the entries that allow more than there is.
fn judge(findings: &BTreeMap<String, Vec<Finding>>, allow: &[Allow]) -> (Vec<String>, Vec<String>) {
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
                format!(
                    "{ALLOW} allows {} {} in {} {}, and there are {n}",
                    a.count,
                    a.rule.name(),
                    a.file,
                    a.function
                )
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
    /// Lines of `unwrap_or_default` calls whose value a strict match rejects.
    matched: Vec<usize>,
}

impl Lint {
    fn report(&mut self, rule: Rule, at: &impl ToTokens, line: usize) {
        let mut text = at.to_token_stream().to_string();
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
            self.tallies |= counts_records(&mac.tokens.to_string());
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

/// An expression that leaves a replay as if nothing were wrong.
fn does_nothing(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Tuple(t) => t.elems.is_empty(),
        syn::Expr::Block(b) => block_does_nothing(&b.block),
        syn::Expr::Call(_) => expr.to_token_stream().to_string() == "Ok (())",
        _ => leaves(expr),
    }
}

fn block_does_nothing(block: &syn::Block) -> bool {
    match block.stmts.as_slice() {
        [] => true,
        [syn::Stmt::Expr(e, _)] => does_nothing(e),
        _ => false,
    }
}

fn leaves(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::Continue(_) => true,
        syn::Expr::Break(b) => b.expr.is_none(),
        syn::Expr::Return(r) => match &r.expr {
            None => true,
            Some(value) => matches!(
                value.to_token_stream().to_string().as_str(),
                "Ok (())" | "Ok (None)" | "None" | "()"
            ),
        },
        _ => false,
    }
}

fn block_leaves(block: &syn::Block) -> bool {
    matches!(block.stmts.as_slice(), [syn::Stmt::Expr(e, _)] if leaves(e))
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

/// Finds a string literal or a `Json`/`Value` variant in a pattern: what records are made of.
struct RecordPattern(bool);

impl<'ast> Visit<'ast> for RecordPattern {
    fn visit_lit_str(&mut self, _: &'ast syn::LitStr) {
        self.0 = true;
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
    node.arms.iter().any(|arm| is_string(&arm.pat))
        && node
            .arms
            .iter()
            .any(|arm| arm.guard.is_none() && catches_all(&arm.pat) && !does_nothing(&arm.body))
}

/// The lines of the `unwrap_or_default` calls an expression ends in, under `as_str`, a reference
/// or the arms of a match.
fn defaulted(expr: &syn::Expr) -> Vec<usize> {
    match expr {
        syn::Expr::MethodCall(call) if call.method == "unwrap_or_default" => {
            vec![call.method.span().start().line]
        }
        syn::Expr::MethodCall(call) if call.method == "as_str" => defaulted(&call.receiver),
        syn::Expr::Reference(r) => defaulted(&r.expr),
        syn::Expr::Paren(p) => defaulted(&p.expr),
        syn::Expr::Match(m) => m.arms.iter().flat_map(|a| defaulted(&a.body)).collect(),
        _ => Vec::new(),
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
                let lines = defaulted(&init.expr);
                if lines.is_empty() {
                    continue;
                }
                let mut strict = StrictMatch {
                    name: &name.ident,
                    found: false,
                };
                for rest in &node.stmts[i + 1..] {
                    strict.visit_stmt(rest);
                }
                if strict.found {
                    self.matched.extend(lines);
                }
            }
        }
        visit::visit_block(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let line = node.method.span().start().line;
        if node.method == "unwrap_or_default" && !self.matched.contains(&line) {
            self.report(Rule::DefaultSlot, node, line);
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

    fn visit_expr_if(&mut self, node: &'ast syn::ExprIf) {
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
            record_branches += usize::from(reads_record(&branch.cond));
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
