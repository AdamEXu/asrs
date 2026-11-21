#![allow(clippy::disallowed_methods)]

//! Replays `parity/specs/dom/tabs/<case>.entry.json` against `asrs_hub::tabs::Tabs` wired to
//! the real `asrs_tabs` controllers and renderers for Table, Console and Metadata, and
//! compares the emitted records with the committed `dom/tabs/<case>` vector, line for line.
//! Other tab kinds keep stub halves whose recorded states are spliced in from the expected
//! records (they are other units' work); their UUID draws still come from the fixture's
//! seeded `Math.random`. `dom`, `canvas`, `measure` and `sourceLists` capture kinds belong to
//! other layers and are skipped while comparing.
//!
//! jsdom's `:nth-child` and `:nth-of-type` go stale once the table header is rebuilt, so
//! table fixtures lay header cells out by `data-i`, set by a `/*table:tag*/` eval step right
//! before each event that reads header rects. Events the harness does not model fail the run.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::{Cell, Ref, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use asrs_hub::tabs::{TabFactory, TabParts, Tabs, TabsHost};
use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{Clock, Rng, create_uuid};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::Log;
use asrs_log::log::LoggableType;
use asrs_model::assets::AdvantageScopeAssets;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_model::selection::Selection;
use asrs_model::selection_impl::SelectionImpl;
use asrs_model::source_list::Rect;
use asrs_model::source_list_config::SourceListTypeMemory;
use asrs_model::tab_controller::{Command, TabCommand, TabController, TabRenderer};
use asrs_model::tab_type::TabType;
use asrs_parity::cve1::Value;
use asrs_parity::json::{self, Json};
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};
use asrs_tabs::console::controller::{ConsoleController, ConsoleDom, ConsoleRenderer};
use asrs_tabs::cx::TabCx;
use asrs_tabs::field_2d::{Field2dController, Field2dDom, Field2dRenderer};
use asrs_tabs::list_cx::{ListParts, SourceListCx};
use asrs_tabs::metadata::{MetadataController, MetadataRenderer};
use asrs_tabs::table::{TableController, TableDom, TableRenderer};

fn err(message: impl Into<String>) -> String {
    message.into()
}

fn js(s: &str) -> JsString {
    JsString::from(s)
}

fn cve_str(s: &str) -> Value {
    js_json::from_js(&JsValue::String(js(s)))
}

fn to_js(json: &Json) -> Result<JsValue, String> {
    js_json::to_js(&Value::from_json_lenient(json))
}

fn truthy(v: &Value) -> bool {
    !matches!(v, Value::Null | Value::Undefined | Value::Bool(false))
        && v.as_f64().is_none_or(|n| n != 0.0 && !n.is_nan())
        && v.as_str().is_none_or(|s| !s.is_empty())
}

struct DomRng(u32);

impl Rng for DomRng {
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4294967296.0
    }
}

struct ScriptedClock(Rc<Cell<f64>>);

impl Clock for ScriptedClock {
    fn now(&self) -> f64 {
        self.0.get()
    }
}

/// `el.matches(selector)` with descendant combinators, `.class` and `tag` compounds plus
/// `:nth-of-type(...)`, on the small element trees the tab renderers own.
struct ElDesc {
    tag: &'static str,
    classes: &'static [&'static str],
    hidden: bool,
    /// Position among same-tag siblings, for `:nth-of-type`.
    pos: usize,
    /// Position among all siblings, for `:nth-child`, where the tree knows it.
    child_pos: Option<usize>,
    /// The `data-i` attribute a `/*table:tag*/` eval step set.
    data_i: Option<usize>,
    parent: Option<Box<ElDesc>>,
}

fn el(
    tag: &'static str,
    classes: &'static [&'static str],
    hidden: bool,
    pos: usize,
    parent: ElDesc,
) -> ElDesc {
    ElDesc {
        tag,
        classes,
        hidden,
        pos,
        child_pos: None,
        data_i: None,
        parent: Some(Box::new(parent)),
    }
}

fn matches_compound(el: &ElDesc, compound: &str) -> bool {
    let compound = match compound.split_once("[data-i=\"") {
        Some((head, rest)) => {
            let value = rest.trim_end_matches("\"]").parse::<usize>().ok();
            if value.is_none() || el.data_i != value {
                return false;
            }
            head
        }
        None => compound,
    };
    let (compound, nth, pos) = if let Some((head, arg)) = compound.split_once(":nth-of-type(") {
        (head, Some(arg.trim_end_matches(')')), Some(el.pos))
    } else if let Some((head, arg)) = compound.split_once(":nth-child(") {
        (head, Some(arg.trim_end_matches(')')), el.child_pos)
    } else {
        (compound, None, None)
    };
    let mut parts = compound.split('.');
    let tag = parts.next().unwrap_or("");
    if !tag.is_empty() && tag != el.tag {
        return false;
    }
    for class in parts {
        if !el.classes.iter().any(|c| c == &class) {
            return false;
        }
    }
    if let Some(arg) = nth {
        let Some(pos) = pos else {
            return false;
        };
        let arg = arg.trim();
        let pos = pos as f64;
        let ok = match arg {
            "odd" => pos % 2.0 == 1.0,
            "even" => pos % 2.0 == 0.0,
            _ if arg.contains('n') => {
                let mut it = arg.splitn(2, 'n');
                let a_raw = it.next().unwrap_or("").trim().trim_end_matches('+');
                let a = match a_raw {
                    "" | "+" => 1.0,
                    "-" => -1.0,
                    _ => a_raw.parse::<f64>().unwrap_or(f64::NAN),
                };
                let b = it
                    .next()
                    .unwrap_or("0")
                    .trim()
                    .parse::<f64>()
                    .unwrap_or(0.0);
                let k = (pos - b) / a;
                a != 0.0 && k >= 0.0 && k.fract() == 0.0
            }
            _ => pos == arg.parse::<f64>().unwrap_or(f64::NAN),
        };
        if !ok {
            return false;
        }
    }
    true
}

fn matches_selector(el: &ElDesc, selector: &str) -> bool {
    let compounds: Vec<&str> = selector.split_whitespace().collect();
    let Some(last) = compounds.last() else {
        return false;
    };
    if !matches_compound(el, last) {
        return false;
    }
    let mut cursor = el.parent.as_deref();
    for compound in compounds[..compounds.len() - 1].iter().rev() {
        let mut found = false;
        while let Some(parent) = cursor {
            if matches_compound(parent, compound) {
                found = true;
                cursor = parent.parent.as_deref();
                break;
            }
            cursor = parent.parent.as_deref();
        }
        if !found {
            return false;
        }
    }
    true
}

fn is_hidden(el: &ElDesc) -> bool {
    let mut cursor = Some(el);
    while let Some(e) = cursor {
        if e.hidden {
            return true;
        }
        cursor = e.parent.as_deref();
    }
    false
}

/// `env.ts::rectOf`: fixture rules first, then the defaults, first match wins. A rule without
/// `rect` resolves to the parent's rect.
struct Layout {
    rules: Vec<(String, Option<[f64; 4]>)>,
}

impl Layout {
    fn rect_of(&self, el: &ElDesc) -> Rect {
        if is_hidden(el) {
            return Rect::default();
        }
        for (selector, rect) in &self.rules {
            if !matches_selector(el, selector) {
                continue;
            }
            if let Some(rect) = rect {
                return Rect {
                    x: rect[0],
                    y: rect[1],
                    width: rect[2],
                    height: rect[3],
                };
            }
            return el
                .parent
                .as_deref()
                .map(|p| self.rect_of(p))
                .unwrap_or_default();
        }
        Rect::default()
    }
}

fn root_el(hidden: bool, pos: usize) -> ElDesc {
    let renderer_content = ElDesc {
        tag: "div",
        classes: &["renderer-content"],
        hidden: false,
        pos: 1,
        child_pos: None,
        data_i: None,
        parent: None,
    };
    el("div", &[], hidden, pos, renderer_content)
}

fn table_container_el(hidden: bool, pos: usize) -> ElDesc {
    el(
        "div",
        &["data-table-container"],
        false,
        2,
        root_el(hidden, pos),
    )
}

fn table_body(hidden: bool, pos: usize) -> ElDesc {
    let table = el(
        "table",
        &["data-table"],
        false,
        1,
        table_container_el(hidden, pos),
    );
    el("tbody", &[], false, 1, table)
}

fn table_th(hidden: bool, tab_pos: usize, th_pos: usize, tagged: bool) -> ElDesc {
    let header = el("tr", &[], false, 1, table_body(hidden, tab_pos));
    ElDesc {
        child_pos: Some(th_pos),
        data_i: tagged.then(|| th_pos - 1),
        ..el("th", &[], false, th_pos, header)
    }
}

fn table_tr(hidden: bool, tab_pos: usize, tr_pos: usize) -> ElDesc {
    el("tr", &[], false, tr_pos, table_body(hidden, tab_pos))
}

fn console_container_el(hidden: bool, pos: usize) -> ElDesc {
    el(
        "div",
        &["console-table-container"],
        false,
        2,
        root_el(hidden, pos),
    )
}

