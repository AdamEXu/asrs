#![allow(clippy::disallowed_methods)]

//! Replays the Blink `form/number-input` and `form/range-input` vectors through the value path
//! the input components use: a [`FieldValue`] over `asrs_tabs::form::NumberInput` for number
//! fields, and the slider's [`RangeModel`] over `asrs_tabs::form::RangeInput`. A record with
//! `op: null` parses a fresh element from its markup; every other record applies its op to the
//! current element. Each recorded read the components expose is compared; the reads they do
//! not expose are listed in [`UNEXPOSED`] and counted, and any other read, op or element fails.

use asrs_js::string::JsString;
use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};
use asrs_tabs::form::{NumberInput, RangeInput};
use asrs_ui::input::{FieldModel, FieldValue, RangeModel, number_accepts};

/// Reads upstream never makes, so the components have no API for them: upstream reads
/// `Number(input.value)`, never `valueAsNumber`, and never reads back a range's attributes.
const UNEXPOSED: [&str; 4] = ["valueAsNumber", "min", "max", "step"];

struct Number(NumberInput);

impl FieldModel for Number {
    fn value(&self) -> String {
        self.0.value().to_string_lossy()
    }

    fn set_value(&mut self, value: &str) {
        self.0.set_value(JsString::from(value));
    }

    fn accepts(&self, c: char) -> bool {
        number_accepts(c)
    }
}

struct Range(RangeInput);

impl RangeModel for Range {
    fn value(&self) -> String {
        self.0.value().to_string_lossy()
    }

    fn set_value(&mut self, value: &str) {
        self.0.set_value(JsString::from(value));
    }

    fn proportion(&self) -> f64 {
        unreachable!("no vector reads the thumb position")
    }

    fn value_at(&self, _: f64) -> String {
        unreachable!("no vector drags the thumb")
    }
}

enum Element {
    Number(FieldValue<Number>),
    Range(Range),
}

/// The value of attribute `name` in the first start tag of `markup`, for the flat markup the
/// form fixtures use (double-quoted values, no entities).
fn attribute<'a>(markup: &'a str, name: &str) -> Option<&'a str> {
    let tag = &markup[..markup.find('>')?];
    let start = tag.find(&format!(" {name}=\""))? + name.len() + 3;
    Some(&tag[start..start + tag[start..].find('"')?])
}

fn parse(markup: &str) -> Result<Element, String> {
    let attr = |name: &str| attribute(markup, name).unwrap_or("");
    match attribute(markup, "type") {
        Some("number") => Ok(Element::Number(FieldValue::new(Number(NumberInput::new(
            attr("value"),
        ))))),
        Some("range") => Ok(Element::Range(Range(range_input(
            attr("value"),
            attr("min"),
            attr("max"),
            attr("step"),
        )))),
        _ => Err(format!("no component for {markup}")),
    }
}

fn range_input(value: &str, min: &str, max: &str, _step: &str) -> RangeInput {
    RangeInput::new(value, min, max)
}

fn arg(op: &[Json], i: usize) -> Result<String, String> {
    op.get(i)
        .and_then(Json::as_str)
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| format!("op argument {i} is not a string"))
}

fn apply(element: &mut Element, op: &[Json]) -> Result<(), String> {
    let kind = arg(op, 0)?;
    let path = arg(op, 1)?;
    let value = arg(op, 2)?;
    match (element, kind.as_str(), path.as_str()) {
        (Element::Number(field), "set", "value") => field.assign(&value),
        (Element::Range(range), "set", "value") => range.set_value(&value),
        (Element::Range(range), "set", "min") => range.0.set_min(JsString::from(value.as_str())),
        (Element::Range(range), "set", "max") => range.0.set_max(JsString::from(value.as_str())),
        (_, kind, path) => return Err(format!("unsupported op {kind} {path}")),
    }
    Ok(())
}

#[derive(Default)]
struct Tally {
    records: usize,
    reads: usize,
    compared: usize,
    unexposed: usize,
    failures: Vec<String>,
}

fn read(element: &Element) -> (String, Option<String>) {
    match element {
        Element::Number(field) => (field.value(), Some(field.shown().to_owned())),
        Element::Range(range) => (range.value(), None),
    }
}

fn replay(id: &str) -> Tally {
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{id}: {e}"));
    let mut tally = Tally::default();
    let mut element: Option<Element> = None;
    for record in &records {
        tally.records += 1;
        if let Err(e) = check(record, &mut element, &mut tally) {
            tally
                .failures
                .push(format!("{id} line {}: {e}", record.line));
        }
    }
    tally
}

fn check(record: &Record, element: &mut Option<Element>, tally: &mut Tally) -> Result<(), String> {
    let markup = record.string("form").ok_or("record has no form")?;
    match record.field("op") {
        Some(Json::Null) => *element = Some(parse(&markup)?),
        Some(Json::Array(op)) => apply(element.as_mut().ok_or("op before markup")?, op)?,
        _ => return Err("record has no op".to_owned()),
    }
    let element = element.as_ref().ok_or("no element")?;
    let reads = record
        .field("values")
        .and_then(Json::as_array)
        .ok_or("record has no values")?;
    tally.reads += reads.len();
    for entry in reads {
        let path = entry
            .get("path")
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .ok_or("read without a path")?;
        if UNEXPOSED.contains(&path.as_str()) {
            tally.unexposed += 1;
            continue;
        }
        if path != "value" {
            return Err(format!("unknown read {path}"));
        }
        let expected = entry
            .get("value")
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .ok_or("value read did not record a string")?;
        let (value, shown) = read(element);
        tally.compared += 1;
        if value != expected {
            return Err(format!("value {value:?}, Blink {expected:?}"));
        }
        if let Some(shown) = shown
            && shown != value
        {
            return Err(format!("editor shows {shown:?} for value {value:?}"));
        }
    }
    Ok(())
}

fn run(id: &str) {
    let t = replay(id);
    eprintln!(
        "{id}: {} records, {} value reads compared, {} unexposed reads",
        t.records, t.compared, t.unexposed
    );
    assert!(t.failures.is_empty(), "{}", t.failures.join("\n"));
    assert_eq!(t.compared, t.records, "{id}: one value read per record");
    assert_eq!(
        t.compared + t.unexposed,
        t.reads,
        "{id}: every read accounted for"
    );
}

#[test]
#[ignore = "main's asrs_tabs::form is jsdom; runs once C08 (#157) lands, see #165"]
fn number_fields_follow_blink() {
    run("form/number-input");
}

#[test]
#[ignore = "main's asrs_tabs::form is jsdom; runs once C08 (#157) lands, see #165"]
fn sliders_follow_blink() {
    run("form/range-input");
}
