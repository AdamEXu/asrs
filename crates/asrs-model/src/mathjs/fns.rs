use std::sync::OnceLock;

use asrs_js::json;
use asrs_js::math;
use asrs_js::number;
use asrs_js::regex::RegExp;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};

use super::{MathError, Param, Signature, UNSUPPORTED, Value, sigs};

const EPSILON: f64 = 1e-12;
const TRUNCATE: usize = 78;
const COMPLEX_EPSILON: f64 = 1e-15;

fn js(s: &str) -> JsString {
    JsString::from(s)
}

fn merror(name: &'static str, message: impl Into<String>) -> MathError {
    MathError {
        name,
        message: JsString::from(message.into()),
    }
}

fn type_error(message: impl Into<String>) -> MathError {
    merror("TypeError", message)
}

fn error(message: impl Into<String>) -> MathError {
    merror("Error", message)
}

fn unsupported() -> MathError {
    merror(UNSUPPORTED, "not in the ported subset")
}

fn is_integer(x: f64) -> bool {
    x.is_finite() && x == math::round(x)
}

fn truthy(x: f64) -> bool {
    x != 0.0 && !x.is_nan()
}

fn nearly_equal(x: f64, y: f64) -> bool {
    if x == y {
        return true;
    }
    if x.is_nan() || y.is_nan() {
        return false;
    }
    if x.is_finite() && y.is_finite() {
        let diff = (x - y).abs();
        if diff < f64::EPSILON {
            return true;
        }
        return diff <= math::max2(x.abs(), y.abs()) * EPSILON;
    }
    false
}

fn coerce_units(v: &Value) -> Vec<u16> {
    match v {
        Value::Number(x) => number::to_string(*x).encode_utf16().collect(),
        Value::String(s) => s.units().to_vec(),
        Value::Boolean(b) => (if *b { "true" } else { "false" }).encode_utf16().collect(),
        Value::Null => "null".encode_utf16().collect(),
        Value::Undefined => "undefined".encode_utf16().collect(),
        Value::Function(_) => "function () {}".encode_utf16().collect(),
        Value::ResultSet(_) => "[".encode_utf16().collect(),
        Value::Complex { re, im } => complex_to_string(*re, *im).encode_utf16().collect(),
    }
}

/// mathjs's `identifier` type test, applied to `String(x)` because typed.js passes `isString`
/// itself rather than calling it.
fn is_identifier(u: &[u16]) -> bool {
    static IDENTIFIER: OnceLock<RegExp> = OnceLock::new();
    IDENTIFIER
        .get_or_init(|| {
            RegExp::new(include_str!("identifier.regex"), "").expect("typed.js identifier regex")
        })
        .test(u)
}

fn type_test(name: &str, arg: Option<&Value>) -> bool {
    match name {
        "any" => true,
        "number" => matches!(arg, Some(Value::Number(_))),
        "string" => matches!(arg, Some(Value::String(_))),
        "boolean" => matches!(arg, Some(Value::Boolean(_))),
        "null" => matches!(arg, Some(Value::Null)),
        "undefined" => arg.is_none() || matches!(arg, Some(Value::Undefined)),
        "identifier" => arg.is_some_and(|a| is_identifier(&coerce_units(a))),
        "Complex" => matches!(arg, Some(Value::Complex { .. })),
        "function" => matches!(arg, Some(Value::Function(_))),
        "ResultSet" => matches!(arg, Some(Value::ResultSet(_))),
        _ => false,
    }
}

fn param_test(param: &Param, arg: Option<&Value>) -> bool {
    param.types.iter().any(|t| type_test(t.name, arg))
}

fn has_rest(params: &[Param]) -> bool {
    params.last().is_some_and(|p| p.rest)
}

fn param_at(params: &[Param], index: usize) -> Option<&Param> {
    if index < params.len() {
        Some(&params[index])
    } else if has_rest(params) {
        params.last()
    } else {
        None
    }
}

fn sig_matches(sig: &Signature, args: &[Value]) -> bool {
    let params = sig.params;
    if has_rest(params) {
        let var_index = params.len() - 1;
        for (i, p) in params[..var_index].iter().enumerate() {
            if !param_test(p, args.get(i)) {
                return false;
            }
        }
        for i in var_index..args.len() {
            if !param_test(&params[var_index], args.get(i)) {
                return false;
            }
        }
        args.len() > var_index
    } else {
        for (i, p) in params.iter().enumerate() {
            if !param_test(p, args.get(i)) {
                return false;
            }
        }
        args.len() == params.len()
    }
}

fn find_type_names(arg: Option<&Value>) -> Vec<&'static str> {
    const TYPE_LIST: &[&str] = &[
        "number",
        "Complex",
        "BigNumber",
        "Fraction",
        "Unit",
        "identifier",
        "string",
        "Chain",
        "Array",
        "Matrix",
        "DenseMatrix",
        "SparseMatrix",
        "Range",
        "Index",
        "boolean",
        "ResultSet",
        "Help",
        "function",
        "Date",
        "RegExp",
        "null",
        "undefined",
        "AccessorNode",
        "ArrayNode",
        "AssignmentNode",
        "BlockNode",
        "ConditionalNode",
        "ConstantNode",
        "FunctionNode",
        "FunctionAssignmentNode",
        "IndexNode",
        "Node",
        "ObjectNode",
        "OperatorNode",
        "ParenthesisNode",
        "RangeNode",
        "RelationalNode",
        "SymbolNode",
        "Map",
        "Object",
    ];
    let out: Vec<&'static str> = TYPE_LIST
        .iter()
        .copied()
        .filter(|&name| type_test(name, arg))
        .collect();
    if out.is_empty() { vec!["any"] } else { out }
}

fn merge_expected(sigs: &[&Signature], index: usize) -> Vec<&'static str> {
    let mut set: Vec<&'static str> = Vec::new();
    for sig in sigs {
        if let Some(param) = param_at(sig.params, index) {
            for t in param.types {
                if !set.contains(&t.name) {
                    set.push(t.name);
                }
            }
        }
    }
    if set.contains(&"any") {
        vec!["any"]
    } else {
        set
    }
}

