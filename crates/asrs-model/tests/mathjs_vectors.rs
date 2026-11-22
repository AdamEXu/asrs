//! Replays `mathjs/*` vectors (oracle/src/genMathjs.ts: mathjs 11.3.0 run by the oracle) against
//! `asrs_model::mathjs`, one layer per vector: the parser's trees and syntax errors, `evaluate`
//! over the evaluation subset, each subset function called directly, `format`, and the
//! typed-function signature tables the argument errors come from.

use std::collections::BTreeMap;

use asrs_js::string::JsString;
use asrs_model::mathjs::{self, Constant, MathError, Node, NodeKind, Value};
use asrs_parity::cve1::Value as Cve;
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};

#[derive(Default)]
struct Tally {
    recorded: BTreeMap<String, usize>,
    compared: BTreeMap<String, usize>,
}

impl Tally {
    fn record(&mut self, kind: &str) {
        *self.recorded.entry(kind.to_owned()).or_default() += 1;
    }

    fn compared(&mut self, kind: &str) {
        *self.compared.entry(kind.to_owned()).or_default() += 1;
    }

    fn finish(&self, id: &str) {
        assert!(!self.recorded.is_empty(), "{id}: no records");
        assert_eq!(
            self.recorded, self.compared,
            "{id}: compared counts differ from recorded counts"
        );
    }
}

/// Runs `check` on every record of `id`, which must all be of `kind` with exactly one of the
/// key sets in `shapes`.
fn replay(
    id: &str,
    kind: &str,
    shapes: &[&[&str]],
    mut check: impl FnMut(&Record) -> Result<(), String>,
) {
    let mut tally = Tally::default();
    vectors::check_records(id, |record| {
        let Json::Object(pairs) = &record.json else {
            return Err("record is not an object".to_owned());
        };
        let recorded_kind = record.string("kind").ok_or("record has no kind")?;
        tally.record(&recorded_kind);
        if recorded_kind != kind {
            return Err(format!("unknown record kind {recorded_kind:?}"));
        }
        let keys: Vec<String> = pairs.iter().map(|(k, _)| k.to_string_lossy()).collect();
        if !shapes
            .iter()
            .any(|shape| shape.len() == keys.len() && shape.iter().zip(&keys).all(|(a, b)| a == b))
        {
            return Err(format!("unexpected record fields {keys:?}"));
        }
        check(record)?;
        tally.compared(kind);
        Ok(())
    });
    tally.finish(id);
}

fn js(s: &Utf16String) -> JsString {
    JsString::from_units(s.units().to_vec())
}

fn utf16(s: &JsString) -> Utf16String {
    Utf16String::from_units(s.units().to_vec())
}

fn jstr(s: &JsString) -> Json {
    Json::String(utf16(s))
}

fn field<'a>(record: &'a Record, key: &str) -> &'a Json {
    record
        .field(key)
        .unwrap_or_else(|| panic!("line {}: no field {key:?}", record.line))
}

fn cve(json: &Json) -> Result<Cve, String> {
    Cve::from_json(json).map_err(|e| format!("bad CVE-1 value: {e}"))
}

fn bits_json(x: f64) -> Json {
    Json::object([("n", Json::string(&format!("{:016x}", x.to_bits())))])
}

/// Equal JSON, except that numbers in CVE-1 `{"n": bits}` leaves go through the platform
/// allowances of `asrs_parity::vectors` (x86_64 NaN bits, one ulp of a platform `pow` off macOS).
fn json_match(expected: &Json, actual: &Json) -> bool {
    match (expected, actual) {
        (Json::Object(e), Json::Object(a))
            if e.len() == 1 && a.len() == 1 && e[0].0.eq_str("n") && a[0].0.eq_str("n") =>
        {
            match (&e[0].1, &a[0].1) {
                (Json::String(eb), Json::String(ab)) => {
                    let parse =
                        |s: &Utf16String| u64::from_str_radix(&s.to_string_lossy(), 16).ok();
                    match (parse(eb), parse(ab)) {
                        (Some(x), Some(y)) => {
                            vectors::pow_bits_match(x, y)
                                || vectors::encoded_match(
                                    &Cve::Number(x).encode(),
                                    &Cve::Number(y).encode(),
                                )
                        }
                        _ => expected == actual,
                    }
                }
                _ => expected == actual,
            }
        }
        (Json::Object(e), Json::Object(a)) => {
            e.len() == a.len()
                && e.iter()
                    .zip(a)
                    .all(|((ek, ev), (ak, av))| ek == ak && json_match(ev, av))
        }
        (Json::Array(e), Json::Array(a)) => {
            e.len() == a.len() && e.iter().zip(a).all(|(x, y)| json_match(x, y))
        }
        _ => expected == actual,
    }
}

