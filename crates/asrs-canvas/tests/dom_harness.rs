use asrs_canvas::text::{FixedAdvance, FontSpec, measure_text};
use asrs_parity::cve1::Value;
use asrs_parity::record_vectors;
use asrs_parity::vectors::Record;

fn text(value: &Value, key: &str) -> Result<String, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string_lossy())
        .ok_or_else(|| format!("measure has no {key}"))
}

record_vectors!(
    recording_canvas_measure_text,
    "dom/canvas/fixed-advance",
    |record: &Record| {
        if record.field("measure").is_none() {
            return Ok(());
        }
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
);

fn traces_round_trip(record: &Record) -> Result<(), String> {
    let Some(calls) = record.field("calls").and_then(|c| c.as_array()) else {
        return Ok(());
    };
    let canvas = record.string("canvas").unwrap_or_default();
    let mut text = String::new();
    for call in calls {
        text.push_str(&call.to_compact());
        text.push('\n');
    }
    let decoded = asrs_canvas::trace::decode(&text).map_err(|e| format!("{canvas}: {e}"))?;
    if asrs_canvas::trace::encode(&decoded) != text {
        return Err(format!(
            "{canvas}: the port re-encodes the trace differently"
        ));
    }
    Ok(())
}

record_vectors!(
    line_graph_trace_decodes,
    "dom/tabs/line-graph",
    traces_round_trip
);
record_vectors!(
    timeline_trace_decodes,
    "dom/hub/selection-sidebar",
    traces_round_trip
);
