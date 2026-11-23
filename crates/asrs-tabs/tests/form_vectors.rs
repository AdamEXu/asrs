#![allow(clippy::disallowed_methods)]

//! Replays the Blink `form/*` vectors against `asrs_tabs::form`. Each fixture step parses one
//! element from markup (a record with `op: null`), then applies the recorded ops in order; every
//! record lists the property reads Chromium made after its op, and each one is compared,
//! including the reads that threw. An unknown element, op or read fails the record.

use asrs_js::string::JsString;
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::record_vectors;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::Record;
use asrs_tabs::form::css::{Property, Style};
use asrs_tabs::form::{
    NumberInput, OptGroup, RangeInput, Select, SelectItem, SelectOption, insertable_number_text,
};

enum Element {
    Number(NumberInput),
    Range(RangeInput),
    Select(Select),
    Div(Style),
}

struct Tag {
    name: String,
    closing: bool,
    attributes: Vec<(String, String)>,
    text: String,
}

impl Tag {
    fn attribute(&self, name: &str) -> Option<&str> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// The start tags of `markup` with the text that follows each, for the flat markup the form
/// fixtures use (double-quoted attributes, no entities, no nesting beyond `select > option`).
fn tags(markup: &str) -> Result<Vec<Tag>, String> {
    let mut out = Vec::new();
    let mut rest = markup;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let end = rest.find('>').ok_or("unterminated tag")?;
        let inside = &rest[..end];
        rest = &rest[end + 1..];
        if let Some(name) = inside.strip_prefix('/') {
            out.push(Tag {
                name: name.to_owned(),
                closing: true,
                attributes: Vec::new(),
                text: String::new(),
            });
            continue;
        }
        let (name, mut attrs) = inside.split_once(' ').unwrap_or((inside, ""));
        let mut attributes = Vec::new();
        loop {
            attrs = attrs.trim_start();
            if attrs.is_empty() {
                break;
            }
            if let Some(after) = attrs.strip_prefix("disabled") {
                attributes.push(("disabled".to_owned(), String::new()));
                attrs = after;
                continue;
            }
            let (attr, after) = attrs.split_once("=\"").ok_or("unquoted attribute")?;
            let (value, after) = after.split_once('"').ok_or("unterminated attribute")?;
            attributes.push((attr.to_owned(), value.to_owned()));
            attrs = after;
        }
        let text = &rest[..rest.find('<').unwrap_or(rest.len())];
        out.push(Tag {
            name: name.to_owned(),
            closing: false,
            attributes,
            text: text.to_owned(),
        });
    }
    Ok(out)
}

fn option_of(tag: &Tag) -> SelectOption {
    SelectOption::new(tag.attribute("value"), &tag.text)
        .disabled(tag.attribute("disabled").is_some())
}

/// The list items of a select's markup: options, and optgroups with the options inside them.
fn select_items(tags: &[Tag]) -> Result<Vec<SelectItem>, String> {
    let mut items = Vec::new();
    let mut in_group = false;
    for t in tags {
        match (t.name.as_str(), t.closing) {
            ("option", false) => items.push(SelectItem::Option {
                option: option_of(t),
                grouped: in_group,
            }),
            ("optgroup", false) => {
                items.push(SelectItem::Group(
                    OptGroup::new(t.attribute("label").unwrap_or(""))
                        .disabled(t.attribute("disabled").is_some()),
                ));
                in_group = true;
            }
            ("optgroup", true) => in_group = false,
            ("option" | "select", true) => {}
            (other, _) => return Err(format!("unexpected <{other}> in a select")),
        }
    }
    Ok(items)
}

/// `optgroup.innerHTML = markup`: every option is removed, then each new one appended.
fn set_group_options(select: &mut Select, group: usize, markup: &str) -> Result<(), String> {
    select.clear_group(group);
    for tag in tags(markup)? {
        match (tag.name.as_str(), tag.closing) {
            ("option", false) => select.append_to_group(group, option_of(&tag)),
            ("option", true) => {}
            (other, _) => return Err(format!("unexpected <{other}> in an optgroup")),
        }
    }
    Ok(())
}

fn child_path(path: &str, prop: &str) -> Option<usize> {
    path.strip_prefix("children.")?
        .strip_suffix(prop)?
        .parse()
        .ok()
}

