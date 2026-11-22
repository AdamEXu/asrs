#![allow(clippy::disallowed_methods)]

//! The one source list read `ListLayout` does not answer: `updateItem`'s `clientWidth` of a
//! detached `div.type-name`. The `dom/source-list` fixtures (Blink, 1100 px window, 1x) capture
//! every item's type name and the `--type-width` Blink set from that read; the port's text
//! stack must give the same width for every one.

#![cfg(target_os = "macos")]

use asrs_app::views::source_list::type_name_client_width;
use asrs_parity::Value;
use asrs_parity::vectors;
use asrs_ui::text::Measurer;

const CASES: [&str; 9] = [
    "field-2d",
    "field-3d",
    "field-3d-no-assets",
    "line-graph",
    "mechanism",
    "points",
    "statistics",
    "swerve",
    "view-drag",
];
const BODY_WIDTH: f64 = 1100.0;

fn string(value: &Value) -> String {
    value
        .as_str()
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| panic!("not a string: {value:?}"))
}

fn array(value: &Value) -> &[Value] {
    match value {
        Value::Array(items) => items,
        other => panic!("not an array: {other:?}"),
    }
}

#[test]
fn type_name_widths_match_blink() {
    let mut measurer = Measurer::default();
    let mut compared = 0;
    let mut named = 0;
    for case in CASES {
        let id = format!("dom/source-list/{case}");
        for record in vectors::load(&id).unwrap() {
            if !record
                .string("eval")
                .is_some_and(|e| e.starts_with("/*sl:widths*/"))
            {
                continue;
            }
            for root in array(&record.value("value").unwrap()) {
                for item in array(&array(root)[1]) {
                    let item = array(item);
                    let (text, blink) = (string(&item[0]), string(&item[1]));
                    if blink.is_empty() {
                        continue;
                    }
                    let width = type_name_client_width(&mut measurer, &text, BODY_WIDTH, 1.0);
                    let port = if width > 0.0 { width + 3.0 } else { 0.0 };
                    assert_eq!(format!("{port}px"), blink, "{id}: {text:?}");
                    compared += 1;
                    named += usize::from(!text.is_empty());
                }
            }
        }
    }
    assert!(named > 0, "no type names compared");
    eprintln!("{compared} type widths, {named} with a name");
}
