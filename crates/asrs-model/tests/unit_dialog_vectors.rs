#![allow(clippy::disallowed_methods)]

//! Replays `unit-dialog/flows` (oracle/src/genUnitDialog.ts: upstream's unitConversion.html in
//! Blink) against `asrs_model::unit_conversion_dialog`. Each flow opens the dialog with the
//! recorded preset, then applies the recorded user steps; after the open and after every step the
//! port's selects, factor text, button classes and the replies, alerts and errors it produced
//! since the previous record must equal the recording.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::collections::BTreeMap;

use asrs_js::string::JsString;
use asrs_model::unit_conversion_dialog::{DialogEvent, SelectState, UnitConversionDialog};
use asrs_parity::cve1::Value as Cve;
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};

fn u16s(s: &JsString) -> Cve {
    Cve::String(Utf16String::from_units(s.units().to_vec()))
}

fn obj(pairs: Vec<(&str, Cve)>) -> Cve {
    Cve::Object(
        pairs
            .into_iter()
            .map(|(k, v)| {
                let mut key = Utf16String::new();
                key.push_str(k);
                (key, v)
            })
            .collect(),
    )
}

fn select(s: &SelectState) -> Cve {
    obj(vec![
        (
            "options",
            Cve::Array(
                s.options
                    .iter()
                    .map(|o| obj(vec![("text", u16s(&o.text)), ("value", u16s(&o.value))]))
                    .collect(),
            ),
        ),
        ("value", u16s(&s.value())),
        (
            "selectedIndex",
            Cve::Number(f64::from(s.selected_index).to_bits()),
        ),
        ("disabled", Cve::Bool(s.disabled)),
    ])
}

fn state(dialog: &UnitConversionDialog, events: &[DialogEvent]) -> Cve {
    let s = dialog.state();
    let mut replies = Vec::new();
    let mut alerts = Vec::new();
    let mut errors = Vec::new();
    for event in events {
        match event {
            DialogEvent::Reply(value) => replies.push(js_json::from_js(value)),
            DialogEvent::Alert(message) => alerts.push(u16s(message)),
            DialogEvent::Error(e) => errors.push(obj(vec![
                ("name", Cve::String(utf16_str(e.name))),
                ("message", Cve::String(utf16_str(&e.message))),
            ])),
        }
    }
    obj(vec![
        ("unitType", select(&s.unit_type)),
        ("fromUnit", select(&s.from_unit)),
        ("toUnit", select(&s.to_unit)),
        ("extraFactor", u16s(&s.extra_factor)),
        (
            "blurred",
            Cve::Array(s.blurred.iter().map(|b| Cve::Bool(*b)).collect()),
        ),
        ("replies", Cve::Array(replies)),
        ("alerts", Cve::Array(alerts)),
        ("errors", Cve::Array(errors)),
    ])
}

fn utf16_str(s: &str) -> Utf16String {
    let mut out = Utf16String::new();
    out.push_str(s);
    out
}

fn cve(record: &Record, key: &str) -> Cve {
    record
        .value(key)
        .unwrap_or_else(|e| panic!("line {}: {key}: {e}", record.line))
}

fn get<'a>(value: &'a Cve, key: &str) -> Option<&'a Cve> {
    match value {
        Cve::Object(pairs) => pairs.iter().find(|(k, _)| k.eq_str(key)).map(|(_, v)| v),
        _ => None,
    }
}

fn keys(record: &Record) -> Vec<String> {
    match &record.json {
        Json::Object(pairs) => pairs.iter().map(|(k, _)| k.to_string_lossy()).collect(),
        _ => Vec::new(),
    }
}

fn compare(what: &str, expected: &Cve, actual: &Cve) -> Result<(), String> {
    if vectors::values_match(expected, actual) {
        Ok(())
    } else {
        Err(format!(
            "{what}\n  expected {}\n       got {}",
            expected.encode(),
            actual.encode()
        ))
    }
}