fn create_error(name: &str, args: &[Value], sigs: &[Signature]) -> MathError {
    let mut matching: Vec<&Signature> = sigs.iter().collect();
    let mut index = 0;
    while index < args.len() {
        let mut next: Vec<&Signature> = Vec::new();
        for sig in &matching {
            let guard = index < sig.params.len() || has_rest(sig.params);
            let test = match param_at(sig.params, index) {
                Some(p) => param_test(p, Some(&args[index])),
                None => true,
            };
            if guard && test {
                next.push(sig);
            }
        }
        if next.is_empty() {
            let expected = merge_expected(&matching, index);
            if !expected.is_empty() {
                let actual = find_type_names(Some(&args[index]));
                return type_error(format!(
                    "Unexpected type of argument in function {} (expected: {}, actual: {}, index: {})",
                    name,
                    expected.join(" or "),
                    actual.join(" | "),
                    index
                ));
            }
        } else {
            matching = next;
        }
        index += 1;
    }
    let lengths: Vec<f64> = matching
        .iter()
        .map(|s| {
            if has_rest(s.params) {
                f64::INFINITY
            } else {
                s.params.len() as f64
            }
        })
        .collect();
    let min_len = math::min(&lengths);
    if (args.len() as f64) < min_len {
        let expected = merge_expected(&matching, args.len());
        return type_error(format!(
            "Too few arguments in function {} (expected: {}, index: {})",
            name,
            expected.join(" or "),
            args.len()
        ));
    }
    let max_len = math::max(&lengths);
    if (args.len() as f64) > max_len {
        return type_error(format!(
            "Too many arguments in function {} (expected: {}, actual: {})",
            name,
            number::to_string(max_len),
            args.len()
        ));
    }
    let arg_types: Vec<String> = args
        .iter()
        .map(|a| find_type_names(Some(a)).join("|"))
        .collect();
    type_error(format!(
        "Arguments of type \"{}\" do not match any of the defined signatures of function {}.",
        arg_types.join(", "),
        name
    ))
}

fn convert_arg(from: &str, to: &str, arg: &Value) -> Result<Value, MathError> {
    match (from, to) {
        ("string", "number") => {
            let Value::String(s) = arg else {
                return Err(unsupported());
            };
            let n = number::to_number(s.units());
            if n.is_nan() {
                let mut u = "Cannot convert \"".encode_utf16().collect::<Vec<u16>>();
                u.extend_from_slice(s.units());
                u.extend_from_slice(&"\" to a number".encode_utf16().collect::<Vec<u16>>());
                Err(MathError {
                    name: "Error",
                    message: JsString::from_units(u),
                })
            } else {
                Ok(Value::Number(n))
            }
        }
        ("boolean", "number") => match arg {
            Value::Boolean(b) => Ok(Value::Number(if *b { 1.0 } else { 0.0 })),
            _ => Err(unsupported()),
        },
        _ => Err(unsupported()),
    }
}

fn convert_param(param: &Param, arg: &Value) -> Result<Value, MathError> {
    for t in param.types {
        if let Some((from, to)) = t.conversion.filter(|(from, _)| type_test(from, Some(arg))) {
            return convert_arg(from, to, arg);
        }
    }
    Ok(arg.clone())
}

fn raw_signature(sig: &Signature) -> String {
    let mut out = String::new();
    for (i, p) in sig.params.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if p.rest {
            out.push_str("...");
        }
        for (j, t) in p.types.iter().enumerate() {
            if j > 0 {
                out.push('|');
            }
            out.push_str(t.conversion.map(|(_, to)| to).unwrap_or(t.name));
        }
    }
    out
}

fn invoke(fn_name: &str, sig: &Signature, args: &[Value]) -> Result<Value, MathError> {
    let mut conv: Vec<Value> = Vec::with_capacity(args.len());
    for (i, arg) in args.iter().enumerate() {
        let param = if i < sig.params.len() {
            &sig.params[i]
        } else {
            sig.params.last().expect("rest param")
        };
        conv.push(convert_param(param, arg)?);
    }
    let raw = raw_signature(sig);
    run_impl(fn_name, &raw, &conv)
}

fn num(args: &[Value], i: usize) -> f64 {
    match args.get(i) {
        Some(Value::Number(x)) => *x,
        _ => f64::NAN,
    }
}

fn value_num(x: f64) -> Value {
    Value::Number(x)
}

fn value_bool(b: bool) -> Value {
    Value::Boolean(b)
}

fn value_cx(z: Cx) -> Value {
    Value::Complex { re: z.re, im: z.im }
}

fn fold2(name: &str, args: &[Value]) -> Result<Value, MathError> {
    let mut result = call(name, &args[0..2])?;
    for v in &args[2..] {
        result = call(name, &[result, v.clone()])?;
    }
    Ok(result)
}

fn bit_binary(
    name: &str,
    x: f64,
    y: f64,
    op: impl FnOnce(i32, i32) -> i32,
) -> Result<Value, MathError> {
    if !is_integer(x) || !is_integer(y) {
        return Err(error(format!("Integers expected in function {name}")));
    }
    Ok(value_num(op(math::to_int32(x), math::to_int32(y)) as f64))
}

fn shift_binary(
    name: &str,
    x: f64,
    y: f64,
    op: impl FnOnce(f64, f64) -> f64,
) -> Result<Value, MathError> {
    if !is_integer(x) || !is_integer(y) {
        return Err(error(format!("Integers expected in function {name}")));
    }
    Ok(value_num(op(x, y)))
}

fn pow_number(x: f64, y: f64) -> f64 {
    if x * x < 1.0 && y == f64::INFINITY || x * x > 1.0 && y == f64::NEG_INFINITY {
        0.0
    } else {
        math::pow(x, y)
    }
}

fn pow_impl(x: f64, y: f64) -> Value {
    if is_integer(y) || x >= 0.0 {
        value_num(pow_number(x, y))
    } else if x * x < 1.0 && y == f64::INFINITY || x * x > 1.0 && y == f64::NEG_INFINITY {
        value_num(0.0)
    } else {
        value_cx(Cx { re: x, im: 0.0 }.cx_pow(Cx { re: y, im: 0.0 }))
    }
}

fn mod_number(x: f64, y: f64) -> Result<f64, MathError> {
    if y > 0.0 {
        Ok(x - y * (x / y).floor())
    } else if y == 0.0 {
        Ok(x)
    } else {
        Err(error("Cannot calculate mod for a negative divisor"))
    }
}

fn unary_plus_arg(arg: &Value) -> f64 {
    match arg {
        Value::Number(x) => *x,
        Value::Boolean(b) => {
            if *b {
                1.0
            } else {
                0.0
            }
        }
        Value::String(s) => number::to_number(s.units()),
        _ => f64::NAN,
    }
}

fn sqrt_impl(x: f64) -> Value {
    if x.is_nan() {
        value_num(f64::NAN)
    } else if x >= 0.0 {
        value_num(x.sqrt())
    } else {
        value_cx(Cx { re: x, im: 0.0 }.cx_sqrt())
    }
}

