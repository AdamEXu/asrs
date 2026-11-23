//! Replays the Blink `form/number-input` and `form/range-input` vectors through the value path
//! the input components use: a [`FieldValue`] over `asrs_tabs::form::NumberInput` for number
//! fields, and the slider's [`RangeModel`] over `asrs_tabs::form::RangeInput`. A record with
//! `op: null` parses a fresh element from its markup; every other record applies its op to the
//! current element. Each recorded read the components expose is compared; the reads they do
//! not expose are listed in [`UNEXPOSED`] and counted, and any other read, op or element fails.

use asrs_app::views::number::FormNumber;
use asrs_app::views::range::FormRange;
use asrs_js::string::JsString;
use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};
use asrs_tabs::form::{NumberInput, RangeInput};
use asrs_ui::input::{FieldValue, RangeControl, RangeModel, SliderEvent, SliderTrack};

/// Reads upstream never makes, so the components have no API for them: upstream reads
/// `Number(input.value)`, never `valueAsNumber`, and never reads back a range's attributes.
const UNEXPOSED: [&str; 4] = ["valueAsNumber", "min", "max", "step"];

enum Element {
    Number(FieldValue<FormNumber>),
    Range(FormRange),
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
        Some("number") => Ok(Element::Number(FieldValue::new(FormNumber(
            NumberInput::new(attr("value")),
        )))),
        Some("range") => Ok(Element::Range(FormRange(range_input(
            attr("value"),
            attr("min"),
            attr("max"),
            attr("step"),
        )))),
        _ => Err(format!("no component for {markup}")),
    }
}

fn range_input(value: &str, min: &str, max: &str, step: &str) -> RangeInput {
    RangeInput::new(value, min, max, step)
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
        (Element::Range(range), "setAttribute", "step") => {
            range.0.set_step(JsString::from(value.as_str()))
        }
        (Element::Range(range), "setAttribute", "value") => {
            range.0.set_value_attribute(JsString::from(value.as_str()))
        }
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
fn number_fields_follow_blink() {
    run("form/number-input");
}

#[test]
fn sliders_follow_blink() {
    run("form/range-input");
}

/// Replays the `form/range-pointer*` vectors, trusted pointer and keyboard input to the video
/// timeline slider, through the control the slider drives. `events` is compared as recorded,
/// apart from `keydown`, which the window dispatches.
fn run_pointer(id: &str, scale: f32) {
    let spec = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../parity/specs")
        .join(format!("{id}.entry.json"));
    let spec = std::fs::read_to_string(&spec).unwrap_or_else(|e| panic!("{id}: {e}"));
    let spec = asrs_parity::json::parse(&spec).expect("spec is JSON");
    let fixture = spec.get("fixture").expect("fixture");
    assert_eq!(
        fixture.get("scale").and_then(Json::as_f64),
        Some(f64::from(scale))
    );
    let steps = fixture
        .get("steps")
        .and_then(Json::as_array)
        .expect("steps");
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{id}: {e}"));
    let (mut compared, mut dragged) = (0, 0);
    let mut current = None;
    for record in &records {
        let what = format!("{id} line {}", record.line);
        let step = &steps[record.field("step").and_then(Json::as_f64).expect("step") as usize];
        let width = step.get("width").and_then(Json::as_f64).expect("width") as f32;
        let at = SliderTrack {
            track: width * scale,
            thumb: 4.0 * scale,
        };
        let op = match record.field("op") {
            Some(Json::Null) => {
                let mut control = RangeControl::new(FormRange(RangeInput::new("0", "1", "1", "1")));
                let sets = step.get("set").and_then(Json::as_array).expect("set");
                for set in sets {
                    set_property(&mut control, set.as_array().expect("pair"));
                }
                current = Some((control, false));
                None
            }
            Some(Json::Array(op)) => Some(op),
            _ => panic!("{what}: no op"),
        };
        let (control, focused) = current.as_mut().expect("an op before its step");
        let mut events: Vec<&str> = Vec::new();
        if let Some(op) = op {
            let name = arg(op, 0).expect("op kind");
            let x = || op[1].as_f64().expect("x") as f32 * scale;
            let was_focused = *focused;
            let mut push = |event: Option<SliderEvent>| {
                events.extend(event.map(|event| match event {
                    SliderEvent::Input => "input",
                    SliderEvent::Change => "change",
                }))
            };
            match name.as_str() {
                "press" => {
                    dragged += 1;
                    push(control.press(x(), at));
                    *focused |= !control.disabled();
                }
                "move" => push(control.drag(x(), at)),
                "release" => push(control.release()),
                "key" => {
                    if *focused {
                        let key = arg(op, 1).expect("key");
                        events.push("keydown");
                        if control.key_down(&key) {
                            events.extend(["input", "change"]);
                        }
                    }
                }
                "set" => {
                    set_property(control, &op[1..]);
                    *focused &= !control.disabled();
                }
                "focus" => *focused = !control.disabled(),
                "blur" => *focused = false,
                other => panic!("{what}: unknown op {other}"),
            }
            if *focused != was_focused {
                events.push(if *focused { "focus" } else { "blur" });
            }
        }
        let recorded: Vec<String> = record
            .field("events")
            .and_then(Json::as_array)
            .expect("events")
            .iter()
            .map(|e| e.as_str().expect("event").to_string_lossy())
            .collect();
        assert_eq!(recorded, events, "{what}: events");
        assert_eq!(
            record.string("value"),
            Some(control.model().value()),
            "{what}: value"
        );
        assert_eq!(
            record.field("focused"),
            Some(&Json::Bool(*focused)),
            "{what}: focused"
        );
        compared += 1;
    }
    assert_eq!(compared, records.len());
    assert!(dragged > 10, "{id}: {dragged} presses");
}

fn set_property(control: &mut RangeControl<FormRange>, pair: &[Json]) {
    let name = arg(pair, 0).expect("property");
    match (name.as_str(), &pair[1]) {
        ("max", Json::String(value)) => {
            let max = JsString::from_units(value.units().to_vec());
            control.model_mut().0.set_max(max);
        }
        ("value", Json::String(value)) => control.model_mut().set_value(&value.to_string_lossy()),
        ("disabled", Json::Bool(disabled)) => control.set_disabled(*disabled),
        other => panic!("unknown property {other:?}"),
    }
}

#[test]
fn dragged_sliders_follow_blink() {
    run_pointer("form/range-pointer", 1.0);
    run_pointer("form/range-pointer-2x", 2.0);
}
