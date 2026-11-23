//! Compares `asrs_canvas::text::line_break` with Blink's line breaking: `dom/line-break/*`,
//! written by `oracle/tools/u14Specs.mjs`.

use asrs_canvas::text::line_break::LineBreaks;
use asrs_parity::vectors::load;
use asrs_parity::{Record, Value};

fn recorded(id: &str) -> Vec<Value> {
    let records = load(id).unwrap_or_else(|e| panic!("{id}: {e}"));
    let is_capture = |r: &&Record| {
        r.field("eval")
            .and_then(|e| e.as_str()?.as_string())
            .is_some_and(|e| e.starts_with("/*line-break*/"))
    };
    records
        .iter()
        .filter(is_capture)
        .map(|r| r.value("value").expect("a value"))
        .collect()
}

fn items(value: &Value) -> &[Value] {
    match value {
        Value::Array(items) => items,
        other => panic!("not an array: {}", other.kind()),
    }
}

fn text(value: &Value) -> String {
    value.as_str().expect("a string").to_string_lossy()
}

/// Where lines start when every line takes as little as it can.
fn line_starts(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut breaks = LineBreaks::new(text);
    while let Some(&next) = breaks.ends().front()
        && next < text.len()
    {
        out.push(next);
        breaks.restart(next);
    }
    out
}

#[test]
fn every_ascii_pair_breaks_where_blink_breaks_it() {
    let captures = recorded("dom/line-break/pairs");
    let [pairs] = &captures[..] else {
        panic!("{} captures", captures.len());
    };
    let mut breaks = 0;
    assert_eq!(items(pairs).len(), 94);
    for (i, row) in items(pairs).iter().enumerate() {
        let row = text(row);
        assert_eq!(row.len(), 94);
        for (j, expected) in row.bytes().enumerate() {
            let (a, b) = (33 + i as u8, 33 + j as u8);
            let string = format!("x{}{}x", char::from(a), char::from(b));
            let breaks_here = line_starts(&string).contains(&2);
            assert_eq!(breaks_here, expected == b'1', "{string}");
            breaks += usize::from(breaks_here);
        }
    }
    assert!(breaks > 100, "{breaks} breaks");
}

#[test]
fn strings_break_where_blink_breaks_them() {
    let captures = recorded("dom/line-break/strings");
    assert!(captures.len() > 300);
    let mut failures = Vec::new();
    for capture in &captures {
        let [string, breaks] = items(capture) else {
            panic!("not [text, breaks]");
        };
        let string = text(string);
        let expected: Vec<usize> = items(breaks)
            .iter()
            .map(|b| b.as_f64().expect("an offset") as usize)
            .collect();
        let actual: Vec<usize> = line_starts(&string)
            .into_iter()
            .map(|byte| string[..byte].encode_utf16().count())
            .collect();
        if expected != actual {
            let at = |offsets: &[usize]| {
                let units: Vec<u16> = string.encode_utf16().collect();
                let around = |&i: &usize| String::from_utf16_lossy(&units[i - 1..=i]);
                offsets.iter().map(around).collect::<Vec<_>>()
            };
            let only = |a: &[usize], b: &[usize]| {
                let mut a = a.to_vec();
                a.retain(|i| !b.contains(i));
                at(&a)
            };
            failures.push((
                string.clone(),
                only(&expected, &actual),
                only(&actual, &expected),
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} strings (only upstream, only port): {failures:?}",
        failures.len(),
        captures.len()
    );
}
