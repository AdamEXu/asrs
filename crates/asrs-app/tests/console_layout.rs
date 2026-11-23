#![allow(clippy::disallowed_methods)]

//! Lays the rows of `parity/specs/dom/console-layout/*` out as the Console view does and
//! compares every row's `clientHeight` and every text node's client rects with Blink's.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;
#[path = "../../asrs-tabs/tests/support/mod.rs"]
mod support;

use std::cell::RefCell;
use std::rc::Rc;

use asrs_app::views::console_layout::{CELL_PADDING, Cell, CellNode, ConsoleLayout, LINE_HEIGHT};
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::selection::SelectionMode;
use asrs_model::tab_controller::TabRenderer;
use asrs_parity::json::{self, Json};
use asrs_parity::{Value, vectors};
use asrs_tabs::console::controller::{ConsoleCommand, ConsoleDom, ConsoleLine, ConsoleRenderer};
use asrs_tabs::cx::TabCx;
use support::Host;

type Rect = [f64; 4];

fn load(path: &str) -> Json {
    let path = vectors::workspace_root()
        .join("parity/specs/dom")
        .join(path);
    json::parse(&std::fs::read_to_string(&path).unwrap()).unwrap()
}

fn lines(log: &Json) -> Vec<ConsoleLine> {
    log.as_array()
        .unwrap()
        .iter()
        .map(|call| {
            let call = call.as_array().unwrap();
            assert_eq!(call[1].as_str().unwrap().to_string_lossy(), "/Wrap");
            ConsoleLine {
                timestamp: call[2].as_f64().unwrap(),
                value: JsString::from_units(call[3].as_str().unwrap().units().to_vec()),
                is_error: false,
                is_warning: false,
            }
        })
        .collect()
}

fn array(value: &Value) -> &[Value] {
    match value {
        Value::Array(items) => items,
        other => panic!("not an array: {other:?}"),
    }
}

fn text(value: &Value) -> String {
    value.as_str().unwrap().to_string_lossy()
}

/// A cell's recorded `[text, rects]` per text node, without the empty rects of line breaks.
fn recorded(cell: &Value) -> (f64, Vec<(String, Vec<Rect>)>) {
    let [width, nodes] = array(cell) else {
        panic!("not [width, nodes]: {cell:?}");
    };
    let nodes = array(nodes)
        .iter()
        .map(|node| {
            let [data, rects] = array(node) else {
                panic!("not [text, rects]: {node:?}");
            };
            let rects = array(rects)
                .iter()
                .map(|r| {
                    let r: Vec<f64> = array(r).iter().map(|v| v.as_f64().unwrap()).collect();
                    [r[0], r[1], r[2], r[3]]
                })
                .filter(|r| r[2] > 0.0)
                .collect();
            (text(data), rects)
        })
        .collect();
    (width.as_f64().unwrap(), nodes)
}

fn laid_out(
    cell: &Cell,
    width: f64,
    right: bool,
    (top, height): (f64, f64),
    scale: f64,
) -> Vec<(String, Vec<Rect>)> {
    let px = |lu: i32| f64::from(lu) / 64.0 / scale;
    let mut out: Vec<(String, Vec<Rect>)> = Vec::new();
    let mut slot = Vec::new();
    for node in &cell.nodes {
        slot.push(out.len());
        if let CellNode::Text { text, .. } = node {
            out.push((text.clone(), Vec::new()));
        }
    }
    for (i, line) in cell.lines.iter().enumerate() {
        let offset = if right {
            width - 2.0 * CELL_PADDING - px(line.width)
        } else {
            0.0
        };
        for piece in &line.pieces {
            out[slot[piece.item]].1.push([
                CELL_PADDING + offset + px(piece.x),
                i as f64 * LINE_HEIGHT + top,
                px(piece.width),
                height,
            ]);
        }
    }
    out
}

fn same(a: &[(String, Vec<Rect>)], b: &[(String, Vec<Rect>)]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|((ta, ra), (tb, rb))| {
            ta == tb
                && ra.len() == rb.len()
                && ra
                    .iter()
                    .flatten()
                    .zip(rb.iter().flatten())
                    .all(|(x, y)| (x - y).abs() < 1e-6)
        })
}

fn renderer(host: &mut dyn TabCx, lines: Vec<ConsoleLine>) -> ConsoleRenderer {
    let mut renderer = ConsoleRenderer::new(Rc::new(RefCell::new(ConsoleDom::default())), true);
    let command = ConsoleCommand {
        key: JsValue::string("/Wrap"),
        key_available: true,
        lines,
        display_offset: 0.0,
        selection_mode: SelectionMode::Idle,
        selected_time: None,
        hovered_time: None,
    };
    TabRenderer::<dyn TabCx>::render(&mut renderer, host, &command).unwrap();
    renderer
}