fn field_2d_container_el(hidden: bool, pos: usize) -> ElDesc {
    el(
        "div",
        &["field-2d-canvas-container"],
        false,
        1,
        root_el(hidden, pos),
    )
}

fn field_2d_scroll_el(hidden: bool, pos: usize) -> ElDesc {
    el("div", &["field-2d-scroll"], false, 2, root_el(hidden, pos))
}

#[derive(Clone)]
enum DomKind {
    Table(Rc<RefCell<TableDom>>),
    Console(Rc<RefCell<ConsoleDom>>),
    Field2d(Rc<RefCell<Field2dDom>>),
    Other,
}

type Doms = Rc<RefCell<Vec<(String, DomKind)>>>;
type CommandSlots = Rc<RefCell<HashMap<String, Rc<RefCell<Option<JsValue>>>>>>;

struct Factory {
    doms: Doms,
    commands: CommandSlots,
}

struct Recorder<T> {
    inner: T,
    slot: Rc<RefCell<Option<JsValue>>>,
}

impl<T, Cx: TabCx + ?Sized> TabRenderer<Cx> for Recorder<T>
where
    T: TabRenderer<Cx> + 'static,
{
    fn save_state(&self, cx: &Cx) -> JsValue {
        self.inner.save_state(cx)
    }

    fn restore_state(&mut self, cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        self.inner.restore_state(cx, state)
    }

    fn aspect_ratio(&self, cx: &Cx) -> Option<f64> {
        self.inner.aspect_ratio(cx)
    }

    fn render(&mut self, cx: &mut Cx, command: &dyn TabCommand) -> Result<(), JsError> {
        *self.slot.borrow_mut() = Some(command.to_js());
        self.inner.render(cx, command)
    }

    fn renderer_uuid(&self) -> Option<&JsString> {
        self.inner.renderer_uuid()
    }

    fn timestamp_range(&self, cx: &Cx) -> Option<[f64; 2]> {
        self.inner.timestamp_range(cx)
    }
}

/// The halves of tab kinds other units own. `command` returns `null` and every state is
/// `undefined`, which the comparison replaces with the value upstream recorded.
struct StubController {
    timeline: bool,
}

impl<Cx: TabCx + ?Sized> TabController<Cx> for StubController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Undefined
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn refresh(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn new_assets(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn active_fields(&self, _cx: &Cx) -> Vec<JsString> {
        Vec::new()
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        self.timeline
    }

    fn command(&mut self, _cx: &mut Cx, _uuid: &JsValue) -> Result<Command, JsError> {
        Ok(Rc::new(JsValue::Null))
    }
}

struct StubRenderer;

impl<Cx: TabCx + ?Sized> TabRenderer<Cx> for StubRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Undefined
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        None
    }

    fn render(&mut self, _cx: &mut Cx, _command: &dyn TabCommand) -> Result<(), JsError> {
        Ok(())
    }
}

impl TabFactory<Host> for Factory {
    fn create(&mut self, cx: &mut Host, tab_type: Option<TabType>) -> TabParts<Host> {
        let uuid = JsString::from(create_uuid(&mut cx.rng));
        let slot: Rc<RefCell<Option<JsValue>>> = Rc::new(RefCell::new(None));
        self.commands
            .borrow_mut()
            .insert(uuid.to_string_lossy(), slot.clone());
        self.doms
            .borrow_mut()
            .push((uuid.to_string_lossy(), DomKind::Other));
        match tab_type {
            Some(TabType::Table) => {
                let dom = Rc::new(RefCell::new(TableDom::default()));
                let renderer_uuid = JsString::from(create_uuid(&mut cx.rng));
                self.doms.borrow_mut().last_mut().unwrap().1 = DomKind::Table(dom.clone());
                TabParts {
                    uuid: uuid.clone(),
                    controller: Box::new(TableController::new(&uuid, dom.clone())),
                    renderer: Box::new(Recorder {
                        inner: TableRenderer::new(renderer_uuid, dom),
                        slot,
                    }),
                }
            }
            Some(TabType::Console) => {
                let dom = Rc::new(RefCell::new(ConsoleDom::default()));
                self.doms.borrow_mut().last_mut().unwrap().1 = DomKind::Console(dom.clone());
                TabParts {
                    uuid: uuid.clone(),
                    controller: Box::new(ConsoleController::new(dom.clone())),
                    renderer: Box::new(Recorder {
                        inner: ConsoleRenderer::new(dom),
                        slot,
                    }),
                }
            }
            Some(TabType::Metadata) => TabParts {
                uuid: uuid.clone(),
                controller: Box::new(MetadataController::new(cx).unwrap()),
                renderer: Box::new(Recorder {
                    inner: MetadataRenderer,
                    slot,
                }),
            },
            Some(TabType::Field2d) => {
                let dom = Rc::new(RefCell::new(Field2dDom::default()));
                self.doms.borrow_mut().last_mut().unwrap().1 = DomKind::Field2d(dom.clone());
                TabParts {
                    uuid: uuid.clone(),
                    controller: Box::new(Field2dController::new(cx)),
                    renderer: Box::new(Recorder {
                        inner: Field2dRenderer::new(dom),
                        slot,
                    }),
                }
            }
            other => {
                let timeline = !matches!(
                    other,
                    Some(
                        TabType::Documentation
                            | TabType::Video
                            | TabType::Table
                            | TabType::Console
                            | TabType::Metadata
                    )
                );
                // Each upstream controller draws `createUUID()` once per `SourceList` it
                // owns, after its own UUID.
                let source_lists = match other {
                    Some(TabType::LineGraph) => 3,
                    Some(
                        TabType::Field3d
                        | TabType::Statistics
                        | TabType::Swerve
                        | TabType::Mechanism
                        | TabType::Points,
                    ) => 1,
                    _ => 0,
                };
                for _ in 0..source_lists {
                    create_uuid(&mut cx.rng);
                }
                TabParts {
                    uuid,
                    controller: Box::new(StubController { timeline }),
                    renderer: Box::new(Recorder {
                        inner: StubRenderer,
                        slot,
                    }),
                }
            }
        }
    }
}

struct Host {
    log: Rc<RefCell<Log>>,
    selection: SelectionImpl,
    preferences: Option<Preferences>,
    assets: Option<AdvantageScopeAssets>,
    assets_raw: JsValue,
    type_memory: SourceListTypeMemory,
    dark: bool,
    rng: DomRng,
    inner_height: f64,
    renderer_top: f64,
    messages: Vec<NamedMessage<HubToMain>>,
}

impl TabCx for Host {
    fn tab_log(&self) -> Ref<'_, Log> {
        self.log.borrow()
    }

    fn tab_selection(&mut self) -> &mut dyn Selection {
        &mut self.selection
    }

    fn tab_preferences(&self) -> Option<&Preferences> {
        self.preferences.as_ref()
    }

    fn tab_assets(&self) -> Option<&AdvantageScopeAssets> {
        self.assets.as_ref()
    }

    fn tab_rng(&mut self) -> &mut dyn Rng {
        &mut self.rng
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.messages.push(message);
    }
}

impl SourceListCx for Host {
    fn tab_dark(&self) -> bool {
        self.dark
    }

    fn list_parts(&mut self) -> ListParts<'_> {
        ListParts {
            log: self.log.borrow(),
            type_memory: &mut self.type_memory,
            selection: &mut self.selection,
            dark: self.dark,
        }
    }
}

impl TabsHost for Host {
    fn inner_height(&self) -> f64 {
        self.inner_height
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        self.renderer_top
    }
}

struct Emit {
    values: Vec<(&'static str, Value)>,
}

fn pair_value(pair: Option<[f64; 2]>) -> Value {
    pair.map_or(Value::Null, |[a, b]| {
        Value::Array(vec![Value::number(a), Value::number(b)])
    })
}

fn selection_value(sel: &mut SelectionImpl) -> Value {
    let nullable = |t: Option<f64>| t.map_or(Value::Null, Value::number);
    let controls = sel.controls();
    let range = sel.get_timeline_range();
    Value::object([
        ("mode", Value::number(sel.get_mode().number())),
        ("hoveredTime", nullable(sel.get_hovered_time())),
        ("selectedTime", nullable(sel.get_selected_time())),
        ("renderTime", nullable(sel.get_render_time())),
        ("currentLiveTime", nullable(sel.get_current_live_time())),
        ("grabZoomRange", pair_value(sel.get_grab_zoom_range())),
        (
            "timelineRange",
            Value::Array(vec![Value::number(range[0]), Value::number(range[1])]),
        ),
        (
            "timelineIsMaxZoom",
            Value::Bool(sel.get_timeline_is_max_zoom()),
        ),
        (
            "buttons",
            Value::object([
                ("play", Value::Bool(controls.play_hidden)),
                ("pause", Value::Bool(controls.pause_hidden)),
                ("lock", Value::Bool(controls.lock_hidden)),
                ("unlock", Value::Bool(controls.unlock_hidden)),
                (
                    "showLockButtons",
                    Value::string(match controls.show_lock_buttons {
                        None => "",
                        Some(true) => "1",
                        Some(false) => "0",
                    }),
                ),
            ]),
        ),
    ])
}

struct Runner {
    layout: Layout,
    host: Host,
    tabs: Tabs<Host>,
    doms: Doms,
    commands: CommandSlots,
    emitted: Vec<Emit>,
    clock: Rc<Cell<f64>>,
    now: f64,
    start: f64,
    last_frame: f64,
    frame_period: f64,
    show_pending: bool,
    frame_loop_dead: bool,
    active_drag: Option<JsValue>,
    /// Table tabs whose header cells carry `data-i` tags. Upstream drops the tags when
    /// `updateFields` rebuilds the header; the harness drops them every frame, so a fixture
    /// re-tags right before each event that reads header rects.
    tagged_tables: Vec<String>,
    sidebar_width: f64,
    sidebar_expanded: Vec<JsString>,
    ignored: Vec<String>,
    device_pixel_ratio: f64,
    images: HashMap<JsString, [f64; 2]>,
}

impl Runner {
    fn emit_main(&mut self, name: &str, data: Value) {
        if self.ignored.iter().any(|i| i == name) {
            return;
        }
        self.emitted.push(Emit {
            values: vec![("main", cve_str(name)), ("data", data)],
        });
    }