fn parse(markup: &str) -> Result<Element, String> {
    let tags = tags(markup)?;
    let root = tags.first().ok_or("empty markup")?;
    let attr = |name: &str| root.attribute(name).unwrap_or("");
    match (root.name.as_str(), root.attribute("type")) {
        ("input", Some("number")) => Ok(Element::Number(NumberInput::new(attr("value")))),
        ("input", Some("range")) => Ok(Element::Range(RangeInput::new(
            attr("value"),
            attr("min"),
            attr("max"),
            attr("step"),
        ))),
        ("select", None) => Ok(Element::Select(Select::with_items(select_items(
            &tags[1..],
        )?))),
        ("div", None) => Ok(Element::Div(Style::new())),
        _ => Err(format!("no model for {markup}")),
    }
}

fn string_arg(op: &[Json], i: usize) -> Result<JsString, String> {
    op.get(i)
        .and_then(Json::as_str)
        .map(|s| JsString::from_units(s.units().to_vec()))
        .ok_or_else(|| format!("op argument {i} is not a string"))
}

fn bool_arg(op: &[Json], i: usize) -> Result<bool, String> {
    match op.get(i) {
        Some(Json::Bool(b)) => Ok(*b),
        _ => Err(format!("op argument {i} is not a boolean")),
    }
}

fn number_arg(op: &[Json], i: usize) -> Result<f64, String> {
    let json = op.get(i).ok_or("missing op argument")?;
    Value::from_json(json)
        .map_err(|e| e.0)?
        .as_f64()
        .ok_or_else(|| format!("op argument {i} is not a number"))
}

fn apply(element: &mut Element, op: &[Json]) -> Result<(), String> {
    let kind = op
        .first()
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy());
    let target = op
        .get(1)
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy());
    match (element, kind.as_deref(), target.as_deref()) {
        (Element::Number(n), Some("set"), Some("value")) => n.set_value(string_arg(op, 2)?),
        (Element::Range(r), Some("set"), Some("value")) => r.set_value(string_arg(op, 2)?),
        (Element::Range(r), Some("set"), Some("min")) => r.set_min(string_arg(op, 2)?),
        (Element::Range(r), Some("set"), Some("max")) => r.set_max(string_arg(op, 2)?),
        (Element::Range(r), Some("setAttribute"), Some("step")) => r.set_step(string_arg(op, 2)?),
        (Element::Range(r), Some("setAttribute"), Some("value")) => {
            r.set_value_attribute(string_arg(op, 2)?)
        }
        (Element::Select(s), Some("set"), Some("value")) => s.set_value(&string_arg(op, 2)?),
        (Element::Select(s), Some("set"), Some("selectedIndex")) => {
            s.set_selected_index(number_arg(op, 2)? as i32)
        }
        (Element::Select(s), Some("remove"), _) => s.remove(number_arg(op, 1)? as usize),
        (Element::Select(s), Some("set"), Some(path))
            if let Some(group) = child_path(path, ".innerHTML") =>
        {
            set_group_options(s, group, &string_arg(op, 2)?.to_string_lossy())?
        }
        (Element::Select(s), Some("set"), Some(path))
            if let Some(group) = child_path(path, ".disabled") =>
        {
            s.set_group_disabled(group, bool_arg(op, 2)?)
        }
        (Element::Select(s), Some("set"), Some(path))
            if let Some(index) = path
                .strip_prefix("options.")
                .and_then(|p| p.strip_suffix(".disabled"))
                .and_then(|i| i.parse().ok()) =>
        {
            s.set_option_disabled(index, bool_arg(op, 2)?)
        }
        (Element::Select(s), Some("appendElement"), Some("option")) => {
            let props = op
                .get(2)
                .and_then(Json::as_array)
                .ok_or("no option props")?;
            let [prop] = props else {
                return Err("an option with other than one property".to_owned());
            };
            let pair = prop.as_array().ok_or("option prop is not a pair")?;
            if pair.first().and_then(Json::as_str) != Some(&Utf16String::from("innerText")) {
                return Err("an option prop other than innerText".to_owned());
            }
            s.append(SelectOption::from_inner_text(&string_arg(pair, 1)?));
        }
        (Element::Div(style), Some("set"), Some(path)) => {
            let property = path
                .strip_prefix("style.")
                .and_then(Property::from_idl)
                .ok_or_else(|| format!("no model of {path}"))?;
            style.set(property, &string_arg(op, 2)?);
        }
        _ => return Err(format!("unknown op {kind:?} {target:?}")),
    }
    Ok(())
}

enum Read {
    Value(Value),
    Threw(String),
}

fn string(s: &JsString) -> Read {
    Read::Value(Value::String(Utf16String::from_units(s.units().to_vec())))
}

fn number(x: f64) -> Read {
    Read::Value(Value::number(x))
}

