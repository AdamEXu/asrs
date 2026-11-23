#![allow(clippy::disallowed_methods)]
#![cfg(target_os = "macos")]

//! Lays the pages of `info/*` (`oracle/src/genInfoWindows.ts`) out as their views do and compares
//! every box and every text node's client rects with Blink's. The recordings use macOS's fonts.

use asrs_app::views::cell_text::{css, length};
use asrs_app::views::info::layout::{
    BetaLayout, HelpLayout, LicensesLayout, LuRect, TextBlock, snapped,
};
use asrs_canvas::text::dom::{LU_PER_PX, Lu};
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use asrs_model::source_list_config::SourceListConfig;
use asrs_parity::cve1::Value;
use asrs_parity::vectors::{self, Record};
use asrs_tabs::info::{BetaWelcome, License, SourceListHelp};

/// Above what single precision client rects lose, and at 1x below a 1/64 device px. Zoomed, a
/// ceiled text width may be one of those off.
fn tolerance(scale: f32) -> f64 {
    let unit = 1.0 / 64.0 / f64::from(scale);
    if scale == 1.0 { 2e-3 } else { unit + 1e-3 }
}

/// 25px bold system text is 0.14 px narrower than Blink's over the 24 characters of the beta
/// page's heading.
const BOLD_25PX: f64 = 0.15;

type Rect = [f64; 4];

/// A box by name, or a text node's line boxes.
#[derive(Debug, PartialEq)]
enum Item {
    Box(String, Rect),
    Text(Vec<Rect>),
}

fn items(value: &Value) -> &[Value] {
    match value {
        Value::Array(items) => items,
        other => panic!("not an array: {}", other.kind()),
    }
}

fn get<'a>(value: &'a Value, key: &str) -> &'a Value {
    let Value::Object(pairs) = value else {
        panic!("not an object: {}", value.kind());
    };
    let found = pairs.iter().find(|(k, _)| k.eq_str(key));
    &found.unwrap_or_else(|| panic!("no {key}")).1
}

fn js_string(value: &Value) -> JsString {
    match value {
        Value::String(s) => JsString::from_units(s.units().to_vec()),
        other => panic!("not a string: {}", other.kind()),
    }
}

fn text(value: &Value) -> String {
    js_string(value).to_string_lossy()
}

fn js(value: &Value) -> JsValue {
    match value {
        Value::Null => JsValue::Null,
        Value::Bool(b) => JsValue::Bool(*b),
        Value::Number(bits) => JsValue::Number(f64::from_bits(*bits)),
        Value::String(_) => JsValue::String(js_string(value)),
        Value::Array(list) => JsValue::Array(list.iter().map(js).collect()),
        Value::Object(pairs) => {
            let mut out = JsObject::new();
            for (k, v) in pairs {
                out.insert(JsString::from_units(k.units().to_vec()), js(v));
            }
            JsValue::Object(out)
        }
        other => panic!("unexpected {}", other.kind()),
    }
}

fn rect(value: &Value) -> Rect {
    [0, 1, 2, 3].map(|i| items(value)[i].as_f64().unwrap())
}

/// The recorded boxes of the elements `name` names and of the text nodes that have any, in
/// document order.
fn recorded(value: &Value, name: &dyn Fn(&str, &str) -> Option<String>, out: &mut Vec<Item>) {
    match items(value) {
        [_, rects] => {
            let rects: Vec<Rect> = items(rects).iter().map(rect).collect();
            if !rects.is_empty() {
                out.push(Item::Text(rects));
            }
        }
        [tag, class, bounds, _, extra, children] => {
            if get(extra, "hidden") == &Value::Bool(true) {
                return;
            }
            if let Some(name) = name(&text(tag), &text(class)) {
                out.push(Item::Box(name, rect(bounds)));
            }
            for child in items(children) {
                recorded(child, name, out);
            }
        }
        other => panic!("{} fields", other.len()),
    }
}

struct Page {
    case: String,
    width: Lu,
    scale: f32,
    data: Value,
    items: Vec<Item>,
    scroll_height: f64,
    viewport_height: f64,
}

fn number(record: &Record, key: &str) -> f64 {
    let json = record.field(key).unwrap_or_else(|| panic!("no {key}"));
    json.as_f64().unwrap_or_else(|| panic!("{key}: {json:?}"))
}