fn expect(expected: &Json, actual: &Json) -> Result<(), String> {
    if json_match(expected, actual) {
        Ok(())
    } else {
        Err(format!(
            "expected {}\n     got {}",
            expected.to_compact(),
            actual.to_compact()
        ))
    }
}

fn constant_json(value: &Constant) -> Json {
    match value {
        Constant::Number(x) => bits_json(*x),
        Constant::String(s) => jstr(s),
        Constant::Boolean(b) => Json::Bool(*b),
        Constant::Null => Json::Null,
        Constant::Undefined => Json::object([("u", Json::Null)]),
    }
}

fn nodes(list: &[Node]) -> Json {
    Json::Array(list.iter().map(dump).collect())
}

/// The same dump genMathjs.ts writes for a mathjs node.
fn dump(node: &Node) -> Json {
    let mut pairs: Vec<(&str, Json)> = vec![("type", Json::string(node.type_name()))];
    if let Some(comment) = &node.comment {
        pairs.push(("comment", jstr(comment)));
    }
    match &node.kind {
        NodeKind::Constant(value) => pairs.push(("value", constant_json(value))),
        NodeKind::Symbol { name } => pairs.push(("name", jstr(name))),
        NodeKind::Operator {
            op,
            fn_name,
            implicit,
            is_percentage,
            args,
        } => {
            pairs.push(("op", Json::string(op)));
            pairs.push(("fn", Json::string(fn_name)));
            pairs.push(("implicit", Json::Bool(*implicit)));
            pairs.push(("isPercentage", Json::Bool(*is_percentage)));
            pairs.push(("args", nodes(args)));
        }
        NodeKind::Parenthesis { content } => pairs.push(("content", dump(content))),
        NodeKind::Function { callee, args } => {
            pairs.push(("fn", dump(callee)));
            pairs.push(("args", nodes(args)));
        }
        NodeKind::Accessor { object, index } => {
            pairs.push(("object", dump(object)));
            pairs.push(("index", dump(index)));
        }
        NodeKind::Index {
            dimensions,
            dot_notation,
        } => {
            pairs.push(("dimensions", nodes(dimensions)));
            pairs.push(("dotNotation", Json::Bool(*dot_notation)));
        }
        NodeKind::Array { items } => pairs.push(("items", nodes(items))),
        NodeKind::Object { properties } => pairs.push((
            "properties",
            Json::Array(
                properties
                    .iter()
                    .map(|(k, v)| Json::Array(vec![jstr(k), dump(v)]))
                    .collect(),
            ),
        )),
        NodeKind::Assignment {
            object,
            index,
            value,
        } => {
            pairs.push(("object", dump(object)));
            pairs.push(("index", index.as_deref().map_or(Json::Null, dump)));
            pairs.push(("value", dump(value)));
        }
        NodeKind::FunctionAssignment {
            name,
            params,
            types,
            expr,
        } => {
            pairs.push(("name", jstr(name)));
            pairs.push(("params", Json::Array(params.iter().map(jstr).collect())));
            pairs.push(("types", Json::Array(types.iter().map(jstr).collect())));
            pairs.push(("expr", dump(expr)));
        }
        NodeKind::Conditional {
            condition,
            true_expr,
            false_expr,
        } => {
            pairs.push(("condition", dump(condition)));
            pairs.push(("trueExpr", dump(true_expr)));
            pairs.push(("falseExpr", dump(false_expr)));
        }
        NodeKind::Range { start, end, step } => {
            pairs.push(("start", dump(start)));
            pairs.push(("end", dump(end)));
            pairs.push(("step", step.as_deref().map_or(Json::Null, dump)));
        }
        NodeKind::Relational {
            conditionals,
            params,
        } => {
            pairs.push((
                "conditionals",
                Json::Array(conditionals.iter().map(|c| Json::string(c)).collect()),
            ));
            pairs.push(("params", nodes(params)));
        }
        NodeKind::Block { blocks } => pairs.push((
            "blocks",
            Json::Array(
                blocks
                    .iter()
                    .map(|(node, visible)| {
                        Json::object([("node", dump(node)), ("visible", Json::Bool(*visible))])
                    })
                    .collect(),
            ),
        )),
    }
    Json::object(pairs)
}

