use asrs_js::json;
use asrs_js::math;
use asrs_js::number;
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

// ---------------------------------------------------------------------------
// typed-function dispatch
// ---------------------------------------------------------------------------

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

fn is_identifier(u: &[u16]) -> bool {
    let mut cps: Vec<u32> = Vec::with_capacity(u.len());
    let mut i = 0;
    while i < u.len() {
        let w = u[i] as u32;
        if (0xd800..0xdc00).contains(&w)
            && i + 1 < u.len()
            && (0xdc00..0xe000).contains(&(u[i + 1] as u32))
        {
            cps.push(0x10000 + ((w - 0xd800) << 10) + (u[i + 1] as u32 - 0xdc00));
            i += 2;
        } else {
            cps.push(w);
            i += 1;
        }
    }
    if cps.is_empty() {
        return false;
    }
    in_ranges(IDENT_START, cps[0]) && cps[1..].iter().all(|&cp| in_ranges(IDENT_CONT, cp))
}

fn in_ranges(ranges: &[(u32, u32)], cp: u32) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| {
            if cp < lo {
                std::cmp::Ordering::Greater
            } else if cp > hi {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
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

// ---------------------------------------------------------------------------
// conversions
// ---------------------------------------------------------------------------

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
        ("boolean", "string") => match arg {
            Value::Boolean(b) => Ok(Value::String(js(if *b { "true" } else { "false" }))),
            _ => Err(unsupported()),
        },
        ("number", "Complex") => match arg {
            Value::Number(x) => Ok(Value::Complex { re: *x, im: 0.0 }),
            _ => Err(unsupported()),
        },
        ("string", "Complex") => {
            let Value::String(s) = arg else {
                return Err(unsupported());
            };
            match parse_complex(s.units()) {
                Some((re, im)) => Ok(Value::Complex { re, im }),
                None => {
                    let mut u = "Cannot convert \"".encode_utf16().collect::<Vec<u16>>();
                    u.extend_from_slice(s.units());
                    u.extend_from_slice(&"\" to Complex".encode_utf16().collect::<Vec<u16>>());
                    Err(MathError {
                        name: "Error",
                        message: JsString::from_units(u),
                    })
                }
            }
        }
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

// ---------------------------------------------------------------------------
// implementations
// ---------------------------------------------------------------------------

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

fn cbrt_complex(z: Cx, all_roots: bool) -> Result<Value, MathError> {
    if all_roots {
        return Err(unsupported());
    }
    let arg3 = math::atan2(z.im, z.re) / 3.0;
    let abs = cx_hypot(z.re, z.im);
    let principal = Cx {
        re: math::cbrt(abs),
        im: 0.0,
    }
    .cx_mul(Cx { re: 0.0, im: arg3 }.cx_exp());
    Ok(value_cx(principal))
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

fn number_from_string(s: &JsString) -> Result<Value, MathError> {
    let u = s.units();
    if string_is(u, "NaN") {
        return Ok(value_num(f64::NAN));
    }
    if let Some((radix, int_part, frac_part)) = non_decimal_parts(u) {
        let mut result = number::parse_int(&int_part, radix as f64);
        for (i, &d) in frac_part.iter().enumerate() {
            let dv = number::parse_int(&[d], radix as f64);
            result += dv / math::pow(radix as f64, i as f64 + 1.0);
        }
        if result.is_nan() {
            let mut m = "String \"".encode_utf16().collect::<Vec<u16>>();
            m.extend_from_slice(u);
            m.extend_from_slice(&"\" is no valid number".encode_utf16().collect::<Vec<u16>>());
            return Err(MathError {
                name: "SyntaxError",
                message: JsString::from_units(m),
            });
        }
        return Ok(value_num(result));
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
    Ok(value_num(n))
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
        ("cbrt", "Complex,boolean") => {
            let Value::Complex { re, im } = args[0] else {
                return Err(unsupported());
            };
            let Value::Boolean(all) = args[1] else {
                return Err(unsupported());
            };
            cbrt_complex(Cx { re, im }, all)
        }
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
            number_from_string(s)
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

// ---------------------------------------------------------------------------
// complex.js
// ---------------------------------------------------------------------------

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

    fn cx_mul(self, z: Cx) -> Cx {
        if (self.is_infinite() && z.is_zero()) || (self.is_zero() && z.is_infinite()) {
            return cx_nan();
        }
        if self.is_infinite() || z.is_infinite() {
            return cx_infinity();
        }
        if z.im == 0.0 && self.im == 0.0 {
            return Cx {
                re: self.re * z.re,
                im: 0.0,
            };
        }
        Cx {
            re: self.re * z.re - self.im * z.im,
            im: self.re * z.im + self.im * z.re,
        }
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

    fn cx_exp(self) -> Cx {
        let tmp = math::exp(self.re);
        Cx {
            re: tmp * math::cos(self.im),
            im: tmp * math::sin(self.im),
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

// complex.js string parser for the string->Complex conversion.
fn parse_complex(u: &[u16]) -> Option<(f64, f64)> {
    let is_digit = |c: u16| u8::try_from(c).is_ok_and(|b| b.is_ascii_digit());
    let mut tokens: Vec<Vec<u16>> = Vec::new();
    let mut i = 0;
    while i < u.len() {
        // \d+\.?\d*e[+-]?\d+ | \d+\.?\d* | \.\d+ | .
        let mut matched: Option<usize> = None;
        if is_digit(u[i]) {
            let mut j = i;
            while j < u.len() && is_digit(u[j]) {
                j += 1;
            }
            if j < u.len() && u[j] == '.' as u16 {
                let mut k = j + 1;
                while k < u.len() && is_digit(u[k]) {
                    k += 1;
                }
                j = k;
            }
            if j < u.len() && u[j] == 'e' as u16 {
                let mut k = j + 1;
                if k < u.len() && (u[k] == '+' as u16 || u[k] == '-' as u16) {
                    k += 1;
                }
                let d0 = k;
                while k < u.len() && is_digit(u[k]) {
                    k += 1;
                }
                if k > d0 {
                    matched = Some(k);
                }
            }
            if matched.is_none() {
                matched = Some(j);
            }
        } else if u[i] == '.' as u16 {
            let mut j = i + 1;
            while j < u.len() && is_digit(u[j]) {
                j += 1;
            }
            if j > i + 1 {
                matched = Some(j);
            }
        }
        match matched {
            Some(end) => {
                tokens.push(u[i..end].to_vec());
                i = end;
            }
            None => {
                // . in JS regex: any unit except line terminators
                if !matches!(u[i], 0x0a | 0x0d | 0x2028 | 0x2029) {
                    tokens.push(vec![u[i]]);
                }
                i += 1;
            }
        }
    }
    if tokens.is_empty() {
        return None;
    }
    let mut re = 0.0f64;
    let mut im = 0.0f64;
    let mut plus = 1i32;
    let mut minus = 0i32;
    let mut i = 0;
    while i < tokens.len() {
        let c = &tokens[i];
        if string_is(c, " ") || string_is(c, "\t") || string_is(c, "\n") {
            // void
        } else if string_is(c, "+") {
            plus += 1;
        } else if string_is(c, "-") {
            minus += 1;
        } else if string_is(c, "i") || string_is(c, "I") {
            if plus + minus == 0 {
                return None;
            }
            let next = tokens.get(i + 1).map(|t| t.as_slice());
            let (next_is_space, next_nan) = match next {
                Some(t) => (string_is(t, " "), number::to_number(t).is_nan()),
                None => (false, true),
            };
            if !next_is_space && !next_nan {
                let mut v = if minus % 2 != 0 {
                    "-".encode_utf16().collect::<Vec<u16>>()
                } else {
                    vec![]
                };
                v.extend_from_slice(next.unwrap_or(&[]));
                im += number::parse_float(&v);
                i += 1;
            } else {
                let v: Vec<u16> = if minus % 2 != 0 {
                    "-1".encode_utf16().collect()
                } else {
                    "1".encode_utf16().collect()
                };
                im += number::parse_float(&v);
            }
            plus = 0;
            minus = 0;
        } else {
            if plus + minus == 0 || number::to_number(c).is_nan() {
                return None;
            }
            let next = tokens.get(i + 1).map(|t| t.as_slice()).unwrap_or(&[]);
            let mut v = if minus % 2 != 0 {
                "-".encode_utf16().collect::<Vec<u16>>()
            } else {
                vec![]
            };
            v.extend_from_slice(c);
            if string_is(next, "i") || string_is(next, "I") {
                im += number::parse_float(&v);
                i += 1;
            } else {
                re += number::parse_float(&v);
            }
            plus = 0;
            minus = 0;
        }
        i += 1;
    }
    if plus + minus > 0 {
        return None;
    }
    Some((re, im))
}

// ---------------------------------------------------------------------------
// format
// ---------------------------------------------------------------------------

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

const IDENT_START: &[(u32, u32)] = &[
    (0x41, 0x5a),
    (0x61, 0x7a),
    (0xaa, 0xaa),
    (0xb5, 0xb5),
    (0xba, 0xba),
    (0xc0, 0xd6),
    (0xd8, 0xf6),
    (0xf8, 0x2c1),
    (0x2c6, 0x2d1),
    (0x2e0, 0x2e4),
    (0x2ec, 0x2ec),
    (0x2ee, 0x2ee),
    (0x370, 0x374),
    (0x376, 0x377),
    (0x37a, 0x37d),
    (0x37f, 0x37f),
    (0x386, 0x386),
    (0x388, 0x38a),
    (0x38c, 0x38c),
    (0x38e, 0x3a1),
    (0x3a3, 0x3f5),
    (0x3f7, 0x481),
    (0x48a, 0x52f),
    (0x531, 0x556),
    (0x559, 0x559),
    (0x560, 0x588),
    (0x5d0, 0x5ea),
    (0x5ef, 0x5f2),
    (0x620, 0x64a),
    (0x66e, 0x66f),
    (0x671, 0x6d3),
    (0x6d5, 0x6d5),
    (0x6e5, 0x6e6),
    (0x6ee, 0x6ef),
    (0x6fa, 0x6fc),
    (0x6ff, 0x6ff),
    (0x710, 0x710),
    (0x712, 0x72f),
    (0x74d, 0x7a5),
    (0x7b1, 0x7b1),
    (0x7ca, 0x7ea),
    (0x7f4, 0x7f5),
    (0x7fa, 0x7fa),
    (0x800, 0x815),
    (0x81a, 0x81a),
    (0x824, 0x824),
    (0x828, 0x828),
    (0x840, 0x858),
    (0x860, 0x86a),
    (0x870, 0x887),
    (0x889, 0x88e),
    (0x8a0, 0x8c9),
    (0x904, 0x939),
    (0x93d, 0x93d),
    (0x950, 0x950),
    (0x958, 0x961),
    (0x971, 0x980),
    (0x985, 0x98c),
    (0x98f, 0x990),
    (0x993, 0x9a8),
    (0x9aa, 0x9b0),
    (0x9b2, 0x9b2),
    (0x9b6, 0x9b9),
    (0x9bd, 0x9bd),
    (0x9ce, 0x9ce),
    (0x9dc, 0x9dd),
    (0x9df, 0x9e1),
    (0x9f0, 0x9f1),
    (0x9fc, 0x9fc),
    (0xa05, 0xa0a),
    (0xa0f, 0xa10),
    (0xa13, 0xa28),
    (0xa2a, 0xa30),
    (0xa32, 0xa33),
    (0xa35, 0xa36),
    (0xa38, 0xa39),
    (0xa59, 0xa5c),
    (0xa5e, 0xa5e),
    (0xa72, 0xa74),
    (0xa85, 0xa8d),
    (0xa8f, 0xa91),
    (0xa93, 0xaa8),
    (0xaaa, 0xab0),
    (0xab2, 0xab3),
    (0xab5, 0xab9),
    (0xabd, 0xabd),
    (0xad0, 0xad0),
    (0xae0, 0xae1),
    (0xaf9, 0xaf9),
    (0xb05, 0xb0c),
    (0xb0f, 0xb10),
    (0xb13, 0xb28),
    (0xb2a, 0xb30),
    (0xb32, 0xb33),
    (0xb35, 0xb39),
    (0xb3d, 0xb3d),
    (0xb5c, 0xb5d),
    (0xb5f, 0xb61),
    (0xb71, 0xb71),
    (0xb83, 0xb83),
    (0xb85, 0xb8a),
    (0xb8e, 0xb90),
    (0xb92, 0xb95),
    (0xb99, 0xb9a),
    (0xb9c, 0xb9c),
    (0xb9e, 0xb9f),
    (0xba3, 0xba4),
    (0xba8, 0xbaa),
    (0xbae, 0xbb9),
    (0xbd0, 0xbd0),
    (0xc05, 0xc0c),
    (0xc0e, 0xc10),
    (0xc12, 0xc28),
    (0xc2a, 0xc39),
    (0xc3d, 0xc3d),
    (0xc58, 0xc5a),
    (0xc5d, 0xc5d),
    (0xc60, 0xc61),
    (0xc80, 0xc80),
    (0xc85, 0xc8c),
    (0xc8e, 0xc90),
    (0xc92, 0xca8),
    (0xcaa, 0xcb3),
    (0xcb5, 0xcb9),
    (0xcbd, 0xcbd),
    (0xcdd, 0xcde),
    (0xce0, 0xce1),
    (0xcf1, 0xcf2),
    (0xd04, 0xd0c),
    (0xd0e, 0xd10),
    (0xd12, 0xd3a),
    (0xd3d, 0xd3d),
    (0xd4e, 0xd4e),
    (0xd54, 0xd56),
    (0xd5f, 0xd61),
    (0xd7a, 0xd7f),
    (0xd85, 0xd96),
    (0xd9a, 0xdb1),
    (0xdb3, 0xdbb),
    (0xdbd, 0xdbd),
    (0xdc0, 0xdc6),
    (0xe01, 0xe30),
    (0xe32, 0xe33),
    (0xe40, 0xe46),
    (0xe81, 0xe82),
    (0xe84, 0xe84),
    (0xe86, 0xe8a),
    (0xe8c, 0xea3),
    (0xea5, 0xea5),
    (0xea7, 0xeb0),
    (0xeb2, 0xeb3),
    (0xebd, 0xebd),
    (0xec0, 0xec4),
    (0xec6, 0xec6),
    (0xedc, 0xedf),
    (0xf00, 0xf00),
    (0xf40, 0xf47),
    (0xf49, 0xf6c),
    (0xf88, 0xf8c),
    (0x1000, 0x102a),
    (0x103f, 0x103f),
    (0x1050, 0x1055),
    (0x105a, 0x105d),
    (0x1061, 0x1061),
    (0x1065, 0x1066),
    (0x106e, 0x1070),
    (0x1075, 0x1081),
    (0x108e, 0x108e),
    (0x10a0, 0x10c5),
    (0x10c7, 0x10c7),
    (0x10cd, 0x10cd),
    (0x10d0, 0x10fa),
    (0x10fc, 0x1248),
    (0x124a, 0x124d),
    (0x1250, 0x1256),
    (0x1258, 0x1258),
    (0x125a, 0x125d),
    (0x1260, 0x1288),
    (0x128a, 0x128d),
    (0x1290, 0x12b0),
    (0x12b2, 0x12b5),
    (0x12b8, 0x12be),
    (0x12c0, 0x12c0),
    (0x12c2, 0x12c5),
    (0x12c8, 0x12d6),
    (0x12d8, 0x1310),
    (0x1312, 0x1315),
    (0x1318, 0x135a),
    (0x1380, 0x138f),
    (0x13a0, 0x13f5),
    (0x13f8, 0x13fd),
    (0x1401, 0x166c),
    (0x166f, 0x167f),
    (0x1681, 0x169a),
    (0x16a0, 0x16ea),
    (0x16f1, 0x16f8),
    (0x1700, 0x1711),
    (0x171f, 0x1731),
    (0x1740, 0x1751),
    (0x1760, 0x176c),
    (0x176e, 0x1770),
    (0x1780, 0x17b3),
    (0x17d7, 0x17d7),
    (0x17dc, 0x17dc),
    (0x1820, 0x1878),
    (0x1880, 0x1884),
    (0x1887, 0x18a8),
    (0x18aa, 0x18aa),
    (0x18b0, 0x18f5),
    (0x1900, 0x191e),
    (0x1950, 0x196d),
    (0x1970, 0x1974),
    (0x1980, 0x19ab),
    (0x19b0, 0x19c9),
    (0x1a00, 0x1a16),
    (0x1a20, 0x1a54),
    (0x1aa7, 0x1aa7),
    (0x1b05, 0x1b33),
    (0x1b45, 0x1b4c),
    (0x1b83, 0x1ba0),
    (0x1bae, 0x1baf),
    (0x1bba, 0x1be5),
    (0x1c00, 0x1c23),
    (0x1c4d, 0x1c4f),
    (0x1c5a, 0x1c7d),
    (0x1c80, 0x1c88),
    (0x1c90, 0x1cba),
    (0x1cbd, 0x1cbf),
    (0x1ce9, 0x1cec),
    (0x1cee, 0x1cf3),
    (0x1cf5, 0x1cf6),
    (0x1cfa, 0x1cfa),
    (0x1d00, 0x1dbf),
    (0x1e00, 0x1f15),
    (0x1f18, 0x1f1d),
    (0x1f20, 0x1f45),
    (0x1f48, 0x1f4d),
    (0x1f50, 0x1f57),
    (0x1f59, 0x1f59),
    (0x1f5b, 0x1f5b),
    (0x1f5d, 0x1f5d),
    (0x1f5f, 0x1f7d),
    (0x1f80, 0x1fb4),
    (0x1fb6, 0x1fbc),
    (0x1fbe, 0x1fbe),
    (0x1fc2, 0x1fc4),
    (0x1fc6, 0x1fcc),
    (0x1fd0, 0x1fd3),
    (0x1fd6, 0x1fdb),
    (0x1fe0, 0x1fec),
    (0x1ff2, 0x1ff4),
    (0x1ff6, 0x1ffc),
    (0x2071, 0x2071),
    (0x207f, 0x207f),
    (0x2090, 0x209c),
    (0x2102, 0x2102),
    (0x2107, 0x2107),
    (0x210a, 0x2113),
    (0x2115, 0x2115),
    (0x2119, 0x211d),
    (0x2124, 0x2124),
    (0x2126, 0x2126),
    (0x2128, 0x2128),
    (0x212a, 0x212d),
    (0x212f, 0x2139),
    (0x213c, 0x213f),
    (0x2145, 0x2149),
    (0x214e, 0x214e),
    (0x2183, 0x2184),
    (0x2c00, 0x2ce4),
    (0x2ceb, 0x2cee),
    (0x2cf2, 0x2cf3),
    (0x2d00, 0x2d25),
    (0x2d27, 0x2d27),
    (0x2d2d, 0x2d2d),
    (0x2d30, 0x2d67),
    (0x2d6f, 0x2d6f),
    (0x2d80, 0x2d96),
    (0x2da0, 0x2da6),
    (0x2da8, 0x2dae),
    (0x2db0, 0x2db6),
    (0x2db8, 0x2dbe),
    (0x2dc0, 0x2dc6),
    (0x2dc8, 0x2dce),
    (0x2dd0, 0x2dd6),
    (0x2dd8, 0x2dde),
    (0x2e2f, 0x2e2f),
    (0x3005, 0x3006),
    (0x3031, 0x3035),
    (0x303b, 0x303c),
    (0x3041, 0x3096),
    (0x309d, 0x309f),
    (0x30a1, 0x30fa),
    (0x30fc, 0x30ff),
    (0x3105, 0x312f),
    (0x3131, 0x318e),
    (0x31a0, 0x31bf),
    (0x31f0, 0x31ff),
    (0x3400, 0x4dbf),
    (0x4e00, 0xa48c),
    (0xa4d0, 0xa4fd),
    (0xa500, 0xa60c),
    (0xa610, 0xa61f),
    (0xa62a, 0xa62b),
    (0xa640, 0xa66e),
    (0xa67f, 0xa69d),
    (0xa6a0, 0xa6e5),
    (0xa717, 0xa71f),
    (0xa722, 0xa788),
    (0xa78b, 0xa7ca),
    (0xa7d0, 0xa7d1),
    (0xa7d3, 0xa7d3),
    (0xa7d5, 0xa7d9),
    (0xa7f2, 0xa801),
    (0xa803, 0xa805),
    (0xa807, 0xa80a),
    (0xa80c, 0xa822),
    (0xa840, 0xa873),
    (0xa882, 0xa8b3),
    (0xa8f2, 0xa8f7),
    (0xa8fb, 0xa8fb),
    (0xa8fd, 0xa8fe),
    (0xa90a, 0xa925),
    (0xa930, 0xa946),
    (0xa960, 0xa97c),
    (0xa984, 0xa9b2),
    (0xa9cf, 0xa9cf),
    (0xa9e0, 0xa9e4),
    (0xa9e6, 0xa9ef),
    (0xa9fa, 0xa9fe),
    (0xaa00, 0xaa28),
    (0xaa40, 0xaa42),
    (0xaa44, 0xaa4b),
    (0xaa60, 0xaa76),
    (0xaa7a, 0xaa7a),
    (0xaa7e, 0xaaaf),
    (0xaab1, 0xaab1),
    (0xaab5, 0xaab6),
    (0xaab9, 0xaabd),
    (0xaac0, 0xaac0),
    (0xaac2, 0xaac2),
    (0xaadb, 0xaadd),
    (0xaae0, 0xaaea),
    (0xaaf2, 0xaaf4),
    (0xab01, 0xab06),
    (0xab09, 0xab0e),
    (0xab11, 0xab16),
    (0xab20, 0xab26),
    (0xab28, 0xab2e),
    (0xab30, 0xab5a),
    (0xab5c, 0xab69),
    (0xab70, 0xabe2),
    (0xac00, 0xd7a3),
    (0xd7b0, 0xd7c6),
    (0xd7cb, 0xd7fb),
    (0xf900, 0xfa6d),
    (0xfa70, 0xfad9),
    (0xfb00, 0xfb06),
    (0xfb13, 0xfb17),
    (0xfb1d, 0xfb1d),
    (0xfb1f, 0xfb28),
    (0xfb2a, 0xfb36),
    (0xfb38, 0xfb3c),
    (0xfb3e, 0xfb3e),
    (0xfb40, 0xfb41),
    (0xfb43, 0xfb44),
    (0xfb46, 0xfbb1),
    (0xfbd3, 0xfd3d),
    (0xfd50, 0xfd8f),
    (0xfd92, 0xfdc7),
    (0xfdf0, 0xfdfb),
    (0xfe70, 0xfe74),
    (0xfe76, 0xfefc),
    (0xff21, 0xff3a),
    (0xff41, 0xff5a),
    (0xff66, 0xffbe),
    (0xffc2, 0xffc7),
    (0xffca, 0xffcf),
    (0xffd2, 0xffd7),
    (0xffda, 0xffdc),
    (0x10000, 0x1000b),
    (0x1000d, 0x10026),
    (0x10028, 0x1003a),
    (0x1003c, 0x1003d),
    (0x1003f, 0x1004d),
    (0x10050, 0x1005d),
    (0x10080, 0x100fa),
    (0x10280, 0x1029c),
    (0x102a0, 0x102d0),
    (0x10300, 0x1031f),
    (0x1032d, 0x10340),
    (0x10342, 0x10349),
    (0x10350, 0x10375),
    (0x10380, 0x1039d),
    (0x103a0, 0x103c3),
    (0x103c8, 0x103cf),
    (0x10400, 0x1049d),
    (0x104b0, 0x104d3),
    (0x104d8, 0x104fb),
    (0x10500, 0x10527),
    (0x10530, 0x10563),
    (0x10570, 0x1057a),
    (0x1057c, 0x1058a),
    (0x1058c, 0x10592),
    (0x10594, 0x10595),
    (0x10597, 0x105a1),
    (0x105a3, 0x105b1),
    (0x105b3, 0x105b9),
    (0x105bb, 0x105bc),
    (0x10600, 0x10736),
    (0x10740, 0x10755),
    (0x10760, 0x10767),
    (0x10780, 0x10785),
    (0x10787, 0x107b0),
    (0x107b2, 0x107ba),
    (0x10800, 0x10805),
    (0x10808, 0x10808),
    (0x1080a, 0x10835),
    (0x10837, 0x10838),
    (0x1083c, 0x1083c),
    (0x1083f, 0x10855),
    (0x10860, 0x10876),
    (0x10880, 0x1089e),
    (0x108e0, 0x108f2),
    (0x108f4, 0x108f5),
    (0x10900, 0x10915),
    (0x10920, 0x10939),
    (0x10980, 0x109b7),
    (0x109be, 0x109bf),
    (0x10a00, 0x10a00),
    (0x10a10, 0x10a13),
    (0x10a15, 0x10a17),
    (0x10a19, 0x10a35),
    (0x10a60, 0x10a7c),
    (0x10a80, 0x10a9c),
    (0x10ac0, 0x10ac7),
    (0x10ac9, 0x10ae4),
    (0x10b00, 0x10b35),
    (0x10b40, 0x10b55),
    (0x10b60, 0x10b72),
    (0x10b80, 0x10b91),
    (0x10c00, 0x10c48),
    (0x10c80, 0x10cb2),
    (0x10cc0, 0x10cf2),
    (0x10d00, 0x10d23),
    (0x10e80, 0x10ea9),
    (0x10eb0, 0x10eb1),
    (0x10f00, 0x10f1c),
    (0x10f27, 0x10f27),
    (0x10f30, 0x10f45),
    (0x10f70, 0x10f81),
    (0x10fb0, 0x10fc4),
    (0x10fe0, 0x10ff6),
    (0x11003, 0x11037),
    (0x11071, 0x11072),
    (0x11075, 0x11075),
    (0x11083, 0x110af),
    (0x110d0, 0x110e8),
    (0x11103, 0x11126),
    (0x11144, 0x11144),
    (0x11147, 0x11147),
    (0x11150, 0x11172),
    (0x11176, 0x11176),
    (0x11183, 0x111b2),
    (0x111c1, 0x111c4),
    (0x111da, 0x111da),
    (0x111dc, 0x111dc),
    (0x11200, 0x11211),
    (0x11213, 0x1122b),
    (0x11280, 0x11286),
    (0x11288, 0x11288),
    (0x1128a, 0x1128d),
    (0x1128f, 0x1129d),
    (0x1129f, 0x112a8),
    (0x112b0, 0x112de),
    (0x11305, 0x1130c),
    (0x1130f, 0x11310),
    (0x11313, 0x11328),
    (0x1132a, 0x11330),
    (0x11332, 0x11333),
    (0x11335, 0x11339),
    (0x1133d, 0x1133d),
    (0x11350, 0x11350),
    (0x1135d, 0x11361),
    (0x11400, 0x11434),
    (0x11447, 0x1144a),
    (0x1145f, 0x11461),
    (0x11480, 0x114af),
    (0x114c4, 0x114c5),
    (0x114c7, 0x114c7),
    (0x11580, 0x115ae),
    (0x115d8, 0x115db),
    (0x11600, 0x1162f),
    (0x11644, 0x11644),
    (0x11680, 0x116aa),
    (0x116b8, 0x116b8),
    (0x11700, 0x1171a),
    (0x11740, 0x11746),
    (0x11800, 0x1182b),
    (0x118a0, 0x118df),
    (0x118ff, 0x11906),
    (0x11909, 0x11909),
    (0x1190c, 0x11913),
    (0x11915, 0x11916),
    (0x11918, 0x1192f),
    (0x1193f, 0x1193f),
    (0x11941, 0x11941),
    (0x119a0, 0x119a7),
    (0x119aa, 0x119d0),
    (0x119e1, 0x119e1),
    (0x119e3, 0x119e3),
    (0x11a00, 0x11a00),
    (0x11a0b, 0x11a32),
    (0x11a3a, 0x11a3a),
    (0x11a50, 0x11a50),
    (0x11a5c, 0x11a89),
    (0x11a9d, 0x11a9d),
    (0x11ab0, 0x11af8),
    (0x11c00, 0x11c08),
    (0x11c0a, 0x11c2e),
    (0x11c40, 0x11c40),
    (0x11c72, 0x11c8f),
    (0x11d00, 0x11d06),
    (0x11d08, 0x11d09),
    (0x11d0b, 0x11d30),
    (0x11d46, 0x11d46),
    (0x11d60, 0x11d65),
    (0x11d67, 0x11d68),
    (0x11d6a, 0x11d89),
    (0x11d98, 0x11d98),
    (0x11ee0, 0x11ef2),
    (0x11fb0, 0x11fb0),
    (0x12000, 0x12399),
    (0x12480, 0x12543),
    (0x12f90, 0x12ff0),
    (0x13000, 0x1342e),
    (0x14400, 0x14646),
    (0x16800, 0x16a38),
    (0x16a40, 0x16a5e),
    (0x16a70, 0x16abe),
    (0x16ad0, 0x16aed),
    (0x16b00, 0x16b2f),
    (0x16b40, 0x16b43),
    (0x16b63, 0x16b77),
    (0x16b7d, 0x16b8f),
    (0x16e40, 0x16e7f),
    (0x16f00, 0x16f4a),
    (0x16f50, 0x16f50),
    (0x16f93, 0x16f9f),
    (0x16fe0, 0x16fe1),
    (0x16fe3, 0x16fe3),
    (0x17000, 0x187f7),
    (0x18800, 0x18cd5),
    (0x18d00, 0x18d08),
    (0x1aff0, 0x1aff3),
    (0x1aff5, 0x1affb),
    (0x1affd, 0x1affe),
    (0x1b000, 0x1b122),
    (0x1b150, 0x1b152),
    (0x1b164, 0x1b167),
    (0x1b170, 0x1b2fb),
    (0x1bc00, 0x1bc6a),
    (0x1bc70, 0x1bc7c),
    (0x1bc80, 0x1bc88),
    (0x1bc90, 0x1bc99),
    (0x1d400, 0x1d454),
    (0x1d456, 0x1d49c),
    (0x1d49e, 0x1d49f),
    (0x1d4a2, 0x1d4a2),
    (0x1d4a5, 0x1d4a6),
    (0x1d4a9, 0x1d4ac),
    (0x1d4ae, 0x1d4b9),
    (0x1d4bb, 0x1d4bb),
    (0x1d4bd, 0x1d4c3),
    (0x1d4c5, 0x1d505),
    (0x1d507, 0x1d50a),
    (0x1d50d, 0x1d514),
    (0x1d516, 0x1d51c),
    (0x1d51e, 0x1d539),
    (0x1d53b, 0x1d53e),
    (0x1d540, 0x1d544),
    (0x1d546, 0x1d546),
    (0x1d54a, 0x1d550),
    (0x1d552, 0x1d6a5),
    (0x1d6a8, 0x1d6c0),
    (0x1d6c2, 0x1d6da),
    (0x1d6dc, 0x1d6fa),
    (0x1d6fc, 0x1d714),
    (0x1d716, 0x1d734),
    (0x1d736, 0x1d74e),
    (0x1d750, 0x1d76e),
    (0x1d770, 0x1d788),
    (0x1d78a, 0x1d7a8),
    (0x1d7aa, 0x1d7c2),
    (0x1d7c4, 0x1d7cb),
    (0x1df00, 0x1df1e),
    (0x1e100, 0x1e12c),
    (0x1e137, 0x1e13d),
    (0x1e14e, 0x1e14e),
    (0x1e290, 0x1e2ad),
    (0x1e2c0, 0x1e2eb),
    (0x1e7e0, 0x1e7e6),
    (0x1e7e8, 0x1e7eb),
    (0x1e7ed, 0x1e7ee),
    (0x1e7f0, 0x1e7fe),
    (0x1e800, 0x1e8c4),
    (0x1e900, 0x1e943),
    (0x1e94b, 0x1e94b),
    (0x1ee00, 0x1ee03),
    (0x1ee05, 0x1ee1f),
    (0x1ee21, 0x1ee22),
    (0x1ee24, 0x1ee24),
    (0x1ee27, 0x1ee27),
    (0x1ee29, 0x1ee32),
    (0x1ee34, 0x1ee37),
    (0x1ee39, 0x1ee39),
    (0x1ee3b, 0x1ee3b),
    (0x1ee42, 0x1ee42),
    (0x1ee47, 0x1ee47),
    (0x1ee49, 0x1ee49),
    (0x1ee4b, 0x1ee4b),
    (0x1ee4d, 0x1ee4f),
    (0x1ee51, 0x1ee52),
    (0x1ee54, 0x1ee54),
    (0x1ee57, 0x1ee57),
    (0x1ee59, 0x1ee59),
    (0x1ee5b, 0x1ee5b),
    (0x1ee5d, 0x1ee5d),
    (0x1ee5f, 0x1ee5f),
    (0x1ee61, 0x1ee62),
    (0x1ee64, 0x1ee64),
    (0x1ee67, 0x1ee6a),
    (0x1ee6c, 0x1ee72),
    (0x1ee74, 0x1ee77),
    (0x1ee79, 0x1ee7c),
    (0x1ee7e, 0x1ee7e),
    (0x1ee80, 0x1ee89),
    (0x1ee8b, 0x1ee9b),
    (0x1eea1, 0x1eea3),
    (0x1eea5, 0x1eea9),
    (0x1eeab, 0x1eebb),
    (0x20000, 0x2a6df),
    (0x2a700, 0x2b738),
    (0x2b740, 0x2b81d),
    (0x2b820, 0x2cea1),
    (0x2ceb0, 0x2ebe0),
    (0x2f800, 0x2fa1d),
    (0x30000, 0x3134a),
];

const IDENT_CONT: &[(u32, u32)] = &[
    (0x30, 0x39),
    (0x41, 0x5a),
    (0x61, 0x7a),
    (0xaa, 0xaa),
    (0xb5, 0xb5),
    (0xba, 0xba),
    (0xc0, 0xd6),
    (0xd8, 0xf6),
    (0xf8, 0x2c1),
    (0x2c6, 0x2d1),
    (0x2e0, 0x2e4),
    (0x2ec, 0x2ec),
    (0x2ee, 0x2ee),
    (0x370, 0x374),
    (0x376, 0x377),
    (0x37a, 0x37d),
    (0x37f, 0x37f),
    (0x386, 0x386),
    (0x388, 0x38a),
    (0x38c, 0x38c),
    (0x38e, 0x3a1),
    (0x3a3, 0x3f5),
    (0x3f7, 0x481),
    (0x48a, 0x52f),
    (0x531, 0x556),
    (0x559, 0x559),
    (0x560, 0x588),
    (0x5d0, 0x5ea),
    (0x5ef, 0x5f2),
    (0x620, 0x64a),
    (0x66e, 0x66f),
    (0x671, 0x6d3),
    (0x6d5, 0x6d5),
    (0x6e5, 0x6e6),
    (0x6ee, 0x6ef),
    (0x6fa, 0x6fc),
    (0x6ff, 0x6ff),
    (0x710, 0x710),
    (0x712, 0x72f),
    (0x74d, 0x7a5),
    (0x7b1, 0x7b1),
    (0x7ca, 0x7ea),
    (0x7f4, 0x7f5),
    (0x7fa, 0x7fa),
    (0x800, 0x815),
    (0x81a, 0x81a),
    (0x824, 0x824),
    (0x828, 0x828),
    (0x840, 0x858),
    (0x860, 0x86a),
    (0x870, 0x887),
    (0x889, 0x88e),
    (0x8a0, 0x8c9),
    (0x904, 0x939),
    (0x93d, 0x93d),
    (0x950, 0x950),
    (0x958, 0x961),
    (0x971, 0x980),
    (0x985, 0x98c),
    (0x98f, 0x990),
    (0x993, 0x9a8),
    (0x9aa, 0x9b0),
    (0x9b2, 0x9b2),
    (0x9b6, 0x9b9),
    (0x9bd, 0x9bd),
    (0x9ce, 0x9ce),
    (0x9dc, 0x9dd),
    (0x9df, 0x9e1),
    (0x9f0, 0x9f1),
    (0x9fc, 0x9fc),
    (0xa05, 0xa0a),
    (0xa0f, 0xa10),
    (0xa13, 0xa28),
    (0xa2a, 0xa30),
    (0xa32, 0xa33),
    (0xa35, 0xa36),
    (0xa38, 0xa39),
    (0xa59, 0xa5c),
    (0xa5e, 0xa5e),
    (0xa72, 0xa74),
    (0xa85, 0xa8d),
    (0xa8f, 0xa91),
    (0xa93, 0xaa8),
    (0xaaa, 0xab0),
    (0xab2, 0xab3),
    (0xab5, 0xab9),
    (0xabd, 0xabd),
    (0xad0, 0xad0),
    (0xae0, 0xae1),
    (0xaf9, 0xaf9),
    (0xb05, 0xb0c),
    (0xb0f, 0xb10),
    (0xb13, 0xb28),
    (0xb2a, 0xb30),
    (0xb32, 0xb33),
    (0xb35, 0xb39),
    (0xb3d, 0xb3d),
    (0xb5c, 0xb5d),
    (0xb5f, 0xb61),
    (0xb71, 0xb71),
    (0xb83, 0xb83),
    (0xb85, 0xb8a),
    (0xb8e, 0xb90),
    (0xb92, 0xb95),
    (0xb99, 0xb9a),
    (0xb9c, 0xb9c),
    (0xb9e, 0xb9f),
    (0xba3, 0xba4),
    (0xba8, 0xbaa),
    (0xbae, 0xbb9),
    (0xbd0, 0xbd0),
    (0xc05, 0xc0c),
    (0xc0e, 0xc10),
    (0xc12, 0xc28),
    (0xc2a, 0xc39),
    (0xc3d, 0xc3d),
    (0xc58, 0xc5a),
    (0xc5d, 0xc5d),
    (0xc60, 0xc61),
    (0xc80, 0xc80),
    (0xc85, 0xc8c),
    (0xc8e, 0xc90),
    (0xc92, 0xca8),
    (0xcaa, 0xcb3),
    (0xcb5, 0xcb9),
    (0xcbd, 0xcbd),
    (0xcdd, 0xcde),
    (0xce0, 0xce1),
    (0xcf1, 0xcf2),
    (0xd04, 0xd0c),
    (0xd0e, 0xd10),
    (0xd12, 0xd3a),
    (0xd3d, 0xd3d),
    (0xd4e, 0xd4e),
    (0xd54, 0xd56),
    (0xd5f, 0xd61),
    (0xd7a, 0xd7f),
    (0xd85, 0xd96),
    (0xd9a, 0xdb1),
    (0xdb3, 0xdbb),
    (0xdbd, 0xdbd),
    (0xdc0, 0xdc6),
    (0xe01, 0xe30),
    (0xe32, 0xe33),
    (0xe40, 0xe46),
    (0xe81, 0xe82),
    (0xe84, 0xe84),
    (0xe86, 0xe8a),
    (0xe8c, 0xea3),
    (0xea5, 0xea5),
    (0xea7, 0xeb0),
    (0xeb2, 0xeb3),
    (0xebd, 0xebd),
    (0xec0, 0xec4),
    (0xec6, 0xec6),
    (0xedc, 0xedf),
    (0xf00, 0xf00),
    (0xf40, 0xf47),
    (0xf49, 0xf6c),
    (0xf88, 0xf8c),
    (0x1000, 0x102a),
    (0x103f, 0x103f),
    (0x1050, 0x1055),
    (0x105a, 0x105d),
    (0x1061, 0x1061),
    (0x1065, 0x1066),
    (0x106e, 0x1070),
    (0x1075, 0x1081),
    (0x108e, 0x108e),
    (0x10a0, 0x10c5),
    (0x10c7, 0x10c7),
    (0x10cd, 0x10cd),
    (0x10d0, 0x10fa),
    (0x10fc, 0x1248),
    (0x124a, 0x124d),
    (0x1250, 0x1256),
    (0x1258, 0x1258),
    (0x125a, 0x125d),
    (0x1260, 0x1288),
    (0x128a, 0x128d),
    (0x1290, 0x12b0),
    (0x12b2, 0x12b5),
    (0x12b8, 0x12be),
    (0x12c0, 0x12c0),
    (0x12c2, 0x12c5),
    (0x12c8, 0x12d6),
    (0x12d8, 0x1310),
    (0x1312, 0x1315),
    (0x1318, 0x135a),
    (0x1380, 0x138f),
    (0x13a0, 0x13f5),
    (0x13f8, 0x13fd),
    (0x1401, 0x166c),
    (0x166f, 0x167f),
    (0x1681, 0x169a),
    (0x16a0, 0x16ea),
    (0x16f1, 0x16f8),
    (0x1700, 0x1711),
    (0x171f, 0x1731),
    (0x1740, 0x1751),
    (0x1760, 0x176c),
    (0x176e, 0x1770),
    (0x1780, 0x17b3),
    (0x17d7, 0x17d7),
    (0x17dc, 0x17dc),
    (0x1820, 0x1878),
    (0x1880, 0x1884),
    (0x1887, 0x18a8),
    (0x18aa, 0x18aa),
    (0x18b0, 0x18f5),
    (0x1900, 0x191e),
    (0x1950, 0x196d),
    (0x1970, 0x1974),
    (0x1980, 0x19ab),
    (0x19b0, 0x19c9),
    (0x1a00, 0x1a16),
    (0x1a20, 0x1a54),
    (0x1aa7, 0x1aa7),
    (0x1b05, 0x1b33),
    (0x1b45, 0x1b4c),
    (0x1b83, 0x1ba0),
    (0x1bae, 0x1baf),
    (0x1bba, 0x1be5),
    (0x1c00, 0x1c23),
    (0x1c4d, 0x1c4f),
    (0x1c5a, 0x1c7d),
    (0x1c80, 0x1c88),
    (0x1c90, 0x1cba),
    (0x1cbd, 0x1cbf),
    (0x1ce9, 0x1cec),
    (0x1cee, 0x1cf3),
    (0x1cf5, 0x1cf6),
    (0x1cfa, 0x1cfa),
    (0x1d00, 0x1dbf),
    (0x1e00, 0x1f15),
    (0x1f18, 0x1f1d),
    (0x1f20, 0x1f45),
    (0x1f48, 0x1f4d),
    (0x1f50, 0x1f57),
    (0x1f59, 0x1f59),
    (0x1f5b, 0x1f5b),
    (0x1f5d, 0x1f5d),
    (0x1f5f, 0x1f7d),
    (0x1f80, 0x1fb4),
    (0x1fb6, 0x1fbc),
    (0x1fbe, 0x1fbe),
    (0x1fc2, 0x1fc4),
    (0x1fc6, 0x1fcc),
    (0x1fd0, 0x1fd3),
    (0x1fd6, 0x1fdb),
    (0x1fe0, 0x1fec),
    (0x1ff2, 0x1ff4),
    (0x1ff6, 0x1ffc),
    (0x2071, 0x2071),
    (0x207f, 0x207f),
    (0x2090, 0x209c),
    (0x2102, 0x2102),
    (0x2107, 0x2107),
    (0x210a, 0x2113),
    (0x2115, 0x2115),
    (0x2119, 0x211d),
    (0x2124, 0x2124),
    (0x2126, 0x2126),
    (0x2128, 0x2128),
    (0x212a, 0x212d),
    (0x212f, 0x2139),
    (0x213c, 0x213f),
    (0x2145, 0x2149),
    (0x214e, 0x214e),
    (0x2183, 0x2184),
    (0x2c00, 0x2ce4),
    (0x2ceb, 0x2cee),
    (0x2cf2, 0x2cf3),
    (0x2d00, 0x2d25),
    (0x2d27, 0x2d27),
    (0x2d2d, 0x2d2d),
    (0x2d30, 0x2d67),
    (0x2d6f, 0x2d6f),
    (0x2d80, 0x2d96),
    (0x2da0, 0x2da6),
    (0x2da8, 0x2dae),
    (0x2db0, 0x2db6),
    (0x2db8, 0x2dbe),
    (0x2dc0, 0x2dc6),
    (0x2dc8, 0x2dce),
    (0x2dd0, 0x2dd6),
    (0x2dd8, 0x2dde),
    (0x2e2f, 0x2e2f),
    (0x3005, 0x3006),
    (0x3031, 0x3035),
    (0x303b, 0x303c),
    (0x3041, 0x3096),
    (0x309d, 0x309f),
    (0x30a1, 0x30fa),
    (0x30fc, 0x30ff),
    (0x3105, 0x312f),
    (0x3131, 0x318e),
    (0x31a0, 0x31bf),
    (0x31f0, 0x31ff),
    (0x3400, 0x4dbf),
    (0x4e00, 0xa48c),
    (0xa4d0, 0xa4fd),
    (0xa500, 0xa60c),
    (0xa610, 0xa61f),
    (0xa62a, 0xa62b),
    (0xa640, 0xa66e),
    (0xa67f, 0xa69d),
    (0xa6a0, 0xa6e5),
    (0xa717, 0xa71f),
    (0xa722, 0xa788),
    (0xa78b, 0xa7ca),
    (0xa7d0, 0xa7d1),
    (0xa7d3, 0xa7d3),
    (0xa7d5, 0xa7d9),
    (0xa7f2, 0xa801),
    (0xa803, 0xa805),
    (0xa807, 0xa80a),
    (0xa80c, 0xa822),
    (0xa840, 0xa873),
    (0xa882, 0xa8b3),
    (0xa8f2, 0xa8f7),
    (0xa8fb, 0xa8fb),
    (0xa8fd, 0xa8fe),
    (0xa90a, 0xa925),
    (0xa930, 0xa946),
    (0xa960, 0xa97c),
    (0xa984, 0xa9b2),
    (0xa9cf, 0xa9cf),
    (0xa9e0, 0xa9e4),
    (0xa9e6, 0xa9ef),
    (0xa9fa, 0xa9fe),
    (0xaa00, 0xaa28),
    (0xaa40, 0xaa42),
    (0xaa44, 0xaa4b),
    (0xaa60, 0xaa76),
    (0xaa7a, 0xaa7a),
    (0xaa7e, 0xaaaf),
    (0xaab1, 0xaab1),
    (0xaab5, 0xaab6),
    (0xaab9, 0xaabd),
    (0xaac0, 0xaac0),
    (0xaac2, 0xaac2),
    (0xaadb, 0xaadd),
    (0xaae0, 0xaaea),
    (0xaaf2, 0xaaf4),
    (0xab01, 0xab06),
    (0xab09, 0xab0e),
    (0xab11, 0xab16),
    (0xab20, 0xab26),
    (0xab28, 0xab2e),
    (0xab30, 0xab5a),
    (0xab5c, 0xab69),
    (0xab70, 0xabe2),
    (0xac00, 0xd7a3),
    (0xd7b0, 0xd7c6),
    (0xd7cb, 0xd7fb),
    (0xf900, 0xfa6d),
    (0xfa70, 0xfad9),
    (0xfb00, 0xfb06),
    (0xfb13, 0xfb17),
    (0xfb1d, 0xfb1d),
    (0xfb1f, 0xfb28),
    (0xfb2a, 0xfb36),
    (0xfb38, 0xfb3c),
    (0xfb3e, 0xfb3e),
    (0xfb40, 0xfb41),
    (0xfb43, 0xfb44),
    (0xfb46, 0xfbb1),
    (0xfbd3, 0xfd3d),
    (0xfd50, 0xfd8f),
    (0xfd92, 0xfdc7),
    (0xfdf0, 0xfdfb),
    (0xfe70, 0xfe74),
    (0xfe76, 0xfefc),
    (0xff21, 0xff3a),
    (0xff41, 0xff5a),
    (0xff66, 0xffbe),
    (0xffc2, 0xffc7),
    (0xffca, 0xffcf),
    (0xffd2, 0xffd7),
    (0xffda, 0xffdc),
    (0x10000, 0x1000b),
    (0x1000d, 0x10026),
    (0x10028, 0x1003a),
    (0x1003c, 0x1003d),
    (0x1003f, 0x1004d),
    (0x10050, 0x1005d),
    (0x10080, 0x100fa),
    (0x10280, 0x1029c),
    (0x102a0, 0x102d0),
    (0x10300, 0x1031f),
    (0x1032d, 0x10340),
    (0x10342, 0x10349),
    (0x10350, 0x10375),
    (0x10380, 0x1039d),
    (0x103a0, 0x103c3),
    (0x103c8, 0x103cf),
    (0x10400, 0x1049d),
    (0x104b0, 0x104d3),
    (0x104d8, 0x104fb),
    (0x10500, 0x10527),
    (0x10530, 0x10563),
    (0x10570, 0x1057a),
    (0x1057c, 0x1058a),
    (0x1058c, 0x10592),
    (0x10594, 0x10595),
    (0x10597, 0x105a1),
    (0x105a3, 0x105b1),
    (0x105b3, 0x105b9),
    (0x105bb, 0x105bc),
    (0x10600, 0x10736),
    (0x10740, 0x10755),
    (0x10760, 0x10767),
    (0x10780, 0x10785),
    (0x10787, 0x107b0),
    (0x107b2, 0x107ba),
    (0x10800, 0x10805),
    (0x10808, 0x10808),
    (0x1080a, 0x10835),
    (0x10837, 0x10838),
    (0x1083c, 0x1083c),
    (0x1083f, 0x10855),
    (0x10860, 0x10876),
    (0x10880, 0x1089e),
    (0x108e0, 0x108f2),
    (0x108f4, 0x108f5),
    (0x10900, 0x10915),
    (0x10920, 0x10939),
    (0x10980, 0x109b7),
    (0x109be, 0x109bf),
    (0x10a00, 0x10a00),
    (0x10a10, 0x10a13),
    (0x10a15, 0x10a17),
    (0x10a19, 0x10a35),
    (0x10a60, 0x10a7c),
    (0x10a80, 0x10a9c),
    (0x10ac0, 0x10ac7),
    (0x10ac9, 0x10ae4),
    (0x10b00, 0x10b35),
    (0x10b40, 0x10b55),
    (0x10b60, 0x10b72),
    (0x10b80, 0x10b91),
    (0x10c00, 0x10c48),
    (0x10c80, 0x10cb2),
    (0x10cc0, 0x10cf2),
    (0x10d00, 0x10d23),
    (0x10e80, 0x10ea9),
    (0x10eb0, 0x10eb1),
    (0x10f00, 0x10f1c),
    (0x10f27, 0x10f27),
    (0x10f30, 0x10f45),
    (0x10f70, 0x10f81),
    (0x10fb0, 0x10fc4),
    (0x10fe0, 0x10ff6),
    (0x11003, 0x11037),
    (0x11071, 0x11072),
    (0x11075, 0x11075),
    (0x11083, 0x110af),
    (0x110d0, 0x110e8),
    (0x11103, 0x11126),
    (0x11144, 0x11144),
    (0x11147, 0x11147),
    (0x11150, 0x11172),
    (0x11176, 0x11176),
    (0x11183, 0x111b2),
    (0x111c1, 0x111c4),
    (0x111da, 0x111da),
    (0x111dc, 0x111dc),
    (0x11200, 0x11211),
    (0x11213, 0x1122b),
    (0x11280, 0x11286),
    (0x11288, 0x11288),
    (0x1128a, 0x1128d),
    (0x1128f, 0x1129d),
    (0x1129f, 0x112a8),
    (0x112b0, 0x112de),
    (0x11305, 0x1130c),
    (0x1130f, 0x11310),
    (0x11313, 0x11328),
    (0x1132a, 0x11330),
    (0x11332, 0x11333),
    (0x11335, 0x11339),
    (0x1133d, 0x1133d),
    (0x11350, 0x11350),
    (0x1135d, 0x11361),
    (0x11400, 0x11434),
    (0x11447, 0x1144a),
    (0x1145f, 0x11461),
    (0x11480, 0x114af),
    (0x114c4, 0x114c5),
    (0x114c7, 0x114c7),
    (0x11580, 0x115ae),
    (0x115d8, 0x115db),
    (0x11600, 0x1162f),
    (0x11644, 0x11644),
    (0x11680, 0x116aa),
    (0x116b8, 0x116b8),
    (0x11700, 0x1171a),
    (0x11740, 0x11746),
    (0x11800, 0x1182b),
    (0x118a0, 0x118df),
    (0x118ff, 0x11906),
    (0x11909, 0x11909),
    (0x1190c, 0x11913),
    (0x11915, 0x11916),
    (0x11918, 0x1192f),
    (0x1193f, 0x1193f),
    (0x11941, 0x11941),
    (0x119a0, 0x119a7),
    (0x119aa, 0x119d0),
    (0x119e1, 0x119e1),
    (0x119e3, 0x119e3),
    (0x11a00, 0x11a00),
    (0x11a0b, 0x11a32),
    (0x11a3a, 0x11a3a),
    (0x11a50, 0x11a50),
    (0x11a5c, 0x11a89),
    (0x11a9d, 0x11a9d),
    (0x11ab0, 0x11af8),
    (0x11c00, 0x11c08),
    (0x11c0a, 0x11c2e),
    (0x11c40, 0x11c40),
    (0x11c72, 0x11c8f),
    (0x11d00, 0x11d06),
    (0x11d08, 0x11d09),
    (0x11d0b, 0x11d30),
    (0x11d46, 0x11d46),
    (0x11d60, 0x11d65),
    (0x11d67, 0x11d68),
    (0x11d6a, 0x11d89),
    (0x11d98, 0x11d98),
    (0x11ee0, 0x11ef2),
    (0x11fb0, 0x11fb0),
    (0x12000, 0x12399),
    (0x12480, 0x12543),
    (0x12f90, 0x12ff0),
    (0x13000, 0x1342e),
    (0x14400, 0x14646),
    (0x16800, 0x16a38),
    (0x16a40, 0x16a5e),
    (0x16a70, 0x16abe),
    (0x16ad0, 0x16aed),
    (0x16b00, 0x16b2f),
    (0x16b40, 0x16b43),
    (0x16b63, 0x16b77),
    (0x16b7d, 0x16b8f),
    (0x16e40, 0x16e7f),
    (0x16f00, 0x16f4a),
    (0x16f50, 0x16f50),
    (0x16f93, 0x16f9f),
    (0x16fe0, 0x16fe1),
    (0x16fe3, 0x16fe3),
    (0x17000, 0x187f7),
    (0x18800, 0x18cd5),
    (0x18d00, 0x18d08),
    (0x1aff0, 0x1aff3),
    (0x1aff5, 0x1affb),
    (0x1affd, 0x1affe),
    (0x1b000, 0x1b122),
    (0x1b150, 0x1b152),
    (0x1b164, 0x1b167),
    (0x1b170, 0x1b2fb),
    (0x1bc00, 0x1bc6a),
    (0x1bc70, 0x1bc7c),
    (0x1bc80, 0x1bc88),
    (0x1bc90, 0x1bc99),
    (0x1d400, 0x1d454),
    (0x1d456, 0x1d49c),
    (0x1d49e, 0x1d49f),
    (0x1d4a2, 0x1d4a2),
    (0x1d4a5, 0x1d4a6),
    (0x1d4a9, 0x1d4ac),
    (0x1d4ae, 0x1d4b9),
    (0x1d4bb, 0x1d4bb),
    (0x1d4bd, 0x1d4c3),
    (0x1d4c5, 0x1d505),
    (0x1d507, 0x1d50a),
    (0x1d50d, 0x1d514),
    (0x1d516, 0x1d51c),
    (0x1d51e, 0x1d539),
    (0x1d53b, 0x1d53e),
    (0x1d540, 0x1d544),
    (0x1d546, 0x1d546),
    (0x1d54a, 0x1d550),
    (0x1d552, 0x1d6a5),
    (0x1d6a8, 0x1d6c0),
    (0x1d6c2, 0x1d6da),
    (0x1d6dc, 0x1d6fa),
    (0x1d6fc, 0x1d714),
    (0x1d716, 0x1d734),
    (0x1d736, 0x1d74e),
    (0x1d750, 0x1d76e),
    (0x1d770, 0x1d788),
    (0x1d78a, 0x1d7a8),
    (0x1d7aa, 0x1d7c2),
    (0x1d7c4, 0x1d7cb),
    (0x1df00, 0x1df1e),
    (0x1e100, 0x1e12c),
    (0x1e137, 0x1e13d),
    (0x1e14e, 0x1e14e),
    (0x1e290, 0x1e2ad),
    (0x1e2c0, 0x1e2eb),
    (0x1e7e0, 0x1e7e6),
    (0x1e7e8, 0x1e7eb),
    (0x1e7ed, 0x1e7ee),
    (0x1e7f0, 0x1e7fe),
    (0x1e800, 0x1e8c4),
    (0x1e900, 0x1e943),
    (0x1e94b, 0x1e94b),
    (0x1ee00, 0x1ee03),
    (0x1ee05, 0x1ee1f),
    (0x1ee21, 0x1ee22),
    (0x1ee24, 0x1ee24),
    (0x1ee27, 0x1ee27),
    (0x1ee29, 0x1ee32),
    (0x1ee34, 0x1ee37),
    (0x1ee39, 0x1ee39),
    (0x1ee3b, 0x1ee3b),
    (0x1ee42, 0x1ee42),
    (0x1ee47, 0x1ee47),
    (0x1ee49, 0x1ee49),
    (0x1ee4b, 0x1ee4b),
    (0x1ee4d, 0x1ee4f),
    (0x1ee51, 0x1ee52),
    (0x1ee54, 0x1ee54),
    (0x1ee57, 0x1ee57),
    (0x1ee59, 0x1ee59),
    (0x1ee5b, 0x1ee5b),
    (0x1ee5d, 0x1ee5d),
    (0x1ee5f, 0x1ee5f),
    (0x1ee61, 0x1ee62),
    (0x1ee64, 0x1ee64),
    (0x1ee67, 0x1ee6a),
    (0x1ee6c, 0x1ee72),
    (0x1ee74, 0x1ee77),
    (0x1ee79, 0x1ee7c),
    (0x1ee7e, 0x1ee7e),
    (0x1ee80, 0x1ee89),
    (0x1ee8b, 0x1ee9b),
    (0x1eea1, 0x1eea3),
    (0x1eea5, 0x1eea9),
    (0x1eeab, 0x1eebb),
    (0x20000, 0x2a6df),
    (0x2a700, 0x2b738),
    (0x2b740, 0x2b81d),
    (0x2b820, 0x2cea1),
    (0x2ceb0, 0x2ebe0),
    (0x2f800, 0x2fa1d),
    (0x30000, 0x3134a),
];