    fn exception(&mut self, where_: String, error: impl Into<String>) {
        self.emitted.push(Emit {
            values: vec![
                ("exception", cve_str(&where_)),
                ("error", cve_str(&error.into())),
            ],
        });
    }

    fn tab_dom(&self, uuid: &str) -> DomKind {
        self.doms
            .borrow()
            .iter()
            .find(|(u, _)| u == uuid)
            .map(|(_, k)| k.clone())
            .unwrap_or(DomKind::Other)
    }

    /// The DOM state each renderer's shared `dom` mirrors: hidden flags, resolved rects.
    fn sync_dom(&mut self) {
        for i in 0..self.tabs.len() {
            let Some(tab) = self.tabs.tab(i) else {
                continue;
            };
            let hidden = tab.content_hidden();
            let uuid = tab.uuid().as_str().map(|s| s.to_string_lossy());
            let Some(uuid) = uuid else { continue };
            let pos = i + 1;
            match self.tab_dom(&uuid) {
                DomKind::Table(dom) => {
                    let tagged = self.tagged_tables.contains(&uuid);
                    let mut dom = dom.borrow_mut();
                    dom.hidden = hidden;
                    dom.container = self.layout.rect_of(&table_container_el(hidden, pos));
                    let cells = dom.header_count.max(1);
                    dom.header_cells = (1..=cells)
                        .map(|n| self.layout.rect_of(&table_th(hidden, pos, n, tagged)))
                        .collect();
                    dom.header_row = self.layout.rect_of(&table_tr(hidden, pos, 1));
                    dom.data_row = self.layout.rect_of(&table_tr(hidden, pos, 2));
                }
                DomKind::Console(dom) => {
                    let mut dom = dom.borrow_mut();
                    dom.hidden = hidden;
                    dom.container = self.layout.rect_of(&console_container_el(hidden, pos));
                    dom.scroll_height = asrs_js::math::round(dom.container.height);
                }
                DomKind::Field2d(dom) => {
                    let mut dom = dom.borrow_mut();
                    let container = self.layout.rect_of(&field_2d_container_el(hidden, pos));
                    let scroll = self.layout.rect_of(&field_2d_scroll_el(hidden, pos));
                    dom.container = [container.x, container.y, container.width, container.height];
                    dom.scroll_container = [scroll.x, scroll.y, scroll.width, scroll.height];
                    dom.device_pixel_ratio = self.device_pixel_ratio;
                    dom.images = self.images.clone();
                }
                DomKind::Other => {}
            }
        }
    }

    fn renderer_top(&self) -> f64 {
        let tab_list = ElDesc {
            tag: "div",
            classes: &["tab-list"],
            hidden: false,
            pos: 1,
            child_pos: None,
            data_i: None,
            parent: None,
        };
        self.layout.rect_of(&tab_list).top()
            + self
                .tabs
                .selected_index()
                .and_then(|i| self.tabs.tab(i))
                .and_then(|t| t.controls_height().as_f64())
                .unwrap_or(0.0)
    }

    /// One animation frame: `periodic` plus the first frame's `show` message.
    fn frame(&mut self) {
        if self.frame_loop_dead {
            return;
        }
        self.host.renderer_top = self.renderer_top();
        self.tagged_tables.clear();
        self.sync_dom();
        for i in 0..self.tabs.len() {
            let index = JsValue::Number(i as f64);
            if let Some(r) = self
                .tabs
                .renderer_mut(&index)
                .ok()
                .and_then(|r| r.downcast_mut::<Recorder<Field2dRenderer>>())
            {
                r.inner.flush_image_loads();
            }
        }
        let out = self.tabs.periodic(&mut self.host);
        if let Some(e) = out.error {
            self.frame_loop_dead = true;
            self.exception("requestAnimationFrame".to_owned(), e.to_string());
        }
        for update in out.satellite_updates {
            let message = update.to_message();
            self.emit_main(message.name.name(), js_json::from_js(&message.data));
        }
        self.flush_messages();
        if self.show_pending {
            self.show_pending = false;
            self.emit_main("show", Value::Undefined);
        }
    }

    fn flush_messages(&mut self) {
        let messages = self.tabs.take_messages();
        let from_tabs = std::mem::take(&mut self.host.messages);
        for message in messages.into_iter().chain(from_tabs) {
            self.emit_main(message.name.name(), js_json::from_js(&message.data));
        }
    }

    fn advance(&mut self, ms: f64) {
        let end = self.now + ms;
        loop {
            let frame_at = self.last_frame + self.frame_period;
            if frame_at <= end {
                self.now = frame_at;
                self.clock.set(frame_at);
                self.last_frame = frame_at;
                self.frame();
            } else {
                self.now = end;
                self.clock.set(end);
                break;
            }
        }
    }

    /// The `drag-update` window listener: the capture's `{"drag"}` record is written first,
    /// then every controller's handler runs in registration order.
    fn dispatch_drag(&mut self, detail: &JsValue) {
        let get = |key: &str| ops::get(detail, key).unwrap_or(JsValue::Undefined);
        self.emitted.push(Emit {
            values: vec![
                ("drag", js_json::from_js(&get("data"))),
                ("end", js_json::from_js(&get("end"))),
                ("x", js_json::from_js(&get("x"))),
                ("y", js_json::from_js(&get("y"))),
            ],
        });
        for i in 0..self.tabs.len() {
            let index = JsValue::Number(i as f64);
            let kind = self.tabs.tab(i).and_then(|t| t.tab_type());
            let result = match kind {
                Some(TabType::Table) => self
                    .tabs
                    .controller_mut(&index)
                    .ok()
                    .and_then(|c| c.downcast_mut::<TableController>())
                    .map(|c| c.handle_drag(&mut self.host, detail)),
                Some(TabType::Console) => self
                    .tabs
                    .controller_mut(&index)
                    .ok()
                    .and_then(|c| c.downcast_mut::<ConsoleController>())
                    .map(|c| c.handle_drag(&mut self.host, detail)),
                _ => None,
            };
            if let Some(Err(e)) = result {
                self.exception(
                    "event drag-update".to_owned(),
                    format!("{}: {}", e.name, e.message),
                );
                return;
            }
        }
    }

    /// A mouse event on the selected tab's renderer root or table body.
    fn root_event(&mut self, etype: &str, x: f64, y: f64) {
        let Some(i) = self.tabs.selected_index() else {
            return;
        };
        let index = JsValue::Number(i as f64);
        let kind = self.tabs.tab(i).and_then(|t| t.tab_type());
        match (kind, etype) {
            (Some(TabType::Table), "mousedown") => {
                if let Some(c) = self
                    .tabs
                    .controller_mut(&index)
                    .ok()
                    .and_then(|c| c.downcast_mut::<TableController>())
                {
                    c.root_mouse_down(x, y);
                }
            }
            (Some(TabType::Table), "mouseup") => {
                if let Some(c) = self
                    .tabs
                    .controller_mut(&index)
                    .ok()
                    .and_then(|c| c.downcast_mut::<TableController>())
                {
                    c.root_mouse_up();
                }
            }
            (Some(TabType::Table), "mousemove") => {
                if let Some(c) = self
                    .tabs
                    .controller_mut(&index)
                    .ok()
                    .and_then(|c| c.downcast_mut::<TableController>())
                    && let Some(drag) = c.root_mouse_move(x, y)
                {
                    self.drag_start_table(drag);
                }
            }
            _ => {}
        }
    }

