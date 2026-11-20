//! `fn` names for `asrs_js::regex`. The regex vector tests include this file too, so the
//! committed vectors and `cargo xtask parity fn` exercise the same dispatch.

use asrs_js::regex::{Match, RegExp, sites};
use asrs_js::string::JsString;
use asrs_parity::{Response, Utf16String, Value};

pub type Function = fn(&[Value]) -> Response;

pub const FUNCTIONS: &[(&str, Function)] = &[
    ("shared/log/LogKeyUtils:isSchema", |a| {
        with_str(a, 0, |s| Value::Bool(sites::schema_type(&s).is_some()))
    }),
    ("shared/log/LogKeyUtils:getSchemaType", |a| {
        with_str(a, 0, |s| opt_string(sites::schema_type(&s)))
    }),
    ("shared/log/LogKeyUtils:isMergePrefix", |a| {
        with_str(a, 0, |s| Value::Bool(sites::merge_prefix(&s).is_some()))
    }),
    ("shared/log/LogKeyUtils:extractMergePrefix", |a| {
        with_str(a, 0, |s| {
            string(sites::merge_prefix(&s).unwrap_or_default())
        })
    }),
];

const EXEC: &str = r#"(p, f, s) => { const r = new RegExp(p, f); const out = []; for (let i = 0; i < 40; i++) { const m = r.exec(s); if (m === null) { out.push(r.lastIndex); break; } out.push([m.index, r.lastIndex, ...m]); if (!r.global && !r.sticky) break; if (m[0].length === 0) r.lastIndex++; } return out; }"#;
const SPLIT: &str = r#"(p, f, s, n) => s.split(new RegExp(p, f), n)"#;
const REPLACE: &str = r#"(p, f, s, t) => s.replace(new RegExp(p, f), t)"#;
const REPLACE_FN: &str = r#"(p, f, s) => s.replace(new RegExp(p, f), (...a) => "<" + a.slice(0, -2).map(String).join(",") + "@" + a[a.length - 2] + ">")"#;
const MATCH: &str = r#"(p, f, s) => { const m = s.match(new RegExp(p, f)); return m === null || m.index === undefined ? m : [m.index, ...m]; }"#;

/// `oracle:call` sources: the upstream regex expressions verbatim, then generic engine probes.
pub const CALLS: &[(&str, Function)] = &[
    (r#"(s) => s.replace(/^0+/, "")"#, |a| {
        with_str(a, 0, |s| string(sites::strip_leading_zeros(&s)))
    }),
    (r#"(s) => s.match(/.{1,2}/g)"#, |a| {
        with_str(a, 0, |s| match sites::pairs_of_units(&s) {
            Some(parts) => strings(parts),
            None => Value::Null,
        })
    }),
    (
        r#"(s) => s.replace(/\\(?:033|x1b|x1B|e)(?=\[[0-9;:]*[a-zA-Z])/g, "\x1b")"#,
        |a| with_str(a, 0, |s| string(sites::console_escapes(&s))),
    ),
    (r#"(s) => s.replace(/\\r\\n|\\n/g, "\n")"#, |a| {
        with_str(a, 0, |s| string(sites::console_newlines(&s)))
    }),
    (r#"(s) => s.split(/(<[^>]*>)/g)"#, |a| {
        with_str(a, 0, |s| strings(sites::split_tags(&s)))
    }),
    (r#"(s) => s.split(/\/|:/)"#, |a| {
        with_str(a, 0, |s| strings(sites::split_key(&s, false)))
    }),
    (r#"(s) => s.split(/\/|:|_/)"#, |a| {
        with_str(a, 0, |s| strings(sites::split_key(&s, true)))
    }),
    (
        r#"(s) => /^([0123456789abcdef]{2}-)+[0123456789abcdef]{2}(-\.\.\.)?$/.test(s)"#,
        |a| with_str(a, 0, |s| Value::Bool(sites::is_csv_bytes(&s))),
    ),
    (r#"(s) => /^[0123456789abcdef]{2}$/.test(s)"#, |a| {
        with_str(a, 0, |s| Value::Bool(sites::is_csv_byte(&s)))
    }),
    (
        r#"(s) => { const m = s.match(/(?:youtube\.com\/(?:[^\/]+\/.+\/|(?:v|e(?:mbed)?)\/|.*[?&]v=)|youtu\.be\/)([^"&?\/\s]{11})/); return m ? m[1] : null; }"#,
        |a| with_str(a, 0, |s| opt_string(sites::youtube_video_id(&s))),
    ),
    (
        r#"(s) => { const m = /, [0-9]+x[0-9]+/.exec(s); return m === null ? null : m.index; }"#,
        |a| with_str(a, 0, |s| opt_index(sites::dimensions_index(&s))),
    ),
    (
        r#"(s) => { const m = /[^0-9]/.exec(s); return m === null ? null : m.index; }"#,
        |a| with_str(a, 0, |s| opt_index(sites::first_non_digit(&s))),
    ),
    (r#"(s) => /^\d/.test(s)"#, |a| {
        with_str(a, 0, |s| Value::Bool(sites::starts_with_digit(&s)))
    }),
    (
        r#"(s) => s.replace(/(^\w|\s\w|\/\w)/g, (m) => m.toUpperCase())"#,
        |a| with_str(a, 0, |s| string(sites::capitalize_words(&s))),
    ),
    (EXEC, exec_loop),
    (SPLIT, split),
    (REPLACE, replace),
    (REPLACE_FN, replace_fn),
    (MATCH, match_fn),
];

