use asrs_canvas::text::{FixedAdvance, FontSpec, measure_text};
use asrs_parity::cve1::Value;
use asrs_parity::tally::{Fields, Tally};
use asrs_parity::vectors;

fn text(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| format!("measure has no {key}"))
}

#[test]
fn recording_canvas_measure_text() {
    const ID: &str = "dom/canvas/fixed-advance";
    let records = vectors::load(ID).unwrap_or_else(|e| panic!("{e}"));
    let mut tally = Tally::by_first_key(ID, &records);
    tally.covered_elsewhere("capture", "nothing: it marks where the fixture measured");
    let measures = records.iter().filter(|r| r.field("measure").is_some());
    for record in measures {
        tally
            .compare("measure", record, compare_measure)
            .unwrap_or_else(|e| panic!("{ID} line {}: {e}", record.line));
    }
    tally.finish();
}

fn compare_measure(record: &Fields<'_>) -> Result<(), String> {
    let measure = record.value("measure").map_err(|e| e.0)?;
    let expected = record.value("metrics").map_err(|e| e.0)?;
    let font = text(&measure, "font")?;
    let Some(spec) = FontSpec::parse(&font) else {
        return match expected {
            Value::Null => Ok(()),
            _ => Err(format!(
                "{font:?}: the port ignores the font, the oracle does not"
            )),
        };
    };
    if expected == Value::Null {
        return Err(format!(
            "{font:?}: the oracle ignores the font, the port does not"
        ));
    }
    let m = measure_text(
        &mut FixedAdvance,
        &spec,
        &text(&measure, "align")?,
        &text(&measure, "baseline")?,
        &text(&measure, "text")?,
    );
    let fields = [
        ("width", m.width),
        ("actualBoundingBoxLeft", m.actual_bounding_box_left),
        ("actualBoundingBoxRight", m.actual_bounding_box_right),
        ("actualBoundingBoxAscent", m.actual_bounding_box_ascent),
        ("actualBoundingBoxDescent", m.actual_bounding_box_descent),
        ("fontBoundingBoxAscent", m.font_bounding_box_ascent),
        ("fontBoundingBoxDescent", m.font_bounding_box_descent),
        ("hangingBaseline", m.hanging_baseline),
        ("alphabeticBaseline", m.alphabetic_baseline),
        ("ideographicBaseline", m.ideographic_baseline),
    ];
    for (key, actual) in fields {
        let want = expected
            .get(key)
            .and_then(Value::as_f64)
            .ok_or_else(|| format!("{font:?}: oracle metrics have no {key}"))?;
        if want.to_bits() != actual.to_bits() {
            return Err(format!(
                "{font:?} {:?}: {key} oracle {want:?}, port {actual:?}",
                text(&measure, "text")?
            ));
        }
    }
    Ok(())
}

fn traces_round_trip(id: &str) {
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut decoded = 0;
    for record in records.iter().filter(|r| r.field("canvas").is_some()) {
        let mut text = String::new();
        let calls = record.field("calls").and_then(|c| c.as_array());
        for call in calls.unwrap_or_else(|| panic!("{id} line {}: no calls", record.line)) {
            text.push_str(&call.to_compact());
            text.push('\n');
        }
        let trace = asrs_canvas::trace::decode(&text)
            .unwrap_or_else(|e| panic!("{id} line {}: {e}", record.line));
        assert_eq!(
            asrs_canvas::trace::encode(&trace),
            text,
            "{id} line {}: the port re-encodes the trace differently",
            record.line
        );
        decoded += 1;
    }
    let mut tally = Tally::by_first_key(id, &records);
    for kind in KINDS_OF_OTHER_HARNESSES {
        if records.iter().any(|r| r.field(kind).is_some()) {
            tally.covered_elsewhere(kind, "asrs-hub's dom_tabs and selection harnesses");
        }
    }
    for _ in 0..decoded {
        tally.compared("canvas");
    }
    tally.finish();
}

const KINDS_OF_OTHER_HARNESSES: [&str; 6] =
    ["main", "capture", "command", "state", "selection", "dom"];

#[test]
fn line_graph_trace_decodes() {
    traces_round_trip("dom/tabs/line-graph");
}

#[test]
fn timeline_trace_decodes() {
    traces_round_trip("dom/hub/selection-sidebar");
}