fn log_number(x: f64) -> Value {
    if x >= 0.0 {
        value_num(math::log(x))
    } else {
        value_cx(Cx { re: x, im: 0.0 }.cx_log())
    }
}

fn log10_number(x: f64) -> Value {
    if x >= 0.0 {
        value_num(math::log10(x))
    } else {
        let z = Cx { re: x, im: 0.0 }.cx_log();
        value_cx(z.cx_div(Cx {
            re: math::LN10,
            im: 0.0,
        }))
    }
}

fn log2_number(x: f64) -> Value {
    if x >= 0.0 {
        value_num(math::log2(x))
    } else {
        let z = Cx { re: x, im: 0.0 };
        let new_x = (z.re * z.re + z.im * z.im).sqrt();
        value_cx(Cx {
            re: math::log2(new_x),
            im: math::atan2(z.im, z.re) / math::LN2,
        })
    }
}

fn log_base(x: &Value, base: &Value) -> Result<Value, MathError> {
    let a = call("log", std::slice::from_ref(x))?;
    let b = call("log", std::slice::from_ref(base))?;
    divide_scalar(&a, &b)
}

fn divide_scalar(x: &Value, y: &Value) -> Result<Value, MathError> {
    match (x, y) {
        (Value::Number(a), Value::Number(b)) => Ok(value_num(a / b)),
        _ => {
            let a = match x {
                Value::Number(n) => Cx { re: *n, im: 0.0 },
                Value::Complex { re, im } => Cx { re: *re, im: *im },
                _ => return Err(unsupported()),
            };
            let b = match y {
                Value::Number(n) => Cx { re: *n, im: 0.0 },
                Value::Complex { re, im } => Cx { re: *re, im: *im },
                _ => return Err(unsupported()),
            };
            Ok(value_cx(a.cx_div(b)))
        }
    }
}

fn round_number(x: f64, decimals: f64) -> Result<f64, MathError> {
    if !is_integer(decimals) || decimals < 0.0 || decimals > 15.0 {
        return Err(error(
            "Number of decimals in function round must be an integer from 0 to 15 inclusive",
        ));
    }
    Ok(number::parse_float(
        to_fixed(x, decimals as i32)
            .encode_utf16()
            .collect::<Vec<u16>>()
            .as_slice(),
    ))
}

fn split_e_parts(x: f64) -> (Vec<u16>, Vec<u16>) {
    let s = format!("{}e", number::to_string(x));
    let u: Vec<u16> = s.encode_utf16().collect();
    let mut parts = u.split(|&c| c == 'e' as u16);
    let a = parts.next().unwrap_or(&[]).to_vec();
    let b = parts.next().unwrap_or(&[]).to_vec();
    (a, b)
}

fn floor_ceil_n(x: f64, n: f64, ceil: bool) -> Result<f64, MathError> {
    let r = round_number(x, n)?;
    if nearly_equal(x, r) {
        return Ok(r);
    }
    let (digits, exponent) = split_e_parts(x);
    let e = number::to_number(&exponent);
    let mut back = digits.clone();
    back.push('e' as u16);
    back.extend_from_slice(
        &number::to_string(e + n)
            .encode_utf16()
            .collect::<Vec<u16>>(),
    );
    let scaled = number::to_number(&back);
    let result = if ceil { scaled.ceil() } else { scaled.floor() };
    let (digits2, exponent2) = split_e_parts(result);
    let e2 = number::to_number(&exponent2);
    let mut back2 = digits2;
    back2.push('e' as u16);
    back2.extend_from_slice(
        &number::to_string(e2 - n)
            .encode_utf16()
            .collect::<Vec<u16>>(),
    );
    Ok(number::to_number(&back2))
}

fn floor_one(x: f64) -> Result<f64, MathError> {
    let r = round_number(x, 0.0)?;
    if nearly_equal(x, r) {
        Ok(r)
    } else {
        Ok(x.floor())
    }
}

fn ceil_one(x: f64) -> Result<f64, MathError> {
    let r = round_number(x, 0.0)?;
    if nearly_equal(x, r) {
        Ok(r)
    } else {
        Ok(x.ceil())
    }
}

fn product(i: f64, n: f64) -> f64 {
    if n < i {
        return 1.0;
    }
    if n == i {
        return n;
    }
    let half = (math::to_int32(n + i) >> 1) as f64;
    product(i, half) * product(half + 1.0, n)
}

const GAMMA_G: f64 = 4.7421875;
const GAMMA_P: [f64; 15] = [
    0.9999999999999971,
    57.15623566586292,
    -59.59796035547549,
    14.136097974741746,
    -0.4919138160976202,
    0.3399464998481189e-4,
    0.4652362892704858e-4,
    -0.9837447530487956e-4,
    0.1580887032249125e-3,
    -0.2102644417241049e-3,
    0.21743961811521264e-3,
    -0.1643181065367639e-3,
    0.8441822398385275e-4,
    -0.2619083840158141e-4,
    0.3689918265953162e-5,
];

fn gamma_number(n: f64) -> f64 {
    if is_integer(n) {
        if n <= 0.0 {
            return if n.is_finite() {
                f64::INFINITY
            } else {
                f64::NAN
            };
        }
        if n > 171.0 {
            return f64::INFINITY;
        }
        return product(1.0, n - 1.0);
    }
    if n < 0.5 {
        return std::f64::consts::PI
            / (math::sin(std::f64::consts::PI * n) * gamma_number(1.0 - n));
    }
    if n >= 171.35 {
        return f64::INFINITY;
    }
    if n > 85.0 {
        let two_n = n * n;
        let three_n = two_n * n;
        let four_n = three_n * n;
        let five_n = four_n * n;
        return (2.0 * std::f64::consts::PI / n).sqrt()
            * math::pow(n / std::f64::consts::E, n)
            * (1.0 + 1.0 / (12.0 * n) + 1.0 / (288.0 * two_n)
                - 139.0 / (51840.0 * three_n)
                - 571.0 / (2488320.0 * four_n)
                + 163879.0 / (209018880.0 * five_n)
                + 5246819.0 / (75246796800.0 * five_n * n));
    }
    let n = n - 1.0;
    let mut x = GAMMA_P[0];
    for (i, &p) in GAMMA_P.iter().enumerate().skip(1) {
        x += p / (n + i as f64);
    }
    let t = n + GAMMA_G + 0.5;
    (2.0 * std::f64::consts::PI).sqrt() * math::pow(t, n + 0.5) * math::exp(-t) * x
}

fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Boolean(_) => "boolean",
        Value::Null => "null",
        Value::Undefined => "undefined",
        Value::Function(_) => "function",
        Value::ResultSet(_) => "ResultSet",
        Value::Complex { .. } => "Complex",
    }
}