    /// `startDrag({tableIndex})`. The `mousemove` that started it then bubbles to the hub's
    /// window listener, which sends the first `drag-update`.
    fn drag_start_table(&mut self, drag: asrs_tabs::table::ColumnDrag) {
        let mut data = JsObject::new();
        data.insert("tableIndex", JsValue::Number(drag.table_index as f64));
        self.active_drag = Some(JsValue::Object(data));
    }
}

fn load_json(path: &Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    json::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn run_log_step(log: &Rc<RefCell<Log>>, calls: &[Value]) -> Result<(), String> {
    for call in calls {
        let parts = call.as_array().ok_or_else(|| err("log call is an array"))?;
        let method = parts
            .first()
            .and_then(Value::as_str)
            .map(|s| s.to_string_lossy())
            .ok_or_else(|| err("log method is a string"))?;
        let args: Vec<JsValue> = parts[1..]
            .iter()
            .map(js_json::to_js)
            .collect::<Result<_, _>>()?;
        let key = |i: usize| -> Result<JsString, String> {
            match args.get(i) {
                Some(JsValue::String(s)) => Ok(s.clone()),
                _ => Err(err("log key is a string")),
            }
        };
        let ts = |i: usize| -> Result<f64, String> {
            args.get(i)
                .and_then(JsValue::as_f64)
                .ok_or_else(|| err("timestamp is a number"))
        };
        let mut log = log.borrow_mut();
        match method.as_str() {
            "putBoolean" => log.put_boolean(&key(0)?, ts(1)?, truthy(&js_json::from_js(&args[2]))),
            "putNumber" => log.put_number(&key(0)?, ts(1)?, args[2].as_f64().unwrap_or(f64::NAN)),
            "putString" => {
                let v = match &args[2] {
                    JsValue::String(s) => s.clone(),
                    _ => return Err(err("putString value is a string")),
                };
                log.put_string(&key(0)?, ts(1)?, v);
            }
            "putBooleanArray" => {
                let JsValue::Array(items) = &args[2] else {
                    return Err(err("putBooleanArray value is an array"));
                };
                let items = items.iter().map(|v| truthy(&js_json::from_js(v))).collect();
                log.put_boolean_array(&key(0)?, ts(1)?, items);
            }
            "putNumberArray" => {
                let JsValue::Array(items) = &args[2] else {
                    return Err(err("putNumberArray value is an array"));
                };
                let items = items
                    .iter()
                    .map(|v| v.as_f64().unwrap_or(f64::NAN))
                    .collect();
                log.put_number_array(&key(0)?, ts(1)?, items);
            }
            "putStringArray" => {
                let JsValue::Array(items) = &args[2] else {
                    return Err(err("putStringArray value is an array"));
                };
                let items = items
                    .iter()
                    .map(|v| match v {
                        JsValue::String(s) => s.clone(),
                        _ => JsString::new(),
                    })
                    .collect();
                log.put_string_array(&key(0)?, ts(1)?, items);
            }
            "putUnknownStruct" => {
                log.put_unknown_struct(&key(0)?, ts(1)?, &args[2], false)
                    .map_err(|e| format!("{e:?}"))?;
            }
            "putRaw" => {
                let v = match &args[2] {
                    JsValue::Bytes(b) => b.clone(),
                    _ => return Err(err("putRaw value is bytes")),
                };
                log.put_raw(&key(0)?, ts(1)?, v)
                    .map_err(|e| format!("{e:?}"))?;
            }
            "putJSON" => {
                let v = match &args[2] {
                    JsValue::String(s) => s.clone(),
                    _ => return Err(err("putJSON value is a string")),
                };
                log.put_json(&key(0)?, ts(1)?, v);
            }
            "putPose" => {
                let pose = &args[2];
                let t = ops::get(pose, "translation").map_err(|e| e.message)?;
                let x = ops::index(&t, 0)
                    .map_err(|e| e.message)?
                    .as_f64()
                    .unwrap_or(f64::NAN);
                let y = ops::index(&t, 1)
                    .map_err(|e| e.message)?
                    .as_f64()
                    .unwrap_or(f64::NAN);
                let r = ops::get(pose, "rotation")
                    .map_err(|e| e.message)?
                    .as_f64()
                    .unwrap_or(f64::NAN);
                log.put_pose(&key(0)?, ts(1)?, [x, y], r);
            }
            "createBlankField" => {
                let ty = match args[1].as_f64() {
                    Some(0.0) => LoggableType::Raw,
                    Some(1.0) => LoggableType::Boolean,
                    Some(2.0) => LoggableType::Number,
                    Some(3.0) => LoggableType::String,
                    Some(4.0) => LoggableType::BooleanArray,
                    Some(5.0) => LoggableType::NumberArray,
                    Some(6.0) => LoggableType::StringArray,
                    Some(7.0) => LoggableType::Empty,
                    _ => return Err(err("bad LoggableType")),
                };
                log.create_blank_field(&key(0)?, ty);
            }
            "setStructuredType" => {
                let ty = match &args[1] {
                    JsValue::Null | JsValue::Undefined => None,
                    JsValue::String(s) => Some(s.clone()),
                    _ => return Err(err("structured type is a string")),
                };
                log.set_structured_type(&key(0)?, ty);
            }
            "setWpilibType" => {
                let ty = match &args[1] {
                    JsValue::String(s) => s.clone(),
                    _ => return Err(err("wpilib type is a string")),
                };
                log.set_wpilib_type(&key(0)?, ty);
            }
            "setGeneratedParent" => log.set_generated_parent(&key(0)?),
            "deleteField" => log.delete_field(&key(0)?),
            "clearBeforeTime" => log.clear_before_time(ts(0)?, truthy(&js_json::from_js(&args[1]))),
            "updateRangeWithTimestamp" => log.update_range_with_timestamp(ts(0)?),
            other => return Err(err(format!("unsupported log method {other}"))),
        }
    }
    Ok(())
}

/// `window.eval` is restricted to the expressions the fixtures use.
fn run_eval(r: &mut Runner, expr: &str) -> Result<JsValue, String> {
    if expr.starts_with("/*table:tag*/") {
        let uuid = r
            .tabs
            .selected_index()
            .and_then(|i| r.tabs.tab(i))
            .and_then(|t| t.uuid().as_str().map(|s| s.to_string_lossy()))
            .ok_or_else(|| err("table:tag needs a selected tab"))?;
        r.tagged_tables.push(uuid);
        r.sync_dom();
        return Ok(JsValue::Undefined);
    }
    if let Some(inner) = expr
        .strip_prefix("[...")
        .and_then(|e| e.strip_suffix(']'))
        .filter(|e| e.starts_with("window.tabs.tabList["))
    {
        return run_eval(r, inner);
    }
    if let Some(container) = expr
        .strip_prefix("document.querySelector('div.renderer-content > :not([hidden]) div.")
        .and_then(|e| e.strip_suffix("').scrollTop"))
    {
        let uuid = r
            .tabs
            .selected_index()
            .and_then(|i| r.tabs.tab(i))
            .and_then(|t| t.uuid().as_str().map(|s| s.to_string_lossy()))
            .unwrap_or_default();
        let scroll_top = match (container, r.tab_dom(&uuid)) {
            ("data-table-container", DomKind::Table(dom)) => dom.borrow().scroll_top,
            ("console-table-container", DomKind::Console(dom)) => dom.borrow().scroll_top,
            _ => return Err(err(format!("unsupported eval {expr}"))),
        };
        return Ok(JsValue::Number(scroll_top));
    }
    if let Some(rest) = expr.strip_prefix("window.tabs.tabList[")
        && let Some(close) = rest.find(']')
    {
        let index = &rest[..close];
        let suffix = &rest[close + 1..];
        let index = if index == "window.tabs.selectedTab" {
            r.tabs.selected().clone()
        } else {
            JsValue::Number(index.parse::<f64>().map_err(|_| err("bad tab index"))?)
        };
        return eval_tab_suffix(r, suffix, &index);
    }
    match expr {
        "window.tabs.saveState()" => Ok(r.tabs.save_state(&r.host)),
        "[...window.tabs.getActiveFields()]" => {
            let fields = r.tabs.active_fields(&r.host, &r.host.log.borrow());
            Ok(JsValue::Array(
                fields.into_iter().map(JsValue::String).collect(),
            ))
        }
        "window.tabs.isValidUUID(window.tabs.tabList[window.tabs.selectedTab].controller.UUID)" => {
            let uuid = r
                .tabs
                .selected_index()
                .and_then(|i| r.tabs.tab(i))
                .map_or(JsValue::Undefined, |t| t.uuid().clone());
            Ok(JsValue::Bool(r.tabs.is_valid_uuid(&uuid)))
        }
        _ => eval_selection_expr(&mut r.host.selection, expr)
            .ok_or_else(|| err(format!("unsupported eval {expr}"))),
    }
}

fn tab_index(r: &Runner, index: &JsValue) -> Option<usize> {
    match index {
        JsValue::Number(n) if *n >= 0.0 && (*n as usize) < r.tabs.len() => Some(*n as usize),
        _ => None,
    }
}

fn eval_tab_suffix(r: &mut Runner, suffix: &str, index: &JsValue) -> Result<JsValue, String> {
    let parse_arg = |arg: &str| -> Result<JsValue, String> {
        let json = json::parse(arg).map_err(|e| err(e.to_string()))?;
        js_json::to_js(&Value::from_json_lenient(&json))
    };
    if let Some(arg) = suffix
        .strip_prefix(".controller.restoreState(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let state = parse_arg(arg)?;
        r.tabs
            .controller_mut(index)
            .map_err(|e| err(e.to_string()))?
            .restore_state(&mut r.host, &state)
            .map_err(|e| e.message)?;
        return Ok(JsValue::Undefined);
    }
    if suffix.starts_with(".renderer.") {
        let i = tab_index(r, index);
        let is_stub = i
            .and_then(|i| r.tabs.tab(i))
            .and_then(|t| t.tab_type())
            .map(|k| !matches!(k, TabType::Table | TabType::Console | TabType::Metadata))
            .unwrap_or(false);
        if is_stub {
            return Ok(JsValue::Undefined);
        }
    }
    if let Some(arg) = suffix
        .strip_prefix(".renderer.restoreState(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let state = parse_arg(arg)?;
        r.tabs
            .renderer_mut(index)
            .map_err(|e| err(e.to_string()))?
            .restore_state(&mut r.host, &state)
            .map_err(|e| e.message)?;
        return Ok(JsValue::Undefined);
    }
    if suffix.starts_with(".controller.") {
        let i = tab_index(r, index);
        let is_stub = i
            .and_then(|i| r.tabs.tab(i))
            .and_then(|t| t.tab_type())
            .map(|k| !matches!(k, TabType::Table | TabType::Console | TabType::Metadata))
            .unwrap_or(false);
        if is_stub {
            return Ok(JsValue::Undefined);
        }
    }
    if suffix == ".controller.getActiveFields()" {
        let fields = r
            .tabs
            .controller_mut(index)
            .map_err(|e| err(e.to_string()))?
            .active_fields(&r.host);
        return Ok(JsValue::Array(
            fields.into_iter().map(JsValue::String).collect(),
        ));
    }
    Err(err(format!("unsupported tab eval suffix {suffix}")))
}

fn eval_selection_expr(sel: &mut SelectionImpl, expr: &str) -> Option<JsValue> {
    let expr = expr.strip_prefix("window.selection.")?;
    if let Some(arg) = expr
        .strip_prefix("setSelectedTime(")
        .and_then(|s| s.strip_suffix(')'))
    {
        sel.set_selected_time(arg.parse::<f64>().ok()?);
        return Some(JsValue::Undefined);
    }
    if let Some(arg) = expr
        .strip_prefix("setHoveredTime(")
        .and_then(|s| s.strip_suffix(')'))
    {
        sel.set_hovered_time(if arg == "null" || arg == "undefined" {
            None
        } else {
            arg.parse::<f64>().ok()
        });
        return Some(JsValue::Undefined);
    }
    match expr {
        "play()" => sel.play(),
        "pause()" => sel.pause(),
        "goIdle()" => sel.go_idle(),
        "togglePlayback()" => sel.toggle_playback(),
        "lock()" => sel.lock(),
        "unlock()" => sel.unlock(),
        "toggleLock()" => sel.toggle_lock(),
        _ => return None,
    }
    Some(JsValue::Undefined)
}

fn json_to_layout(fixture: &Json) -> Result<Layout, String> {
    let mut rules = Vec::new();
    if let Some(list) = fixture.get("layout").and_then(Json::as_array) {
        for rule in list {
            let selector = rule
                .get("selector")
                .and_then(Json::as_str)
                .ok_or_else(|| err("layout rule needs a selector"))?
                .to_string_lossy();
            let rect = if let Some(rect) = rule.get("rect").and_then(Json::as_array) {
                let n = |i: usize| rect.get(i).and_then(Json::as_f64).unwrap_or(0.0);
                Some([n(0), n(1), n(2), n(3)])
            } else if rule.get("text").and_then(Json::as_f64).is_some() {
                return Err(err("text layout rules are not modeled"));
            } else {
                None
            };
            rules.push((selector, rect));
        }
    }
    rules.push((
        "div.renderer-content".to_owned(),
        Some([300.0, 92.0, 800.0, 358.0]),
    ));
    rules.push(("div.renderer-content *".to_owned(), None));
    Ok(Layout { rules })
}

fn skippable(record: &Record) -> bool {
    record.field("dom").is_some()
        || record.field("canvas").is_some()
        || record.field("sourceLists").is_some()
        || record.field("measure").is_some()
        || record.field("tuner").is_some()
        || record
            .field("main")
            .and_then(Json::as_str)
            .is_some_and(|m| m.to_string_lossy().ends_with("coordinate-system-warning"))
}

fn selection_step(
    sel: &mut SelectionImpl,
    parts: &[Json],
    live_time: &Rc<Cell<Option<f64>>>,
) -> Result<(), String> {
    let method = parts
        .first()
        .and_then(Json::as_str)
        .ok_or_else(|| err("selection method is a string"))?
        .to_string_lossy();
    let arg = |i: usize| -> Value {
        parts
            .get(1 + i)
            .map(Value::from_json_lenient)
            .unwrap_or(Value::Undefined)
    };
    let num = |i: usize| -> Result<f64, String> {
        arg(i).as_f64().ok_or_else(|| err("expected a number"))
    };
    match method.as_str() {
        "play" => sel.play(),
        "pause" => sel.pause(),
        "goIdle" => sel.go_idle(),
        "togglePlayback" => sel.toggle_playback(),
        "lock" => sel.lock(),
        "unlock" => sel.unlock(),
        "toggleLock" => sel.toggle_lock(),
        "stepCycle" => sel.step_cycle(truthy(&arg(0))),
        "setSelectedTime" => sel.set_selected_time(num(0)?),
        "setHoveredTime" => sel.set_hovered_time(arg(0).as_f64()),
        "setPlaybackSpeed" => sel.set_playback_speed(num(0)?),
        "setPlaybackLooping" => sel.set_playback_looping(truthy(&arg(0))),
        "setGrabZoomRange" => sel.set_grab_zoom_range(match arg(0) {
            Value::Array(a) if a.len() == 2 => {
                Some([a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)])
            }
            _ => None,
        }),
        "finishGrabZoom" => sel.finish_grab_zoom(),
        "setTimelineRange" => {
            let pair = match arg(0) {
                Value::Array(a) if a.len() == 2 => {
                    [a[0].as_f64().unwrap_or(0.0), a[1].as_f64().unwrap_or(0.0)]
                }
                _ => return Err(err("setTimelineRange needs a pair")),
            };
            sel.set_timeline_range(pair, truthy(&arg(1)));
        }
        "applyTimelineScroll" => {
            sel.apply_timeline_scroll(num(0)?, num(1)?, num(2)?);
        }
        "setLiveConnected" => {
            let live_time = live_time.clone();
            sel.set_live_connected(Rc::new(move || live_time.get().unwrap_or(f64::NAN)));
        }
        "setLiveDisconnected" => sel.set_live_disconnected(),
        other => return Err(format!("unknown selection method {other}")),
    }
    Ok(())
}

fn replay(spec_path: &Path) -> Result<Vec<Emit>, String> {
    let spec = load_json(spec_path)?;
    let fixture = spec.get("fixture").ok_or_else(|| err("no fixture"))?;
    let start = fixture
        .get("startTime")
        .and_then(Json::as_f64)
        .unwrap_or(1767225600000.0);
    let window = fixture.get("window");
    let wnum = |key: &str, default: f64| {
        window
            .and_then(|w| w.get(key))
            .and_then(Json::as_f64)
            .unwrap_or(default)
    };
    let frame_period = wnum("framePeriod", 16.0);
    let inner_height = wnum("height", 650.0);
    let seed = fixture.get("seed").and_then(Json::as_f64).unwrap_or(1.0) as u32;
    let layout = json_to_layout(fixture)?;

    let clock_cell = Rc::new(Cell::new(start));
    let live_time: Rc<Cell<Option<f64>>> = Rc::new(Cell::new(None));
    let log = Rc::new(RefCell::new(Log::new(true)));
    let selection = SelectionImpl::new(log.clone(), Rc::new(ScriptedClock(clock_cell.clone())));
    let theme_dark = window
        .and_then(|w| w.get("theme"))
        .and_then(Json::as_str)
        .is_some_and(|t| t.to_string_lossy() == "dark");
    let device_pixel_ratio = wnum("devicePixelRatio", 1.0);
    let mut images = HashMap::new();
    if let Some(Json::Object(map)) = fixture.get("images") {
        for (src, size) in map {
            if let Some(pair) = size.as_array()
                && pair.len() == 2
            {
                images.insert(
                    JsString::from(src.to_string_lossy()),
                    [
                        pair[0].as_f64().unwrap_or(0.0),
                        pair[1].as_f64().unwrap_or(0.0),
                    ],
                );
            }
        }
    }
    let mut host = Host {
        log: log.clone(),
        selection,
        preferences: Some(Preferences::default()),
        assets: fixture
            .get("assets")
            .map(|a| to_js(a).unwrap_or_default())
            .filter(|a| matches!(a, JsValue::Object(_)))
            .map(|a| AdvantageScopeAssets::from_js(&a)),
        assets_raw: fixture
            .get("assets")
            .map(|a| to_js(a).unwrap_or_default())
            .unwrap_or(JsValue::Null),
        type_memory: SourceListTypeMemory::default(),
        dark: theme_dark,
        rng: DomRng(seed),
        inner_height,
        renderer_top: 92.0,
        messages: Vec::new(),
    };
    if let Some(prefs) = fixture.get("preferences") {
        let prefs = js_json::to_js(&Value::from_json_lenient(prefs))?;
        host.preferences
            .as_mut()
            .unwrap()
            .merge(&prefs)
            .map_err(|e| err(format!("preferences: {e:?}")))?;
    }
    if let Some(boot) = fixture.get("boot") {
        host.preferences = None;
        for message in boot.as_array().ok_or_else(|| err("boot is a list"))? {
            let name = message
                .as_array()
                .and_then(|m| m.first())
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy())
                .ok_or_else(|| err("boot entries are [name, data]"))?;
            let data = message
                .as_array()
                .and_then(|m| m.get(1))
                .map(Value::from_json_lenient)
                .unwrap_or(Value::Undefined);
            match name.as_str() {
                "set-preferences" => {
                    let mut p = Preferences::default();
                    p.merge(&js_json::to_js(&data)?)
                        .map_err(|e| err(format!("boot set-preferences: {e:?}")))?;
                    host.preferences = Some(p);
                }
                "set-assets" => {
                    let data = js_json::to_js(&data).unwrap_or_default();
                    let same = asrs_js::json::stringify(&host.assets_raw).ok().flatten()
                        == asrs_js::json::stringify(&data).ok().flatten();
                    if !same {
                        host.assets_raw = data.clone();
                        host.assets = if matches!(data, JsValue::Object(_)) {
                            Some(AdvantageScopeAssets::from_js(&data))
                        } else {
                            None
                        };
                    }
                }
                "set-version"
                | "set-fullscreen"
                | "set-battery"
                | "show-update-button"
                | "show-feedback-button"
                | "show-when-ready"
                | "set-active-satellites" => {}
                other => return Err(err(format!("boot message {other} is not modeled"))),
            }
        }
    }

