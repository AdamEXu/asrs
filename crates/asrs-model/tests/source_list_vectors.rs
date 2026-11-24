#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/functions.rs"]
#[allow(dead_code)]
mod functions;
#[path = "../../asrs-cli/src/js_color.rs"]
#[allow(dead_code)]
mod js_color;
#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;
#[path = "../../asrs-cli/src/js_math.rs"]
#[allow(dead_code)]
mod js_math;
#[path = "../../asrs-cli/src/js_msgpack.rs"]
#[allow(dead_code)]
mod js_msgpack;
#[path = "../../asrs-cli/src/js_ops.rs"]
#[allow(dead_code)]
mod js_ops;
#[path = "../../asrs-cli/src/js_regex.rs"]
#[allow(dead_code)]
mod js_regex;
#[path = "../../asrs-cli/src/js_string.rs"]
#[allow(dead_code)]
mod js_string;
#[path = "../../asrs-cli/src/js_time.rs"]
#[allow(dead_code)]
mod js_time;
#[path = "../../asrs-cli/src/log_field.rs"]
#[allow(dead_code)]
mod log_field;
#[path = "../../asrs-cli/src/log_keys.rs"]
#[allow(dead_code)]
mod log_keys;
#[path = "../../asrs-cli/src/log_run.rs"]
#[allow(dead_code)]
mod log_run;
#[path = "../../asrs-cli/src/log_struct.rs"]
#[allow(dead_code)]
mod log_struct;
#[path = "../../asrs-cli/src/photon.rs"]
#[allow(dead_code)]
mod photon;
#[path = "../../asrs-cli/src/shared.rs"]
#[allow(dead_code)]
mod shared;
#[path = "../../asrs-cli/src/state.rs"]
#[allow(dead_code)]
mod state;

mod tabs_console {
    pub const FUNCTIONS: &[(&str, crate::functions::Function)] = &[];
    pub const CALLS: &[(&str, crate::functions::Function)] = &[];
}

use std::collections::VecDeque;
use std::sync::OnceLock;

use asrs_js::string::JsString;
use asrs_js::time::Rng;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::Log;
use asrs_log::realm::JsRealm;
use asrs_model::source_list::layout::{ListLayout, ListRoot, controller_roots};
use asrs_model::source_list::{
    ClearPrompt, Env, ItemDragPayload, ItemDragStart, ItemView, Length, ListButton, Rect,
    SourceList, SourceListDom, SourceListError, TypePrompt, configs,
};
use asrs_model::source_list_config::{
    SourceListConfig, SourceListItemState, SourceListOptionValueConfig,
};
use asrs_model::tab_type::TabType;
use asrs_parity::Value;
use asrs_parity::geometry::{GeometryRecord, ScriptedReads};
use asrs_parity::json::Json;
use asrs_parity::tally::Tally;
use asrs_parity::vectors::{Record, load, vectors_dir};

const CASES: &str = "dom/source-list";

struct NoRandom;

impl Rng for NoRandom {
    fn next_f64(&mut self) -> f64 {
        panic!("source list fixtures draw no Math.random values")
    }
}

struct Slot {
    list: SourceList,
    number_preview: bool,
}

#[derive(Default)]
struct Stats {
    ops: usize,
    dumps: usize,
    previews: usize,
    reads: usize,
    widths: usize,
    checked: usize,
    configs: Vec<&'static str>,
}

/// Answers the lists' layout reads from the run's `read` records on `div.source-list`
/// elements, in order. A list's root element is the first recorded element of its own class
/// that one of its reads lands in, and no two lists share a root.
///
/// Every answer is also checked against [`ListLayout`], the layout the app draws: roots placed
/// by `controller_roots` in the fixtures' controls box, the rest from the list's items as they
/// were when the op began.
struct ScriptedDom {
    reads: ScriptedReads,
    /// uuid, the class of the list's root in hub.html, and its element path once seen.
    roots: Vec<(String, &'static str, Option<String>)>,
    /// Per list: the items when the current op began, whether its root was laid out when last
    /// read (until then, whether the read itself found a box), and the `scrollTop` a
    /// `/*sl:scroll*/` capture gave its `div.list`.
    views: Vec<Vec<ItemView>>,
    shown: Vec<Option<bool>>,
    scroll: Vec<f64>,
    checked: usize,
    type_names: usize,
    scroll_writes: usize,
    error: Option<String>,
}

/// `div.controls-content` in the fixtures' default 1100 x 650 window: right of the 300 px
/// sidebar, 200 px tall at the bottom.
const CONTROLS: Rect = Rect {
    x: 300.0,
    y: 450.0,
    width: 800.0,
    height: 200.0,
};

/// The controller a root class belongs to and the list's place among its lists.
fn controller_of(class: &str) -> (TabType, usize) {
    match class {
        "line-graph-left" => (TabType::LineGraph, 0),
        "line-graph-right" => (TabType::LineGraph, 1),
        "line-graph-discrete" => (TabType::LineGraph, 2),
        "field-2d-sources" => (TabType::Field2d, 0),
        "field-3d-sources" => (TabType::Field3d, 0),
        "stats-sources" => (TabType::Statistics, 0),
        "swerve-sources" => (TabType::Swerve, 0),
        "mechanism-sources" => (TabType::Mechanism, 0),
        _ => (TabType::Points, 0),
    }
}

fn bits(r: Rect) -> [u64; 4] {
    [r.x, r.y, r.width, r.height].map(f64::to_bits)
}

const LIST: &str = "div.list:4";

impl ScriptedDom {
    fn root(&mut self, list: &str) -> String {
        let Some(slot) = self.roots.iter().position(|(uuid, _, _)| uuid == list) else {
            self.error
                .get_or_insert(format!("a read on unregistered list {list}"));
            return String::from("?");
        };
        if let Some(path) = &self.roots[slot].2 {
            return path.clone();
        }
        let class = format!(".{}", self.roots[slot].1);
        let path = self.reads.peek().and_then(|g| {
            let segments: Vec<&str> = g.el.split('>').collect();
            let i = segments
                .iter()
                .position(|s| s.contains(".source-list") && s.contains(class.as_str()))?;
            Some(segments[..=i].join(">"))
        });
        match path {
            Some(path) if !self.roots.iter().any(|(_, _, p)| p.as_ref() == Some(&path)) => {
                self.roots[slot].2 = Some(path.clone());
                path
            }
            _ => {
                self.error.get_or_insert(format!(
                    "list {list} read its layout, upstream's next access is {:?}",
                    self.reads.peek().map(|g| (&g.name, &g.el, g.line))
                ));
                String::from("?")
            }
        }
    }