fn to_jsvalue(v: &Value) -> JsValue {
    match v {
        Value::Number(x) => JsValue::Number(*x),
        Value::String(s) => JsValue::String(s.clone()),
        Value::Boolean(b) => JsValue::Bool(*b),
        Value::Null => JsValue::Null,
        Value::Undefined => JsValue::Undefined,
        Value::Function(_) | Value::Complex { .. } => JsValue::Undefined,
        Value::ResultSet(entries) => {
            let mut o = JsObject::new();
            o.insert("mathjs", JsValue::String(js("ResultSet")));
            o.insert(
                "entries",
                JsValue::Array(entries.iter().map(to_jsvalue).collect()),
            );
            JsValue::Object(o)
        }
    }
}

fn json_str(v: &Value) -> String {
    match json::stringify(&to_jsvalue(v)) {
        Ok(Some(s)) => s,
        _ => "undefined".to_owned(),
    }
}

fn improve_error(err: MathError, fn_name: &str, value: &Value) -> MathError {
    let m = err.message.to_string_lossy();
    if m.contains("Unexpected type") {
        return type_error(format!(
            "Cannot calculate {}, unexpected type of argument (type: {}, value: {})",
            fn_name,
            type_of(value),
            json_str(value)
        ));
    }
    if m.contains("complex numbers") {
        return type_error(format!(
            "Cannot calculate {}, no ordering relation is defined for complex numbers (type: {}, value: {})",
            fn_name,
            type_of(value),
            json_str(value)
        ));
    }
    err
}