    let ignored: Vec<String> = match fixture.get("ignoreMain").and_then(Json::as_array) {
        Some(names) => names
            .iter()
            .filter_map(Json::as_str)
            .map(|s| s.to_string_lossy())
            .collect(),
        None => ["save-state", "save-type-memory", "update-touch-bar-slider"]
            .map(str::to_owned)
            .into(),
    };

    let doms: Doms = Rc::new(RefCell::new(Vec::new()));
    let commands: CommandSlots = Rc::new(RefCell::new(HashMap::new()));
    let factory = Factory {
        doms: doms.clone(),
        commands: commands.clone(),
    };
    host.rng.next_f64();
    let tabs = Tabs::new(Box::new(factory), &mut host);
    let mut runner = Runner {
        layout,
        host,
        tabs,
        doms,
        commands,
        emitted: Vec::new(),
        clock: clock_cell.clone(),
        now: start,
        start,
        last_frame: start,
        frame_period,
        show_pending: true,
        frame_loop_dead: false,
        active_drag: None,
        tagged_tables: Vec::new(),
        sidebar_width: 300.0,
        sidebar_expanded: Vec::new(),
        ignored,
        device_pixel_ratio,
        images,
    };

    // `fixture.state` boots through `restore-state`.
    if let Some(state) = fixture.get("state") {
        if let Some(tabs_state) = state.get("tabs") {
            let tabs_state = js_json::to_js(&Value::from_json_lenient(tabs_state))?;
            runner
                .tabs
                .restore_state(&mut runner.host, &tabs_state)
                .map_err(|e| err(e.to_string()))?;
        }
        if let Some(sidebar) = state.get("sidebar") {
            if let Some(w) = sidebar.get("width").and_then(Json::as_f64) {
                runner.sidebar_width = w;
            }
            if let Some(expanded) = sidebar.get("expanded").and_then(Json::as_array) {
                runner.sidebar_expanded = expanded
                    .iter()
                    .filter_map(|e| e.as_str())
                    .map(|s| js(&s.to_string_lossy()))
                    .collect();
            }
        }
    }