fn apply(dialog: &mut UnitConversionDialog, step: &Cve) -> Result<Vec<DialogEvent>, String> {
    let op = match get(step, "op") {
        Some(Cve::String(s)) => s.to_string_lossy(),
        _ => return Err(format!("step without op: {}", step.encode())),
    };
    let value = get(step, "value");
    let expected_keys = if value.is_some() { 2 } else { 1 };
    if !matches!(step, Cve::Object(pairs) if pairs.len() == expected_keys) {
        return Err(format!("unexpected step fields: {}", step.encode()));
    }
    let text = || match value {
        Some(Cve::String(s)) => Ok(JsString::from_units(s.units().to_vec())),
        _ => Err(format!("{op} needs a string value")),
    };
    Ok(match op.as_str() {
        "type" => dialog.pick_unit_type(&text()?),
        "from" => dialog.pick_from_unit(&text()?),
        "to" => dialog.pick_to_unit(&text()?),
        "factor" => dialog.set_extra_factor(text()?),
        "confirm" if value.is_none() => dialog.click_confirm(),
        "exit" if value.is_none() => dialog.click_exit(),
        "key" => dialog.key_down(&text()?.to_string_lossy()),
        "focus" => match value {
            Some(Cve::Bool(b)) => dialog.set_focused(*b),
            _ => return Err("focus needs a boolean".to_owned()),
        },
        other => return Err(format!("unknown step op {other:?}")),
    })
}

#[test]
fn unit_conversion_dialog_flows() {
    let id = "unit-dialog/flows";
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut recorded: BTreeMap<String, usize> = BTreeMap::new();
    let mut compared: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    let mut i = 0;
    while i < records.len() {
        let open = &records[i];
        let flow = open.string("flow").unwrap_or_default();
        let mut end = i + 1;
        while end < records.len() && records[end].string("kind").as_deref() != Some("end") {
            end += 1;
        }
        let result = (|| -> Result<(), String> {
            *recorded.entry("open".to_owned()).or_default() += 1;
            if open.string("kind").as_deref() != Some("open")
                || keys(open) != ["kind", "flow", "preset", "state"]
            {
                return Err(format!("line {}: expected an open record", open.line));
            }
            let preset = js_json::to_js(&cve(open, "preset"))?;
            let (mut dialog, events) = UnitConversionDialog::open(&preset);
            let mut replied = events.iter().any(|e| matches!(e, DialogEvent::Reply(_)));
            compare("open", &cve(open, "state"), &state(&dialog, &events))?;
            *compared.entry("open".to_owned()).or_default() += 1;
            for record in &records[i + 1..end] {
                let kind = record.string("kind").unwrap_or_default();
                *recorded.entry(kind.clone()).or_default() += 1;
                if kind != "step" || keys(record) != ["kind", "flow", "step", "state"] {
                    return Err(format!("line {}: unexpected record {kind:?}", record.line));
                }
                if record.string("flow").as_deref() != Some(flow.as_str()) {
                    return Err(format!("line {}: record of another flow", record.line));
                }
                if replied {
                    return Err(format!(
                        "line {}: the port replied before this step",
                        record.line
                    ));
                }
                let step = cve(record, "step");
                let events = apply(&mut dialog, &step)?;
                replied = events.iter().any(|e| matches!(e, DialogEvent::Reply(_)));
                compare(
                    &format!("line {}: step {}", record.line, step.encode()),
                    &cve(record, "state"),
                    &state(&dialog, &events),
                )?;
                *compared.entry("step".to_owned()).or_default() += 1;
            }
            let Some(last) = records.get(end) else {
                return Err("flow has no end record".to_owned());
            };
            *recorded.entry("end".to_owned()).or_default() += 1;
            if keys(last) != ["kind", "flow", "replied"]
                || last.string("flow").as_deref() != Some(flow.as_str())
            {
                return Err(format!("line {}: bad end record", last.line));
            }
            if last.field("replied") != Some(&Json::Bool(replied)) {
                return Err(format!("line {}: replied is {replied}", last.line));
            }
            *compared.entry("end".to_owned()).or_default() += 1;
            Ok(())
        })();
        if let Err(e) = result {
            failures.push(format!("flow {flow:?}: {e}"));
        }
        i = end + 1;
    }
    assert!(
        failures.is_empty(),
        "{id}: {} flows differ\n\n{}",
        failures.len(),
        failures
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
    assert!(!recorded.is_empty(), "{id}: no records");
    assert_eq!(
        recorded, compared,
        "{id}: compared counts differ from recorded counts"
    );
}