fn error_json(e: &MathError) -> Json {
    Json::object([
        ("name", Json::string(e.name)),
        ("message", jstr(&e.message)),
    ])
}

fn value_json(value: &Value) -> Json {
    match value {
        Value::Number(x) => {
            Json::object([("type", Json::string("number")), ("value", bits_json(*x))])
        }
        Value::Boolean(b) => {
            Json::object([("type", Json::string("boolean")), ("value", Json::Bool(*b))])
        }
        Value::String(s) => Json::object([("type", Json::string("string")), ("value", jstr(s))]),
        Value::Null => Json::object([("type", Json::string("null"))]),
        Value::Undefined => Json::object([("type", Json::string("undefined"))]),
        Value::Function(f) => {
            Json::object([("type", Json::string("function")), ("name", jstr(&f.name))])
        }
        Value::ResultSet(entries) => Json::object([
            ("type", Json::string("ResultSet")),
            (
                "entries",
                Json::Array(entries.iter().map(value_json).collect()),
            ),
        ]),
        other => Json::object([
            ("type", Json::string("other")),
            (
                "typeOf",
                Json::string(
                    other
                        .other_type_of()
                        .expect("other_type_of names every other value"),
                ),
            ),
        ]),
    }
}

/// `{"ok": result}` or `{"throw": error}`, as genMathjs's `outcome`.
fn outcome_json(result: &Result<Value, MathError>) -> Json {
    match result {
        Ok(value) => Json::object([("ok", value_json(value))]),
        Err(e) => Json::object([("throw", error_json(e))]),
    }
}

fn recorded_outcome(record: &Record) -> Json {
    match (record.field("ok"), record.field("throw")) {
        (Some(ok), None) => Json::object([("ok", ok.clone())]),
        (None, Some(throw)) => Json::object([("throw", throw.clone())]),
        _ => panic!("line {}: needs exactly one of ok and throw", record.line),
    }
}

fn expr_of(record: &Record) -> Result<JsString, String> {
    match cve(field(record, "expr"))? {
        Cve::String(s) => Ok(js(&s)),
        other => Err(format!("expr is {}", other.kind())),
    }
}

fn arg_value(arg: &Cve) -> Result<Value, String> {
    Ok(match arg {
        Cve::Number(bits) => Value::Number(f64::from_bits(*bits)),
        Cve::Bool(b) => Value::Boolean(*b),
        Cve::String(s) => Value::String(js(s)),
        Cve::Null => Value::Null,
        Cve::Undefined => Value::Undefined,
        other => return Err(format!("argument {}", other.kind())),
    })
}

#[test]
fn parse_trees_and_syntax_errors() {
    replay(
        "mathjs/parse",
        "parse",
        &[&["kind", "expr", "ast"], &["kind", "expr", "throw"]],
        |record| {
            let expr = expr_of(record)?;
            let actual = match mathjs::parse(&expr) {
                Ok(node) => Json::object([("ast", dump(&node))]),
                Err(e) => Json::object([("throw", error_json(&e))]),
            };
            let expected = match (record.field("ast"), record.field("throw")) {
                (Some(ast), None) => Json::object([("ast", ast.clone())]),
                (None, Some(throw)) => Json::object([("throw", throw.clone())]),
                _ => return Err("needs exactly one of ast and throw".to_owned()),
            };
            expect(&expected, &actual)
                .map_err(|e| format!("parse {:?}\n  {e}", expr.to_string_lossy()))
        },
    );
}