    let steps = fixture
        .get("steps")
        .and_then(Json::as_array)
        .map(|a| a.to_vec())
        .unwrap_or_default();
    for (i, step) in steps.iter().enumerate() {
        let where_ = || format!("step {i}");
        if let Some(log_step) = step.get("log") {
            let calls: Vec<Value> = match log_step.as_str() {
                Some(path) => {
                    let path = spec_dir_join(spec_path, &path.to_string_lossy());
                    let ops = load_json(&path)?;
                    ops.as_array()
                        .ok_or_else(|| err(format!("{}: not an array", path.display())))?
                        .iter()
                        .map(Value::from_json_lenient)
                        .collect()
                }
                None => log_step
                    .as_array()
                    .ok_or_else(|| err("log step is an array or a path"))?
                    .iter()
                    .map(Value::from_json_lenient)
                    .collect(),
            };
            if let Err(e) = run_log_step(&log, &calls) {
                runner.exception("log".to_owned(), e);
            }
            if !matches!(step.get("newFields"), Some(Json::Bool(false)))
                && let Err(e) = runner.tabs.refresh(&mut runner.host)
            {
                runner.exception("log".to_owned(), e.to_string());
            }
        } else if let Some(sel_step) = step.get("selection") {
            let parts = sel_step
                .as_array()
                .ok_or_else(|| err("selection step is [method, ...args]"))?;
            if let Err(e) = selection_step(&mut runner.host.selection, parts, &live_time) {
                let method = parts
                    .first()
                    .and_then(Json::as_str)
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                runner.exception(format!("selection {method}"), e);
            }
        } else if let Some(live) = step.get("liveTime") {
            live_time.set(Value::from_json_lenient(live).as_f64());
        } else if let Some(main) = step.get("main") {
            let name = main
                .as_str()
                .ok_or_else(|| err("main step name is a string"))?
                .to_string_lossy();
            let data = step
                .get("data")
                .map(to_js)
                .transpose()?
                .unwrap_or(JsValue::Undefined);
            if let Err(e) = runner.main_message(&name, &data) {
                runner.exception(format!("main {name}"), e.to_string());
            }
        } else if let Some(event) = step.get("event") {
            let result = runner.dispatch_event(event);
            runner.flush_messages();
            if let Err(e) = result {
                let etype = event
                    .get("type")
                    .and_then(Json::as_str)
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                runner.exception(format!("event {etype}"), e);
            }
        } else if let Some(ms) = step.get("advance").and_then(Json::as_f64) {
            runner.advance(ms);
        } else if let Some(n) = step.get("frames").and_then(Json::as_f64) {
            for _ in 0..n as usize {
                runner.advance(runner.frame_period);
            }
        } else if let Some(capture) = step.get("capture") {
            let label = step
                .get("label")
                .and_then(Json::as_str)
                .map(|s| s.to_string_lossy());
            runner.capture(capture, label)?;
        } else if let Some(eval) = step.get("eval").and_then(Json::as_str) {
            let expr = eval.to_string_lossy();
            if let Err(e) = run_eval(&mut runner, &expr) {
                runner.exception("eval".to_owned(), e);
            }
        } else {
            return Err(format!("{}: unknown step", where_()));
        }
    }
    Ok(runner.emitted)
}

fn spec_dir_join(spec: &Path, rel: &str) -> PathBuf {
    spec.parent().unwrap().join(rel)
}