fn str_arg(args: &[Value], i: usize) -> Result<JsString, Response> {
    match args.get(i) {
        Some(Value::String(s)) => Ok(JsString::from_units(s.units().to_vec())),
        other => Err(Response::Error(format!(
            "argument {i} must be a string, got {other:?}"
        ))),
    }
}

fn with_str(args: &[Value], i: usize, f: impl FnOnce(JsString) -> Value) -> Response {
    match str_arg(args, i) {
        Ok(s) => Response::Ok(f(s)),
        Err(r) => r,
    }
}

fn string(s: JsString) -> Value {
    Value::String(Utf16String::from_units(s.into_units()))
}

fn opt_string(s: Option<JsString>) -> Value {
    s.map_or(Value::Null, string)
}

fn strings(items: Vec<JsString>) -> Value {
    Value::Array(items.into_iter().map(string).collect())
}

fn index(i: usize) -> Value {
    Value::number(i as f64)
}

fn opt_index(i: Option<usize>) -> Value {
    i.map_or(Value::Null, index)
}

fn group(input: &[u16], m: &Match, i: usize) -> Value {
    m.group(input, i).map_or(Value::Undefined, string)
}

fn groups(input: &[u16], m: &Match) -> impl Iterator<Item = Value> {
    (0..=m.captures()).map(move |i| group(input, m, i))
}

/// The pattern, flags and subject of an engine probe.
fn probe(args: &[Value]) -> Result<(RegExp, JsString), Response> {
    let pattern = str_arg(args, 0)?
        .as_string()
        .ok_or_else(|| Response::Error("pattern has lone surrogates".to_owned()))?;
    let flags = str_arg(args, 1)?.to_string_lossy();
    let re = RegExp::new(&pattern, &flags).map_err(|e| Response::Error(e.to_string()))?;
    Ok((re, str_arg(args, 2)?))
}

fn run(args: &[Value], f: impl FnOnce(RegExp, &[u16]) -> Result<Value, Response>) -> Response {
    match probe(args).and_then(|(re, s)| f(re, s.units())) {
        Ok(v) => Response::Ok(v),
        Err(r) => r,
    }
}

fn exec_loop(args: &[Value]) -> Response {
    run(args, |re, s| {
        let mut out = Vec::new();
        let mut last_index = 0;
        for _ in 0..40 {
            let Some(m) = re.exec(s, &mut last_index) else {
                out.push(index(last_index));
                break;
            };
            let mut row = vec![index(m.index()), index(last_index)];
            row.extend(groups(s, &m));
            out.push(Value::Array(row));
            if !re.global() && !re.sticky() {
                break;
            }
            if m.index() == m.end() {
                last_index += 1;
            }
        }
        Ok(Value::Array(out))
    })
}

/// `ToUint32` of a split limit; `None` for `undefined`.
fn limit(value: Option<&Value>) -> Result<Option<u32>, Response> {
    match value {
        None | Some(Value::Undefined) => Ok(None),
        Some(v) => {
            let n = v
                .as_f64()
                .ok_or_else(|| Response::Error(format!("limit must be a number, got {v:?}")))?;
            if !n.is_finite() {
                return Ok(Some(0));
            }
            Ok(Some(n.trunc().rem_euclid(4294967296.0) as u32))
        }
    }
}

fn split(args: &[Value]) -> Response {
    run(args, |re, s| {
        let parts = re.split(s, limit(args.get(3))?);
        Ok(Value::Array(
            parts
                .into_iter()
                .map(|p| p.map_or(Value::Undefined, string))
                .collect(),
        ))
    })
}

fn replace(args: &[Value]) -> Response {
    run(args, |re, s| {
        let template = str_arg(args, 3)?;
        Ok(string(re.replace(s, template.units())))
    })
}

fn replace_fn(args: &[Value]) -> Response {
    run(args, |re, s| {
        Ok(string(re.replace_with(s, |m| {
            let mut units: Vec<u16> = vec![u16::from(b'<')];
            for i in 0..=m.captures() {
                if i > 0 {
                    units.push(u16::from(b','));
                }
                match m.group(s, i) {
                    Some(g) => units.extend_from_slice(g.units()),
                    None => units.extend("undefined".encode_utf16()),
                }
            }
            units.extend(format!("@{}>", m.index()).encode_utf16());
            JsString::from_units(units)
        })))
    })
}

fn match_fn(args: &[Value]) -> Response {
    run(args, |re, s| {
        if re.global() {
            return Ok(re.match_global(s).map_or(Value::Null, strings));
        }
        Ok(match re.exec(s, &mut 0) {
            None => Value::Null,
            Some(m) => {
                let mut row = vec![index(m.index())];
                row.extend(groups(s, &m));
                Value::Array(row)
            }
        })
    })
}
