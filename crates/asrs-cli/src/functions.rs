use asrs_parity::{Response, Value};

pub type Function = fn(&[Value]) -> Response;

/// Rust counterparts of oracle `fn` names. Each port unit registers the functions it ports
/// under the upstream name the oracle uses.
pub const FUNCTIONS: &[(&str, Function)] = &[("oracle:identity", identity)];

pub fn lookup(name: &str) -> Option<Function> {
    FUNCTIONS.iter().find(|(n, _)| *n == name).map(|(_, f)| *f)
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Undefined)
}

fn identity(args: &[Value]) -> Response {
    Response::Ok(arg(args, 0))
}