    fn rect(&mut self, target: &str) -> Rect {
        let [x, y, width, height] = self.reads.read_rect(target);
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn slot(&self, list: &str) -> usize {
        self.roots
            .iter()
            .position(|(uuid, _, _)| uuid == list)
            .unwrap_or(usize::MAX)
    }

    fn controller_root(&self, slot: usize) -> ListRoot {
        let (tab_type, index) = controller_of(self.roots[slot].1);
        controller_roots(tab_type, CONTROLS)[index]
    }

    fn layout(&self, slot: usize, blink: Rect) -> ListLayout {
        let shown = self.shown[slot].unwrap_or(blink.width != 0.0 || blink.height != 0.0);
        let root = if shown {
            self.controller_root(slot)
        } else {
            ListRoot::default()
        };
        ListLayout::new(root, &self.views[slot], self.scroll[slot])
    }

    fn check(
        &mut self,
        list: &str,
        what: String,
        blink: Rect,
        port: impl FnOnce(&ListLayout) -> Rect,
    ) {
        let slot = self.slot(list);
        if slot == usize::MAX {
            return;
        }
        let port = port(&self.layout(slot, blink));
        self.checked += 1;
        if bits(port) != bits(blink) && !blink.x.is_nan() {
            self.error.get_or_insert(format!(
                "list {list}: {what} is {blink:?} in Blink, {port:?} in ListLayout"
            ));
        }
    }
}

impl SourceListDom for ScriptedDom {
    fn root_rect(&mut self, list: &str) -> Rect {
        let root = self.root(list);
        let blink = self.rect(&root);
        let slot = self.slot(list);
        if slot != usize::MAX {
            let displayed = blink.width != 0.0 || blink.height != 0.0;
            self.shown[slot] = Some(displayed);
            let port = if displayed {
                self.controller_root(slot).rect
            } else {
                Rect::default()
            };
            self.checked += 1;
            if bits(port) != bits(blink) && !blink.x.is_nan() {
                self.error.get_or_insert(format!(
                    "list {list}: root is {blink:?} in Blink, {port:?} from controller_roots"
                ));
            }
        }
        blink
    }
    fn list_rect(&mut self, list: &str) -> Rect {
        let root = self.root(list);
        let blink = self.rect(&format!("{root}>{LIST}"));
        self.check(list, "the list".into(), blink, |l| l.list_rect());
        blink
    }
    fn item_rect(&mut self, list: &str, index: usize) -> Rect {
        let root = self.root(list);
        let blink = self.rect(&format!("{root}>{LIST}>div.item:{index}"));
        self.check(list, format!("item {index}"), blink, |l| l.item_rect(index));
        blink
    }
    fn item_client_width(&mut self, list: &str, index: usize) -> f64 {
        let root = self.root(list);
        let blink = self
            .reads
            .read_f64("clientWidth", &format!("{root}>{LIST}>div.item:{index}"));
        let width = |w: f64| Rect {
            width: w,
            ..Rect::default()
        };
        self.check(
            list,
            format!("item {index} clientWidth"),
            width(blink),
            |l| width(l.item_client_width(index)),
        );
        blink
    }
    fn button_rect(&mut self, list: &str, button: ListButton) -> Rect {
        let root = self.root(list);
        let target = match button {
            ListButton::Edit => format!("{root}>button.edit:1"),
            ListButton::Clear => format!("{root}>button.clear:2"),
            ListButton::Type(index) => format!("{root}>{LIST}>div.item:{index}>button.type:0"),
        };
        let blink = self.rect(&target);
        self.check(list, format!("{button:?}"), blink, |l| {
            l.button_rect(button)
        });
        blink
    }
    fn type_name_width(&mut self, _text: &str) -> f64 {
        self.type_names += 1;
        self.reads
            .read_f64("clientWidth", "body:1>div.source-list:*>div.type-name:0")
    }
}

/// The hub.html class of a list's root: the controller templates' `*-sources` elements and
/// the line graph's three axis columns.
fn root_class(config: &str, title: &str) -> Result<&'static str, String> {
    Ok(match (config, title) {
        ("line_graph_numeric", "Left Axis") => "line-graph-left",
        ("line_graph_numeric", "Right Axis") => "line-graph-right",
        ("line_graph_discrete", _) => "line-graph-discrete",
        ("field_2d", _) => "field-2d-sources",
        ("field_3d", _) => "field-3d-sources",
        ("statistics", _) => "stats-sources",
        ("swerve", _) => "swerve-sources",
        ("mechanism", _) => "mechanism-sources",
        ("points", _) => "points-sources",
        other => return Err(format!("no root element for {other:?}")),
    })
}

struct Harness {
    log: Log,
    realm: JsRealm,
    memory: JsValue,
    dom: ScriptedDom,
    lists: Vec<Slot>,
    pending_errors: VecDeque<String>,
    pending_prompt: Option<TypePrompt>,
    pending_clear: Option<ClearPrompt>,
    pending_drag: Option<(usize, ItemDragStart)>,
    stats: Stats,
}

fn undefined() -> &'static JsValue {
    static UNDEFINED: OnceLock<JsValue> = OnceLock::new();
    UNDEFINED.get_or_init(JsValue::default)
}

