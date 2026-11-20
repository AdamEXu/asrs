//! `fn` names for the `shared/util.ts` helpers ported in `asrs_log::util`.
//! Wrappers reproduce the JS evaluation semantics upstream's callers get:
//! property access on non-array arguments, `===` and abstract relational
//! comparisons, `+` coercion, default parameters and TypeErrors.

use std::cmp::Ordering;

use asrs_js::math::{self, BigInt};
use asrs_js::number;
use asrs_js::string::JsString;
use asrs_js::time::{self, NamedZone};
use asrs_js::value::array_index;
use asrs_log::util;
use asrs_parity::{Response, Utf16String, Value};

use crate::functions::{arg, array_join, ok_number, to_number, to_units, type_error};

pub type Function = fn(&[Value]) -> Response;

pub const FUNCTIONS: &[(&str, Function)] = &[
    ("shared/util:arraysEqual", arrays_equal_fn),
    ("shared/util:lowerBound", |a| bound_fn(a, Bound::Lower)),
    ("shared/util:upperBound", |a| bound_fn(a, Bound::Upper)),
    ("shared/util:setsEqual", sets_equal_fn),
    ("shared/util:checkArrayType", check_array_type_fn),
    ("shared/util:indexArray", index_array_fn),
    ("shared/util:charIsCapital", char_is_capital_fn),
    ("shared/util:calcMockProgress", |a| {
        let t = match to_number(&arg(a, 0)) {
            Ok(x) => x,
            Err(r) => return r,
        };
        let mp = match &arg(a, 1) {
            Value::Undefined => 0.6,
            v => match to_number(v) {
                Ok(x) => x,
                Err(r) => return r,
            },
        };
        ok_number(util::calc_mock_progress(t, mp))
    }),
    ("shared/util:shiftColor", shift_color_fn),
    ("shared/util:zfill", zfill_fn),
    ("shared/util:cleanFloat", |a| {
        unary_num(a, util::clean_float)
    }),
    ("shared/util:scaleValue", scale_fn),
    ("shared/util:scaleValueClamped", |a| scale_fn_clamped(a)),
    ("new shared/util:ValueScaler", value_scaler_fn),
    (
        "shared/util:ValueScaler.prototype.calculate",
        value_scaler_calculate_fn,
    ),
    ("shared/util:clampValue", |a| {
        ternary_num(a, util::clamp_value)
    }),
    ("shared/util:transformPx", transform_px_fn),
    ("shared/util:wrapRadians", |a| {
        unary_num(a, util::wrap_radians)
    }),
    ("shared/util:concatBuffers", concat_buffers_fn),
    ("shared/util:calcAxisStepSize", calc_axis_step_size_fn),
    ("shared/util:getSpiralIndex", |a| {
        match (to_number(&arg(a, 0)), to_number(&arg(a, 1))) {
            (Ok(x), Ok(y)) => ok_number(util::get_spiral_index(x, y)),
            (Err(r), _) | (_, Err(r)) => r,
        }
    }),
];

fn unary_num(args: &[Value], f: impl Fn(f64) -> f64) -> Response {
    match to_number(&arg(args, 0)) {
        Ok(x) => ok_number(f(x)),
        Err(r) => r,
    }
}

fn ternary_num(args: &[Value], f: impl Fn(f64, f64, f64) -> f64) -> Response {
    let (x, y, z) = match (
        to_number(&arg(args, 0)),
        to_number(&arg(args, 1)),
        to_number(&arg(args, 2)),
    ) {
        (Ok(x), Ok(y), Ok(z)) => (x, y, z),
        (Err(r), _, _) | (_, Err(r), _) | (_, _, Err(r)) => return r,
    };
    ok_number(f(x, y, z))
}

fn range_error(message: &str) -> Response {
    Response::Throw(Value::Error("RangeError".into(), message.into()))
}

fn zone() -> NamedZone {
    std::env::var("ASRS_ORACLE_ZONE")
        .ok()
        .filter(|n| !n.is_empty())
        .or_else(|| std::env::var("TZ").ok().filter(|n| !n.is_empty()))
        .and_then(|name| NamedZone::get(&name).ok())
        .unwrap_or_else(NamedZone::system)
}

