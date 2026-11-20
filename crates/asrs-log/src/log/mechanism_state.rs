//! `shared/log/LogMechanismState.ts`.

use asrs_js::math;
use asrs_js::string::JsString;

use crate::units;

use super::Log;
use super::log_field_tree::LogFieldTree;
use super::log_util::{get_or_default, get_value_at};

/// `MechanismState`.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanismState {
    pub background_color: JsString,
    pub dimensions: Vec<f64>,
    pub lines: Vec<MechanismLine>,
}

/// `MechanismLine`. A coordinate or weight is `None` where upstream holds
/// `null`: `mergeMechanismStates` copies lines through `jsonCopy`, which turns
/// non-finite numbers into `null`. `start` has the length of the logged
/// `position` array; `end` always has two entries.
#[derive(Clone, Debug, PartialEq)]
pub struct MechanismLine {
    pub start: Vec<Option<f64>>,
    pub end: Vec<Option<f64>>,
    /// `None` is upstream's numeric `0` default for a missing or non-string
    /// `color` field.
    pub color: Option<JsString>,
    pub weight: Option<f64>,
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
        background_color,
        dimensions,
        lines,
    })
}

struct Abort;

fn add_root_lines(log: &Log, key: &JsString, time: f64, lines: &mut Vec<MechanismLine>) {
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
    lines: &mut Vec<MechanismLine>,
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
    let color = get_value_at::<JsString>(log, &full_key_path(key, color_node), time, None);
    let weight = get_or_default::<f64>(log, &full_key_path(key, weight_node), time, 0.0, None);

    let end_rotation =
        start_rotation + units::convert(angle, "degrees", "radians").unwrap_or(f64::NAN);
    let end = [
        start.first().copied().unwrap_or(f64::NAN) + math::cos(end_rotation) * length,
        start.get(1).copied().unwrap_or(f64::NAN) + math::sin(end_rotation) * length,
    ];
    lines.push(MechanismLine {
        start: start.iter().map(|&x| Some(x)).collect(),
        end: end.iter().map(|&x| Some(x)).collect(),
        color,
        weight: Some(weight),
    });

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

/// `JSON.parse(JSON.stringify(x))` for a number slot.
fn json_copy_number(x: Option<f64>) -> Option<f64> {
    x.filter(|x| x.is_finite())
        .map(|x| if x == 0.0 { 0.0 } else { x })
}

/// `newLine[prop][0] += x_offset` after `jsonCopy`: `null` reads as `0` and a
/// missing index 0 as `NaN`.
fn add_offset(coords: &mut Vec<Option<f64>>, x_offset: f64) {
    let base = match coords.first() {
        Some(Some(x)) => *x,
        Some(None) => 0.0,
        None => f64::NAN,
    };
    let value = Some(base + x_offset);
    match coords.first_mut() {
        Some(slot) => *slot = value,
        None => coords.push(value),
    }
}

/// `mergeMechanismStates`, or `None` for no states, where upstream's callers
/// return `null` instead of calling it.
pub fn merge_mechanism_states(states: &[MechanismState]) -> Option<MechanismState> {
    let first = states.first()?;
    let dimension =
        |state: &MechanismState, i: usize| state.dimensions.get(i).copied().unwrap_or(f64::NAN);
    let widths: Vec<f64> = states.iter().map(|s| dimension(s, 0)).collect();
    let heights: Vec<f64> = states.iter().map(|s| dimension(s, 1)).collect();
    let new_width = math::max(&widths);
    let new_height = math::max(&heights);

    let mut lines = Vec::new();
    for (state, width) in states.iter().zip(&widths) {
        let x_offset = (new_width - width) / 2.0;
        for line in &state.lines {
            let mut start: Vec<Option<f64>> =
                line.start.iter().map(|&x| json_copy_number(x)).collect();
            let mut end: Vec<Option<f64>> = line.end.iter().map(|&x| json_copy_number(x)).collect();
            add_offset(&mut start, x_offset);
            add_offset(&mut end, x_offset);
            lines.push(MechanismLine {
                start,
                end,
                color: line.color.clone(),
                weight: json_copy_number(line.weight),
            });
        }
    }

    Some(MechanismState {
        background_color: first.background_color.clone(),
        dimensions: vec![new_width, new_height],
        lines,
    })
}