fn js(record: &Record, key: &str) -> Result<JsValue, String> {
    js_json::to_js(&record.value(key).map_err(|e| e.0)?)
}

fn at(args: &[JsValue], i: usize) -> &JsValue {
    args.get(i).unwrap_or_else(|| undefined())
}

fn num(v: &JsValue) -> Result<f64, String> {
    v.as_f64()
        .ok_or_else(|| format!("expected a number, got {v:?}"))
}

fn jstr(v: &JsValue) -> Result<JsString, String> {
    v.as_str()
        .cloned()
        .ok_or_else(|| format!("expected a string, got {v:?}"))
}

fn text(v: &JsValue) -> Result<String, String> {
    Ok(jstr(v)?.to_string_lossy())
}

fn boolean(v: &JsValue) -> Result<bool, String> {
    v.as_bool()
        .ok_or_else(|| format!("expected a boolean, got {v:?}"))
}

fn get<'a>(v: &'a JsValue, key: &str) -> &'a JsValue {
    v.as_object()
        .and_then(|o| o.get(key))
        .unwrap_or_else(|| undefined())
}

fn index(v: &JsValue) -> Result<usize, String> {
    let x = num(v)?;
    if x < 0.0 || x.fract() != 0.0 {
        return Err(format!("expected an index, got {x}"));
    }
    Ok(x as usize)
}

fn states(v: &JsValue) -> Result<Vec<SourceListItemState>, String> {
    Ok(v.as_array()
        .ok_or("expected an array of item states")?
        .iter()
        .map(SourceListItemState::restored)
        .collect())
}

fn rect(v: &JsValue) -> Result<Rect, String> {
    Ok(Rect {
        x: num(get(v, "x"))?,
        y: num(get(v, "y"))?,
        width: num(get(v, "width"))?,
        height: num(get(v, "height"))?,
    })
}

fn same_rect(a: Rect, b: Rect) -> bool {
    [a.x, a.y, a.width, a.height]
        .iter()
        .zip([b.x, b.y, b.width, b.height])
        .all(|(p, q)| p.to_bits() == q.to_bits())
}

fn supplemental(v: &JsValue) -> Result<Vec<Vec<SourceListItemState>>, String> {
    v.as_array()
        .ok_or("supplemental states")?
        .iter()
        .map(states)
        .collect()
}

fn keys(v: &JsValue) -> Result<Vec<JsString>, String> {
    match v {
        JsValue::Undefined => Ok(Vec::new()),
        v => v
            .as_array()
            .ok_or("expected keys")?
            .iter()
            .map(jstr)
            .collect(),
    }
}

fn encode(v: &JsValue) -> String {
    js_json::from_js(v).encode()
}

/// Kinds no replay compares for these vectors (#397), with the suites that compare the same code
/// on their own.
const COVERED: [(&str, &str); 4] = [
    ("capture", "asrs-hub's dom_tabs"),
    ("main:show", "asrs-hub's dom_tabs"),
    ("main:frc-coordinate-system-warning", "asrs-hub's dom_tabs"),
    (HUB_GEOMETRY, "asrs-hub's tabs_dom and sidebar_dom, in part"),
];

const LIST_GEOMETRY: &str = "list geometry";
const HUB_GEOMETRY: &str = "hub geometry";

fn on_a_list(record: &Record) -> bool {
    record
        .string("el")
        .is_some_and(|el| el.contains(".source-list"))
}

fn kind_of(record: &Record) -> String {
    if let Some(name) = record.string("main") {
        return format!("main:{name}");
    }
    let key = match &record.json {
        Json::Object(pairs) if !pairs.is_empty() => pairs[0].0.to_string_lossy(),
        _ => panic!("line {}: the record has no first key", record.line),
    };
    match key.as_str() {
        "read" | "write" if on_a_list(record) => LIST_GEOMETRY.to_owned(),
        "read" | "write" => HUB_GEOMETRY.to_owned(),
        _ => key,
    }
}

