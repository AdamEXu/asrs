//! `shared/log/LogMechanismState.ts`.

use asrs_js::json::json_copy;
use asrs_js::math;
use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};

use crate::units;

use super::Log;
use super::log_field_tree::LogFieldTree;
use super::log_util::{get_or_default, get_value_at};

/// `MechanismState`. The fields are `JsValue`-level because
/// `mergeMechanismStates` passes input state properties through `jsonCopy`
/// unchanged: a missing color stays `0`, non-finite line elements become
/// `null`, and merged lines keep any extra keys the input carried.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanismState {
    pub background_color: JsValue,
    pub dimensions: Vec<f64>,
    pub lines: Vec<JsValue>,
}

/// A `MechanismLine` view of one entry in `MechanismState::lines`, for
/// callers that want the fields upstream declares rather than the object.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanismLine {
    pub start: Vec<JsValue>,
    pub end: Vec<JsValue>,
    pub color: JsValue,
    pub weight: JsValue,
}

impl MechanismLine {
    /// `{start, end, color, weight}` of a line object, or `None` when it is
    /// not an object with array `start` and `end`.
    pub fn of(value: &JsValue) -> Option<Self> {
        let object = value.as_object()?;
        Some(MechanismLine {
            start: object.get("start")?.as_array()?.to_vec(),
            end: object.get("end")?.as_array()?.to_vec(),
            color: object.get("color").cloned().unwrap_or(JsValue::Undefined),
            weight: object.get("weight").cloned().unwrap_or(JsValue::Undefined),
        })
    }
}

fn ascii(s: &str) -> JsString {
    JsString::from(s)
}

/// `key + "/" + child`.
fn key_join(key: &JsString, child: &str) -> JsString {
    let mut out = key.clone();
    out.push_unit(u16::from(b'/'));
    out.push_str(&ascii(child));
    out
}

/// `key + "/" + node.fullKey`; a `null` `fullKey` concats as `"null"`.
fn full_key_path(key: &JsString, node: &LogFieldTree) -> JsString {
    let mut out = key.clone();
    out.push_unit(u16::from(b'/'));
    match &node.full_key {
        Some(full_key) => out.push_str(full_key),
        None => out.push_str(&ascii("null")),
    }
    out
}

fn is(key: &JsString, name: &str) -> bool {
    key.units() == ascii(name).units()
}

/// `getMechanismState`: `None` when `backgroundColor` or `dims` is missing or
/// has another type. Traversal errors abort the walk the way upstream's
/// `try`/`catch` does, keeping the lines collected so far.
pub fn get_mechanism_state(log: &Log, key: &JsString, time: f64) -> Option<MechanismState> {
    let background_color =
        get_value_at::<JsString>(log, &key_join(key, "backgroundColor"), time, None)?;
    let dimensions = get_value_at::<Vec<f64>>(log, &key_join(key, "dims"), time, None)?;
    let mut lines = Vec::new();
    add_root_lines(log, key, time, &mut lines);
    Some(MechanismState {
        background_color: JsValue::String(background_color),
        dimensions,
        lines,
    })
}

struct Abort;

fn add_root_lines(log: &Log, key: &JsString, time: f64, lines: &mut Vec<JsValue>) {
    let mut prefix = key.clone();
    prefix.push_unit(u16::from(b'/'));
    let tree = log.get_field_tree(true, &prefix);
    for (child_key, child_tree) in tree.iter() {
        if child_key.starts_with(&ascii("."), None)
            || is(child_key, "backgroundColor")
            || is(child_key, "dims")
        {
            continue;
        }
        let translation: Vec<f64> = match child_tree.children.get_key(&ascii("position")) {
            Some(position) => get_or_default(
                log,
                &full_key_path(key, position),
                time,
                vec![0.0, 0.0],
                None,
            ),
            None => {
                let (Some(x), Some(y)) = (
                    child_tree.children.get_key(&ascii("x")),
                    child_tree.children.get_key(&ascii("y")),
                ) else {
                    return;
                };
                vec![
                    get_or_default(log, &full_key_path(key, x), time, 0.0, None),
                    get_or_default(log, &full_key_path(key, y), time, 0.0, None),
                ]
            }
        };
        for (root_child_key, root_child_tree) in child_tree.children.iter() {
            if is(root_child_key, "x") || is(root_child_key, "y") || is(root_child_key, "position")
            {
                continue;
            }
            if add_line(log, key, time, lines, root_child_tree, &translation, 0.0).is_err() {
                return;
            }
        }
    }
}