/// `.length` on a value: arrays, typed arrays, strings and byte buffers have
/// one; objects may carry it as an own property; everything else is
/// `undefined`.
fn length_prop(v: &Value) -> Value {
    match v {
        Value::Array(items) | Value::TypedArray(_, items) => Value::number(items.len() as f64),
        Value::Bytes(b) | Value::ArrayBuffer(b) | Value::DataView(b) => {
            Value::number(b.len() as f64)
        }
        Value::String(s) => Value::number(s.units().len() as f64),
        Value::Object(_) | Value::Instance(..) => {
            v.get("length").cloned().unwrap_or(Value::Undefined)
        }
        _ => Value::Undefined,
    }
}

/// `.size`: only `Map` and `Set` have it.
fn size_prop(v: &Value) -> Value {
    match v {
        Value::Set(items) => Value::number(items.len() as f64),
        Value::Map(items) => Value::number(items.len() as f64),
        _ => Value::Undefined,
    }
}

/// `.byteLength`.
fn byte_length(v: &Value) -> Value {
    match v {
        Value::Bytes(b) | Value::ArrayBuffer(b) | Value::DataView(b) => {
            Value::number(b.len() as f64)
        }
        Value::TypedArray(name, items) => {
            let width = match name.as_str() {
                "Int16Array" | "Uint16Array" => 2.0,
                "Int32Array" | "Uint32Array" | "Float32Array" => 4.0,
                "Float64Array" | "BigInt64Array" | "BigUint64Array" => 8.0,
                _ => 1.0,
            };
            Value::number(items.len() as f64 * width)
        }
        _ => Value::Undefined,
    }
}

/// The property key `ToPropertyKey(v)` produces for reads on plain
/// containers. Numbers become their canonical string form (`-0` reads `"0"`,
/// `NaN` reads `"NaN"`); symbols never match a string-keyed container.
fn prop_key(v: &Value) -> Vec<u16> {
    match v {
        Value::Number(bits) => number::to_string(f64::from_bits(*bits))
            .encode_utf16()
            .collect(),
        Value::String(s) => s.units().to_vec(),
        Value::Bool(b) => if *b { "true" } else { "false" }.encode_utf16().collect(),
        Value::Null => "null".encode_utf16().collect(),
        Value::Undefined | Value::Hole => "undefined".encode_utf16().collect(),
        Value::BigInt(digits) => digits.encode_utf16().collect(),
        Value::Symbol(_) => vec![0xd800, 0xdc00],
        other => match to_units(other) {
            Ok(u) => u,
            Err(_) => "[object Object]".encode_utf16().collect(),
        },
    }
}

/// `container[name]` for the containers this harness models: arrays (and
/// typed arrays) by canonical index, strings by index, objects by name.
fn get_prop(container: &Value, name: &[u16]) -> Value {
    let at_index = |len: usize, f: &dyn Fn(usize) -> Value| -> Value {
        match array_index(name) {
            Some(i) if (i as usize) < len => f(i as usize),
            _ => Value::Undefined,
        }
    };
    match container {
        Value::Array(items) => at_index(items.len(), &|i| match &items[i] {
            Value::Hole => Value::Undefined,
            v => v.clone(),
        }),
        Value::TypedArray(_, items) => at_index(items.len(), &|i| items[i].clone()),
        Value::Bytes(bytes) => at_index(bytes.len(), &|i| Value::number(f64::from(bytes[i]))),
        Value::String(s) => at_index(s.units().len(), &|i| {
            Value::String(Utf16String::from_units(vec![s.units()[i]]))
        }),
        Value::Object(pairs) | Value::Instance(_, pairs) => pairs
            .iter()
            .find(|(k, _)| k.units() == name)
            .map(|(_, v)| v.clone())
            .unwrap_or(Value::Undefined),
        _ => Value::Undefined,
    }
}

/// `container[i]` where `i` is already a JS number.
fn get_prop_num(container: &Value, i: f64) -> Value {
    get_prop(
        container,
        &number::to_string(i).encode_utf16().collect::<Vec<u16>>(),
    )
}

/// `===` strict equality: same tag, numbers by IEEE value (`NaN` never
/// equal, `-0` equals `+0`), strings by code units, BigInts by digits, and
/// reference equality for everything else (never true across CVE-1 values).
fn strict_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Undefined | Value::Hole, Value::Undefined | Value::Hole) => true,
        (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Number(x), Value::Number(y)) => f64::from_bits(*x) == f64::from_bits(*y),
        (Value::String(x), Value::String(y)) => x.units() == y.units(),
        (Value::BigInt(x), Value::BigInt(y)) => x == y,
        _ => false,
    }
}