fn pages(id: &str, name: &dyn Fn(&str, &str) -> Option<String>) -> Vec<Page> {
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{id}: {e}"));
    let mut pages = Vec::new();
    for record in &records {
        if record.string("kind").as_deref() != Some("dump") {
            continue;
        }
        if record.value("layout").unwrap() != Value::Bool(true) {
            continue;
        }
        let state = record.value("state").unwrap();
        let mut found = Vec::new();
        recorded(get(&state, "body"), name, &mut found);
        let scale = number(record, "scale") as f32;
        pages.push(Page {
            case: record.string("case").unwrap(),
            // The viewport is whole device px.
            width: (number(record, "width") * f64::from(scale)).ceil() as Lu * LU_PER_PX,
            scale,
            data: record.value("data").unwrap(),
            items: found,
            scroll_height: items(get(&state, "scroll"))[1].as_f64().unwrap(),
            viewport_height: number(record, "height"),
        });
    }
    pages
}

fn css_rect(r: LuRect, scale: f32) -> Rect {
    r.map(|v| css(v, scale))
}

#[derive(Default)]
struct Laid {
    items: Vec<Item>,
    scale: f32,
    /// Items compared within [`BOLD_25PX`].
    bold_25px: Vec<usize>,
}

impl Laid {
    fn named(&mut self, name: &str, r: LuRect) {
        self.items
            .push(Item::Box(name.to_owned(), css_rect(r, self.scale)));
    }

    fn text(&mut self, rects: Vec<LuRect>) {
        if !rects.is_empty() {
            let rects = rects.into_iter().map(|r| css_rect(r, self.scale)).collect();
            self.items.push(Item::Text(rects));
        }
    }

    fn block(&mut self, block: &TextBlock, count: usize) {
        for item in 0..count {
            self.text(block.rects(item));
        }
    }
}

fn near(a: &Rect, b: &Rect, tolerance: f64) -> bool {
    a.iter().zip(b).all(|(a, b)| (a - b).abs() <= tolerance)
}

/// Compares and returns how many line boxes and boxes were equal.
fn same(page: &Page, laid: &Laid, height: Lu, failures: &mut Vec<String>) -> usize {
    let mut equal = 0;
    for (i, (want, got)) in page.items.iter().zip(&laid.items).enumerate() {
        let tolerance = if laid.bold_25px.contains(&i) {
            BOLD_25PX
        } else {
            tolerance(page.scale)
        };
        let near = |a: &Rect, b: &Rect| near(a, b, tolerance);
        let matches = match (want, got) {
            (Item::Box(a, ra), Item::Box(b, rb)) => a == b && near(ra, rb),
            (Item::Text(a), Item::Text(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(a, b)| near(a, b))
            }
            _ => false,
        };
        if matches {
            equal += 1;
        } else {
            failures.push(format!(
                "{} #{i}:\n  want {want:?}\n  got  {got:?}",
                page.case
            ));
        }
    }
    if page.items.len() != laid.items.len() {
        let counts = (page.items.len(), laid.items.len());
        failures.push(format!("{}: {counts:?} items", page.case));
    }
    let scroll = css(snapped(height), page.scale).max(page.viewport_height);
    if (scroll - page.scroll_height).abs() > 0.5 + 1e-6 {
        failures.push(format!(
            "{}: {scroll} tall, not {}",
            page.case, page.scroll_height
        ));
    }
    equal
}

fn report(failures: &[String], equal: usize) {
    let shown: Vec<&String> = failures.iter().take(12).collect();
    assert!(
        failures.is_empty(),
        "{} differ, {equal} equal: {shown:#?}",
        failures.len()
    );
}

#[test]
fn licenses() {
    let name = |tag: &str, class: &str| (tag == "div").then(|| class.to_owned());
    let (mut failures, mut equal, mut recorded) = (Vec::new(), 0, 0);
    let pages = pages("info/licenses", &name);
    assert_eq!(pages.len(), 15);
    for page in pages {
        let licenses: Vec<License> = items(&page.data)
            .iter()
            .map(|l| License::new(&js_string(get(l, "module")), &js_string(get(l, "text"))))
            .collect();
        let layout = LicensesLayout::new(&licenses, page.width, page.scale);
        let mut laid = Laid {
            scale: page.scale,
            ..Laid::default()
        };
        for block in &layout.blocks {
            let class = if block.module {
                "module-text"
            } else {
                "license-text"
            };
            let [x, w] = [layout.left, layout.width];
            laid.named(class, [x, block.top, w, block.text.height]);
            let mut lines = block.text.lines.iter().peekable();
            while let Some(line) = lines.next() {
                let mut rects = vec![line];
                while let Some(more) = lines.next_if(|l| l.part == line.part) {
                    rects.push(more);
                }
                let height = block.text.box_height;
                let boxes = rects
                    .iter()
                    .map(|l| [x, block.top + l.box_top, l.width, height]);
                laid.text(boxes.collect());
            }
        }
        recorded += page.items.len();
        equal += same(&page, &laid, layout.height, &mut failures);
    }
    report(&failures, equal);
    assert_eq!(equal, recorded);
}

