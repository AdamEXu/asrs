//! `fn` names for the console renderer port in `asrs_tabs`. Registry entries only move
//! values between CVE-1 and `JsString`; all behaviour lives in `asrs_tabs::ansi` and
//! `asrs_tabs::console`.

use asrs_js::string::JsString;
use asrs_parity::{Response, Utf16String, Value};
use asrs_tabs::ansi::AnsiUp;
use asrs_tabs::console;

pub type Function = fn(&[Value]) -> Response;

pub const FUNCTIONS: &[(&str, Function)] = &[
    ("oracle:ansiUp", |a| {
        with_str(a, 0, |s| string(AnsiUp::new().ansi_to_html(&s)))
    }),
    ("oracle:consoleFormatValue", |a| {
        match (str_arg(a, 0), str_arg(a, 1)) {
            (Ok(v), Ok(f)) => match console::format_value(&v, &f) {
                Ok(html) => Response::Ok(string(html)),
                Err(e) => Response::Throw(Value::Error(e.name.into(), e.message.into())),
            },
            (Err(r), _) | (_, Err(r)) => r,
        }
    }),
    ("oracle:consoleExport", |a| {
        let Some(lines) = a.first().and_then(Value::as_array) else {
            return Response::Error("argument 0 must be an array".to_owned());
        };
        let mut out = JsString::new();
        for (i, item) in lines.iter().enumerate() {
            let Some(s) = item.as_str() else {
                return Response::Error(format!("argument 0[{i}] must be a string"));
            };
            if i > 0 {
                out.push_str(&JsString::from("\n"));
            }
            out.push_str(&JsString::from_units(s.units().to_vec()));
        }
        Response::Ok(string(out))
    }),
];

/// `oracle:call` sources: the row-filter predicate inlined in
/// `ConsoleRenderer.updateValueAvailability`, transcribed verbatim.
pub const CALLS: &[(&str, Function)] = &[(
    r#"(value, input) => { const filter = input.toLowerCase(); if (!(filter.startsWith("!") ? filter.length > 1 : filter.length > 0)) return true; return filter.startsWith("!") ? !value.toLowerCase().includes(filter.slice(1).toLowerCase()) : value.toLowerCase().includes(filter.toLowerCase()); }"#,
    |a| match (str_arg(a, 0), str_arg(a, 1)) {
        (Ok(v), Ok(f)) => Response::Ok(Value::Bool(console::line_visible(&v, &f))),
        (Err(r), _) | (_, Err(r)) => r,
    },
)];

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