fn replay(name: &str) -> (usize, usize) {
    let spec = load(&format!("console-layout/{name}.entry.json"));
    let window = spec.get("fixture").unwrap().get("window").unwrap();
    let number = |key: &str| window.get(key).and_then(Json::as_f64);
    let scale = number("devicePixelRatio").unwrap_or(1.0);
    let container = [
        300.0,
        88.0,
        number("width").unwrap() - 300.0,
        number("height").unwrap() - 88.0,
    ];
    let mut host = Host::default();
    let host: &mut dyn TabCx = &mut host;
    let mut renderer = renderer(host, lines(&load("logs/console-wrap.json")));

    let (mut rows_compared, mut nodes_compared) = (0, 0);
    let mut failures = Vec::new();
    let records = vectors::load(&format!("dom/console-layout/{name}")).unwrap();
    let captures: Vec<_> = records
        .iter()
        .filter(|r| {
            r.string("eval")
                .is_some_and(|e| e.starts_with("/*console-layout:rows*/"))
        })
        .collect();
    assert_eq!(captures.len(), 2, "{name}: a plain and a filtered capture");
    for (capture, filter) in captures.iter().zip(["", "ro"]) {
        renderer
            .filter_input(host, &JsString::from(filter))
            .unwrap();
        let layout = ConsoleLayout::default();
        let rows = &renderer.view().rows;
        layout.sync(container, scale as f32, rows);
        let value = capture.value("value").unwrap();
        let expected = array(&value);
        assert_eq!(expected.len(), rows.len(), "{name} {filter:?}: row count");
        for (i, (row, expected)) in rows.iter().zip(expected).enumerate() {
            let [height, time, value] = array(expected) else {
                panic!("not [height, time, value]: {expected:?}");
            };
            let (value_cell, time_cell) = layout.cells(i, row);
            let (time_width, time_nodes) = recorded(time);
            let (value_width, value_nodes) = recorded(value);
            let text_box = layout.text_box();
            let ours_time = laid_out(&time_cell, time_width, true, text_box, scale);
            let ours_value = laid_out(&value_cell, value_width, false, text_box, scale);
            let ours_height = layout.row(i).1;
            if ours_height != height.as_f64().unwrap()
                || !same(&ours_time, &time_nodes)
                || !same(&ours_value, &value_nodes)
            {
                failures.push(format!(
                    "{name} {filter:?} row {i}: {ours_height} px\n  ours {ours_time:?} {ours_value:?}\n  Blink {height:?} {time_nodes:?} {value_nodes:?}"
                ));
            }
            rows_compared += 1;
            nodes_compared += time_nodes.len() + value_nodes.len();
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {rows_compared} rows differ\n{}",
        failures.len(),
        failures
            .iter()
            .take(4)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    (rows_compared, nodes_compared)
}

#[test]
fn rows_lay_out_as_in_blink() {
    if !cfg!(target_os = "macos") {
        println!("console layout: skipped, the recordings use macOS's Courier");
        return;
    }
    for name in ["wrap", "wrap-2x", "wrap-narrow"] {
        let (rows, nodes) = replay(name);
        println!("{name}: {rows} rows, {nodes} text nodes equal");
        assert!(
            rows > 300 && nodes > 1000,
            "{name}: {rows} rows, {nodes} nodes"
        );
    }
}

#[test]
fn scroll_anchoring_follows_blink() {
    if !cfg!(target_os = "macos") {
        println!("console scroll anchoring: skipped, the recording uses macOS's Courier");
        return;
    }
    let spec = load("console-layout/anchor.entry.json");
    let steps = spec.get("fixture").and_then(|f| f.get("steps")).unwrap();
    let steps = steps.as_array().unwrap();
    let mut host = Host::default();
    let host: &mut dyn TabCx = &mut host;
    let mut renderer = renderer(host, lines(&load("logs/console-anchor.json")));
    let layout = ConsoleLayout::default();
    let container = [300.0, 88.0, 800.0, 562.0];
    layout.sync(container, 1.0, &renderer.view().rows);
    let records = vectors::load("dom/console-layout/anchor").unwrap();
    let mut expected = records.iter().filter(|r| {
        r.string("eval")
            .is_some_and(|e| e.starts_with("/*console-layout:top*/"))
    });
    let mut compared = 0;
    for step in &steps[1..] {
        if let Some(eval) = step.get("eval").and_then(Json::as_str) {
            let eval = eval.to_string_lossy();
            if let Some(rest) = eval.strip_prefix("/*console-layout:scroll ") {
                let top = rest.split("*/").next().unwrap().parse().unwrap();
                layout.set_scroll_top(top);
            }
        } else if let Some(event) = step.get("event") {
            let value = event.get("props").and_then(|p| p.get("value")).unwrap();
            let value = JsString::from_units(value.as_str().unwrap().units().to_vec());
            renderer.filter_input(host, &value).unwrap();
            layout.sync(container, 1.0, &renderer.view().rows);
        } else if step.get("capture").is_some() {
            let record = expected.next().expect("a recorded scroll position");
            let top = record.value("value").unwrap().as_f64().unwrap();
            assert_eq!(layout.scroll_top(), top, "line {}", record.line);
            compared += 1;
        }
    }
    assert!(expected.next().is_none());
    println!("{compared} scroll positions equal");
    assert!(compared > 20);
}