/// `SameValueZero` (`Set.has`, `Map.has`): `===` except `NaN` equals `NaN`.
fn same_value_zero(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (f64::from_bits(*x), f64::from_bits(*y));
            x == y || (x.is_nan() && y.is_nan())
        }
        _ => strict_equal(a, b),
    }
}

/// `ToPrimitive` with a number hint for the values a relational comparison
/// or `+` can meet: wrappers unwrap, dates read their time, array-likes join.
fn to_primitive(v: &Value, default_hint_string: bool) -> Value {
    match v {
        Value::Boxed(inner) => to_primitive(inner, default_hint_string),
        Value::Date(bits) => {
            if default_hint_string {
                Value::String(Utf16String::from(time::date_to_string(
                    f64::from_bits(*bits),
                    &zone(),
                )))
            } else {
                Value::Number(*bits)
            }
        }
        Value::Array(items) | Value::TypedArray(_, items) => {
            Value::String(Utf16String::from_units(array_join(items)))
        }
        Value::Bytes(bytes) => Value::String(Utf16String::from_units(array_join(
            &bytes
                .iter()
                .map(|&b| Value::number(f64::from(b)))
                .collect::<Vec<_>>(),
        ))),
        Value::Map(_) => Value::string("[object Map]"),
        Value::Set(_) => Value::string("[object Set]"),
        Value::ArrayBuffer(_) => Value::string("[object ArrayBuffer]"),
        Value::DataView(_) => Value::string("[object DataView]"),
        Value::Error(name, message) => {
            let mut text = name.to_string_lossy();
            let message = message.to_string_lossy();
            if !message.is_empty() {
                if !text.is_empty() {
                    text.push_str(": ");
                }
                text.push_str(&message);
            }
            Value::string(&text)
        }
        Value::Function(name) => {
            let name = name.to_string_lossy();
            Value::string(&format!("function {name}() {{ [native code] }}"))
        }
        Value::Object(_) | Value::Instance(..) => Value::string("[object Object]"),
        primitive => primitive.clone(),
    }
}

#[derive(Clone, Copy)]
enum RelOp {
    Lt,
    Gt,
    Ge,
}

/// An abstract relational comparison: string-vs-string is code-unit order;
/// a symbol throws; a BigInt compares mathematically; otherwise `ToNumber`
/// both sides (any NaN makes every operator false).
fn rel_cmp(a: &Value, b: &Value, op: RelOp) -> Result<bool, Response> {
    let pa = to_primitive(a, false);
    let pb = to_primitive(b, false);
    if let (Value::String(x), Value::String(y)) = (&pa, &pb) {
        let (x, y) = (x.units(), y.units());
        return Ok(match op {
            RelOp::Lt => x < y,
            RelOp::Gt => x > y,
            RelOp::Ge => x >= y,
        });
    }
    if matches!(pa, Value::Symbol(_)) || matches!(pb, Value::Symbol(_)) {
        return Err(type_error("Cannot convert a Symbol value to a number"));
    }
    if matches!(pa, Value::BigInt(_)) || matches!(pb, Value::BigInt(_)) {
        return Ok(match bigint_cmp(&pa, &pb) {
            Some(Ordering::Less) => matches!(op, RelOp::Lt),
            Some(Ordering::Greater) => matches!(op, RelOp::Gt | RelOp::Ge),
            Some(Ordering::Equal) => matches!(op, RelOp::Ge),
            None => false,
        });
    }
    let x = to_number(&pa)?;
    let y = to_number(&pb)?;
    Ok(match op {
        RelOp::Lt => x < y,
        RelOp::Gt => x > y,
        RelOp::Ge => x >= y,
    })
}

/// `StringToBigInt` for the decimal case (`StringToNumeric` would also take
/// radix literals, which callers never send).
fn string_to_bigint(s: &Utf16String) -> Option<BigInt> {
    let trimmed = JsString::from_units(s.units().to_vec())
        .trim()
        .to_string_lossy();
    BigInt::parse(&trimmed).ok()
}