fn item_dump(view: &ItemView) -> JsValue {
    let mut out: Vec<JsValue> = Vec::new();
    match &view.rendered {
        None => {
            out.push("".to_js());
            out.push(JsValue::Null);
            out.push("".to_js());
            out.push("".to_js());
            out.push("".to_js());
            out.push("".to_js());
            out.push("symbols/visibility.svg".to_js());
            out.push("Toggle field visibility".to_js());
            out.push(false.to_js());
        }
        Some(r) => {
            out.push(r.symbol_path.to_js());
            out.push(r.color.to_js());
            out.push(r.type_name.to_js());
            out.push(r.key_text.to_js());
            out.push(if r.available { "" } else { "line-through" }.to_js());
            out.push(r.log_key.to_js());
            out.push(r.hide_icon().to_js());
            out.push(r.hide_title().to_js());
            out.push((!r.visible).to_js());
        }
    }
    out.push(view.child.to_js());
    out.push(view.parent_highlight.to_js());
    let hidden = view.preview_shown != Some(true);
    out.push(hidden.to_js());
    out.push(hidden.to_js());
    out.push(view.preview_text.to_js());
    out.push(
        match view.preview_shown {
            None => "",
            Some(true) => "50px",
            Some(false) => "30px",
        }
        .to_js(),
    );
    JsValue::Array(out)
}

fn list_dump(list: &SourceList, log: &Log) -> JsValue {
    let mut o = JsObject::new();
    o.insert("title", list.title().to_js());
    o.insert("state", list.state().to_js());
    o.insert(
        "displayed",
        list.displayed_state(log).expect("modeled items").to_js(),
    );
    o.insert("stopped", list.is_stopped().to_js());
    let hand = if list.hand_icon_shown() {
        ["0.15", "opacity 1s ease-in 1s"]
    } else {
        ["0", ""]
    };
    o.insert("handIcon", hand.to_vec().to_js());
    let highlight = list.drag_highlight();
    let (left, top, width, height) = match highlight.placement {
        None => (String::new(), String::new(), String::new(), String::new()),
        Some((top, height)) => (
            Length::Percent(0.0).css(),
            top.css(),
            Length::Percent(100.0).css(),
            height.css(),
        ),
    };
    o.insert(
        "highlight",
        JsValue::Array(vec![
            highlight.hidden.to_js(),
            left.to_js(),
            top.to_js(),
            width.to_js(),
            height.to_js(),
        ]),
    );
    let edit = list.has_edit_button();
    o.insert("buttons", vec![!edit, edit, edit].to_js());
    o.insert(
        "items",
        JsValue::Array(list.items().map(item_dump).collect()),
    );
    JsValue::Object(o)
}

fn split_pair(lists: &mut [Slot], a: usize, b: usize) -> (&mut SourceList, &mut SourceList) {
    if a < b {
        let (lo, hi) = lists.split_at_mut(b);
        (&mut lo[a].list, &mut hi[0].list)
    } else {
        let (lo, hi) = lists.split_at_mut(a);
        (&mut hi[0].list, &mut lo[b].list)
    }
}

impl Harness {
    fn new(records: &[Record]) -> Result<Harness, String> {
        let mut ours = Vec::new();
        for record in records {
            if GeometryRecord::parse(record)?.is_some() && on_a_list(record) {
                ours.push(record.clone());
            }
        }
        Ok(Harness {
            log: Log::new(true),
            realm: JsRealm::new(Box::new(NoRandom)),
            memory: JsValue::Object(JsObject::new()),
            dom: ScriptedDom {
                reads: ScriptedReads::from_records(&ours)?,
                roots: Vec::new(),
                views: Vec::new(),
                shown: Vec::new(),
                scroll: Vec::new(),
                checked: 0,
                type_names: 0,
                scroll_writes: 0,
                error: None,
            },
            lists: Vec::new(),
            pending_errors: VecDeque::new(),
            pending_prompt: None,
            pending_clear: None,
            pending_drag: None,
            stats: Stats::default(),
        })
    }

    fn apply_log(&mut self, ops: &Value) -> Result<(), String> {
        let mut results = Vec::new();
        for op in ops.as_array().ok_or("slLog is a list of ops")? {
            let op = op.as_array().ok_or("a log op is an array")?;
            let converted;
            let op = match op {
                [name, key, time, pose] if name.as_str().is_some_and(|n| n.eq_str("putPose")) => {
                    converted = vec![
                        name.clone(),
                        key.clone(),
                        time.clone(),
                        pose.get("translation").cloned().unwrap_or(Value::Undefined),
                        pose.get("rotation").cloned().unwrap_or(Value::Undefined),
                    ];
                    &converted[..]
                }
                op => op,
            };
            log_run::apply(&mut self.log, &mut self.realm, op, &mut results)?;
        }
        Ok(())
    }

    fn env(&mut self, dark: bool) -> (Env<'_>, &mut Vec<Slot>) {
        (
            Env {
                log: &self.log,
                type_memory: &mut self.memory,
                dark,
                dom: &mut self.dom,
            },
            &mut self.lists,
        )
    }

    fn create(&mut self, record: &Record) -> Result<(), String> {
        let create = js(record, "create")?;
        let config = SourceListConfig::from_js(get(&create, "config"))
            .ok_or("create: the config does not parse")?;
        let (name, _) = configs::all()
            .into_iter()
            .find(|(_, builtin)| *builtin == config)
            .ok_or_else(|| format!("create: no built-in config equals {:?}", config.title))?;
        if !self.stats.configs.contains(&name) {
            self.stats.configs.push(name);
        }
        let mut list = SourceList::new(
            config,
            text(get(&create, "uuid"))?,
            boolean(get(&create, "editButton"))?,
        );
        let title = text(get(&create, "title"))?;
        if title != list.title() {
            list.set_title(&title);
        }
        self.dom
            .roots
            .push((list.uuid().to_owned(), root_class(name, &title)?, None));
        self.dom.views.push(Vec::new());
        self.dom.shown.push(None);
        self.dom.scroll.push(0.0);
        self.lists.push(Slot {
            list,
            number_preview: boolean(get(&create, "numberPreview"))?,
        });
        Ok(())
    }