fn read(element: &Element, path: &str) -> Result<Read, String> {
    Ok(match (element, path) {
        (Element::Number(n), "value") => string(n.value()),
        (Element::Number(n), "valueAsNumber") => number(n.value_as_number()),
        (Element::Range(r), "value") => string(r.value()),
        (Element::Range(r), "valueAsNumber") => number(r.value_as_number()),
        (Element::Range(r), "min") => string(r.min()),
        (Element::Range(r), "max") => string(r.max()),
        (Element::Range(r), "step") => string(r.step()),
        (Element::Select(s), "value") => string(&s.value()),
        (Element::Select(s), "selectedIndex") => number(f64::from(s.selected_index())),
        (Element::Select(s), "options.length") => number(s.length() as f64),
        (Element::Select(s), path) if path.starts_with("options.") => {
            let (index, prop) = path["options.".len()..]
                .split_once('.')
                .ok_or_else(|| format!("unknown read {path}"))?;
            let index: usize = index.parse().map_err(|_| format!("unknown read {path}"))?;
            let Some(option) = s.option(index) else {
                return Ok(Read::Threw(format!(
                    "TypeError: Cannot read properties of undefined (reading '{prop}')"
                )));
            };
            match prop {
                "value" => string(&option.value()),
                "text" | "label" => string(&option.text()),
                "textContent" | "innerText" => string(option.text_content()),
                "disabled" => Read::Value(Value::Bool(option.is_disabled())),
                _ => return Err(format!("unknown read {path}")),
            }
        }
        (Element::Div(style), "style.cssText") => string(&style.css_text()),
        (Element::Div(style), path) => {
            let property = path
                .strip_prefix("style.")
                .and_then(Property::from_idl)
                .ok_or_else(|| format!("unknown read {path}"))?;
            string(&style.get(property))
        }
        _ => return Err(format!("unknown read {path}")),
    })
}