/// A BigInt's mathematical order against an f64, without converting the
/// BigInt: compare it to `floor(x)` and use the fraction to break the tie.
fn bigint_cmp_f64(big: &BigInt, x: f64) -> Option<Ordering> {
    if x.is_nan() {
        return None;
    }
    let floor = math::floor(x);
    let Ok(bound) = BigInt::from_f64(floor) else {
        return Some(if x > 0.0 {
            Ordering::Less
        } else {
            Ordering::Greater
        });
    };
    Some(match big.cmp(&bound) {
        Ordering::Equal if floor == x => Ordering::Equal,
        Ordering::Equal => Ordering::Less,
        other => other,
    })
}

fn bigint_cmp(a: &Value, b: &Value) -> Option<Ordering> {
    let to_big = |v: &Value| -> Option<BigInt> {
        match v {
            Value::BigInt(d) => BigInt::parse(d).ok(),
            Value::Bool(x) => Some(BigInt::from_i64(i64::from(*x))),
            Value::Null => Some(BigInt::zero()),
            Value::String(s) => string_to_bigint(s),
            _ => None,
        }
    };
    if let Value::Number(x) = a {
        return bigint_cmp_f64(&to_big(b)?, f64::from_bits(*x)).map(Ordering::reverse);
    }
    if let Value::Number(y) = b {
        return bigint_cmp_f64(&to_big(a)?, f64::from_bits(*y));
    }
    Some(to_big(a)?.cmp(&to_big(b)?))
}

/// `a + b`: string when either primitive side is a string, BigInt addition
/// for two BigInts (and the mix TypeError for one), numeric otherwise.
fn js_add(a: &Value, b: &Value) -> Result<Value, Response> {
    let pa = to_primitive(a, true);
    let pb = to_primitive(b, true);
    if matches!(pa, Value::Symbol(_)) || matches!(pb, Value::Symbol(_)) {
        return Err(type_error("Cannot convert a Symbol value to a number"));
    }
    match (&pa, &pb) {
        (Value::BigInt(x), Value::BigInt(y)) => {
            let x = BigInt::parse(x).unwrap_or_default();
            let y = BigInt::parse(y).unwrap_or_default();
            Ok(Value::BigInt(format!("{}", x.add(&y))))
        }
        (Value::BigInt(_), _) | (_, Value::BigInt(_)) => Err(type_error(
            "Cannot mix BigInt and other types, use explicit conversions",
        )),
        (Value::String(_), _) | (_, Value::String(_)) => {
            let mut units = to_units(&pa)?;
            units.extend(to_units(&pb)?);
            Ok(Value::String(Utf16String::from_units(units)))
        }
        _ => {
            let x = to_number(&pa)?;
            let y = to_number(&pb)?;
            Ok(Value::number(x + y))
        }
    }
}

fn arrays_equal_fn(args: &[Value]) -> Response {
    let a = arg(args, 0);
    let b = arg(args, 1);
    if !strict_equal(&length_prop(&a), &length_prop(&b)) {
        return Response::Ok(Value::Bool(false));
    }
    let items: &[Value] = match &a {
        Value::Array(items) | Value::TypedArray(_, items) => items,
        Value::Bytes(bytes) => {
            for (i, byte) in bytes.iter().enumerate() {
                if !strict_equal(
                    &Value::number(f64::from(*byte)),
                    &get_prop_num(&b, i as f64),
                ) {
                    return Response::Ok(Value::Bool(false));
                }
            }
            return Response::Ok(Value::Bool(true));
        }
        _ => return type_error("a.every is not a function"),
    };
    for (i, item) in items.iter().enumerate() {
        if matches!(item, Value::Hole) {
            continue;
        }
        if !strict_equal(item, &get_prop_num(&b, i as f64)) {
            return Response::Ok(Value::Bool(false));
        }
    }
    Response::Ok(Value::Bool(true))
}

#[derive(Clone, Copy)]
enum Bound {
    Lower,
    Upper,
}