#[test]
fn evaluate_results_and_errors() {
    replay(
        "mathjs/evaluate",
        "evaluate",
        &[&["kind", "expr", "ok"], &["kind", "expr", "throw"]],
        |record| {
            let expr = expr_of(record)?;
            expect(
                &recorded_outcome(record),
                &outcome_json(&mathjs::evaluate(&expr)),
            )
            .map_err(|e| format!("evaluate {:?}\n  {e}", expr.to_string_lossy()))
        },
    );
}

#[test]
fn subset_functions_called_directly() {
    replay(
        "mathjs/call",
        "call",
        &[
            &["kind", "fn", "args", "ok"],
            &["kind", "fn", "args", "throw"],
        ],
        |record| {
            let name = record.string("fn").ok_or("fn is not a string")?;
            let Cve::Array(args) = cve(field(record, "args"))? else {
                return Err("args is not an array".to_owned());
            };
            let args: Vec<Value> = args.iter().map(arg_value).collect::<Result<_, _>>()?;
            expect(
                &recorded_outcome(record),
                &outcome_json(&mathjs::call(&name, &args)),
            )
            .map_err(|e| format!("{name}{}\n  {e}", field(record, "args").to_compact()))
        },
    );
}

#[test]
fn format_with_truncate() {
    replay(
        "mathjs/format",
        "format",
        &[&["kind", "value", "text"]],
        |record| {
            let value = arg_value(&cve(field(record, "value"))?)?;
            let expected = field(record, "text").clone();
            expect(&expected, &jstr(&mathjs::format(&value)))
                .map_err(|e| format!("format {}\n  {e}", field(record, "value").to_compact()))
        },
    );
}

#[test]
fn typed_function_signature_tables() {
    replay(
        "mathjs/signatures",
        "signatures",
        &[&["kind", "fn", "name", "signatures"]],
        |record| {
            let name = record.string("fn").ok_or("fn is not a string")?;
            let (js_name, signatures) =
                mathjs::signatures(&name).ok_or_else(|| format!("no signatures for {name}"))?;
            let table = Json::Array(
                signatures
                    .iter()
                    .map(|s| {
                        Json::object([(
                            "params",
                            Json::Array(
                                s.params
                                    .iter()
                                    .map(|p| {
                                        Json::object([
                                            (
                                                "types",
                                                Json::Array(
                                                    p.types
                                                        .iter()
                                                        .map(|t| Json::string(t.name))
                                                        .collect(),
                                                ),
                                            ),
                                            (
                                                "conversions",
                                                Json::Array(
                                                    p.types
                                                        .iter()
                                                        .map(|t| match t.conversion {
                                                            Some((from, to)) => Json::Array(vec![
                                                                Json::string(from),
                                                                Json::string(to),
                                                            ]),
                                                            None => Json::Null,
                                                        })
                                                        .collect(),
                                                ),
                                            ),
                                            ("rest", Json::Bool(p.rest)),
                                        ])
                                    })
                                    .collect(),
                            ),
                        )])
                    })
                    .collect(),
            );
            expect(field(record, "name"), &Json::string(js_name))?;
            expect(field(record, "signatures"), &table).map_err(|e| format!("{name}: {e}"))
        },
    );
}

#[test]
fn every_subset_function_has_call_and_signature_vectors() {
    let names = |id: &str| -> Vec<String> {
        let mut names: Vec<String> = vectors::load(id)
            .unwrap_or_else(|e| panic!("{e}"))
            .iter()
            .map(|r| r.string("fn").expect("fn"))
            .collect();
        names.dedup();
        names
    };
    let calls = names("mathjs/call");
    let signatures = names("mathjs/signatures");
    for name in mathjs::OPERATOR_FUNCTIONS.iter().chain(mathjs::FUNCTIONS) {
        assert!(
            calls.iter().any(|n| n == name),
            "no call vectors for {name}"
        );
        assert!(
            signatures.iter().any(|n| n == name),
            "no signature vector for {name}"
        );
    }
    for name in mathjs::HELPER_FUNCTIONS {
        assert!(
            signatures.iter().any(|n| n == name),
            "no signature vector for {name}"
        );
    }
}