    fn op(&mut self, record: &Record, id: usize) -> Result<(), String> {
        self.stats.ops += 1;
        let op = js(record, "op")?;
        let args = op.as_array().ok_or("an op is an array")?;
        let name = text(at(args, 0))?;
        let threw = match record.field("threw") {
            Some(_) => Some(text(&js(record, "threw")?)?),
            None => None,
        };
        let recorded = record.field("threw").is_some() || hooked(&name);
        let dark = record
            .value("dark")
            .ok()
            .and_then(|d| d.as_bool())
            .unwrap_or(false);
        self.dom.views = self
            .lists
            .iter()
            .map(|slot| slot.list.items().cloned().collect())
            .collect();
        let result = self.dispatch(record, id, dark, &name, args)?;
        let error = result.err().map(|e| e.to_string());
        if recorded && error != threw {
            return Err(format!(
                "{name}: oracle threw {threw:?}, port returned {error:?}"
            ));
        }
        if let Some(error) = error {
            self.pending_errors.push_back(error);
        }
        Ok(())
    }

    fn dispatch(
        &mut self,
        record: &Record,
        id: usize,
        dark: bool,
        name: &str,
        args: &[JsValue],
    ) -> Result<Result<(), SourceListError>, String> {
        if id >= self.lists.len() {
            return Err(format!("op on unregistered list {id}"));
        }
        Ok(match name {
            "setTitle" => {
                self.lists[id].list.set_title(&text(at(args, 1))?);
                Ok(())
            }
            "setState" => {
                let (mut env, lists) = self.env(dark);
                lists[id].list.restore(&mut env, at(args, 1))
            }
            "clear" => {
                self.lists[id].list.clear();
                Ok(())
            }
            "refresh" => {
                let (mut env, lists) = self.env(dark);
                lists[id].list.refresh(&mut env)
            }
            "stop" => {
                self.lists[id].list.stop();
                Ok(())
            }
            "setOptionValues" => {
                let values = SourceListOptionValueConfig::list_from_js(at(args, 3))
                    .ok_or("setOptionValues values")?;
                let (type_key, option) = (text(at(args, 1))?, text(at(args, 2))?);
                let (mut env, lists) = self.env(dark);
                lists[id]
                    .list
                    .set_option_values(&mut env, &type_key, &option, values)
            }
            "addField" => {
                let key = jstr(at(args, 1))?;
                let parent = match at(args, 2) {
                    JsValue::Undefined => None,
                    v => Some(index(v)?),
                };
                let extra = supplemental(at(args, 3))?;
                let extra: Vec<&[SourceListItemState]> = extra.iter().map(Vec::as_slice).collect();
                let (mut env, lists) = self.env(dark);
                lists[id].list.add_field(&mut env, &extra, &key, parent)
            }
            "itemDrag" => {
                let data = at(args, 1);
                let source = match get(data, "source") {
                    JsValue::Null => return Err("item drag from an unregistered list".to_owned()),
                    v => index(v)?,
                };
                let payload = ItemDragPayload {
                    uuid: self.lists[source].list.uuid().to_owned(),
                    config_str: if boolean(get(data, "sameConfig"))? {
                        self.lists[id].list.config_str().to_owned()
                    } else {
                        String::from("\u{0}other")
                    },
                    index: index(get(data, "index"))?,
                    is_child: boolean(get(data, "isChild"))?,
                };
                let (x, y, end) = (
                    num(get(data, "x"))?,
                    num(get(data, "y"))?,
                    boolean(get(data, "end"))?,
                );
                let mut env = Env {
                    log: &self.log,
                    type_memory: &mut self.memory,
                    dark,
                    dom: &mut self.dom,
                };
                if source == id {
                    self.lists[id]
                        .list
                        .handle_item_drag(&mut env, &payload, None, x, y, end)
                } else {
                    let (target, source) = split_pair(&mut self.lists, id, source);
                    target.handle_item_drag(&mut env, &payload, Some(source), x, y, end)
                }
            }
            "fieldDrag" => {
                let data = at(args, 1);
                let extra = supplemental(at(args, 3))?;
                let extra: Vec<&[SourceListItemState]> = extra.iter().map(Vec::as_slice).collect();
                let fields = keys(get(data, "fields"))?;
                let children = keys(get(data, "children"))?;
                let (x, y, end) = (
                    num(get(data, "x"))?,
                    num(get(data, "y"))?,
                    boolean(get(data, "end"))?,
                );
                let (mut env, lists) = self.env(dark);
                lists[id]
                    .list
                    .handle_field_drag(&mut env, &extra, &fields, &children, x, y, end)
            }
            "frame" => return self.frame(record, id, args),
            "mouseDown" => {
                self.lists[id]
                    .list
                    .mouse_down(num(at(args, 1))?, num(at(args, 2))?);
                Ok(())
            }
            "mouseUp" => {
                self.lists[id].list.mouse_up();
                Ok(())
            }
            "mouseMove" => {
                let (x, y) = (num(at(args, 1))?, num(at(args, 2))?);
                let start = self.lists[id].list.mouse_move(&mut self.dom, x, y);
                start.map(|start| {
                    if let Some(start) = start {
                        self.pending_drag = Some((id, start));
                    }
                })
            }
            "contextMenu" => {
                self.pending_prompt = self.lists[id].list.context_menu(
                    index(at(args, 1))?,
                    num(at(args, 2))?,
                    num(at(args, 3))?,
                );
                Ok(())
            }
            "promptType" => {
                self.pending_prompt = self.lists[id]
                    .list
                    .prompt_type(&mut self.dom, index(at(args, 1))?);
                Ok(())
            }
            "typeResponse" => {
                let state = SourceListItemState::restored(at(args, 1));
                let (mut env, lists) = self.env(dark);
                lists[id].list.type_response(&mut env, state)
            }
            "toggleHidden" => {
                let (mut env, lists) = self.env(dark);
                lists[id].list.toggle_hidden(&mut env, index(at(args, 1))?)
            }
            "nameClick" => {
                let (i, now) = (index(at(args, 1))?, num(at(args, 2))?);
                let (mut env, lists) = self.env(dark);
                lists[id].list.name_click(&mut env, i, now)
            }
            "remove" => self.lists[id].list.remove(index(at(args, 1))?),
            "clearClick" => {
                self.pending_clear = Some(self.lists[id].list.request_clear(&mut self.dom));
                Ok(())
            }
            "clearResponse" => {
                self.lists[id].list.clear_response();
                Ok(())
            }
            other => return Err(format!("unknown op {other:?}")),
        })
    }