#[test]
fn source_list_help() {
    let name = |tag: &str, class: &str| match tag {
        "body" | "tbody" | "span" => None,
        "div" => Some(class.to_owned()),
        other => Some(other.to_owned()),
    };
    let (mut failures, mut equal, mut recorded) = (Vec::new(), 0, 0);
    let pages = pages("info/source-list-help", &name);
    assert_eq!(pages.len(), 96);
    for page in pages {
        let mut help = SourceListHelp::default();
        for message in items(&page.data) {
            if text(get(message, "name")) == "set-config" {
                let config = SourceListConfig::from_js(&js(get(message, "data"))).unwrap();
                let _ = help.set_config(&config);
            }
        }
        let layout = HelpLayout::new(help.items(), page.width, page.scale);
        let mut laid = Laid {
            scale: page.scale,
            ..Laid::default()
        };
        for (item, l) in help.items().iter().zip(&layout.items) {
            laid.named("type-header", l.header);
            laid.named("type-icon-container", l.icon);
            let icon = length(24.0, page.scale);
            let inset = |side: Lu| (side - icon) / 2;
            let [x, y, w, h] = l.icon;
            laid.named("object", [x + inset(w), y + inset(h), icon, icon]);
            laid.named("type-title", l.title);
            laid.text(l.title_rects(&item.title, page.scale));
            if let Some((rect, block)) = &l.warning {
                laid.named("parent-warning", *rect);
                laid.block(block, 1);
            }
            if let Some((rect, rows)) = &l.table {
                laid.named("table", *rect);
                for (row, option) in rows.iter().zip(&item.options) {
                    laid.named("tr", row.row);
                    laid.named("td", row.name_cell);
                    laid.block(&row.name, 1);
                    laid.named("td", row.value_cell);
                    laid.block(&row.values, option.values.len());
                }
            }
            laid.named("source-types", l.sources);
            laid.block(&l.sources_text, 1);
        }
        recorded += page.items.len();
        equal += same(&page, &laid, layout.height, &mut failures);
    }
    report(&failures, equal);
    assert_eq!(equal, recorded);
}

#[test]
fn beta_welcome() {
    let name = |tag: &str, _: &str| match tag {
        "body" | "tbody" | "span" => None,
        other => Some(other.to_owned()),
    };
    let (mut failures, mut equal, mut recorded) = (Vec::new(), 0, 0);
    let pages = pages("info/beta-welcome", &name);
    assert_eq!(pages.len(), 7);
    for page in pages {
        let mut beta = BetaWelcome::default();
        for message in items(&page.data) {
            beta.message(&js(message));
        }
        let layout = BetaLayout::new(&beta, "AdvantageScope", page.width, page.scale);
        let mut laid = Laid {
            scale: page.scale,
            ..Laid::default()
        };
        laid.named("h1", layout.title.0);
        laid.block(&layout.title.1, 1);
        for (rect, block) in &layout.headings {
            laid.named("h2", *rect);
            laid.bold_25px.push(laid.items.len());
            laid.block(block, 1);
        }
        laid.named("p", layout.intro.0);
        laid.block(&layout.intro.1, 1);
        laid.named("table", layout.table);
        for row in &layout.rows {
            laid.named("tr", row.row);
            laid.named("td", row.cells[0]);
            laid.named("input", row.checkbox);
            laid.named("td", row.cells[1]);
            laid.block(&row.text, 1);
        }
        laid.named("p", layout.outro.0);
        laid.block(&layout.outro.1, 3);
        laid.named("button", layout.button);
        laid.block(&layout.button_text, 1);
        recorded += page.items.len();
        equal += same(&page, &laid, layout.height, &mut failures);
    }
    report(&failures, equal);
    assert_eq!(equal, recorded);
}