/// `lowerBound`/`upperBound` on fully numeric input goes through the
/// `asrs-log` port; anything else takes the generic JS path below.
fn bound_fn(args: &[Value], kind: Bound) -> Response {
    let array = arg(args, 0);
    let target = arg(args, 1);
    let start = arg(args, 2);
    if let Value::Array(items) = &array
        && items.iter().all(|v| matches!(v, Value::Number(_)))
        && let Value::Number(_) = target
        && matches!(start, Value::Number(_) | Value::Undefined)
    {
        let ts: Vec<f64> = items
            .iter()
            .map(|v| v.as_f64().unwrap_or_default())
            .collect();
        let t = target.as_f64().unwrap_or_default();
        let s = start.as_f64().unwrap_or(0.0);
        let result = match kind {
            Bound::Lower => util::lower_bound(&ts, t, s),
            Bound::Upper => util::upper_bound(&ts, t, s),
        };
        return ok_number(result);
    }
    match generic_bound(&array, &target, &start, kind) {
        Ok(v) => Response::Ok(v),
        Err(r) => r,
    }
}

fn generic_bound(
    array: &Value,
    target: &Value,
    start: &Value,
    kind: Bound,
) -> Result<Value, Response> {
    let length = length_prop(array);
    if strict_equal(&length, &Value::number(0.0)) {
        return Ok(length);
    }
    let direction = match kind {
        Bound::Lower => RelOp::Ge,
        Bound::Upper => RelOp::Gt,
    };
    if rel_cmp(start, &length, RelOp::Ge)? {
        return Ok(length);
    }
    let length_n = to_number(&length)?;
    if !rel_cmp(&get_prop_num(array, length_n - 1.0), target, direction)? {
        return Ok(length);
    }
    if rel_cmp(start, &Value::number(0.0), RelOp::Gt)? {
        if rel_cmp(&get_prop(array, &prop_key(start)), target, direction)? {
            return Ok(start.clone());
        }
        let next = js_add(start, &Value::number(1.0))?;
        if rel_cmp(&next, &length, RelOp::Lt)?
            && rel_cmp(&get_prop(array, &prop_key(&next)), target, direction)?
        {
            return Ok(next);
        }
    }
    let mut low = start.clone();
    let mut high = js_add(&length, &Value::number(-1.0))?;
    loop {
        if !rel_cmp(&low, &high, RelOp::Lt)? {
            return Ok(low);
        }
        let sum = js_add(&low, &high)?;
        let mid = to_number(&sum)?;
        let mid = f64::from(math::int::shift_right_unsigned(mid, 1.0));
        if rel_cmp(&get_prop_num(array, mid), target, direction)? {
            high = Value::number(mid);
        } else {
            low = Value::number(mid + 1.0);
        }
    }
}

fn sets_equal_fn(args: &[Value]) -> Response {
    let a = arg(args, 0);
    let b = arg(args, 1);
    if !strict_equal(&size_prop(&a), &size_prop(&b)) {
        return Response::Ok(Value::Bool(false));
    }
    let entries: Vec<Value> = match &a {
        Value::Set(items) => items.clone(),
        Value::Map(pairs) => pairs
            .iter()
            .map(|(k, v)| Value::Array(vec![k.clone(), v.clone()]))
            .collect(),
        _ => Vec::new(),
    };
    for entry in &entries {
        let present = match &b {
            Value::Set(items) => items.iter().any(|v| same_value_zero(v, entry)),
            Value::Map(pairs) => pairs.iter().any(|(k, _)| same_value_zero(k, entry)),
            _ => return type_error("b.has is not a function"),
        };
        if !present {
            return Response::Ok(Value::Bool(false));
        }
    }
    Response::Ok(Value::Bool(true))
}

fn js_typeof(v: &Value) -> &'static str {
    match v {
        Value::Undefined | Value::Hole => "undefined",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::BigInt(_) => "bigint",
        Value::Symbol(_) => "symbol",
        Value::Function(_) => "function",
        _ => "object",
    }
}

fn check_array_type_fn(args: &[Value]) -> Response {
    let items = match &arg(args, 0) {
        Value::Array(items) => items.clone(),
        _ => return Response::Ok(Value::Bool(false)),
    };
    let type_name = arg(args, 1);
    let ok = items.iter().all(|item| match &type_name {
        Value::String(s) => s.eq_str(js_typeof(item)),
        _ => false,
    });
    Response::Ok(Value::Bool(ok))
}

fn index_array_fn(args: &[Value]) -> Response {
    let length = match to_number(&arg(args, 0)) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let n = asrs_js::string::to_length(length) as usize;
    Response::Ok(Value::Array(
        (0..n).map(|i| Value::number(i as f64)).collect(),
    ))
}

