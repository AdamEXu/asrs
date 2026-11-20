use asrs_parity::{Response, Value};

use crate::js_math;

pub type Function = fn(&[Value]) -> Response;

/// Rust counterparts of oracle `fn` names. Each port unit registers the functions it ports
/// under the upstream name the oracle uses.
pub const FUNCTIONS: &[(&str, Function)] =
    &[("oracle:identity", identity), ("oracle:call", oracle_call)];

pub fn lookup(name: &str) -> Option<Function> {
    FUNCTIONS
        .iter()
        .chain(js_math::FUNCTIONS)
        .find(|(n, _)| *n == name)
        .map(|(_, f)| *f)
}

/// `oracle:call` runs a JS function source in the oracle; here each supported source text maps
/// to its port.
fn oracle_call(args: &[Value]) -> Response {
    let source = match args.first().and_then(Value::as_str) {
        Some(s) => s.to_string_lossy(),
        None => return Response::Error("oracle:call needs a source string".to_owned()),
    };
    match js_math::CALLS.iter().find(|(s, _)| *s == source) {
        Some((_, f)) => f(&args[1..]),
        None => Response::Error(format!("oracle:call: no port of {source:?}")),
    }
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

fn identity(args: &[Value]) -> Response {
    Response::Ok(arg(args, 0))
}
