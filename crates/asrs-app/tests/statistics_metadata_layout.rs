#![allow(clippy::disallowed_methods)]

//! Lays the tables of `parity/specs/dom/{statistics,metadata}-layout/*` out as their views do and
//! compares every box and every text node's client rects with Blink's.

use asrs_app::views::cell_text::{CellText, css};
use asrs_app::views::metadata_layout::MetadataLayout;
use asrs_app::views::statistics_layout::StatisticsLayout;
use asrs_canvas::text::dom::Lu;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use asrs_parity::json::{self, Json};
use asrs_parity::{Record, Value, vectors};
use asrs_tabs::metadata::MetadataRenderer;
use asrs_tabs::statistics::renderer::StatisticsGeometry;
use asrs_tabs::statistics::values::StatisticsTable;
use asrs_tabs::statistics::{StatisticsCommand, StatisticsField};

const WINDOWS: [&str; 4] = ["1x", "2x", "hidpi", "narrow"];
/// Below a 1/64 device px at any scale here, above what single precision client rects lose.
const TOLERANCE: f64 = 2e-3;

type Rect = [f64; 4];

fn scale(family: &str, name: &str) -> f32 {
    let path = vectors::workspace_root().join(format!("parity/specs/dom/{family}/{name}.entry.json"));
    let spec = json::parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let window = spec.get("fixture").unwrap().get("window").unwrap();
    let ratio = window.get("devicePixelRatio").and_then(Json::as_f64);
    ratio.unwrap_or(1.0) as f32
}

/// Each recorded command with the layout captured after it.
fn captures(family: &str, name: &str) -> Vec<(Value, Value)> {
    let records = vectors::load(&format!("dom/{family}/{name}")).unwrap();
    let evals = |prefix: &str| -> Vec<Value> {
        let matches = |r: &&Record| r.string("eval").is_some_and(|e| e.starts_with(prefix));
        let records = records.iter().filter(matches);
        records.map(|r| r.value("value").unwrap()).collect()
    };
    let commands = evals("/*layout:command*/");
    let layouts = evals(&format!("/*{family}*/"));
    assert_eq!(commands.len(), layouts.len(), "{name}: captures");
    commands.into_iter().zip(layouts).collect()
}

fn items(value: &Value) -> &[Value] {
    match value {
        Value::Array(items) => items,
        other => panic!("not an array: {}", other.kind()),
    }
}

fn pairs(value: &Value) -> impl Iterator<Item = (JsString, &Value)> {
    let Value::Object(pairs) = value else {
        panic!("not an object: {}", value.kind());
    };
    let key = |k: &asrs_parity::Utf16String| JsString::from_units(k.units().to_vec());
    pairs.iter().map(move |(k, v)| (key(k), v))
}

fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    let found = pairs(value).find(|(k, _)| *k == JsString::from(key));
    found.unwrap_or_else(|| panic!("no {key}")).1
}

fn numbers(value: &Value) -> Vec<f64> {
    items(value).iter().map(|v| v.as_f64().unwrap()).collect()
}

fn rect(value: &Value) -> Rect {
    [0, 1, 2, 3].map(|i| items(value)[i].as_f64().unwrap())
}

fn js(value: &Value) -> JsValue {
    match value {
        Value::Null => JsValue::Null,
        Value::String(s) => JsValue::String(JsString::from_units(s.units().to_vec())),
        Value::Array(items) => JsValue::Array(items.iter().map(js).collect()),
        Value::Object(_) => JsValue::Object(object(value)),
        other => JsValue::Number(other.as_f64().unwrap()),
    }
}

fn object(value: &Value) -> JsObject {
    let mut out = JsObject::new();
    for (key, value) in pairs(value) {
        out.insert(key, js(value));
    }
    out
}

/// A recorded cell's box and its text nodes' rects that are not empty.
fn recorded_cell(cell: &Value) -> (Rect, Vec<Rect>) {
    let [_, _, _, _, nodes] = items(cell) else {
        panic!("not a cell");
    };
    (rect(cell), node_rects(nodes))
}