    fn frame(
        &mut self,
        record: &Record,
        id: usize,
        args: &[JsValue],
    ) -> Result<Result<(), SourceListError>, String> {
        let dark = boolean(at(args, 1))?;
        let time = match at(args, 2) {
            JsValue::Null => None,
            v => Some(num(v)?),
        };
        let mut expected: VecDeque<(JsString, f64, Option<String>)> = VecDeque::new();
        if record.field("numberPreviews").is_some() {
            for call in js(record, "numberPreviews")?
                .as_array()
                .ok_or("numberPreviews")?
            {
                let call = call.as_array().ok_or("a number preview call")?;
                let result = match at(call, 2) {
                    JsValue::Null => None,
                    v => Some(text(v)?),
                };
                expected.push_back((jstr(at(call, 0))?, num(at(call, 1))?, result));
            }
        }
        let mut mismatch: Option<String> = None;
        let mut preview = |key: &JsString, t: f64| -> Result<Option<String>, SourceListError> {
            Ok(match expected.pop_front() {
                Some((k, et, result)) if &k == key && et.to_bits() == t.to_bits() => result,
                other => {
                    mismatch.get_or_insert(format!(
                        "number preview ({}, {t}) called, oracle called {other:?}",
                        key.to_string_lossy()
                    ));
                    None
                }
            })
        };
        let has_preview = self.lists[id].number_preview;
        let (mut env, lists) = self.env(dark);
        let result = lists[id].list.frame(
            &mut env,
            time,
            if has_preview {
                Some(&mut preview)
            } else {
                None
            },
        );
        if let Some(m) = mismatch {
            return Err(m);
        }
        if !expected.is_empty() {
            return Err(format!(
                "oracle made more number preview calls: {expected:?}"
            ));
        }
        self.stats.previews += self.lists[id].list.items().len();
        Ok(result)
    }