fn replay(element: &mut Option<Element>, record: &Record) -> Result<(), String> {
    let markup = record.string("form").ok_or("record has no form")?;
    match record.field("op") {
        Some(Json::Null) => *element = Some(parse(&markup)?),
        Some(op) => apply(
            element.as_mut().ok_or("an op before the element")?,
            op.as_array().ok_or("op is not an array")?,
        )?,
        None => return Err("record has no op".to_owned()),
    }
    let element = element.as_ref().ok_or("no element")?;
    let values = record
        .field("values")
        .and_then(Json::as_array)
        .ok_or("record has no values")?;
    if values.is_empty() {
        return Err("a record with no reads".to_owned());
    }
    let mut failures = Vec::new();
    for entry in values {
        let path = entry
            .get("path")
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .ok_or("read has no path")?;
        let expected = match (entry.get("value"), entry.get("threw")) {
            (Some(v), None) => Read::Value(Value::from_json(v).map_err(|e| e.0)?),
            (None, Some(t)) => Read::Threw(
                t.as_str()
                    .map(|s| s.to_string_lossy())
                    .ok_or("threw is not a string")?,
            ),
            _ => return Err(format!("read {path} has neither value nor threw")),
        };
        let actual = read(element, &path)?;
        let same = match (&expected, &actual) {
            (Read::Value(e), Read::Value(a)) => e.encode() == a.encode(),
            (Read::Threw(e), Read::Threw(a)) => e == a,
            _ => false,
        };
        if !same {
            let show = |r: &Read| match r {
                Read::Value(v) => v.encode(),
                Read::Threw(t) => format!("threw {t}"),
            };
            failures.push(format!(
                "{path}: expected {} actual {}",
                show(&expected),
                show(&actual)
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{markup} after {}:\n{}",
            record
                .field("op")
                .map(|j| format!("{j:?}"))
                .unwrap_or_default(),
            failures.join("\n")
        ))
    }
}

macro_rules! form_vectors {
    ($name:ident, $id:expr) => {
        record_vectors!($name, $id, {
            let mut element = None;
            move |record: &Record| replay(&mut element, record)
        });
    };
}

form_vectors!(number_input, "form/number-input");
form_vectors!(range_input, "form/range-input");
form_vectors!(select, "form/select");
form_vectors!(css_value, "form/css-value");

record_vectors!(number_insert, "form/number-insert", |record: &Record| {
    let Json::Object(fields) = &record.json else {
        return Err("record is not an object".to_owned());
    };
    let names: Vec<String> = fields.iter().map(|(k, _)| k.to_string_lossy()).collect();
    if names != ["shown", "start", "end", "insert", "result", "value"] {
        return Err(format!("unexpected fields {names:?}"));
    }
    let units = |name: &str| {
        let text = record.field(name).and_then(Json::as_str);
        Ok::<_, String>(
            text.ok_or(format!("{name} is not a string"))?
                .units()
                .to_vec(),
        )
    };
    let index = |name: &str| match record.field(name) {
        Some(Json::Number(n)) if n.fract() == 0.0 && *n >= 0.0 => Ok(*n as usize),
        other => Err(format!("{name} is {other:?}")),
    };
    let (shown, insert) = (units("shown")?, units("insert")?);
    let (left, right) = (&shown[..index("start")?], &shown[index("end")?..]);
    let kept = insertable_number_text(left, &insert, right);
    let result = [left, &kept, right].concat();
    let mut input = NumberInput::default();
    input.set_visible_value(&JsString::from_units(result.clone()));
    let expected = (units("result")?, units("value")?);
    if (&result, input.value().units()) == (&expected.0, &expected.1[..]) {
        return Ok(());
    }
    let text = |u: &[u16]| String::from_utf16_lossy(u);
    Err(format!(
        "{:?} in {:?}: expected {:?} with value {:?}, got {:?} with value {:?}",
        text(&insert),
        (text(left), text(right)),
        text(&expected.0),
        text(&expected.1),
        text(&result),
        input.value().to_string_lossy()
    ))
});

#[test]
fn arrow_keys_step_number_inputs_as_in_blink() {
    let specs = std::fs::read_to_string(
        asrs_parity::vectors::workspace_root().join("parity/specs/form/number-keys.entry.json"),
    )
    .unwrap();
    let spec = asrs_parity::json::parse(&specs).unwrap();
    let steps = spec.get("fixture").and_then(|f| f.get("steps")).unwrap();
    let markup: Vec<String> = steps
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.get("markup").and_then(Json::as_str).unwrap().to_string_lossy())
        .collect();
    let attribute = |step: usize, name: &str| {
        let rest = markup[step].split_once(&format!(" {name}=\""))?.1;
        Some(rest.split_once('"').unwrap().0.to_owned())
    };
    let records = asrs_parity::vectors::load("form/number-keys").unwrap();
    let mut input = NumberInput::default();
    let mut visible = String::new();
    let mut committed = JsString::new();
    let mut stepped = 0;
    for record in records.iter() {
        let at = format!("line {}", record.line);
        let op = record.field("op").unwrap();
        let events: Vec<String> = record
            .field("events")
            .and_then(Json::as_array)
            .unwrap()
            .iter()
            .map(|e| e.as_str().unwrap().to_string_lossy())
            .filter(|e| e == "input" || e == "change")
            .collect();
        let mut expected: Vec<&str> = Vec::new();
        if let Some(op) = op.as_array() {
            let word = |i: usize| op[i].as_str().unwrap().to_string_lossy();
            match word(0).as_str() {
                "set" if word(1) == "step" => input.set_step(JsString::from(word(2))),
                "set" => {
                    assert_eq!(word(1), "value", "{at}");
                    input.set_value(JsString::from(word(2)));
                    visible = input.value().to_string_lossy();
                    committed = input.value().clone();
                }
                "char" => {
                    visible.push_str(&word(1));
                    input.set_visible_value(&JsString::from(visible.as_str()));
                    expected.push("input");
                }
                "key" => {
                    let alt = op.get(3).is_some_and(|m| m.to_compact().contains("alt"));
                    let up = word(1) == "ArrowUp";
                    if (up || !alt) && input.arrow_key(up) {
                        visible = input.value().to_string_lossy();
                        committed = input.value().clone();
                        expected.extend(["input", "change"]);
                    }
                    stepped += 1;
                }
                "blur" => {
                    if committed != *input.value() {
                        committed = input.value().clone();
                        expected.push("change");
                    }
                }
                "focus" => {}
                other => panic!("{at}: unknown op {other}"),
            }
        } else {
            let step = record.field("step").and_then(Json::as_f64).unwrap() as usize;
            input = NumberInput::new(&attribute(step, "value").unwrap_or_default());
            input.set_step(JsString::from(attribute(step, "step").unwrap_or_default()));
            visible = input.value().to_string_lossy();
            committed = input.value().clone();
        }
        let value = record.string("value").unwrap();
        assert_eq!(input.value().to_string_lossy(), value, "{at}: value");
        assert_eq!(events, expected, "{at}: events");
    }
    assert!(stepped > 800, "{stepped} keys");
}