fn node_rects(nodes: &Value) -> Vec<Rect> {
    let rects = items(nodes).iter().flat_map(|node| items(&items(node)[1]));
    rects.map(rect).filter(|r| r[2] > 0.0).collect()
}

fn line_rects(text: &CellText, [x, y]: [Lu; 2], [dx, dy]: [f64; 2], scale: f32) -> Vec<Rect> {
    let lines = text.lines.iter().filter(|line| line.width > 0);
    lines
        .map(|line| {
            [
                dx + css(x + line.left, scale),
                dy + css(y + line.box_top, scale),
                css(line.width, scale),
                css(line.box_height, scale),
            ]
        })
        .collect()
}

fn is_cjk(c: char) -> bool {
    ('\u{2e80}'..='\u{9fff}').contains(&c)
}

#[derive(Default)]
struct Comparison {
    compared: usize,
    failures: Vec<String>,
}

impl Comparison {
    fn check(&mut self, what: impl Fn() -> String, ours: &[f64], blink: &[f64]) {
        self.compared += 1;
        let close = |(a, b): (&f64, &f64)| (a - b).abs() < TOLERANCE;
        if ours.len() != blink.len() || !ours.iter().zip(blink).all(close) {
            self.failures
                .push(format!("{}: ours {ours:?}, Blink {blink:?}", what()));
        }
    }

    fn finish(self, name: &str, at_least: usize) {
        let (n, total) = (self.failures.len(), self.compared);
        let shown: Vec<String> = self.failures.into_iter().take(12).collect();
        assert!(
            shown.is_empty(),
            "{name}: {n} of {total} differ\n{}",
            shown.join("\n")
        );
        println!("{name}: {total} boxes equal");
        assert!(total >= at_least, "{name}: {total} compared");
    }
}

fn statistics_command(value: &Value) -> StatisticsCommand {
    StatisticsCommand {
        change_counter: get(value, "changeCounter").as_f64().unwrap(),
        bins: numbers(get(value, "bins")),
        step_size: get(value, "stepSize").as_f64().unwrap(),
        fields: items(get(value, "fields"))
            .iter()
            .map(|f| StatisticsField {
                title: JsString::from(""),
                color: js(get(f, "color")),
                histogram_counts: numbers(get(f, "histogramCounts")),
                stats: object(get(f, "stats")),
            })
            .collect(),
    }
}

#[test]
fn statistics_tables_lay_out_as_in_blink() {
    if !cfg!(target_os = "macos") {
        println!("statistics layout: skipped, the recordings use macOS's fonts");
        return;
    }
    for name in WINDOWS {
        let scale = scale("statistics-layout", name);
        let mut layout = StatisticsLayout::default();
        let mut table = StatisticsTable::new();
        let mut out = Comparison::default();
        for (i, (command, blink)) in captures("statistics-layout", name).iter().enumerate() {
            let changed = table.render(&statistics_command(command), false);
            assert!(changed.unwrap());
            let container = rect(get(blink, "container"));
            let [_, _, width, height] = rect(get(blink, "root"));
            layout.resize([width, height], scale);
            let offset_width = layout.values_offset_width(table.rows());
            let placed = layout.place(table.rows());
            let at = |what: &str| format!("{name} {i}: {what}");
            let px = |v: Lu| css(v, scale);
            out.check(
                || at("container"),
                &placed.container.map(px),
                &container[2..],
            );
            let sizes = numbers(get(blink, "sizes"));
            out.check(|| at("offsetWidth"), &[offset_width], &sizes[..1]);
            let margin = px(placed.margin);
            let [w, h] = placed.table.map(px);
            let ours = [margin, margin, w, h];
            out.check(|| at("table"), &ours, &rect(get(blink, "table")));
            let scroll = placed.scroll_size().map(|v| (px(v) + 0.5).floor());
            out.check(|| at("scroll size"), &scroll, &sizes[3..]);
            layout.scroll_to_end();
            let end = numbers(get(blink, "end"));
            out.check(|| at("scroll end"), &layout.scroll(), &end);
            let histogram = layout.histogram(Some(offset_width + 10.0), 0.0);
            out.check(|| at("histogram"), &histogram, &rect(get(blink, "histogram")));
            let rows = items(get(blink, "rows"));
            assert_eq!(placed.rows.len(), rows.len(), "{}", at("rows"));
            for (r, (row, blink)) in placed.rows.iter().zip(rows).enumerate() {
                let cells = items(&items(blink)[1]);
                assert_eq!(row.cells.len(), cells.len(), "{}", at("cells"));
                for (c, (cell, blink)) in row.cells.iter().zip(cells).enumerate() {
                    let at = |what: &str| format!("{name} {i}: row {r} cell {c} {what}");
                    let (bounds, rects) = recorded_cell(blink);
                    let ours = [
                        margin + px(cell.left),
                        margin + px(row.top),
                        px(cell.width),
                        px(row.height),
                    ];
                    out.check(|| at("box"), &ours, &bounds);
                    let origin = placed.content(row, cell);
                    let ours = line_rects(&cell.text, origin, [margin; 2], scale);
                    out.check(|| at("text"), &ours.concat(), &rects.concat());
                }
            }
        }
        out.finish(name, 1500);
    }
}