fn char_is_capital_fn(args: &[Value]) -> Response {
    let input = arg(args, 0);
    let text = match &input {
        Value::String(s) => JsString::from_units(s.units().to_vec()),
        Value::Boxed(inner) => match inner.as_ref() {
            Value::String(s) => JsString::from_units(s.units().to_vec()),
            _ => return type_error("input.charAt is not a function"),
        },
        Value::Null => {
            return type_error("Cannot read properties of null (reading 'charAt')");
        }
        Value::Undefined => {
            return type_error("Cannot read properties of undefined (reading 'charAt')");
        }
        _ => return type_error("input.charAt is not a function"),
    };
    let index = match to_number(&arg(args, 1)) {
        Ok(x) => x,
        Err(r) => return r,
    };
    Response::Ok(Value::Bool(util::char_is_capital(&text, index)))
}

fn shift_color_fn(args: &[Value]) -> Response {
    let sliced = match &arg(args, 0) {
        Value::String(s) => JsString::from_units(s.units().to_vec()).slice(1.0, None),
        Value::Boxed(inner) => match inner.as_ref() {
            Value::String(s) => JsString::from_units(s.units().to_vec()).slice(1.0, None),
            Value::Array(_) => {
                return type_error("color.slice(...).match is not a function");
            }
            _ => return type_error("color.slice is not a function"),
        },
        Value::Array(_) | Value::TypedArray(..) | Value::Bytes(_) => {
            return type_error("color.slice(...).match is not a function");
        }
        Value::Null => {
            return type_error("Cannot read properties of null (reading 'slice')");
        }
        Value::Undefined => {
            return type_error("Cannot read properties of undefined (reading 'slice')");
        }
        _ => return type_error("color.slice is not a function"),
    };
    let shift = arg(args, 1);
    let pairs = asrs_js::regex::sites::pairs_of_units(&sliced);
    let mut channels: Vec<Value> = (0..3)
        .map(|i| match &pairs {
            Some(p) => p
                .get(i)
                .map(|s| Value::number(number::parse_int(s.units(), 16.0)))
                .unwrap_or_else(|| Value::number(f64::NAN)),
            None => Value::number(0.0),
        })
        .collect();
    for channel in &mut channels {
        let shifted = match js_add(channel, &shift) {
            Ok(v) => v,
            Err(r) => return r,
        };
        let lt0 = match rel_cmp(&shifted, &Value::number(0.0), RelOp::Lt) {
            Ok(x) => x,
            Err(r) => return r,
        };
        let gt255 = match rel_cmp(&shifted, &Value::number(255.0), RelOp::Gt) {
            Ok(x) => x,
            Err(r) => return r,
        };
        *channel = if lt0 {
            Value::number(0.0)
        } else if gt255 {
            Value::number(255.0)
        } else {
            shifted
        };
    }
    let mut units: Vec<u16> = "rgb(".encode_utf16().collect();
    units.extend(array_join(&channels));
    units.extend(")".encode_utf16());
    Response::Ok(Value::String(Utf16String::from_units(units)))
}

fn zfill_fn(args: &[Value]) -> Response {
    let mut number = arg(args, 0);
    let length = arg(args, 1);
    loop {
        let shorter = match rel_cmp(&length_prop(&number), &length, RelOp::Lt) {
            Ok(x) => x,
            Err(r) => return r,
        };
        if !shorter {
            break;
        }
        number = match js_add(&Value::string("0"), &number) {
            Ok(v) => v,
            Err(r) => return r,
        };
    }
    Response::Ok(number)
}

fn scale_ranges(args: &[Value]) -> Result<(f64, [f64; 2], [f64; 2]), Response> {
    let value = to_number(&arg(args, 0))?;
    let range = |v: &Value| -> Result<[f64; 2], Response> {
        Ok([
            to_number(&get_prop_num(v, 0.0))?,
            to_number(&get_prop_num(v, 1.0))?,
        ])
    };
    Ok((value, range(&arg(args, 1))?, range(&arg(args, 2))?))
}

fn scale_fn(args: &[Value]) -> Response {
    match scale_ranges(args) {
        Ok((v, old, new)) => ok_number(util::scale_value(v, old, new)),
        Err(r) => r,
    }
}

fn scale_fn_clamped(args: &[Value]) -> Response {
    match scale_ranges(args) {
        Ok((v, old, new)) => ok_number(util::scale_value_clamped(v, old, new)),
        Err(r) => r,
    }
}

