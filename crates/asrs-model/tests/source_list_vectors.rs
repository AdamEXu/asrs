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
use asrs_model::source_list::{
    Env, ItemDragPayload, ItemDragStart, ItemView, Length, ListGeometry, Rect, SourceList,
    SourceListError, TypePrompt, configs,
};
use asrs_model::source_list_config::{
    SourceListConfig, SourceListItemState, SourceListOptionValueConfig, SourceListTypeMemory,
    SourceListTypeMemoryEntry,
};
use asrs_parity::Value;
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
    configs: Vec<&'static str>,
}

struct Harness {
    log: Log,
    realm: JsRealm,
    memory: SourceListTypeMemory,
    lists: Vec<Slot>,
    pending_errors: VecDeque<String>,
    pending_prompt: Option<TypePrompt>,
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

fn item_state(v: &JsValue) -> Result<SourceListItemState, String> {
    SourceListItemState::from_js(v).ok_or_else(|| format!("not an item state: {v:?}"))
}

fn states(v: &JsValue) -> Result<Vec<SourceListItemState>, String> {
    v.as_array()
        .ok_or("expected an array of item states")?
        .iter()
        .map(item_state)
        .collect()
}

fn rect(v: &JsValue) -> Result<Rect, String> {
    let a = v.as_array().ok_or("a rect is [x, y, width, height]")?;
    Ok(Rect {
        x: num(at(a, 0))?,
        y: num(at(a, 1))?,
        width: num(at(a, 2))?,
        height: num(at(a, 3))?,
    })
}

fn geometry(v: &JsValue) -> Result<ListGeometry, String> {
    Ok(ListGeometry {
        root: rect(get(v, "root"))?,
        list: rect(get(v, "list"))?,
        items: get(v, "items")
            .as_array()
            .ok_or("geometry items")?
            .iter()
            .map(rect)
            .collect::<Result<_, _>>()?,
    })
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
            out.push(r.log_key.to_js());
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
    o.insert("displayed", list.displayed_state(log).to_js());
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

fn memory_js(memory: &SourceListTypeMemory) -> JsValue {
    memory.to_js()
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
    fn new() -> Harness {
        Harness {
            log: Log::new(true),
            realm: JsRealm::new(Box::new(NoRandom)),
            memory: JsObject::new(),
            lists: Vec::new(),
            pending_errors: VecDeque::new(),
            pending_prompt: None,
            pending_drag: None,
            stats: Stats::default(),
        }
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
                let states = states(at(args, 1))?;
                let (mut env, lists) = self.env(dark);
                lists[id].list.set_state(&mut env, states)
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
                let geometry = geometry(at(args, 2))?;
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
                };
                if source == id {
                    self.lists[id]
                        .list
                        .handle_item_drag(&mut env, &payload, None, x, y, end, &geometry)
                } else {
                    let (target, source) = split_pair(&mut self.lists, id, source);
                    target.handle_item_drag(&mut env, &payload, Some(source), x, y, end, &geometry)
                }
            }
            "fieldDrag" => {
                let data = at(args, 1);
                let geometry = geometry(at(args, 2))?;
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
                    .handle_field_drag(&mut env, &extra, &fields, &children, x, y, end, &geometry)
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
                let geometry = geometry(at(args, 3))?;
                if let Some(start) = self.lists[id].list.mouse_move(
                    num(at(args, 1))?,
                    num(at(args, 2))?,
                    &geometry.items,
                ) {
                    self.pending_drag = Some((id, start));
                }
                Ok(())
            }
            "contextMenu" => {
                self.pending_prompt = self.lists[id].list.context_menu(index(at(args, 1))?);
                Ok(())
            }
            "promptType" => {
                self.pending_prompt = self.lists[id].list.prompt_type(index(at(args, 1))?);
                Ok(())
            }
            "typeResponse" => {
                let state = item_state(at(args, 1))?;
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
            "remove" => {
                self.lists[id].list.remove(index(at(args, 1))?);
                Ok(())
            }
            "clearClick" => {
                self.lists[id].list.request_clear();
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
            let value = js(record, "slTypeMemory")?;
            self.memory = SourceListTypeMemoryEntry::memory_from_js(&value)
                .ok_or("slTypeMemory does not parse")?;
            return Ok(());
        }
        if record.field("slMemory").is_some() {
            let expected = js_json::from_js(&js(record, "slMemory")?).encode();
            let actual = encode(&memory_js(&self.memory));
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
            let state = item_state(get(&data, "state"))?;
            let config = SourceListConfig::from_js(get(&data, "config")).ok_or("prompt config")?;
            if prompt.uuid != text(get(&data, "uuid"))?
                || prompt.state != state
                || prompt.config != config
            {
                return Err(format!("type prompt differs: port {prompt:?}"));
            }
            return Ok(());
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
            return Ok(());
        }
        let Some(id) = record.field("sl") else {
            return Ok(());
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

fn run_case(id: &str) -> Result<Stats, String> {
    let mut harness = Harness::new();
    for record in load(id)? {
        harness
            .record(&record)
            .map_err(|e| format!("{id} line {}: {e}", record.line))?;
    }
    if let Some((list, start)) = harness.pending_drag {
        return Err(format!(
            "{id}: list {list} started a drag at the end: {start:?}"
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

#[test]
fn source_list_fixtures() {
    let ids = case_ids();
    assert!(!ids.is_empty(), "no {CASES} vectors");
    let mut failures = Vec::new();
    let mut configs: Vec<&str> = Vec::new();
    for id in &ids {
        match run_case(id) {
            Ok(stats) => {
                assert!(stats.dumps > 0, "{id}: no source list dumps");
                eprintln!(
                    "{id}: {} ops, {} dumps, {} item previews",
                    stats.ops, stats.dumps, stats.previews
                );
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
    for (name, _) in configs::all() {
        assert!(
            configs.contains(&name),
            "no fixture builds a {name} source list"
        );
    }
}