fn add_line(
    log: &Log,
    key: &JsString,
    time: f64,
    lines: &mut Vec<JsValue>,
    line_tree: &LogFieldTree,
    start: &[f64],
    start_rotation: f64,
) -> Result<(), Abort> {
    let angle_node = line_tree.children.get_key(&ascii("angle")).ok_or(Abort)?;
    let length_node = line_tree.children.get_key(&ascii("length")).ok_or(Abort)?;
    let color_node = line_tree.children.get_key(&ascii("color")).ok_or(Abort)?;
    let weight_node = line_tree.children.get_key(&ascii("weight")).ok_or(Abort)?;
    let angle = get_or_default::<f64>(log, &full_key_path(key, angle_node), time, 0.0, None);
    let length = get_or_default::<f64>(log, &full_key_path(key, length_node), time, 0.0, None);
    let color = get_value_at::<JsString>(log, &full_key_path(key, color_node), time, None)
        .map(JsValue::String)
        .unwrap_or(JsValue::Number(0.0));
    let weight = get_or_default::<f64>(log, &full_key_path(key, weight_node), time, 0.0, None);

    let end_rotation =
        start_rotation + units::convert(angle, "degrees", "radians").unwrap_or(f64::NAN);
    let end = [
        start.first().copied().unwrap_or(f64::NAN) + math::cos(end_rotation) * length,
        start.get(1).copied().unwrap_or(f64::NAN) + math::sin(end_rotation) * length,
    ];
    let mut line = JsObject::new();
    line.insert(
        "start",
        JsValue::Array(start.iter().map(|&x| JsValue::Number(x)).collect()),
    );
    line.insert(
        "end",
        JsValue::Array(vec![JsValue::Number(end[0]), JsValue::Number(end[1])]),
    );
    line.insert("color", color);
    line.insert("weight", JsValue::Number(weight));
    lines.push(JsValue::Object(line));

    for (child_key, child_tree) in line_tree.children.iter() {
        if is(child_key, ".type")
            || is(child_key, "angle")
            || is(child_key, "color")
            || is(child_key, "length")
            || is(child_key, "weight")
        {
            continue;
        }
        add_line(log, key, time, lines, child_tree, &end, end_rotation)?;
    }
    Ok(())
}

fn type_error(message: impl Into<String>) -> JsError {
    JsError::type_error(message)
}

fn json_error(e: asrs_js::json::JsonError) -> JsError {
    JsError::new(e.name, e.message.to_string_lossy())
}

/// `v.name` for the property names the merge reads. Non-object receivers see
/// `undefined`, except `null` and `undefined` which throw.
fn read_prop(v: &JsValue, name: &str) -> Result<JsValue, JsError> {
    match v {
        JsValue::Null => Err(type_error(format!(
            "Cannot read properties of null (reading '{name}')"
        ))),
        JsValue::Undefined => Err(type_error(format!(
            "Cannot read properties of undefined (reading '{name}')"
        ))),
        JsValue::Object(o) => Ok(o.get(name).cloned().unwrap_or(JsValue::Undefined)),
        _ => Ok(JsValue::Undefined),
    }
}