impl Runner {
    fn main_message(
        &mut self,
        name: &str,
        data: &JsValue,
    ) -> Result<(), asrs_hub::tabs::TabsError> {
        use asrs_hub::tabs::TabsError;
        match name {
            "new-tab" => self.tabs.add_tab(&mut self.host, data)?,
            "move-tab" => self.tabs.move_selection(&self.host, data)?,
            "shift-tab" => self.tabs.shift_selected(&self.host, data)?,
            "close-tab" => {
                let index = self.tabs.selected().clone();
                self.tabs.close(&self.host, &index, data)?;
            }
            "rename-tab" => {
                let index = data.get("index").cloned().unwrap_or_default();
                let name = data.get("name").cloned().unwrap_or_default();
                self.tabs.rename_tab(&index, name)?;
            }
            "toggle-controls" => self.tabs.toggle_controls_visible(&self.host)?,
            "new-satellite" => self.tabs.new_satellite()?,
            "set-active-satellites" => {
                let JsValue::Array(uuids) = data else {
                    return Err(TabsError::Thrown(JsError::type_error(
                        "set-active-satellites needs an array",
                    )));
                };
                self.tabs.set_active_satellites(uuids.clone());
            }
            "restore-state" => {
                let tabs = data.get("tabs").cloned().unwrap_or_default();
                self.tabs.restore_state(&mut self.host, &tabs)?;
            }
            "add-table-range" => {
                let renderer_uuid = match data.get("rendererUUID") {
                    Some(JsValue::String(s)) => s.clone(),
                    _ => {
                        return Err(TabsError::Thrown(JsError::type_error(
                            "add-table-range needs a rendererUUID",
                        )));
                    }
                };
                let range = match data.get("range") {
                    Some(JsValue::Array(r)) => match (r.first(), r.get(1)) {
                        (Some(JsValue::Number(a)), Some(JsValue::Number(b))) => Some([*a, *b]),
                        _ => None,
                    },
                    _ => None,
                };
                let controller = data.get("controllerUUID").cloned().unwrap_or_default();
                self.tabs
                    .add_table_range(&mut self.host, &controller, &renderer_uuid, range)?;
            }
            "set-preferences" => {
                let mut p = Preferences::default();
                p.merge(data).map_err(|e| {
                    TabsError::Thrown(JsError::type_error(format!("{}: {}", e.name, e.message)))
                })?;
                self.host.preferences = Some(p);
            }
            "call-selection-setter" => {
                let method = data
                    .get("method")
                    .and_then(JsValue::as_str)
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default();
                let args = match data.get("args") {
                    Some(JsValue::Array(a)) => a.clone(),
                    _ => Vec::new(),
                };
                let sel = &mut self.host.selection;
                let num = |i: usize| args.get(i).and_then(JsValue::as_f64).unwrap_or(f64::NAN);
                match method.as_str() {
                    "setSelectedTime" => sel.set_selected_time(num(0)),
                    "setHoveredTime" => sel.set_hovered_time(args[0].as_f64()),
                    "setPlaybackSpeed" => sel.set_playback_speed(num(0)),
                    "setPlaybackLooping" => {
                        sel.set_playback_looping(truthy(&js_json::from_js(&args[0])))
                    }
                    "lock" => sel.lock(),
                    "unlock" => sel.unlock(),
                    "toggleLock" => sel.toggle_lock(),
                    "play" => sel.play(),
                    "pause" => sel.pause(),
                    "goIdle" => sel.go_idle(),
                    "togglePlayback" => sel.toggle_playback(),
                    _ => {}
                }
            }
            "set-assets" => {
                let same = asrs_js::json::stringify(&self.host.assets_raw)
                    .ok()
                    .flatten()
                    == asrs_js::json::stringify(data).ok().flatten();
                if !same {
                    let failures = |v: &JsValue| {
                        truthy(&js_json::from_js(v))
                            && v.get("loadFailures")
                                .and_then(JsValue::as_array)
                                .is_some_and(|f| !f.is_empty())
                    };
                    let old_had_failed = failures(&self.host.assets_raw);
                    let new_had_failed = failures(data);
                    self.host.assets_raw = data.clone();
                    self.host.assets = if matches!(data, JsValue::Object(_)) {
                        Some(AdvantageScopeAssets::from_js(data))
                    } else {
                        None
                    };
                    self.tabs.new_assets(&mut self.host)?;
                    if new_had_failed && !old_had_failed {
                        let mut data = JsObject::new();
                        data.insert("title", "Failed to load assets".to_js());
                        data.insert(
                            "content",
                            "Some asset files failed to load. Check the developer console for details.".to_js(),
                        );
                        self.emit_main("error", js_json::from_js(&JsValue::Object(data)));
                    }
                }
            }
            "set-2d-camera" => {
                if let Some(r) = self
                    .tabs
                    .selected_renderer_as::<Recorder<Field2dRenderer>>(TabType::Field2d)?
                {
                    r.inner
                        .set_2d_camera(&mut self.host, data.as_f64().unwrap_or(f64::NAN));
                }
            }
            "set-version"
            | "set-fullscreen"
            | "set-battery"
            | "show-update-button"
            | "show-feedback-button"
            | "show-when-ready"
            | "toggle-sidebar"
            | "set-playback-options" => {}
            other => {
                return Err(TabsError::Thrown(JsError::type_error(format!(
                    "unhandled main message {other}"
                ))));
            }
        }
        for message in self.tabs.take_messages() {
            self.emit_main(message.name.name(), js_json::from_js(&message.data));
        }
        Ok(())
    }

    fn dispatch_event(&mut self, event: &Json) -> Result<(), String> {
        let etype = event
            .get("type")
            .and_then(Json::as_str)
            .ok_or_else(|| err("event needs a type"))?
            .to_string_lossy();
        let target = event
            .get("target")
            .and_then(Json::as_str)
            .ok_or_else(|| err("event needs a target"))?
            .to_string_lossy();
        let index = event.get("index").and_then(Json::as_f64).unwrap_or(0.0) as usize;
        let init = event.get("init").unwrap_or(&Json::Null);
        let iv = |name: &str| init.get(name).and_then(Json::as_f64).unwrap_or(0.0);
        let (x, y) = (iv("clientX"), iv("clientY"));
        let props = event.get("props");
        self.sync_dom();

        match (target.as_str(), etype.as_str()) {
            ("window", "drag-update") => {
                let detail = init
                    .get("detail")
                    .ok_or_else(|| err("drag-update needs detail"))?;
                let detail = js_json::to_js(&Value::from_json_lenient(detail))?;
                self.dispatch_drag(&detail);
            }
            ("window", _) => self.window_mouse(&etype, x, y),
            ("div.renderer-content > :not([hidden])", _) => {
                self.root_event(&etype, x, y);
                self.window_mouse(&etype, x, y);
            }
            (t, "mousemove" | "mouseleave") if t.ends_with("tbody") => {
                self.body_event(&etype, y);
                self.window_mouse(&etype, x, y);
            }
            (t, "input") if t.contains("data-table-jump-input") => self.set_jump_value(props),
            (t, "keydown") if t.contains("data-table-jump-input") => {
                self.set_jump_value(props);
                if init
                    .get("code")
                    .and_then(Json::as_str)
                    .map(|c| c.to_string_lossy())
                    == Some("Enter".to_owned())
                {
                    self.jump();
                }
            }
            (t, "click") if t.contains("data-table-jump-button") => self.jump(),
            // Match 0 is the hidden header template's button; match i + 1 closes field i.
            (t, "click") if t.contains("data-table-key-delete") => {
                if let Some(field) = index.checked_sub(1) {
                    self.close_field(Some(field as f64));
                }
            }
            (t, "click") if t.contains("console-table-key-delete") => self.close_field(None),
            (t, "click") if t.contains("highlight-button") => self.console_highlight_click(),
            // Match 0 is the header row; match i + 1 is data row i.
            (t, "click" | "contextmenu") if t.ends_with(" tr") => {
                if let Some(row) = index.checked_sub(1) {
                    self.row_event(&etype, row);
                }
            }
            (t, "scroll") => self.scroll_event(t, props),
            // DOM-only (row filtering and the console jump scroll belong to the view layer).
            (t, "input") if t.contains("input.filter") => {}
            (t, "keydown" | "click") if t.contains("console-table-jump") => {}
            _ => return Err(format!("event {etype} on {target} is not modeled")),
        }
        Ok(())
    }

    /// The hub's window listeners that drive a drag session (`dragMove`, `dragEnd`).
    fn window_mouse(&mut self, etype: &str, x: f64, y: f64) {
        let end = match etype {
            "mousemove" => false,
            "mouseup" | "mouseleave" | "touchend" => true,
            _ => return,
        };
        let data = if end {
            self.active_drag.take()
        } else {
            self.active_drag.clone()
        };
        if let Some(data) = data {
            let mut detail = JsObject::new();
            detail.insert("data", data);
            detail.insert("end", JsValue::Bool(end));
            detail.insert("x", JsValue::Number(x));
            detail.insert("y", JsValue::Number(y));
            self.dispatch_drag(&JsValue::Object(detail));
        }
    }

    fn selected_table_dom(&self) -> Option<Rc<RefCell<TableDom>>> {
        let uuid = self
            .tabs
            .tab(self.tabs.selected_index()?)?
            .uuid()
            .as_str()?
            .to_string_lossy();
        match self.tab_dom(&uuid) {
            DomKind::Table(dom) => Some(dom),
            _ => None,
        }
    }

    fn selected_table_renderer(&mut self) -> Option<&mut TableRenderer> {
        let index = JsValue::Number(self.tabs.selected_index()? as f64);
        self.tabs
            .renderer_mut(&index)
            .ok()?
            .downcast_mut::<Recorder<TableRenderer>>()
            .map(|r| &mut r.inner)
    }

    fn set_jump_value(&mut self, props: Option<&Json>) {
        let value = props
            .and_then(|p| p.get("value"))
            .and_then(|v| js_json::to_js(&Value::from_json_lenient(v)).ok());
        if let (Some(JsValue::String(value)), Some(dom)) = (value, self.selected_table_dom()) {
            dom.borrow_mut().jump_value = value;
        }
    }

    fn scroll_event(&mut self, target: &str, props: Option<&Json>) {
        let scroll_top = props
            .and_then(|p| p.get("scrollTop"))
            .and_then(Json::as_f64)
            .unwrap_or(f64::NAN);
        if let Some(i) = self.tabs.selected_index()
            && let Some(uuid) = self
                .tabs
                .tab(i)
                .and_then(|t| t.uuid().as_str())
                .map(|s| s.to_string_lossy())
        {
            match self.tab_dom(&uuid) {
                DomKind::Table(dom) if target.contains("data-table") => {
                    dom.borrow_mut().scroll_top = scroll_top;
                }
                DomKind::Console(dom) if target.contains("console-table") => {
                    dom.borrow_mut().scroll_top = scroll_top;
                }
                _ => {}
            }
        }
    }

    fn body_event(&mut self, etype: &str, y: f64) {
        if let Some(r) = self.selected_table_renderer() {
            match etype {
                "mousemove" => r.body_mouse_move(y),
                _ => r.body_mouse_leave(),
            }
        }
    }