    fn record(&mut self, record: &Record) -> Result<(), String> {
        if GeometryRecord::parse(record)?.is_some() {
            return Ok(());
        }
        if record.field("dragStart").is_none()
            && let Some((id, start)) = self.pending_drag.take()
        {
            return Err(format!(
                "list {id}: the port started a drag the oracle did not: {start:?}"
            ));
        }
        if let Some(ops) = record.field("slLog") {
            let ops = Value::from_json(ops).map_err(|e| e.0)?;
            return self.apply_log(&ops);
        }
        if record.field("slTypeMemory").is_some() {
            self.memory = js(record, "slTypeMemory")?;
            return Ok(());
        }
        if record.field("slMemory").is_some() {
            let expected = js_json::from_js(&js(record, "slMemory")?).encode();
            let actual = encode(&self.memory);
            if expected != actual {
                return Err(format!("type memory\n oracle {expected}\n port   {actual}"));
            }
            return Ok(());
        }
        if record.field("exception").is_some() {
            let error = record.string("error").unwrap_or_default();
            return match self.pending_errors.pop_front() {
                Some(port) if port == error => Ok(()),
                port => Err(format!("oracle exception {error:?}, port {port:?}")),
            };
        }
        if record.string("main").as_deref() == Some("source-list-type-prompt") {
            let data = js(record, "data")?;
            let prompt = self
                .pending_prompt
                .take()
                .ok_or("the oracle prompted for a type, the port did not")?;
            let state = SourceListItemState::restored(get(&data, "state"));
            let config = SourceListConfig::from_js(get(&data, "config")).ok_or("prompt config")?;
            if prompt.uuid != text(get(&data, "uuid"))?
                || prompt.state != state
                || prompt.config != config
                || !same_rect(prompt.rect, rect(get(&data, "rect"))?)
            {
                return Err(format!("type prompt differs: port {prompt:?}"));
            }
            return Ok(());
        }
        if record.string("main").as_deref() == Some("source-list-clear-prompt") {
            let data = js(record, "data")?;
            let prompt = self
                .pending_clear
                .take()
                .ok_or("the oracle prompted to clear, the port did not")?;
            if prompt.uuid != text(get(&data, "uuid"))?
                || !same_rect(prompt.rect, rect(get(&data, "rect"))?)
            {
                return Err(format!("clear prompt differs: port {prompt:?}"));
            }
            return Ok(());
        }
        if record
            .string("eval")
            .is_some_and(|e| e.starts_with("/*sl:widths*/"))
        {
            return self.type_widths(record);
        }
        if record
            .string("eval")
            .is_some_and(|e| e.starts_with("/*sl:scroll*/"))
        {
            return self.scroll(record);
        }
        if record.field("capture").is_some() {
            if !self.pending_errors.is_empty() {
                return Err(format!(
                    "port errors the oracle never threw: {:?}",
                    self.pending_errors
                ));
            }
            if let Some(prompt) = self.pending_prompt.take() {
                return Err(format!(
                    "the port prompted for a type, the oracle did not: {prompt:?}"
                ));
            }
            if let Some(prompt) = self.pending_clear.take() {
                return Err(format!(
                    "the port prompted to clear, the oracle did not: {prompt:?}"
                ));
            }
            return Ok(());
        }
        if let Some(name) = record.string("main") {
            return match name.as_str() {
                "show" | "frc-coordinate-system-warning" => Ok(()),
                other => Err(format!("the main message {other} is not modeled here")),
            };
        }
        let Some(id) = record.field("sl") else {
            return Err(format!("record kind {:?} is not modeled", kind_of(record)));
        };
        let id = Value::from_json(id)
            .map_err(|e| e.0)?
            .as_f64()
            .ok_or("sl id")? as usize;
        if record.field("create").is_some() {
            if id != self.lists.len() {
                return Err(format!("list {id} registered out of order"));
            }
            return self.create(record);
        }
        if record.field("op").is_some() {
            return self.op(record, id);
        }
        if record.field("dragStart").is_some() {
            let data = js(record, "dragStart")?;
            let Some((list, start)) = self.pending_drag.take() else {
                return Err("the oracle started a drag the port did not".to_owned());
            };
            let same = list == id
                && start.x.to_bits() == num(get(&data, "x"))?.to_bits()
                && start.y.to_bits() == num(get(&data, "y"))?.to_bits()
                && start.offset_x.to_bits() == num(get(&data, "offsetX"))?.to_bits()
                && start.offset_y.to_bits() == num(get(&data, "offsetY"))?.to_bits()
                && start.payload.index == index(get(&data, "index"))?
                && start.payload.is_child == boolean(get(&data, "isChild"))?;
            if !same {
                return Err(format!(
                    "drag start differs: port {start:?}, oracle {}",
                    encode(&data)
                ));
            }
            return Ok(());
        }
        if record.field("dump").is_some() {
            self.stats.dumps += 1;
            let mut expected = js(record, "dump")?;
            let expected_config = expected
                .as_object_mut()
                .and_then(|o| o.remove("config"))
                .ok_or("dump has no config")?;
            let list = &self.lists[id].list;
            let config = SourceListConfig::from_js(&expected_config).ok_or("dump config")?;
            if &config != list.config() {
                return Err(format!(
                    "list {id}: config differs\n oracle {config:?}\n port   {:?}",
                    list.config()
                ));
            }
            let expected = encode(&expected);
            let actual = encode(&list_dump(list, &self.log));
            if expected != actual {
                return Err(format!(
                    "list {id} dump\n oracle {expected}\n port   {actual}"
                ));
            }
            return Ok(());
        }
        Err("sl record without create, op, dragStart or dump".to_owned())
    }
}

impl Harness {
    /// A fixture set the `scrollTop` of the one displayed list with this root class.
    fn scroll(&mut self, record: &Record) -> Result<(), String> {
        let value = js(record, "value")?;
        let class = text(at(value.as_array().ok_or("sl:scroll is a pair")?, 0))?;
        let top = num(at(value.as_array().ok_or("sl:scroll is a pair")?, 1))?;
        let mut slots = (0..self.dom.roots.len())
            .filter(|&i| self.dom.roots[i].1 == class && self.dom.shown[i] == Some(true));
        let (Some(slot), None) = (slots.next(), slots.next()) else {
            return Err(format!("sl:scroll: not one displayed {class} list"));
        };
        let root = self.dom.roots[slot]
            .2
            .clone()
            .ok_or_else(|| format!("sl:scroll: list {slot} was never read"))?;
        self.dom
            .reads
            .write_f64("scrollTop", &format!("{root}>{LIST}"), top);
        self.dom.scroll[slot] = top;
        self.dom.scroll_writes += 1;
        Ok(())
    }