/// `v[index]` for a whole-number index.
fn read_index(v: &JsValue, index: u32) -> Result<JsValue, JsError> {
    match v {
        JsValue::Null => Err(type_error(format!(
            "Cannot read properties of null (reading '{index}')"
        ))),
        JsValue::Undefined => Err(type_error(format!(
            "Cannot read properties of undefined (reading '{index}')"
        ))),
        JsValue::Array(items) => Ok(items
            .get(index as usize)
            .cloned()
            .unwrap_or(JsValue::Undefined)),
        JsValue::Object(o) => Ok(o
            .get(format!("{index}").as_str())
            .cloned()
            .unwrap_or(JsValue::Undefined)),
        JsValue::String(s) => Ok(s
            .at(f64::from(index))
            .map(JsValue::String)
            .unwrap_or(JsValue::Undefined)),
        JsValue::Bytes(b) => Ok(b
            .get(index as usize)
            .map(|&x| JsValue::Number(f64::from(x)))
            .unwrap_or(JsValue::Undefined)),
        _ => Ok(JsValue::Undefined),
    }
}

/// `Array.prototype.join(",")`: `null` and `undefined` join as empty text.
fn join_units(items: &[JsValue]) -> Vec<u16> {
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(u16::from(b','));
        }
        match item {
            JsValue::Undefined | JsValue::Null => {}
            other => out.extend_from_slice(js_to_string(other).units()),
        }
    }
    out
}

/// `ToString` over the post-`jsonCopy` domain (plus the primitives and
/// `Bytes` a pre-copy input can still reach in `join`/`+` contexts).
fn js_to_string(v: &JsValue) -> JsString {
    match v {
        JsValue::Undefined => ascii("undefined"),
        JsValue::Null => ascii("null"),
        JsValue::Bool(b) => ascii(if *b { "true" } else { "false" }),
        JsValue::Number(x) => ascii(&number::to_string(*x)),
        JsValue::String(s) => s.clone(),
        JsValue::BigInt(b) => ascii(&format!("{b}")),
        JsValue::Array(items) => JsString::from_units(join_units(items)),
        JsValue::Bytes(bytes) => {
            let mut out = Vec::new();
            for (i, byte) in bytes.iter().enumerate() {
                if i > 0 {
                    out.push(u16::from(b','));
                }
                out.extend(format!("{byte}").encode_utf16());
            }
            JsString::from_units(out)
        }
        JsValue::Object(_) | JsValue::ExtData(..) | JsValue::Date(_) => ascii("[object Object]"),
    }
}

/// `ToNumber` over the same domain.
fn js_to_number(v: &JsValue) -> Result<f64, JsError> {
    Ok(match v {
        JsValue::Undefined => f64::NAN,
        JsValue::Null => 0.0,
        JsValue::Bool(b) => f64::from(u8::from(*b)),
        JsValue::Number(x) => *x,
        JsValue::String(s) => number::to_number(s.units()),
        JsValue::BigInt(_) => {
            return Err(type_error("Cannot convert a BigInt value to a number"));
        }
        JsValue::Array(items) => number::to_number(&join_units(items)),
        JsValue::Bytes(bytes) => {
            let mut text = Vec::new();
            for (i, byte) in bytes.iter().enumerate() {
                if i > 0 {
                    text.push(u16::from(b','));
                }
                text.extend(format!("{byte}").encode_utf16());
            }
            number::to_number(&text)
        }
        JsValue::Date(t) => *t,
        JsValue::Object(_) | JsValue::ExtData(..) => f64::NAN,
    })
}

/// `v + x` for a post-`jsonCopy` `v` and a number `x`: numeric coercion for
/// numbers, `null`, booleans and `undefined`; string concatenation once
/// `ToPrimitive(v)` is a string (strings, arrays, objects).
fn js_add(v: &JsValue, x: f64) -> JsValue {
    match v {
        JsValue::Number(n) => JsValue::Number(n + x),
        JsValue::Null => JsValue::Number(x),
        JsValue::Bool(b) => JsValue::Number(f64::from(u8::from(*b)) + x),
        JsValue::Undefined => JsValue::Number(f64::NAN + x),
        JsValue::BigInt(_) => v.clone(),
        other => {
            let mut s = js_to_string(other);
            s.push_str(&ascii(&number::to_string(x)));
            JsValue::String(s)
        }
    }
}