fn value_scaler_fn(args: &[Value]) -> Response {
    let range = |v: &Value| -> Result<[f64; 2], Response> {
        Ok([
            to_number(&get_prop_num(v, 0.0))?,
            to_number(&get_prop_num(v, 1.0))?,
        ])
    };
    let (old, new) = match (range(&arg(args, 0)), range(&arg(args, 1))) {
        (Ok(o), Ok(n)) => (o, n),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    let a = (new[1] - new[0]) / (old[1] - old[0]);
    let b = new[0] - a * old[0];
    Response::Ok(Value::Instance(
        "ValueScaler".into(),
        vec![
            ("a".into(), Value::number(a)),
            ("b".into(), Value::number(b)),
        ],
    ))
}

fn value_scaler_calculate_fn(args: &[Value]) -> Response {
    let receiver = arg(args, 0);
    match &receiver {
        Value::Null => {
            return type_error("Cannot read properties of null (reading 'a')");
        }
        Value::Undefined => {
            return type_error("Cannot read properties of undefined (reading 'a')");
        }
        _ => {}
    }
    let value = match to_number(&arg(args, 1)) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let get = |key: &str| receiver.get(key).cloned().unwrap_or(Value::Undefined);
    let (a, b) = match (to_number(&get("a")), to_number(&get("b"))) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    ok_number(value * a + b)
}

fn transform_px_fn(args: &[Value]) -> Response {
    let pair = |v: &Value| -> Result<[f64; 2], Response> {
        Ok([
            to_number(&get_prop_num(v, 0.0))?,
            to_number(&get_prop_num(v, 1.0))?,
        ])
    };
    let (origin, rel) = match (pair(&arg(args, 0)), pair(&arg(args, 2))) {
        (Ok(o), Ok(r)) => (o, r),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    let rotation = match to_number(&arg(args, 1)) {
        Ok(x) => x,
        Err(r) => return r,
    };
    let [x, y] = util::transform_px(origin, rotation, rel);
    Response::Ok(Value::Array(vec![Value::number(x), Value::number(y)]))
}

fn concat_buffers_fn(args: &[Value]) -> Response {
    let items: Vec<Value> = match &arg(args, 0) {
        Value::Array(items) | Value::TypedArray(_, items) => items.clone(),
        Value::Bytes(bytes) => bytes.iter().map(|&b| Value::number(f64::from(b))).collect(),
        Value::Null => {
            return type_error("Cannot read properties of null (reading 'forEach')");
        }
        Value::Undefined => {
            return type_error("Cannot read properties of undefined (reading 'forEach')");
        }
        _ => return type_error("arrays.forEach is not a function"),
    };
    let mut size = 0.0;
    for item in &items {
        let len = match to_number(&byte_length(item)) {
            Ok(x) => x,
            Err(r) => return r,
        };
        size += len;
    }
    if !(0.0..=4_294_967_295.0).contains(&size) {
        return range_error("Invalid typed array length");
    }
    let mut out = Vec::with_capacity(size as usize);
    for item in &items {
        match item {
            Value::Bytes(bytes) | Value::ArrayBuffer(bytes) | Value::DataView(bytes) => {
                out.extend_from_slice(bytes);
            }
            Value::TypedArray(_, elements) | Value::Array(elements) => {
                for element in elements {
                    let x = match to_number(element) {
                        Ok(x) => x,
                        Err(r) => return r,
                    };
                    out.push(math::int::to_uint8(x));
                }
            }
            _ => {}
        }
    }
    Response::Ok(Value::Bytes(out))
}

fn calc_axis_step_size_fn(args: &[Value]) -> Response {
    let range = |v: &Value| -> Result<[f64; 2], Response> {
        Ok([
            to_number(&get_prop_num(v, 0.0))?,
            to_number(&get_prop_num(v, 1.0))?,
        ])
    };
    let data = match range(&arg(args, 0)) {
        Ok(r) => r,
        Err(r) => return r,
    };
    let (pixel_range, step_target) = match (to_number(&arg(args, 1)), to_number(&arg(args, 2))) {
        (Ok(x), Ok(y)) => (x, y),
        (Err(r), _) | (_, Err(r)) => return r,
    };
    ok_number(util::calc_axis_step_size(data, pixel_range, step_target))
}
