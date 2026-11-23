//! Compares `asrs_tabs::info` with the DOM upstream's pages build: `info/*`, written by
//! `oracle/src/genInfoWindows.ts`.

use asrs_js::color::hex_rgb;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::source_list_config::SourceListConfig;
use asrs_parity::cve1::Value;
use asrs_parity::vectors::{self, Record};
use asrs_tabs::info::beta_welcome::CHECKBOXES;
use asrs_tabs::info::source_list_help::{HelpItem, HelpText};
use asrs_tabs::info::{BetaWelcome, License, SourceListHelp};

fn load(id: &str) -> Vec<Record> {
    vectors::load(id).unwrap_or_else(|e| panic!("{id}: {e}"))
}

fn kind(record: &Record) -> String {
    record.string("kind").expect("a kind")
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
        Value::Undefined => JsValue::Undefined,
        Value::Bool(b) => JsValue::Bool(*b),
        Value::Number(bits) => JsValue::Number(f64::from_bits(*bits)),
        Value::String(_) => JsValue::String(js_string(value)),
        Value::Array(list) => JsValue::Array(list.iter().map(js).collect()),
        Value::Object(pairs) => {
            let mut out = asrs_js::value::JsObject::new();
            for (k, v) in pairs {
                out.insert(JsString::from_units(k.units().to_vec()), js(v));
            }
            JsValue::Object(out)
        }
        other => panic!("unexpected {}", other.kind()),
    }
}

/// A recorded element or text node.
enum Node<'a> {
    Text(JsString),
    Element {
        tag: String,
        class: String,
        color: String,
        extra: &'a Value,
        children: Vec<Node<'a>>,
    },
}

fn node(value: &Value) -> Node<'_> {
    match items(value) {
        [data, _] => Node::Text(js_string(data)),
        [tag, class, _, color, extra, children] => Node::Element {
            tag: text(tag),
            class: text(class),
            color: text(color),
            extra,
            children: items(children).iter().map(node).collect(),
        },
        other => panic!("{} fields", other.len()),
    }
}

/// The body's children but the whitespace the HTML file leaves.
fn body(state: &Value) -> Vec<Node<'_>> {
    let Node::Element { children, .. } = node(get(state, "body")) else {
        panic!("no body");
    };
    let blank = |n: &Node<'_>| matches!(n, Node::Text(t) if t.to_string_lossy().trim().is_empty());
    children.into_iter().filter(|n| !blank(n)).collect()
}