/// The write half of `container[0] += x` throwing on a primitive container.
fn write_index_error(v: &JsValue) -> JsError {
    let detail = match v {
        JsValue::Bool(b) => format!("boolean '{b}'"),
        JsValue::Number(x) => format!("number '{}'", number::to_string(*x)),
        JsValue::BigInt(b) => format!("bigint '{b}'"),
        JsValue::String(s) if !s.is_empty() => {
            return type_error(format!(
                "Cannot assign to read only property '0' of string '{}'",
                s.to_string_lossy()
            ));
        }
        JsValue::String(s) => format!("string '{}'", s.to_string_lossy()),
        _ => v.type_of().to_owned(),
    };
    type_error(format!("Cannot create property '0' on {detail}"))
}

/// `line.prop[0] += x` for `"start"`/`"end"` on a merged line.
fn add_to_index(line: &mut JsValue, prop: &str, x: f64) -> Result<(), JsError> {
    let container = read_prop(line, prop)?;
    match &container {
        JsValue::Array(_) | JsValue::Object(_) => {}
        JsValue::Null => {
            return Err(type_error("Cannot read properties of null (reading '0')"));
        }
        JsValue::Undefined => {
            return Err(type_error(
                "Cannot read properties of undefined (reading '0')",
            ));
        }
        _ => {
            let base = read_index(&container, 0)?;
            let _ = js_add(&base, x);
            return Err(write_index_error(&container));
        }
    }
    let Some(container) = (match line {
        JsValue::Object(o) => o.get_mut(prop),
        _ => None,
    }) else {
        return Ok(());
    };
    match container {
        JsValue::Array(items) => {
            let base = items.first().cloned().unwrap_or(JsValue::Undefined);
            let result = js_add(&base, x);
            if items.is_empty() {
                items.push(result);
            } else {
                items[0] = result;
            }
        }
        JsValue::Object(o) => {
            let base = o.get("0").cloned().unwrap_or(JsValue::Undefined);
            let result = js_add(&base, x);
            o.insert("0", result);
        }
        _ => {}
    }
    Ok(())
}

/// `mergeMechanismStates`.
pub fn merge_mechanism_states(states: &[JsValue]) -> Result<MechanismState, JsError> {
    let mut widths = Vec::with_capacity(states.len());
    let mut heights = Vec::with_capacity(states.len());
    for state in states {
        let dims = read_prop(state, "dimensions")?;
        widths.push(js_to_number(&read_index(&dims, 0)?)?);
        heights.push(js_to_number(&read_index(&dims, 1)?)?);
    }
    let new_width = math::max(&widths);
    let new_height = math::max(&heights);

    let mut lines = Vec::new();
    for (i, state) in states.iter().enumerate() {
        let x_offset = (new_width - widths[i]) / 2.0;
        let state_lines = read_prop(state, "lines")?;
        let items = match &state_lines {
            JsValue::Array(items) => items,
            JsValue::Null => {
                return Err(type_error(
                    "Cannot read properties of null (reading 'forEach')",
                ));
            }
            JsValue::Undefined => {
                return Err(type_error(
                    "Cannot read properties of undefined (reading 'forEach')",
                ));
            }
            _ => return Err(type_error("state.lines.forEach is not a function")),
        };
        for line in items {
            let mut new_line = json_copy(line).map_err(json_error)?;
            add_to_index(&mut new_line, "start", x_offset)?;
            add_to_index(&mut new_line, "end", x_offset)?;
            lines.push(new_line);
        }
    }

    Ok(MechanismState {
        background_color: read_prop(
            states.first().unwrap_or(&JsValue::Undefined),
            "backgroundColor",
        )?,
        dimensions: vec![new_width, new_height],
        lines,
    })
}