    /// Each item's type name and `--type-width`, per list root in document order. A root
    /// class names the lists created with it, which hub.html lays out in creation order.
    fn type_widths(&mut self, record: &Record) -> Result<(), String> {
        let value = js(record, "value")?;
        let mut seen: Vec<&'static str> = Vec::new();
        for root in value.as_array().ok_or("sl:widths is a list")? {
            let class_name = text(at(root.as_array().ok_or("a root")?, 0))?;
            let items = at(root.as_array().ok_or("a root")?, 1)
                .as_array()
                .ok_or("root items")?;
            let class = self
                .dom
                .roots
                .iter()
                .map(|(_, class, _)| *class)
                .find(|class| class_name.split(' ').any(|c| c == *class))
                .ok_or_else(|| format!("no list has the root {class_name:?}"))?;
            let nth = seen.iter().filter(|c| **c == class).count();
            seen.push(class);
            let id = self
                .dom
                .roots
                .iter()
                .enumerate()
                .filter(|(_, (_, c, _))| *c == class)
                .nth(nth)
                .map(|(id, _)| id)
                .ok_or_else(|| format!("{class_name:?} #{nth} was never created"))?;
            let port: Vec<(String, String)> = self.lists[id]
                .list
                .items()
                .map(|item| {
                    let name = item
                        .rendered
                        .as_ref()
                        .map_or(String::new(), |r| r.type_name.clone());
                    let width = item.type_width.map_or(String::new(), |w| {
                        format!("{}px", asrs_js::number::to_string(w))
                    });
                    (name, width)
                })
                .collect();
            let blink = items
                .iter()
                .map(|item| {
                    let item = item.as_array().ok_or("an item")?;
                    Ok((text(at(item, 0))?, text(at(item, 1))?))
                })
                .collect::<Result<Vec<_>, String>>()?;
            if port != blink {
                return Err(format!(
                    "list {id} type widths\n oracle {blink:?}\n port   {port:?}"
                ));
            }
            self.stats.widths += port.len();
        }
        Ok(())
    }
}

fn hooked(name: &str) -> bool {
    matches!(
        name,
        "setTitle"
            | "setState"
            | "clear"
            | "refresh"
            | "stop"
            | "setOptionValues"
            | "addField"
            | "itemDrag"
            | "fieldDrag"
            | "frame"
    )
}

fn case_ids() -> Vec<String> {
    let dir = vectors_dir().join(CASES);
    let mut ids: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().into_string().ok()?;
            let stem = name.split('.').next()?.to_owned();
            Some(format!("{CASES}/{stem}"))
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

fn finish_case(id: &str, mut harness: Harness) -> Result<Stats, String> {
    harness.stats.checked = harness.dom.checked;
    let (type_names, scroll_writes) = (harness.dom.type_names, harness.dom.scroll_writes);
    harness.stats.reads = harness
        .dom
        .reads
        .finish()
        .map_err(|e| format!("{id}: {e}"))?;
    if harness.stats.checked + type_names + scroll_writes != harness.stats.reads {
        return Err(format!(
            "{id}: {} layout reads, {} checked against ListLayout, {type_names} type names, \
             {scroll_writes} scroll writes",
            harness.stats.reads, harness.stats.checked
        ));
    }
    if let Some((list, start)) = harness.pending_drag {
        return Err(format!(
            "{id}: list {list} started a drag at the end: {start:?}"
        ));
    }
    if let Some(prompt) = harness.pending_prompt.take() {
        return Err(format!(
            "{id}: the port prompted for a type at the end: {prompt:?}"
        ));
    }
    if let Some(prompt) = harness.pending_clear.take() {
        return Err(format!(
            "{id}: the port prompted to clear at the end: {prompt:?}"
        ));
    }
    if !harness.pending_errors.is_empty() {
        return Err(format!(
            "{id}: port errors never thrown: {:?}",
            harness.pending_errors
        ));
    }
    Ok(harness.stats)
}

fn run_case(id: &str) -> Result<Stats, String> {
    let records = load(id)?;
    let mut tally = Tally::new(id, &records, kind_of);
    for (kind, by) in COVERED {
        if records.iter().any(|r| kind_of(r) == kind) {
            tally.covered_elsewhere(kind, by);
        }
    }
    let mut harness = Harness::new(&records)?;
    let mut result = Ok(());
    for record in &records {
        match harness.record(record) {
            Err(e) => {
                result = Err(format!("{id} line {}: {e}", record.line));
                break;
            }
            Ok(()) => {
                let kind = kind_of(record);
                if kind != LIST_GEOMETRY && !COVERED.iter().any(|(k, _)| *k == kind) {
                    tally.compared(&kind);
                }
                if let Some(e) = harness.dom.error.take() {
                    result = Err(format!("{id} line {}: {e}", record.line));
                    break;
                }
            }
        }
    }
    match result.and_then(|()| finish_case(id, harness)) {
        Err(e) => {
            std::mem::forget(tally);
            Err(e)
        }
        Ok(stats) => {
            (0..stats.reads).for_each(|_| tally.compared(LIST_GEOMETRY));
            tally.finish();
            Ok(stats)
        }
    }
}

#[test]
fn source_list_fixtures() {
    let ids = case_ids();
    assert!(!ids.is_empty(), "no {CASES} vectors");
    let mut failures = Vec::new();
    let mut configs: Vec<&str> = Vec::new();
    let mut reads = 0;
    for id in &ids {
        match run_case(id) {
            Ok(stats) => {
                assert!(stats.dumps > 0, "{id}: no source list dumps");
                eprintln!(
                    "{id}: {} ops, {} dumps, {} item previews, {} layout reads ({} against \
                     ListLayout), {} type widths",
                    stats.ops,
                    stats.dumps,
                    stats.previews,
                    stats.reads,
                    stats.checked,
                    stats.widths
                );
                reads += stats.reads;
                for name in stats.configs {
                    if !configs.contains(&name) {
                        configs.push(name);
                    }
                }
            }
            Err(e) => failures.push(e),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        ids.len(),
        failures.join("\n")
    );
    assert!(reads > 0, "no layout reads replayed");
    for (name, _) in configs::all() {
        assert!(
            configs.contains(&name),
            "no fixture builds a {name} source list"
        );
    }
}