    fn row_event(&mut self, etype: &str, row: usize) {
        let Some(i) = self.tabs.selected_index() else {
            return;
        };
        let index = JsValue::Number(i as f64);
        if let Some(r) = self
            .tabs
            .renderer_mut(&index)
            .ok()
            .and_then(|r| r.downcast_mut::<Recorder<TableRenderer>>())
        {
            match etype {
                "click" => r.inner.row_click(&mut self.host, row),
                _ => r.inner.row_context_menu(&mut self.host),
            }
        }
    }

    fn jump(&mut self) {
        let Some(i) = self.tabs.selected_index() else {
            return;
        };
        let index = JsValue::Number(i as f64);
        if let Some(r) = self
            .tabs
            .renderer_mut(&index)
            .ok()
            .and_then(|r| r.downcast_mut::<Recorder<TableRenderer>>())
        {
            r.inner.jump(&mut self.host);
        }
    }

    fn close_field(&mut self, index: Option<f64>) {
        let Some(i) = self.tabs.selected_index() else {
            return;
        };
        let ti = JsValue::Number(i as f64);
        match self.tabs.tab(i).and_then(|t| t.tab_type()) {
            Some(TabType::Table) => {
                if let Some(c) = self
                    .tabs
                    .controller_mut(&ti)
                    .ok()
                    .and_then(|c| c.downcast_mut::<TableController>())
                {
                    c.close_field(&index.map_or(JsValue::Undefined, JsValue::Number));
                }
            }
            Some(TabType::Console) => {
                if let Some(c) = self
                    .tabs
                    .controller_mut(&ti)
                    .ok()
                    .and_then(|c| c.downcast_mut::<ConsoleController>())
                {
                    c.close_field();
                }
            }
            _ => {}
        }
    }

    fn console_highlight_click(&mut self) {
        let Some(i) = self.tabs.selected_index() else {
            return;
        };
        let index = JsValue::Number(i as f64);
        if let Some(r) = self
            .tabs
            .renderer_mut(&index)
            .ok()
            .and_then(|r| r.downcast_mut::<Recorder<ConsoleRenderer>>())
        {
            r.inner.highlight_click();
        }
    }

    fn capture(&mut self, spec: &Json, label: Option<String>) -> Result<(), String> {
        self.emitted.push(Emit {
            values: vec![
                ("capture", cve_str(&label.unwrap_or_default())),
                ("time", Value::number(self.now - self.start)),
            ],
        });
        let kinds: Vec<&Json> = match spec.as_array() {
            Some(list) => list.iter().collect(),
            None => vec![spec],
        };
        for kind in kinds {
            let kind_name = match kind {
                Json::String(s) => s.to_string_lossy(),
                Json::Object(_) => kind
                    .get("kind")
                    .and_then(Json::as_str)
                    .map(|s| s.to_string_lossy())
                    .unwrap_or_default(),
                _ => continue,
            };
            match kind_name.as_str() {
                "commands" => {
                    for i in 0..self.tabs.len() {
                        let Some(tab) = self.tabs.tab(i) else {
                            continue;
                        };
                        let Some(uuid) = tab.uuid().as_str().map(|s| s.to_string_lossy()) else {
                            continue;
                        };
                        let slot = self
                            .commands
                            .borrow()
                            .get(&uuid)
                            .cloned()
                            .unwrap_or_default();
                        let Some(command) = slot.borrow_mut().take() else {
                            continue;
                        };
                        let meta = Value::object([
                            ("index", Value::number(i as f64)),
                            ("type", js_json::from_js(tab.type_value())),
                            ("title", js_json::from_js(tab.title())),
                            ("uuid", js_json::from_js(&JsValue::String(js(&uuid)))),
                        ]);
                        self.emitted.push(Emit {
                            values: vec![("command", meta), ("value", js_json::from_js(&command))],
                        });
                    }
                }
                "state" => {
                    let sidebar = Value::object([
                        ("width", Value::number(self.sidebar_width)),
                        (
                            "expanded",
                            Value::Array(
                                self.sidebar_expanded
                                    .iter()
                                    .map(|e| Value::String(Utf16String::from(e.to_string_lossy())))
                                    .collect(),
                            ),
                        ),
                    ]);
                    let state = Value::object([
                        ("sidebar", sidebar),
                        ("tabs", js_json::from_js(&self.tabs.save_state(&self.host))),
                    ]);
                    self.emitted.push(Emit {
                        values: vec![("state", state)],
                    });
                }
                "selection" => {
                    let value = selection_value(&mut self.host.selection);
                    self.emitted.push(Emit {
                        values: vec![("selection", value)],
                    });
                }
                "eval" => {
                    let expr = kind
                        .get("expr")
                        .and_then(Json::as_str)
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default();
                    match run_eval(self, &expr) {
                        Ok(value) => self.emitted.push(Emit {
                            values: vec![
                                ("eval", cve_str(&expr)),
                                ("value", js_json::from_js(&value)),
                            ],
                        }),
                        Err(e) => self.emitted.push(Emit {
                            values: vec![("eval", cve_str(&expr)), ("throw", cve_str(&e))],
                        }),
                    }
                }
                "dom" | "canvas" | "sourceLists" | "measure" | "tuner" => {}
                other => return Err(err(format!("capture kind {other} is not modeled"))),
            }
        }
        Ok(())
    }
}

/// `state` records carry every tab's saved state; stub-kind tabs keep the values upstream
/// recorded (their halves are other units' work) while Table, Console and Metadata are
/// compared through the real port.
fn patch_state(expected: &Value, actual: &mut Value) {
    let Some(expected_tabs) = expected
        .get("tabs")
        .and_then(|t| t.get("tabs"))
        .and_then(Value::as_array)
        .map(|a| a.to_vec())
    else {
        return;
    };
    let Value::Object(state_fields) = actual else {
        return;
    };
    let Some(tabs_value) = state_fields
        .iter_mut()
        .find(|(k, _)| k.eq_str("tabs"))
        .map(|(_, v)| v)
    else {
        return;
    };
    let Value::Object(tabs_fields) = tabs_value else {
        return;
    };
    let Some(Value::Array(actual_tabs)) = tabs_fields
        .iter_mut()
        .find(|(k, _)| k.eq_str("tabs"))
        .map(|(_, v)| v)
    else {
        return;
    };
    for (expected_tab, actual_tab) in expected_tabs.iter().zip(actual_tabs.iter_mut()) {
        let ty = expected_tab.get("type").and_then(Value::as_f64);
        if matches!(ty, Some(4.0 | 5.0 | 12.0)) {
            continue;
        }
        let Value::Object(fields) = actual_tab else {
            continue;
        };
        for key in ["controller", "renderer"] {
            if let Some(v) = expected_tab.get(key).cloned() {
                match fields.iter_mut().find(|(k, _)| k.eq_str(key)) {
                    Some((_, slot)) => *slot = v,
                    None => fields.push((Utf16String::from(key), v)),
                }
            }
        }
    }
}

fn compare(spec: &Path, vector_id: &str) -> Result<usize, String> {
    let emitted = replay(spec)?;
    let records = vectors::load(vector_id)?;
    let mut expected = records.iter().peekable();
    let mut skipped = 0;
    for emit in &emitted {
        while let Some(record) = expected.peek() {
            if skippable(record) {
                expected.next();
                skipped += 1;
                continue;
            }
            break;
        }
        let record = expected
            .next()
            .ok_or_else(|| err("port emitted more records than the vector has"))?;
        let stub_command = emit
            .values
            .iter()
            .find(|(k, _)| *k == "command")
            .and_then(|(_, v)| v.get("type"))
            .and_then(Value::as_f64)
            .is_some_and(|t| !matches!(t, 4.0 | 5.0 | 12.0));
        for (key, value) in &emit.values {
            let expected_value = record
                .value(key)
                .map_err(|e| format!("line {} missing {key}: {e}", record.line))?;
            if *key == "value" && stub_command {
                continue;
            }
            let mut actual = value.clone();
            if *key == "state" {
                patch_state(&expected_value, &mut actual);
            }
            if let Some(report) = asrs_parity::diff::report(&expected_value, &actual) {
                return Err(format!("line {} ({key}): {report}", record.line));
            }
        }
    }
    for record in expected {
        if !skippable(record) {
            return Err(format!(
                "line {}: the port emitted nothing for this record",
                record.line
            ));
        }
        skipped += 1;
    }
    Ok(skipped)
}

fn spec_files() -> Vec<(PathBuf, String)> {
    let dir = vectors::workspace_root()
        .join("parity/specs/dom")
        .join("tabs");
    let mut out: Vec<(PathBuf, String)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.ends_with(".entry.json"))
        })
        .map(|p| {
            let name = p
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .trim_end_matches(".entry.json")
                .to_owned();
            (p, format!("dom/tabs/{name}"))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn dom_tabs_fixtures() {
    let specs = spec_files();
    assert!(!specs.is_empty(), "no dom/tabs specs");
    let mut failures = Vec::new();
    let mut skipped = 0;
    for (spec, id) in &specs {
        match compare(spec, id) {
            Ok(n) => skipped += n,
            Err(e) => failures.push(format!("{id}: {e}")),
        }
    }
    eprintln!(
        "dom/tabs: {} fixtures, {skipped} records skipped for other layers",
        specs.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