fn min_max(fn_name: &str, cmp: &str, args: &[Value]) -> Result<Value, MathError> {
    let mut best = Value::Undefined;
    for value in args {
        let step = (|| -> Result<(), MathError> {
            match value {
                Value::Number(n) if n.is_nan() => {
                    best = Value::Number(f64::NAN);
                }
                _ => {
                    let take = if matches!(best, Value::Undefined) {
                        true
                    } else {
                        matches!(
                            call(cmp, &[value.clone(), best.clone()])?,
                            Value::Boolean(true)
                        )
                    };
                    if take {
                        best = value.clone();
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = step {
            return Err(improve_error(e, fn_name, value));
        }
    }
    if matches!(best, Value::Undefined) {
        return Err(error(format!(
            "Cannot calculate {} of an empty array",
            fn_name
        )));
    }
    if matches!(best, Value::String(_)) {
        return call("number", &[best]);
    }
    Ok(best)
}

fn equal_any(x: &Value, y: &Value) -> Result<Value, MathError> {
    if matches!(x, Value::Null) {
        return Ok(value_bool(matches!(y, Value::Null)));
    }
    if matches!(y, Value::Null) {
        return Ok(value_bool(matches!(x, Value::Null)));
    }
    if matches!(x, Value::Undefined) {
        return Ok(value_bool(matches!(y, Value::Undefined)));
    }
    if matches!(y, Value::Undefined) {
        return Ok(value_bool(matches!(x, Value::Undefined)));
    }
    call("equalScalar", &[x.clone(), y.clone()])
}

fn unequal_any(x: &Value, y: &Value) -> Result<Value, MathError> {
    if matches!(x, Value::Null) {
        return Ok(value_bool(!matches!(y, Value::Null)));
    }
    if matches!(y, Value::Null) {
        return Ok(value_bool(!matches!(x, Value::Null)));
    }
    if matches!(x, Value::Undefined) {
        return Ok(value_bool(!matches!(y, Value::Undefined)));
    }
    if matches!(y, Value::Undefined) {
        return Ok(value_bool(!matches!(x, Value::Undefined)));
    }
    match call("equalScalar", &[x.clone(), y.clone()])? {
        Value::Boolean(b) => Ok(value_bool(!b)),
        other => Ok(other),
    }
}

// JS regex /(0[box])([0-9a-fA-F]*)\.([0-9a-fA-F]*)/, leftmost match.
fn non_decimal_parts(u: &[u16]) -> Option<(u32, Vec<u16>, Vec<u16>)> {
    let is_hex = |c: u16| u8::try_from(c).is_ok_and(|b| b.is_ascii_hexdigit());
    for i in 0..u.len().saturating_sub(1) {
        if u[i] == '0' as u16 {
            let radix = match u[i + 1] {
                c if c == 'x' as u16 => 16,
                c if c == 'o' as u16 => 8,
                c if c == 'b' as u16 => 2,
                _ => continue,
            };
            let mut j = i + 2;
            while j < u.len() && is_hex(u[j]) {
                j += 1;
            }
            if j < u.len() && u[j] == '.' as u16 {
                let mut k = j + 1;
                while k < u.len() && is_hex(u[k]) {
                    k += 1;
                }
                return Some((radix, u[i + 2..j].to_vec(), u[j + 1..k].to_vec()));
            }
        }
    }
    None
}

// JS regex /(0[box][0-9a-fA-F]*)i([0-9]*)/, leftmost match.
fn word_size_suffix(u: &[u16]) -> Option<(Vec<u16>, Vec<u16>)> {
    let is_hex = |c: u16| u8::try_from(c).is_ok_and(|b| b.is_ascii_hexdigit());
    for i in 0..u.len().saturating_sub(1) {
        if u[i] == '0' as u16
            && matches!(u[i + 1], c if c == 'x' as u16 || c == 'o' as u16 || c == 'b' as u16)
        {
            let mut j = i + 2;
            while j < u.len() && is_hex(u[j]) {
                j += 1;
            }
            if j < u.len() && u[j] == 'i' as u16 {
                let mut k = j + 1;
                while k < u.len() && u8::try_from(u[k]).is_ok_and(|b| b.is_ascii_digit()) {
                    k += 1;
                }
                return Some((u[i..j].to_vec(), u[j + 1..k].to_vec()));
            }
        }
    }
    None
}

fn string_is(u: &[u16], s: &str) -> bool {
    u == s.encode_utf16().collect::<Vec<u16>>().as_slice()
}

/// The `number` function's `string` signature, which the parser also applies to number literals.
pub(super) fn number_from_string(u: &[u16]) -> Result<f64, MathError> {
    if string_is(u, "NaN") {
        return Ok(f64::NAN);
    }
    if let Some((radix, int_part, frac_part)) = non_decimal_parts(u) {
        let n = number::parse_int(&int_part, radix as f64);
        let mut f = 0.0;
        for (i, &d) in frac_part.iter().enumerate() {
            let dv = number::parse_int(&[d], radix as f64);
            f += dv / math::pow(radix as f64, i as f64 + 1.0);
        }
        let result = n + f;
        if result.is_nan() {
            let mut m = "String \"".encode_utf16().collect::<Vec<u16>>();
            m.extend_from_slice(u);
            m.extend_from_slice(&"\" is no valid number".encode_utf16().collect::<Vec<u16>>());
            return Err(MathError {
                name: "SyntaxError",
                message: JsString::from_units(m),
            });
        }
        return Ok(result);
    }
    let mut size = 0.0;
    let mut x: Vec<u16> = u.to_vec();
    let mut had_suffix = false;
    if let Some((lit, digits)) = word_size_suffix(u) {
        size = number::to_number(&digits);
        x = lit;
        had_suffix = true;
    }
    let mut n = number::to_number(&x);
    if n.is_nan() {
        let mut m = "String \"".encode_utf16().collect::<Vec<u16>>();
        m.extend_from_slice(&x);
        m.extend_from_slice(&"\" is no valid number".encode_utf16().collect::<Vec<u16>>());
        return Err(MathError {
            name: "SyntaxError",
            message: JsString::from_units(m),
        });
    }
    if had_suffix {
        if n > math::pow(2.0, size) - 1.0 {
            let mut m = "String \"".encode_utf16().collect::<Vec<u16>>();
            m.extend_from_slice(&x);
            m.extend_from_slice(&"\" is out of range".encode_utf16().collect::<Vec<u16>>());
            return Err(MathError {
                name: "SyntaxError",
                message: JsString::from_units(m),
            });
        }
        if n >= math::pow(2.0, size - 1.0) {
            n -= math::pow(2.0, size);
        }
    }
    Ok(n)
}

fn run_impl(fn_name: &str, raw: &str, args: &[Value]) -> Result<Value, MathError> {
    let x = num(args, 0);
    match (fn_name, raw) {
        ("add", "number,number") => Ok(value_num(x + num(args, 1))),
        ("add", "any,any") => call("addScalar", args),
        ("subtract", "number,number") => Ok(value_num(x - num(args, 1))),
        ("multiply", "number,number") => Ok(value_num(x * num(args, 1))),
        ("multiply", "any,any") => call("multiplyScalar", args),
        ("divide", "number,number") => Ok(value_num(x / num(args, 1))),
        ("pow", "number,number") => Ok(pow_impl(x, num(args, 1))),
        ("mod", "number,number") => Ok(value_num(mod_number(x, num(args, 1))?)),
        ("unaryMinus", "number") => Ok(value_num(-x)),
        ("unaryPlus", "number") => Ok(value_num(x)),
        ("unaryPlus", "boolean" | "string") => Ok(value_num(unary_plus_arg(&args[0]))),
        ("factorial", "number") => {
            if x < 0.0 {
                return Err(error("Value must be non-negative"));
            }
            call("gamma", &[Value::Number(x + 1.0)])
        }
        ("ctranspose", "any") => call("conj", args),
        ("bitAnd", "number,number") => bit_binary("bitAnd", x, num(args, 1), |a, b| a & b),
        ("bitOr", "number,number") => bit_binary("bitOr", x, num(args, 1), |a, b| a | b),
        ("bitXor", "number,number") => bit_binary("bitXor", x, num(args, 1), |a, b| a ^ b),
        ("bitNot", "number") => {
            if !is_integer(x) {
                return Err(error("Integer expected in function bitNot"));
            }
            Ok(value_num(!math::to_int32(x) as f64))
        }
        ("leftShift", "number,number") => shift_binary("leftShift", x, num(args, 1), |a, b| {
            math::shift_left(a, b) as f64
        }),
        ("rightArithShift", "number,number") => {
            shift_binary("rightArithShift", x, num(args, 1), |a, b| {
                math::shift_right(a, b) as f64
            })
        }
        ("rightLogShift", "number,number") => {
            shift_binary("rightLogShift", x, num(args, 1), |a, b| {
                math::shift_right_unsigned(a, b) as f64
            })
        }
        ("and", "number,number") => Ok(value_bool(truthy(x) && truthy(num(args, 1)))),
        ("or", "number,number") => Ok(value_bool(truthy(x) || truthy(num(args, 1)))),
        ("xor", "number,number") => Ok(value_bool(truthy(x) != truthy(num(args, 1)))),
        ("not", "number") => Ok(value_bool(!truthy(x))),
        ("not", "null" | "undefined") => Ok(value_bool(true)),
        ("equal", "number,number" | "boolean,boolean") => call("equalScalar", args),
        ("equal", "any,any") => equal_any(&args[0], &args[1]),
        ("unequal", "any,any") => unequal_any(&args[0], &args[1]),
        ("smaller", "number,number") => Ok(value_bool(
            x < num(args, 1) && !nearly_equal(x, num(args, 1)),
        )),
        ("smaller", "boolean,boolean") => {
            let (Value::Boolean(a), Value::Boolean(b)) = (&args[0], &args[1]) else {
                return Err(unsupported());
            };
            Ok(value_bool(!(*a) && *b))
        }
        ("larger", "number,number") => Ok(value_bool(
            x > num(args, 1) && !nearly_equal(x, num(args, 1)),
        )),
        ("larger", "boolean,boolean") => {
            let (Value::Boolean(a), Value::Boolean(b)) = (&args[0], &args[1]) else {
                return Err(unsupported());
            };
            Ok(value_bool(*a && !(*b)))
        }
        ("smallerEq", "number,number") => Ok(value_bool(
            x <= num(args, 1) || nearly_equal(x, num(args, 1)),
        )),
        ("smallerEq", "boolean,boolean") => {
            let (Value::Boolean(a), Value::Boolean(b)) = (&args[0], &args[1]) else {
                return Err(unsupported());
            };
            Ok(value_bool(!(*a) || *b))
        }
        ("largerEq", "number,number") => Ok(value_bool(
            x >= num(args, 1) || nearly_equal(x, num(args, 1)),
        )),
        ("largerEq", "boolean,boolean") => {
            let (Value::Boolean(a), Value::Boolean(b)) = (&args[0], &args[1]) else {
                return Err(unsupported());
            };
            Ok(value_bool(*a || !(*b)))
        }
        ("dotMultiply", "number,number") => call("multiplyScalar", args),
        ("dotDivide", "number,number") => divide_scalar(&args[0], &args[1]),
        ("dotPow", "number,number") => call("pow", args),
        ("abs", "number") => Ok(value_num(x.abs())),
        ("sqrt", "number") => Ok(sqrt_impl(x)),
        ("cbrt", "number") => Ok(value_num(math::cbrt(x))),
        ("exp", "number") => Ok(value_num(math::exp(x))),
        ("log", "number") => Ok(log_number(x)),
        ("log", "any,any") => log_base(&args[0], &args[1]),
        ("log10", "number") => Ok(log10_number(x)),
        ("log2", "number") => Ok(log2_number(x)),
        ("round", "number") => Ok(value_num(round_number(x, 0.0)?)),
        ("round", "number,number") => Ok(value_num(round_number(x, num(args, 1))?)),
        ("floor", "number") => Ok(value_num(floor_one(x)?)),
        ("floor", "number,number") => Ok(value_num(floor_ceil_n(x, num(args, 1), false)?)),
        ("ceil", "number") => Ok(value_num(ceil_one(x)?)),
        ("ceil", "number,number") => Ok(value_num(floor_ceil_n(x, num(args, 1), true)?)),
        ("fix", "number") => Ok(value_num(if x > 0.0 {
            floor_one(x)?
        } else {
            ceil_one(x)?
        })),
        ("fix", "number,number") => Ok(value_num(if x > 0.0 {
            floor_ceil_n(x, num(args, 1), false)?
        } else {
            floor_ceil_n(x, num(args, 1), true)?
        })),
        ("sign", "number") => Ok(value_num(math::sign(x))),
        ("min", "...any") => min_max("min", "smaller", args),
        ("max", "...any") => min_max("max", "larger", args),
        ("sin", "number") => Ok(value_num(math::sin(x))),
        ("cos", "number") => Ok(value_num(math::cos(x))),
        ("tan", "number") => Ok(value_num(math::tan(x))),
        ("asin", "number") => {
            if (-1.0..=1.0).contains(&x) {
                Ok(value_num(math::asin(x)))
            } else {
                Ok(value_cx(Cx { re: x, im: 0.0 }.cx_asin()))
            }
        }
        ("acos", "number") => {
            if (-1.0..=1.0).contains(&x) {
                Ok(value_num(math::acos(x)))
            } else {
                Ok(value_cx(Cx { re: x, im: 0.0 }.cx_acos()))
            }
        }
        ("atan", "number") => Ok(value_num(math::atan(x))),
        ("atan2", "number,number") => Ok(value_num(math::atan2(x, num(args, 1)))),
        ("number", "") => Ok(value_num(0.0)),
        ("number", "number") => Ok(value_num(x)),
        ("number", "null") => Ok(value_num(0.0)),
        ("number", "string") => {
            let Value::String(s) = &args[0] else {
                return Err(unsupported());
            };
            Ok(value_num(number_from_string(s.units())?))
        }
        ("square", "number") => Ok(value_num(x * x)),
        ("cube", "number") => Ok(value_num(x * x * x)),
        ("gamma", "number") => Ok(value_num(gamma_number(x))),
        ("addScalar", "number,number") => Ok(value_num(x + num(args, 1))),
        ("multiplyScalar", "number,number") => Ok(value_num(x * num(args, 1))),
        ("equalScalar", "number,number") => Ok(value_bool(nearly_equal(x, num(args, 1)))),
        ("equalScalar", "boolean,boolean") => {
            let (Value::Boolean(a), Value::Boolean(b)) = (&args[0], &args[1]) else {
                return Err(unsupported());
            };
            Ok(value_bool(a == b))
        }
        ("conj", "number") => Ok(value_num(x)),
        ("conj", "Complex") => {
            let Value::Complex { re, im } = args[0] else {
                return Err(unsupported());
            };
            Ok(Value::Complex { re, im: -im })
        }
        ("add", "any,any,...any") | ("multiply", "any,any,...any") => fold2(fn_name, args),
        _ => Err(unsupported()),
    }
}

pub fn call(name: &str, args: &[Value]) -> Result<Value, MathError> {
    let Some((js_name, signatures)) = sigs::lookup(name) else {
        return Err(unsupported());
    };
    for sig in signatures {
        if sig_matches(sig, args) {
            return invoke(name, sig, args);
        }
    }
    Err(create_error(js_name, args, signatures))
}

#[derive(Clone, Copy)]
struct Cx {
    re: f64,
    im: f64,
}

fn cx_nan() -> Cx {
    Cx {
        re: f64::NAN,
        im: f64::NAN,
    }
}

fn cx_infinity() -> Cx {
    Cx {
        re: f64::INFINITY,
        im: f64::INFINITY,
    }
}

fn cx_hypot(x: f64, y: f64) -> f64 {
    let mut a = x.abs();
    let mut b = y.abs();
    if a < 3000.0 && b < 3000.0 {
        return (a * a + b * b).sqrt();
    }
    if a < b {
        a = b;
        b = x / y;
    } else {
        b = y / x;
    }
    a * (1.0 + b * b).sqrt()
}

fn log_hypot(a: f64, b: f64) -> f64 {
    let abs_a = a.abs();
    let abs_b = b.abs();
    if a == 0.0 {
        return math::log(abs_b);
    }
    if b == 0.0 {
        return math::log(abs_a);
    }
    if abs_a < 3000.0 && abs_b < 3000.0 {
        return math::log(a * a + b * b) * 0.5;
    }
    let a = a / 2.0;
    let b = b / 2.0;
    0.5 * math::log(a * a + b * b) + math::LN2
}

impl Cx {
    fn is_nan(self) -> bool {
        self.re.is_nan() || self.im.is_nan()
    }
    fn is_zero(self) -> bool {
        self.im == 0.0 && self.re == 0.0
    }
    fn is_finite_cx(self) -> bool {
        self.re.is_finite() && self.im.is_finite()
    }
    fn is_infinite(self) -> bool {
        !(self.is_nan() || self.is_finite_cx())
    }

    fn cx_div(self, z: Cx) -> Cx {
        if (self.is_zero() && z.is_zero()) || (self.is_infinite() && z.is_infinite()) {
            return cx_nan();
        }
        if self.is_infinite() || z.is_zero() {
            return cx_infinity();
        }
        if self.is_zero() || z.is_infinite() {
            return Cx { re: 0.0, im: 0.0 };
        }
        let a = self.re;
        let b = self.im;
        let c = z.re;
        let d = z.im;
        if d == 0.0 {
            return Cx {
                re: a / c,
                im: b / c,
            };
        }
        if c.abs() < d.abs() {
            let x = c / d;
            let t = c * x + d;
            Cx {
                re: (a * x + b) / t,
                im: (b * x - a) / t,
            }
        } else {
            let x = d / c;
            let t = d * x + c;
            Cx {
                re: (a + b * x) / t,
                im: (b - a * x) / t,
            }
        }
    }

    fn cx_log(self) -> Cx {
        Cx {
            re: log_hypot(self.re, self.im),
            im: math::atan2(self.im, self.re),
        }
    }

    fn cx_sqrt(self) -> Cx {
        let a = self.re;
        let b = self.im;
        let r = cx_hypot(a, b);
        if a >= 0.0 && b == 0.0 {
            return Cx {
                re: a.sqrt(),
                im: 0.0,
            };
        }
        let re = if a >= 0.0 {
            0.5 * (2.0 * (r + a)).sqrt()
        } else {
            b.abs() / (2.0 * (r - a)).sqrt()
        };
        let im = if a <= 0.0 {
            0.5 * (2.0 * (r - a)).sqrt()
        } else {
            b.abs() / (2.0 * (r + a)).sqrt()
        };
        Cx {
            re,
            im: if b < 0.0 { -im } else { im },
        }
    }

    fn cx_pow(self, z: Cx) -> Cx {
        let a = self.re;
        let b = self.im;
        if z.is_zero() {
            return Cx { re: 1.0, im: 0.0 };
        }
        if z.im == 0.0 {
            if b == 0.0 && a > 0.0 {
                return Cx {
                    re: math::pow(a, z.re),
                    im: 0.0,
                };
            } else if a == 0.0 {
                match math::rem(math::rem(z.re, 4.0) + 4.0, 4.0) as i32 {
                    0 => {
                        return Cx {
                            re: math::pow(b, z.re),
                            im: 0.0,
                        };
                    }
                    1 => {
                        return Cx {
                            re: 0.0,
                            im: math::pow(b, z.re),
                        };
                    }
                    2 => {
                        return Cx {
                            re: -math::pow(b, z.re),
                            im: 0.0,
                        };
                    }
                    3 => {
                        return Cx {
                            re: 0.0,
                            im: -math::pow(b, z.re),
                        };
                    }
                    _ => {}
                }
            }
        }
        if a == 0.0 && b == 0.0 && z.re > 0.0 && z.im >= 0.0 {
            return Cx { re: 0.0, im: 0.0 };
        }
        let arg = math::atan2(b, a);
        let loh = log_hypot(a, b);
        let a2 = math::exp(z.re * loh - z.im * arg);
        let b2 = z.im * loh + z.re * arg;
        Cx {
            re: a2 * math::cos(b2),
            im: a2 * math::sin(b2),
        }
    }

    fn cx_asin(self) -> Cx {
        let a = self.re;
        let b = self.im;
        let t1 = Cx {
            re: b * b - a * a + 1.0,
            im: -2.0 * a * b,
        }
        .cx_sqrt();
        let t2 = Cx {
            re: t1.re - b,
            im: t1.im + a,
        }
        .cx_log();
        Cx {
            re: t2.im,
            im: -t2.re,
        }
    }

    fn cx_acos(self) -> Cx {
        let a = self.re;
        let b = self.im;
        let t1 = Cx {
            re: b * b - a * a + 1.0,
            im: -2.0 * a * b,
        }
        .cx_sqrt();
        let t2 = Cx {
            re: t1.re - b,
            im: t1.im + a,
        }
        .cx_log();
        Cx {
            re: std::f64::consts::PI / 2.0 - t2.im,
            im: t2.re,
        }
    }
}

struct Split {
    sign: &'static str,
    coefficients: Vec<u8>,
    exponent: i32,
}

fn split_number(value: f64) -> Split {
    let s = number::to_string(value);
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r.to_owned()),
        None => (false, s),
    };
    let (mant, exp_part) = match rest.split_once('e') {
        Some((m, e)) => (m.to_owned(), Some(e.to_owned())),
        None => (rest, None),
    };
    let mut exponent: i32 = match &exp_part {
        Some(e) => number::parse_float(&e.encode_utf16().collect::<Vec<u16>>()) as i32,
        None => 0,
    };
    let dot = mant.find('.');
    exponent += match dot {
        Some(d) => d as i32 - 1,
        None => mant.len() as i32 - 1,
    };
    let digit_str: String = mant.chars().filter(|&c| c != '.').collect();
    let stripped = digit_str.trim_start_matches('0');
    exponent -= (digit_str.len() - stripped.len()) as i32;
    let stripped = stripped.trim_end_matches('0');
    let mut coefficients: Vec<u8> = stripped.bytes().map(|b| b - b'0').collect();
    if coefficients.is_empty() {
        coefficients.push(0);
        exponent += 1;
    }
    Split {
        sign: if neg { "-" } else { "" },
        coefficients,
        exponent,
    }
}

fn zeros(n: i32) -> Vec<u8> {
    if n <= 0 {
        Vec::new()
    } else {
        vec![0; n as usize]
    }
}

fn round_digits(split: &Split, mut precision: i32) -> Split {
    let mut rounded = Split {
        sign: split.sign,
        coefficients: split.coefficients.clone(),
        exponent: split.exponent,
    };
    while precision <= 0 {
        rounded.coefficients.insert(0, 0);
        rounded.exponent += 1;
        precision += 1;
    }
    let c = &mut rounded.coefficients;
    if c.len() > precision as usize {
        let removed: Vec<u8> = c.split_off(precision as usize);
        if removed[0] >= 5 {
            let mut i = precision - 1;
            c[i as usize] += 1;
            while c[i as usize] == 10 {
                c.pop();
                if i == 0 {
                    c.insert(0, 0);
                    rounded.exponent += 1;
                    i += 1;
                }
                i -= 1;
                c[i as usize] += 1;
            }
        }
    }
    rounded
}

fn digits_string(c: &[u8]) -> String {
    c.iter().map(|&d| (b'0' + d) as char).collect()
}

fn to_fixed(value: f64, precision: i32) -> String {
    if value.is_nan() || !value.is_finite() {
        return number::to_string(value);
    }
    let split = split_number(value);
    let rounded = round_digits(&split, split.exponent + 1 + precision);
    let mut c = rounded.coefficients.clone();
    let mut p = rounded.exponent + 1;
    let pp = p + precision;
    if (c.len() as i32) < pp {
        c.extend(zeros(pp - c.len() as i32));
    }
    if p < 0 {
        let mut z = zeros(-p + 1);
        z.extend(c);
        c = z;
        p = 1;
    }
    let mut body = String::new();
    for (i, &d) in c.iter().enumerate() {
        if i as i32 == p && p < c.len() as i32 {
            if p == 0 {
                body.push_str("0.");
            } else {
                body.push('.');
            }
        }
        body.push((b'0' + d) as char);
    }
    format!("{}{}", rounded.sign, body)
}

fn to_exponential(value: f64, precision: Option<i32>) -> String {
    if value.is_nan() || !value.is_finite() {
        return number::to_string(value);
    }
    let split = split_number(value);
    let rounded = match precision {
        Some(p) => round_digits(&split, p),
        None => split,
    };
    let mut c = rounded.coefficients.clone();
    let e = rounded.exponent;
    if let Some(p) = precision.filter(|&p| (c.len() as i32) < p) {
        c.extend(zeros(p - c.len() as i32));
    }
    let first = c.remove(0);
    let rest = digits_string(&c);
    format!(
        "{}{}{}e{}{}",
        rounded.sign,
        (b'0' + first) as char,
        if c.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        },
        if e >= 0 { "+" } else { "" },
        e
    )
}

fn to_precision(value: f64, precision: Option<i32>, lower_exp: f64, upper_exp: f64) -> String {
    if value.is_nan() || !value.is_finite() {
        return number::to_string(value);
    }
    let split = split_number(value);
    let rounded = match precision {
        Some(p) => round_digits(&split, split.exponent + 1 + p),
        None => split,
    };
    if (rounded.exponent as f64) < lower_exp || rounded.exponent as f64 >= upper_exp {
        to_exponential(value, precision)
    } else {
        let mut c = rounded.coefficients.clone();
        let e = rounded.exponent;
        if let Some(p) = precision.filter(|&p| (c.len() as i32) < p) {
            c.extend(zeros(p - c.len() as i32));
        }
        let extra = match precision {
            Some(p) if (c.len() as i32) < p => p - c.len() as i32,
            _ => 0,
        };
        c.extend(zeros(e - c.len() as i32 + 1 + extra));
        let mut prefixed = zeros(-e);
        prefixed.extend(c);
        c = prefixed;
        let dot = if e > 0 { e } else { 0 };
        let mut body = String::new();
        for (i, &d) in c.iter().enumerate() {
            if i as i32 == dot + 1 && dot < c.len() as i32 - 1 {
                body.push('.');
            }
            body.push((b'0' + d) as char);
        }
        format!("{}{}", rounded.sign, body)
    }
}

fn trailing_zero_replace(s: &str) -> String {
    let b = s.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'.' {
            let mut j = i + 1;
            while j < b.len() {
                if b[j] == b'0' {
                    let mut k = j;
                    while k < b.len() && b[k] == b'0' {
                        k += 1;
                    }
                    if k == b.len() || b[k] == b'e' {
                        let digits = &s[i..j];
                        let e_part = if k < b.len() { "e" } else { "" };
                        let replacement = if digits == "." {
                            e_part.to_owned()
                        } else {
                            format!("{digits}{e_part}")
                        };
                        let consumed = if k < b.len() { k + 1 } else { k };
                        return format!("{}{}{}", &s[..i], replacement, &s[consumed..]);
                    }
                    j += 1;
                } else if b[j].is_ascii_digit() {
                    j += 1;
                } else {
                    break;
                }
            }
        }
    }
    s.to_owned()
}

fn format_number(x: f64) -> String {
    trailing_zero_replace(&to_precision(x, None, -3.0, 5.0))
}

fn complex_to_string(re: f64, im: f64) -> String {
    let (mut a, mut b) = (re, im);
    if a.is_nan() || b.is_nan() {
        return "NaN".to_owned();
    }
    if !(a.is_finite() && b.is_finite()) {
        return "Infinity".to_owned();
    }
    if a.abs() < COMPLEX_EPSILON {
        a = 0.0;
    }
    if b.abs() < COMPLEX_EPSILON {
        b = 0.0;
    }
    if b == 0.0 {
        return number::to_string(a);
    }
    let mut ret = String::new();
    if a != 0.0 {
        ret += &number::to_string(a);
        ret.push(' ');
        if b < 0.0 {
            b = -b;
            ret.push('-');
        } else {
            ret.push('+');
        }
        ret.push(' ');
    } else if b < 0.0 {
        b = -b;
        ret.push('-');
    }
    if b != 1.0 {
        ret += &number::to_string(b);
    }
    ret.push('i');
    ret
}

fn join_to_string(v: &Value) -> JsString {
    match v {
        Value::Null | Value::Undefined => JsString::new(),
        Value::Number(x) => js(&number::to_string(*x)),
        Value::String(s) => s.clone(),
        Value::Boolean(b) => js(if *b { "true" } else { "false" }),
        Value::ResultSet(entries) => result_set_string(entries),
        Value::Complex { re, im } => js(&complex_to_string(*re, *im)),
        Value::Function(_) => js("function () {}"),
    }
}

fn result_set_string(entries: &[Value]) -> JsString {
    let mut out = js("[");
    for (i, e) in entries.iter().enumerate() {
        if i > 0 {
            out.push_str(&js(", "));
        }
        out.push_str(&join_to_string(e));
    }
    out.push_str(&js("]"));
    out
}

fn complex_format(re: f64, im: f64) -> String {
    let str_re = format_number(re);
    let str_im = format_number(im);
    if im == 0.0 {
        str_re
    } else if re == 0.0 {
        if im == 1.0 {
            "i".to_owned()
        } else if im == -1.0 {
            "-i".to_owned()
        } else {
            format!("{str_im}i")
        }
    } else if im < 0.0 {
        if im == -1.0 {
            format!("{str_re} - i")
        } else {
            format!("{} - {}i", str_re, &str_im[1..])
        }
    } else if im == 1.0 {
        format!("{str_re} + i")
    } else {
        format!("{str_re} + {str_im}i")
    }
}

fn _format(value: &Value) -> JsString {
    match value {
        Value::Number(x) => js(&format_number(*x)),
        Value::String(s) => {
            let mut u = vec!['"' as u16];
            u.extend_from_slice(s.units());
            u.push('"' as u16);
            JsString::from_units(u)
        }
        Value::Boolean(b) => js(if *b { "true" } else { "false" }),
        Value::Null => js("null"),
        Value::Undefined => js("undefined"),
        Value::Function(_) => js("function"),
        Value::ResultSet(entries) => result_set_string(entries),
        Value::Complex { re, im } => js(&complex_format(*re, *im)),
    }
}

pub fn format(value: &Value) -> JsString {
    let result = _format(value);
    if result.len() > TRUNCATE {
        let mut u = result.units()[..TRUNCATE - 3].to_vec();
        u.extend_from_slice(
            "..."
                .as_bytes()
                .iter()
                .map(|&b| b as u16)
                .collect::<Vec<u16>>()
                .as_slice(),
        );
        JsString::from_units(u)
    } else {
        result
    }
}