impl Node<'_> {
    fn div(&self, name: &str) -> &[Node<'_>] {
        match self {
            Node::Element {
                tag,
                class,
                children,
                ..
            } if tag == "div" && class == name => children,
            Node::Element { tag, class, .. } => panic!("{tag}.{class} is not div.{name}"),
            Node::Text(t) => panic!("{t:?} is not div.{name}"),
        }
    }

    fn element(&self, name: &str) -> (&str, &Value, &[Node<'_>]) {
        match self {
            Node::Element {
                tag,
                color,
                extra,
                children,
                ..
            } if tag == name => (color, extra, children),
            Node::Element { tag, .. } => panic!("{tag} is not {name}"),
            Node::Text(t) => panic!("{t:?} is not {name}"),
        }
    }

    /// The text of an element that holds one text node.
    fn only_text(nodes: &[Node<'_>]) -> String {
        match nodes {
            [Node::Text(t)] => t.to_string_lossy(),
            [] => String::new(),
            _ => panic!("{} nodes", nodes.len()),
        }
    }

    /// Text nodes with a `<br>` before each but the first.
    fn lines(nodes: &[Node<'_>], empty_nodes: bool) -> Vec<JsString> {
        let mut out = vec![JsString::new()];
        let mut filled = false;
        for node in nodes {
            match node {
                Node::Text(t) => {
                    assert!(!filled, "two text nodes on a line");
                    assert!(empty_nodes || !t.is_empty());
                    filled = true;
                    *out.last_mut().unwrap() = t.clone();
                }
                Node::Element { tag, .. } => {
                    assert_eq!(tag, "br");
                    assert!(filled || !empty_nodes, "a line without a text node");
                    filled = false;
                    out.push(JsString::new());
                }
            }
        }
        out
    }
}

#[test]
fn license_texts_are_munged_as_upstream_munges_them() {
    let (mut pages, mut compared) = (0, 0);
    for record in load("info/licenses") {
        if kind(&record) != "dump" {
            assert_eq!(kind(&record), "end");
            continue;
        }
        pages += 1;
        let case = record.string("case").unwrap();
        let (data, state) = (
            record.value("data").unwrap(),
            record.value("state").unwrap(),
        );
        assert_eq!(
            text(get(&state, "title")),
            "Licenses \u{2014} AdvantageScope"
        );
        assert!(items(get(&state, "errors")).is_empty(), "{case}");
        let body = body(&state);
        let data = items(&data);
        assert_eq!(body.len(), 2 * data.len(), "{case}");
        for (entry, divs) in data.iter().zip(body.chunks(2)) {
            let (module, text) = (
                js_string(get(entry, "module")),
                js_string(get(entry, "text")),
            );
            let license = License::new(&module, &text);
            let want = Node::lines(divs[0].div("module-text"), false);
            assert_eq!(license.module, want, "{case}: {module:?}");
            let want = Node::lines(divs[1].div("license-text"), true);
            assert_eq!(license.parts, want, "{case}: {text:?}");
            compared += 1;
        }
    }
    assert_eq!(pages, 19);
    assert_eq!(compared, 1126);
}

fn rgb(color: &str) -> String {
    let [r, g, b] = hex_rgb(color);
    format!("rgb({r}, {g}, {b})")
}

fn same_values(values: &[HelpText], nodes: &[Node<'_>], theme: usize, plain: &str) {
    assert_eq!(values.len(), nodes.len());
    for (value, node) in values.iter().zip(nodes) {
        match node {
            Node::Text(t) => {
                assert!(!value.span);
                assert_eq!(value.text, t.to_string_lossy());
            }
            Node::Element { .. } => {
                let (color, _, children) = node.element("span");
                assert!(value.span);
                assert_eq!(value.text, Node::only_text(children));
                let want = value.colors.as_ref().map(|c| rgb(&c[theme]));
                assert_eq!(want.as_deref().unwrap_or(plain), color);
            }
        }
    }
}

/// Compares `item` with the nodes it made and returns how many they are.
fn same_item(item: &HelpItem, nodes: &[Node<'_>], theme: usize) -> usize {
    let plain = ["rgb(34, 34, 34)", "rgb(238, 238, 238)"][theme];
    let [icon, title] = nodes[0].div("type-header") else {
        panic!("type-header");
    };
    let [object] = icon.div("type-icon-container") else {
        panic!("type-icon-container");
    };
    let (_, extra, _) = object.element("object");
    let path = format!("symbols/sourceList/{}.svg", item.symbol);
    assert_eq!(text(get(extra, "data")), path);
    assert_eq!(text(get(extra, "color")), rgb(&item.colors[theme]));
    assert_eq!(item.title, Node::only_text(title.div("type-title")));
    let mut at = 1;
    if let Some(warning) = &item.parent_warning {
        assert_eq!(warning, &Node::only_text(nodes[at].div("parent-warning")));
        at += 1;
    }
    if !item.options.is_empty() {
        let (_, _, table) = nodes[at].element("table");
        let [tbody] = table else { panic!("tbody") };
        let (_, _, rows) = tbody.element("tbody");
        assert_eq!(rows.len(), item.options.len());
        for (option, row) in item.options.iter().zip(rows) {
            let (_, _, cells) = row.element("tr");
            let [name, values] = cells else {
                panic!("cells")
            };
            assert_eq!(option.name, Node::only_text(name.element("td").2));
            same_values(&option.values, values.element("td").2, theme, plain);
        }
        at += 1;
    }
    assert_eq!(item.sources, Node::only_text(nodes[at].div("source-types")));
    at + 1
}

#[test]
fn help_pages_say_what_upstreams_say() {
    let (mut pages, mut compared, mut threw) = (0, 0, 0);
    for record in load("info/source-list-help") {
        if kind(&record) != "dump" {
            assert_eq!(kind(&record), "end");
            continue;
        }
        pages += 1;
        let theme = usize::from(record.value("dark").unwrap() == Value::Bool(true));
        let (data, state) = (
            record.value("data").unwrap(),
            record.value("state").unwrap(),
        );
        let mut help = SourceListHelp::default();
        let mut errors = Vec::new();
        for message in items(&data) {
            if text(get(message, "name")) != "set-config" {
                continue;
            }
            let config = SourceListConfig::from_js(&js(get(message, "data")))
                .unwrap_or_else(|| panic!("{:?}: not a config", record.string("case")));
            if let Err(e) = help.set_config(&config) {
                errors.push(e);
            }
        }
        let want = items(get(&state, "errors"));
        assert_eq!(errors.len(), want.len());
        for (got, want) in errors.iter().zip(want) {
            assert_eq!(got.name, text(get(want, "name")));
            assert_eq!(got.message, text(get(want, "message")));
            threw += 1;
        }
        let title = format!("{} \u{2014} AdvantageScope", help.title());
        assert_eq!(title, text(get(&state, "title")));
        let body = body(&state);
        let mut at = 0;
        for item in help.items() {
            at += same_item(item, &body[at..], theme);
            compared += 1;
        }
        assert_eq!(at, body.len());
    }
    assert_eq!((pages, threw), (96, 1));
    assert!(compared > 900, "{compared} items");
}

#[test]
fn beta_flows() {
    let mut page = BetaWelcome::default();
    let (mut steps, mut dumps) = (0, 0);
    for record in load("info/beta-welcome") {
        match kind(&record).as_str() {
            "dump" => {
                dumps += 1;
                page = BetaWelcome::default();
                for message in items(&record.value("data").unwrap()) {
                    page.message(&js(message));
                }
                let state = record.value("state").unwrap();
                let shown = |name: &str| {
                    let found = body(&state).into_iter().find_map(|n| match n {
                        Node::Element { class, extra, .. } if class == name => Some(extra),
                        _ => None,
                    });
                    get(found.expect("the heading"), "hidden") == &Value::Bool(false)
                };
                assert_eq!(page.shows_beta(), shown("beta-only"));
                assert_eq!(page.shows_alpha(), shown("alpha-only"));
                assert!(!page.enabled());
            }
            "step" => {
                steps += 1;
                let state = record.value("state").unwrap();
                let step = record.string("step").unwrap();
                let want: Vec<bool> = items(get(&state, "checked"))
                    .iter()
                    .map(|c| *c == Value::Bool(true))
                    .collect();
                assert_eq!(want.len(), CHECKBOXES);
                let changed: Vec<usize> = (0..CHECKBOXES)
                    .filter(|&i| page.checked()[i] != want[i])
                    .collect();
                assert!(changed.len() <= 1, "{step}");
                if let Some(index) = step.strip_prefix("click ").and_then(|i| i.parse().ok()) {
                    assert_eq!(changed, [index]);
                }
                for index in changed {
                    page.toggle(index);
                }
                assert_eq!(
                    get(&state, "disabled"),
                    &Value::Bool(!page.enabled()),
                    "{step}"
                );
                let replies = items(get(&state, "replies")).len();
                if step == "click button" {
                    assert_eq!(usize::from(page.click_continue()), replies);
                }
                assert!(items(get(&state, "errors")).is_empty());
            }
            other => assert_eq!(other, "end"),
        }
    }
    assert_eq!(dumps, 26);
    assert!(steps > 100, "{steps} steps");
}