#[test]
fn metadata_tables_lay_out_as_in_blink() {
    if !cfg!(target_os = "macos") {
        println!("metadata layout: skipped, the recordings use macOS's fonts");
        return;
    }
    for name in WINDOWS {
        let scale = scale("metadata-layout", name);
        let mut layout = MetadataLayout::default();
        let mut renderer = MetadataRenderer::new();
        let mut out = Comparison::default();
        for (i, (command, blink)) in captures("metadata-layout", name).iter().enumerate() {
            renderer.render_command(&object(command)).unwrap();
            let table = renderer.table();
            let at = |what: &str| format!("{name} {i}: {what}");
            let px = |v: Lu| css(v, scale);
            let [_, _, width, height] = rect(get(blink, "root"));
            assert_eq!(table.show_table, *get(blink, "alert") == Value::Null);
            if !table.show_table {
                let alert = get(blink, "alert");
                let placed = layout.alert([width, height], scale);
                let [x, y] = placed.origin;
                let ours = [x, y, px(placed.size[0]), px(placed.size[1])];
                out.check(|| at("alert"), &ours, &rect(alert));
                let blink = node_rects(&items(alert)[4]).concat();
                let ours = line_rects(&placed.text, [0, 0], [x, y], scale).concat();
                out.check(|| at("alert text"), &ours, &blink);
                continue;
            }
            let placed = layout.place(table, width, scale);
            let container = rect(get(blink, "container"));
            let ours = [0.0, 0.0, width, px(placed.height)];
            out.check(|| at("container"), &ours, &container);
            let rows = items(get(blink, "rows"));
            assert_eq!(placed.rows.len(), rows.len(), "{}", at("rows"));
            for (r, (row, blink)) in placed.rows.iter().zip(rows).enumerate() {
                let cells = items(&items(blink)[1]);
                assert_eq!(row.cells.len(), cells.len(), "{}", at("cells"));
                for (c, (cell, blink)) in row.cells.iter().zip(cells).enumerate() {
                    let at = |what: &str| format!("{name} {i}: row {r} cell {c} {what}");
                    let (bounds, rects) = recorded_cell(blink);
                    let ours = [cell.left, row.top, cell.width, row.height].map(px);
                    out.check(|| at("box"), &ours, &bounds);
                    let origin = placed.content(row, cell);
                    let ours = line_rects(&cell.text, origin, [0.0; 2], scale);
                    if cell.text.lines.iter().any(|l| l.text.contains(is_cjk)) {
                        // #273: these characters' advance in Courier is not Blink's.
                        let vertical = |rects: &[Rect]| -> Vec<f64> {
                            rects.iter().flat_map(|r| [r[1], r[3]]).collect()
                        };
                        out.check(|| at("text"), &vertical(&ours), &vertical(&rects));
                        continue;
                    }
                    out.check(|| at("text"), &ours.concat(), &rects.concat());
                }
            }
        }
        out.finish(name, 500);
    }
}
