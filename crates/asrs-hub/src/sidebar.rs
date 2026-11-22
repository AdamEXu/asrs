//! `src/hub/Sidebar.ts`: the headless sidebar model. Rows live in an arena in creation order;
//! `roots`/`children` give display order. Every layout read and scroll write upstream makes goes
//! through [`SidebarGeometry`] at the same point and in the same order, so a view answers them
//! from its own layout and a replay answers them from Blink's records.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use asrs_js::collation::locale_compare_numeric;
use asrs_js::json;
use asrs_js::math::{abs, floor, max2, min2, rem, round};
use asrs_js::number;
use asrs_js::sort::{array_sort, code_unit_cmp};
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::log_util::{TYPE_KEY, get_value_at};
use asrs_log::log::{Log, LogFieldTree, LoggableType, search_fields};
use asrs_log::util::arrays_equal;
use asrs_model::hub_state::SidebarState;
use asrs_model::selection::{Selection, SelectionMode};
use indexmap::IndexSet;

use crate::custom_schemas;
use crate::drag::{Drag, DragItem, DragPayload};

pub const DEFAULT_SIDEBAR_WIDTH: f64 = 300.0;
pub const INDENT_SIZE_PX: f64 = 20.0;
pub const FIELD_DRAG_THRESHOLD_PX: f64 = 3.0;
pub const VALUE_WIDTH_MARGIN_PX: f64 = 12.0;
pub const MERGED_KEY: &str = "Log";
pub const KNOWN_KEYS: &[&str] = &[
    "DriverStation",
    "NetworkTables",
    "RealOutputs",
    "ReplayOutputs",
    "SystemStats",
    "PowerDistribution",
    "Tuning",
    "NetworkInputs",
    "DashboardInputs",
    "RadioStatus",
    "Timestamp",
    "AdvantageKit",
    "DS",
    "NT",
    "NTConnection",
    "messages",
    "systemTime",
    "DSLog",
    "DSEvents",
];
pub const HIDDEN_KEYS: &[&str] = &[".schema", "Metadata", "RealMetadata", "ReplayMetadata"];
pub const OPEN_LOG_TITLE: &str = "Open a log or connect to view data";

const TYPE_WARNING_TEXT: &str = "Values with conflicting types have been written to this field. \
Only values compatible with the initial type can be viewed and exported.";
const TUNING_INTERVAL_MS: f64 = 250.0;
const HIGHLIGHT_TIMEOUT_MS: f64 = 3000.0;
const HANDLE_DOUBLE_CLICK_MS: f64 = 400.0;
const SCROLL_BLUR_THRESHOLD: f64 = 50.0;

fn js(text: &str) -> JsString {
    JsString::from(text)
}

fn starts_with(key: &JsString, prefix: &str) -> bool {
    key.starts_with(&js(prefix), None)
}

fn ends_with(key: &JsString, suffix: &str) -> bool {
    key.ends_with(&js(suffix), None)
}

fn is_hidden_key(key: &JsString) -> bool {
    HIDDEN_KEYS.iter().any(|k| key == &js(k))
}

/// `DOMRect` reduced to what the model reads.
#[derive(Clone, Copy, Debug, Default)]
pub struct RowRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl RowRect {
    pub fn top(&self) -> f64 {
        self.y
    }

    pub fn on_screen(&self, inner_height: f64) -> bool {
        self.height > 0.0
            && self.width > 0.0
            && self.top() >= -self.height
            && self.top() <= inner_height
    }
}

/// One row as [`SidebarGeometry`] sees it.
#[derive(Clone, Copy)]
pub struct RowRef<'a> {
    nodes: &'a [Node],
    id: usize,
    display_index: Option<usize>,
}

impl RowRef<'_> {
    /// The row's index in [`Sidebar::rows`]; `None` while a collapsed ancestor hides it or it
    /// is out of the DOM.
    pub fn display_index(&self) -> Option<usize> {
        self.display_index
    }

    /// The oracle's `elementPath` of the row's `.field-item-container`: from `#fieldList`
    /// (`span:3` of `.side-bar`) while attached, from the removed root once a rebuild took it
    /// out of the DOM.
    pub fn container_path(&self) -> String {
        let nodes = self.nodes;
        let node = &nodes[self.id];
        if node.parent.is_none() && node.detached {
            return "div.field-item-container:0".to_owned();
        }
        let mut segments = vec![format!(
            "div.field-item-container:{}",
            element_index(nodes, self.id)
        )];
        let mut current = self.id;
        loop {
            match nodes[current].parent {
                None => {
                    segments.push("span:3".to_owned());
                    break;
                }
                Some(parent) if nodes[parent].parent.is_none() && nodes[parent].detached => {
                    segments.push("span:0".to_owned());
                    break;
                }
                Some(parent) => {
                    segments.push(format!("span:{}", element_index(nodes, parent) + 1));
                    current = parent;
                }
            }
        }
        segments.reverse();
        segments.join(">")
    }
}

/// The index of `id`'s container among its parent element's children: each earlier sibling
/// row is its container followed by its child span, if it got one.
fn element_index(nodes: &[Node], id: usize) -> usize {
    let siblings: Vec<usize> = match nodes[id].parent {
        Some(parent) => nodes[parent].children.clone(),
        None => (0..nodes.len())
            .filter(|&i| {
                nodes[i].parent.is_none()
                    && nodes[i].generation == nodes[id].generation
                    && nodes[i].detached == nodes[id].detached
            })
            .collect(),
    };
    siblings
        .iter()
        .take_while(|&&sibling| sibling != id)
        .map(|&sibling| 1 + usize::from(nodes[sibling].child_span))
        .sum()
}

/// The sidebar's layout reads and scroll writes, in the order upstream makes them.
pub trait SidebarGeometry {
    /// `fieldElement.getBoundingClientRect()`.
    fn field_item_rect(&mut self, row: RowRef<'_>) -> RowRect;
    /// `valueElement.clientWidth` while it shows `value`; `hidden` is its `hidden` attribute.
    fn value_client_width(&mut self, row: RowRef<'_>, value: &RowValue, hidden: bool) -> f64;
    /// `label.getBoundingClientRect()`.
    fn label_rect(&mut self, row: RowRef<'_>) -> RowRect;
    /// `fieldElement.scrollIntoViewIfNeeded()`.
    fn scroll_into_view_if_needed(&mut self, row: RowRef<'_>);
    /// `SEARCH_INPUT.getBoundingClientRect()`.
    fn search_input_rect(&mut self) -> RowRect;
    /// `.search-results-item` number `index`: `scrollIntoView({ block: "nearest" })`.
    fn scroll_search_item_into_view(&mut self, index: usize);
    /// `SEARCH_RESULTS.scrollTop = 0`.
    fn reset_search_results_scroll(&mut self);
    /// `SIDEBAR.scrollTop`.
    fn scroll_top(&mut self) -> f64;
    /// `SIDEBAR.scrollTop = value`.
    fn set_scroll_top(&mut self, value: f64);
    /// `.title-bar-menu` button `index`: `getBoundingClientRect()`.
    fn menu_button_rect(&mut self, index: usize) -> RowRect;
    /// `SEARCH_INPUT.blur()`: whether it had focus, so that a `blur` event fired.
    fn blur_search_input(&mut self) -> bool;
}

/// A page without layout, as jsdom gives one with no layout rule for the sidebar: every rect
/// and size is 0 and scroll writes do nothing.
pub struct NoLayout;

impl SidebarGeometry for NoLayout {
    fn field_item_rect(&mut self, _row: RowRef<'_>) -> RowRect {
        RowRect::default()
    }

    fn value_client_width(&mut self, _row: RowRef<'_>, _value: &RowValue, _hidden: bool) -> f64 {
        0.0
    }

    fn label_rect(&mut self, _row: RowRef<'_>) -> RowRect {
        RowRect::default()
    }

    fn scroll_into_view_if_needed(&mut self, _row: RowRef<'_>) {}

    fn search_input_rect(&mut self) -> RowRect {
        RowRect::default()
    }

    fn scroll_search_item_into_view(&mut self, _index: usize) {}

    fn reset_search_results_scroll(&mut self) {}

    fn scroll_top(&mut self) -> f64 {
        0.0
    }

    fn set_scroll_top(&mut self, _value: f64) {}

    fn menu_button_rect(&mut self, _index: usize) -> RowRect {
        RowRect::default()
    }

    fn blur_search_input(&mut self) -> bool {
        false
    }
}

/// `window.tuner`.
pub trait Tuner {
    fn has_tunable_fields(&self) -> bool;
    fn is_tunable(&self, key: &JsString) -> bool;
    fn publish(&self, key: &JsString, value: &JsValue);
    fn unpublish(&self, key: &JsString);
}

/// The `window.*` services upstream `Sidebar` reads.
pub struct SidebarEnv<'a> {
    pub log: &'a RefCell<Log>,
    pub selection: &'a mut dyn Selection,
    pub tuner: Option<&'a dyn Tuner>,
    pub dark_theme: bool,
    pub inner_height: f64,
    pub loading_fields: &'a HashSet<JsString>,
    pub geometry: &'a mut dyn SidebarGeometry,
}

/// Whether the upstream listener or `refresh` threw part-way, leaving rows half built.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventOutcome {
    Ok,
    /// A merged-log root such as `Log-1` or `Log0.5` indexes past the filenames, and
    /// `htmlEncode(undefined)` throws.
    MergedFilenameMissing,
}

type Aborted = EventOutcome;
type Attempt<T> = Result<T, Aborted>;

#[derive(Clone, Debug)]
struct Timer {
    due: f64,
    seq: u64,
    kind: TimerKind,
}

#[derive(Clone, Copy, Debug)]
enum TimerKind {
    TuningPublish,
    ClearHighlight(u64),
}

#[derive(Clone, Debug, Default)]
struct BoolState {
    fill: Option<JsString>,
    line_hidden: Option<bool>,
    tunable: bool,
    first_update: bool,
    last_value: Option<bool>,
    last_hidden: Option<bool>,
}

#[derive(Clone, Debug, Default)]
struct NumberState {
    input_value: JsString,
    input_hidden: bool,
    span_hidden: bool,
    span_text: JsString,
    placeholder: Option<JsString>,
    focused: bool,
    last_has_value: bool,
    first_update: bool,
    last_value: Option<f64>,
    last_char_count: Option<usize>,
}

#[derive(Clone, Debug, Default)]
struct TextState {
    text: JsString,
    first_update: bool,
    last_value: Option<JsString>,
    last_char_count: Option<usize>,
    last_sidebar_width: f64,
}

#[derive(Clone, Debug, Default)]
enum ValueState {
    #[default]
    None,
    Bool(BoolState),
    Number(Box<NumberState>),
    Text(TextState),
}

impl ValueState {
    fn to_row(&self) -> RowValue {
        match self {
            ValueState::None => RowValue::None,
            ValueState::Bool(state) => RowValue::Bool {
                fill: state.fill.clone(),
                line_hidden: state.line_hidden,
                tunable: state.tunable,
            },
            ValueState::Number(state) => RowValue::Number {
                input_value: state.input_value.clone(),
                input_hidden: state.input_hidden,
                span_hidden: state.span_hidden,
                span_text: state.span_text.clone(),
                placeholder: state.placeholder.clone(),
            },
            ValueState::Text(state) => RowValue::Text {
                text: state.text.clone(),
            },
        }
    }
}

/// `searchExpandCallbacks`: `highlightForSearch` for full-key rows and
/// `expandForSearch` for parents, in row-creation order. Never cleared.
struct FieldEntry<'a> {
    title: JsString,
    full_title: JsString,
    field: &'a LogFieldTree,
}

#[derive(Clone, Copy)]
enum SearchCallback {
    Highlight(usize),
    Expand(usize),
}

#[derive(Clone, Debug)]
struct Node {
    id: u64,
    title: JsString,
    full_title: JsString,
    full_key: Option<JsString>,
    tree_children: JsObject<LogFieldTree>,
    /// Out of the DOM: replaced by a rebuild, or built later under such a row by a stale
    /// `searchExpandCallbacks` entry (upstream never clears that list).
    detached: bool,
    /// The `refresh` whose callback lists this row registered into. Rows built under a
    /// detached parent still join the current lists, so this is not `!detached`.
    generation: u64,
    /// Whether the full-key block finished, registering the metadata and loading callbacks.
    full_key_built: bool,
    has_children: bool,
    child_span: bool,
    expand_listeners: bool,
    children_generated: bool,
    indent: f64,
    generated: bool,
    known_label: bool,
    label_loading: bool,
    label_bold: Option<bool>,
    label_title: Option<JsString>,
    type_label: Option<JsString>,
    warning_hidden: bool,
    warning_last_hidden: bool,
    highlighted: bool,
    mouse_down: Option<[f64; 4]>,
    metadata_last: Option<JsString>,
    expanded: bool,
    children_materialized: bool,
    children: Vec<usize>,
    parent: Option<usize>,
    value_element_hidden: bool,
    value_width: f64,
    value: ValueState,
}

/// One row in display order, everything the view needs to render it.
#[derive(Clone, Debug)]
pub struct SidebarRow {
    /// The row's index in [`Sidebar::rows`], which the row handlers take.
    pub index: usize,
    pub title: JsString,
    pub full_title: JsString,
    pub full_key: Option<JsString>,
    /// Position among sibling `.field-item-container` elements, 1-indexed (`:nth-of-type`).
    pub position: usize,
    pub indent: f64,
    pub generated: bool,
    pub known_label: bool,
    pub expanded: bool,
    pub has_children: bool,
    /// Whether the children's container exists; false when building the row threw first.
    pub child_span: bool,
    pub label_loading: bool,
    /// The label's inline `font-weight`: unset until the row joins or leaves a select group.
    pub label_bold: Option<bool>,
    pub label_title: Option<JsString>,
    pub type_label: Option<JsString>,
    pub warning_hidden: bool,
    pub warning_title: JsString,
    pub highlighted: bool,
    pub value_hidden: bool,
    /// The container's `--value-width` in px.
    pub value_width: f64,
    pub value: RowValue,
}

#[derive(Clone, Debug)]
pub enum RowValue {
    None,
    Bool {
        fill: Option<JsString>,
        line_hidden: Option<bool>,
        tunable: bool,
    },
    Number {
        input_value: JsString,
        input_hidden: bool,
        span_hidden: bool,
        span_text: JsString,
        placeholder: Option<JsString>,
    },
    Text {
        text: JsString,
    },
}

pub struct Sidebar {
    get_filenames: Box<dyn Fn() -> Vec<JsString>>,
    sidebar_handle_active: bool,
    sidebar_width: f64,
    field_count: usize,
    is_tuning_mode: bool,
    last_field_keys: Vec<JsString>,
    expanded_fields: IndexSet<JsString>,
    active_fields: IndexSet<JsString>,
    select_group: Vec<JsString>,
    search_key: Option<JsString>,
    search_hovered_index: Option<usize>,
    search_results: Vec<JsString>,
    search_items_hovered: Vec<bool>,
    tuning_value_cache: JsObject<JsString>,
    nodes: Vec<Node>,
    roots: Vec<usize>,
    search_callbacks: Vec<SearchCallback>,
    next_node_id: u64,
    title_text: JsString,
    search_value: JsString,
    search_focused: bool,
    search_results_hidden: bool,
    search_results_top: Option<f64>,
    search_results_min_width: Option<f64>,
    tuning_button_hidden: bool,
    tuning_button_active: bool,
    shadow_shown: bool,
    last_handle_click: f64,
    menu_active: Vec<bool>,
    main_messages: Vec<(JsString, JsValue)>,
    timers: Vec<Timer>,
    next_timer_seq: u64,
    generation: u64,
}

impl Sidebar {
    pub fn new(now: f64, get_filenames: impl Fn() -> Vec<JsString> + 'static) -> Self {
        let mut sidebar = Sidebar {
            get_filenames: Box::new(get_filenames),
            sidebar_handle_active: false,
            sidebar_width: DEFAULT_SIDEBAR_WIDTH,
            field_count: 0,
            is_tuning_mode: false,
            last_field_keys: Vec::new(),
            expanded_fields: IndexSet::new(),
            active_fields: IndexSet::new(),
            select_group: Vec::new(),
            search_key: None,
            search_hovered_index: None,
            search_results: Vec::new(),
            search_items_hovered: Vec::new(),
            tuning_value_cache: JsObject::new(),
            nodes: Vec::new(),
            roots: Vec::new(),
            search_callbacks: Vec::new(),
            next_node_id: 0,
            title_text: js(OPEN_LOG_TITLE),
            search_value: JsString::new(),
            search_focused: false,
            search_results_hidden: true,
            search_results_top: None,
            search_results_min_width: None,
            tuning_button_hidden: true,
            tuning_button_active: false,
            shadow_shown: false,
            last_handle_click: 0.0,
            menu_active: Vec::new(),
            main_messages: Vec::new(),
            timers: Vec::new(),
            next_timer_seq: 0,
            generation: 0,
        };
        sidebar.schedule_interval(now, TUNING_INTERVAL_MS, TimerKind::TuningPublish);
        sidebar
    }

    fn schedule_interval(&mut self, now: f64, interval: f64, kind: TimerKind) {
        let seq = self.next_timer_seq;
        self.next_timer_seq += 1;
        self.timers.push(Timer {
            due: now + interval,
            seq,
            kind,
        });
    }

    fn schedule_timeout(&mut self, now: f64, delay: f64, kind: TimerKind) {
        self.schedule_interval(now, delay, kind);
    }

    /// Earliest pending timer `due`, if any.
    pub fn next_timer_due(&self) -> Option<f64> {
        self.timers.iter().map(|t| t.due).reduce(min2)
    }

    /// Runs every timer due at `now`, in `(due, seq)` order like env.ts.
    pub fn fire_timers(&mut self, now: f64, env: &mut SidebarEnv<'_>) {
        loop {
            let Some(pos) = self
                .timers
                .iter()
                .enumerate()
                .filter(|(_, t)| t.due <= now)
                .min_by(|(_, a), (_, b)| {
                    a.due
                        .partial_cmp(&b.due)
                        .unwrap_or(Ordering::Equal)
                        .then(a.seq.cmp(&b.seq))
                })
                .map(|(i, _)| i)
            else {
                return;
            };
            let mut timer = self.timers.remove(pos);
            match timer.kind {
                TimerKind::TuningPublish => {
                    self.tuning_publish_tick(env);
                    timer.due += TUNING_INTERVAL_MS;
                    timer.seq = self.next_timer_seq;
                    self.next_timer_seq += 1;
                    self.timers.push(timer);
                }
                TimerKind::ClearHighlight(id) => {
                    if let Some(node) = self.nodes.iter_mut().find(|n| n.id == id) {
                        node.highlighted = false;
                    }
                }
            }
        }
    }

    fn send_main(&mut self, name: &str, data: JsValue) {
        self.main_messages.push((js(name), data));
    }

    pub fn take_main_messages(&mut self) -> Vec<(JsString, JsValue)> {
        std::mem::take(&mut self.main_messages)
    }

    /// `saveState`.
    pub fn save_state(&self) -> SidebarState {
        SidebarState {
            width: self.sidebar_width,
            expanded: self.expanded_fields.iter().cloned().collect(),
        }
    }

    /// `restoreState`.
    pub fn restore_state(
        &mut self,
        env: &mut SidebarEnv<'_>,
        state: &SidebarState,
    ) -> EventOutcome {
        let expanded_set: IndexSet<JsString> = state.expanded.iter().cloned().collect();
        let expanded_equal = self.expanded_fields.len() == expanded_set.len()
            && self
                .expanded_fields
                .iter()
                .all(|k| expanded_set.contains(k));
        self.sidebar_width = state.width;
        self.expanded_fields = expanded_set;
        if !expanded_equal && let Err(outcome) = self.refresh(env, true) {
            return outcome;
        }
        EventOutcome::Ok
    }

    /// `toggleVisible`.
    pub fn toggle_visible(&mut self) {
        if self.sidebar_width == 0.0 {
            self.sidebar_width = DEFAULT_SIDEBAR_WIDTH;
        } else {
            self.sidebar_width *= -1.0;
        }
    }

    /// `--side-bar-width`, `Math.max(sidebarWidth, 0)`.
    pub fn applied_width(&self) -> f64 {
        max2(self.sidebar_width, 0.0)
    }

    /// `--show-side-bar`.
    pub fn sidebar_shown(&self) -> bool {
        self.applied_width() != 0.0
    }

    pub fn handle_mouse_down(&mut self) {
        self.sidebar_handle_active = true;
    }

    /// The handle's `window` `mouseup` listener, which runs before hub.ts's `dragEnd`.
    pub fn handle_mouse_up(&mut self) {
        self.sidebar_handle_active = false;
    }

    /// The handle's `window` `mousemove` listener, which runs before hub.ts's `dragMove`.
    pub fn handle_mouse_move(&mut self, client_x: f64) {
        if !self.sidebar_handle_active {
            return;
        }
        let mut width = client_x;
        if width > 500.0 {
            width = 500.0;
        }
        if (80.0..160.0).contains(&width) {
            width = 160.0;
        }
        if width < 80.0 {
            if self.sidebar_width > 0.0 {
                width = 0.0;
            } else {
                width = self.sidebar_width;
            }
        }
        self.sidebar_width = width;
    }

    /// The field labels' `window` `mousemove` listeners, which run after hub.ts's `dragMove`,
    /// so the move that starts a drag dispatches no `drag-update`. `offset` is the pressed
    /// label's `[offsetX, offsetY]` at mousedown.
    pub fn fields_mouse_move(&mut self, drag: &mut Drag, client_x: f64, client_y: f64) {
        for id in 0..self.nodes.len() {
            let Some(down) = self.nodes[id].mouse_down else {
                continue;
            };
            if abs(client_x - down[0]) >= FIELD_DRAG_THRESHOLD_PX
                || abs(client_y - down[1]) >= FIELD_DRAG_THRESHOLD_PX
            {
                self.drag_event(drag, id, down[0], down[1], down[2], down[3]);
                self.nodes[id].mouse_down = None;
            }
        }
    }

    /// `side-bar-handle` click: toggles on a double click within 400 ms.
    pub fn handle_click(&mut self, now: f64) {
        if now - self.last_handle_click < HANDLE_DOUBLE_CLICK_MS {
            self.toggle_visible();
            self.last_handle_click = 0.0;
        } else {
            self.last_handle_click = now;
        }
    }

    /// `side-bar` scroll: the shadow listener, then the search-blur listener, each reading
    /// `scrollTop`.
    pub fn sidebar_scroll(&mut self, geometry: &mut dyn SidebarGeometry) {
        self.shadow_shown = geometry.scroll_top() != 0.0;
        if geometry.scroll_top() > SCROLL_BLUR_THRESHOLD && self.search_focused {
            self.blur_search_input(geometry);
        }
    }

    /// `SEARCH_INPUT.blur()`, whose `blur` event only fires when the input had focus.
    fn blur_search_input(&mut self, geometry: &mut dyn SidebarGeometry) {
        if geometry.blur_search_input() {
            self.search_blur();
        }
    }

    pub fn menu_button_click(&mut self, geometry: &mut dyn SidebarGeometry, index: usize) {
        if self.menu_active.len() <= index {
            self.menu_active.resize(index + 1, false);
        }
        if self.menu_active[index] {
            self.menu_active[index] = false;
            let mut data = JsObject::new();
            data.insert("index", JsValue::Number(index as f64));
            self.send_main("close-app-menu", JsValue::Object(data));
        } else {
            self.menu_active[index] = true;
            let rect = geometry.menu_button_rect(index);
            let mut rect_obj = JsObject::new();
            rect_obj.insert("x", JsValue::Number(rect.x));
            rect_obj.insert("y", JsValue::Number(rect.y));
            rect_obj.insert("width", JsValue::Number(rect.width));
            rect_obj.insert("height", JsValue::Number(rect.height));
            let mut data = JsObject::new();
            data.insert("index", JsValue::Number(index as f64));
            data.insert("rect", JsValue::Object(rect_obj));
            self.send_main("open-app-menu", JsValue::Object(data));
        }
    }

    pub fn menu_button_mouse_leave(&mut self, index: usize) {
        if index < self.menu_active.len() {
            self.menu_active[index] = false;
        }
    }

    pub fn search_focus(&mut self) {
        self.search_focused = true;
    }

    pub fn search_blur(&mut self) {
        self.search_focused = false;
    }

    /// `search` input event: the view supplies the element's new `value`.
    pub fn search_input(&mut self, env: &mut SidebarEnv<'_>, value: JsString) {
        self.search_value = value;
        self.update_search_results(env);
    }

    fn update_search_hovered(&mut self, scroll: Option<&mut dyn SidebarGeometry>) {
        for (index, hovered) in self.search_items_hovered.iter_mut().enumerate() {
            *hovered = Some(index) == self.search_hovered_index;
        }
        if let (Some(geometry), Some(index)) = (scroll, self.search_hovered_index)
            && index < self.search_items_hovered.len()
        {
            geometry.scroll_search_item_into_view(index);
        }
    }

    fn update_search_results(&mut self, env: &SidebarEnv<'_>) {
        self.search_results = if self.search_value.is_empty() {
            Vec::new()
        } else {
            search_fields(&env.log.borrow(), &self.search_value)
        };
        self.search_results.retain(|field| {
            !HIDDEN_KEYS.iter().any(|hidden| {
                let hidden = js(hidden);
                let mut prefixed = js("/");
                prefixed.push_str(&hidden);
                if field.starts_with(&prefixed, None) {
                    return true;
                }
                let mut nt = js("NT:");
                nt.push_str(&prefixed);
                if field.starts_with(&nt, None) {
                    return true;
                }
                let mut nt_akit = js("NT:/AdvantageKit");
                nt_akit.push_str(&prefixed);
                if field.starts_with(&nt_akit, None) {
                    return true;
                }
                let mut ds = js("DS:");
                ds.push_str(&prefixed);
                if field.starts_with(&ds, None) {
                    return true;
                }
                let mut middle = prefixed.clone();
                middle.push_str(&js("/"));
                if field.includes(&middle, None) {
                    return true;
                }
                field.ends_with(&prefixed, None)
            })
        });
        self.search_items_hovered = vec![false; self.search_results.len()];
    }

    fn search_periodic(&mut self, geometry: &mut dyn SidebarGeometry) {
        let input = geometry.search_input_rect();
        let bottom = input.y + input.height;
        if Some(bottom) != self.search_results_top {
            self.search_results_top = Some(bottom);
        }
        if Some(input.width) != self.search_results_min_width {
            self.search_results_min_width = Some(input.width);
        }
        let hidden =
            !self.search_focused || self.search_value.is_empty() || self.search_results.is_empty();
        if hidden != self.search_results_hidden {
            if hidden && !self.search_results_hidden {
                geometry.reset_search_results_scroll();
                self.search_hovered_index = None;
                self.update_search_hovered(None);
            }
            self.search_results_hidden = hidden;
        }
    }

    /// `search` keydown (`event.code`).
    pub fn search_key_down(
        &mut self,
        env: &mut SidebarEnv<'_>,
        code: &str,
        now: f64,
    ) -> EventOutcome {
        let results_count = self.search_results.len();
        if code == "ArrowDown" {
            match self.search_hovered_index {
                None => self.search_hovered_index = Some(0),
                Some(i) if i >= results_count => self.search_hovered_index = Some(0),
                Some(i) if i < results_count.saturating_sub(1) => {
                    self.search_hovered_index = Some(i + 1)
                }
                _ => {}
            }
            self.update_search_hovered(Some(&mut *env.geometry));
            EventOutcome::Ok
        } else if code == "ArrowUp" {
            match self.search_hovered_index {
                None => self.search_hovered_index = Some(0),
                Some(i) if i >= results_count => self.search_hovered_index = Some(0),
                Some(i) if i > 0 => self.search_hovered_index = Some(i - 1),
                _ => {}
            }
            self.update_search_hovered(Some(&mut *env.geometry));
            EventOutcome::Ok
        } else if code == "Enter" {
            self.run_search(env, now)
        } else {
            EventOutcome::Ok
        }
    }

    /// `.search-results-item` mousemove.
    pub fn search_item_mouse_move(&mut self, index: usize) {
        if index >= self.search_results.len() {
            return;
        }
        self.search_hovered_index = Some(index);
        self.update_search_hovered(None);
    }

    /// `.search-results-item` mousedown and touchdown.
    pub fn search_item_activate(
        &mut self,
        env: &mut SidebarEnv<'_>,
        index: usize,
        now: f64,
    ) -> EventOutcome {
        if index >= self.search_results.len() {
            return EventOutcome::Ok;
        }
        self.search_hovered_index = Some(index);
        self.run_search(env, now)
    }

    /// `runSearch`. On a throw `searchKey` stays set, the remaining callbacks never run, and
    /// the input is not blurred.
    fn run_search(&mut self, env: &mut SidebarEnv<'_>, now: f64) -> EventOutcome {
        let Some(hovered) = self.search_hovered_index else {
            return EventOutcome::Ok;
        };
        if hovered >= self.search_results.len() {
            return EventOutcome::Ok;
        }
        self.search_key = Some(self.search_results[hovered].clone());
        // `forEach` fixes the length up front; callbacks pushed by expansions
        // during this pass run on the next `runSearch`.
        let len = self.search_callbacks.len();
        for i in 0..len {
            let outcome = match self.search_callbacks[i] {
                SearchCallback::Highlight(id) => {
                    self.highlight_for_search(env, id, now);
                    Ok(())
                }
                SearchCallback::Expand(id) => self.expand_for_search(env, id, now),
            };
            if let Err(outcome) = outcome {
                return outcome;
            }
        }
        self.search_key = None;
        self.blur_search_input(env.geometry);
        EventOutcome::Ok
    }

    /// `highlightForSearch`.
    fn highlight_for_search(&mut self, env: &mut SidebarEnv<'_>, id: usize, now: f64) {
        let (Some(key), Some(full_key)) = (&self.search_key, &self.nodes[id].full_key) else {
            return;
        };
        if full_key != key {
            return;
        }
        env.geometry.scroll_into_view_if_needed(self.row_ref(id));
        self.nodes[id].highlighted = true;
        let node_id = self.nodes[id].id;
        self.schedule_timeout(
            now,
            HIGHLIGHT_TIMEOUT_MS,
            TimerKind::ClearHighlight(node_id),
        );
    }

    /// `expandForSearch` + `setExpanded` machinery.
    fn expand_for_search(&mut self, env: &mut SidebarEnv<'_>, id: usize, now: f64) -> Attempt<()> {
        let Some(key) = self.search_key.clone() else {
            return Ok(());
        };
        if !self.tree_contains_key(&self.nodes[id].tree_children, &key) {
            return Ok(());
        }
        if !self.nodes[id].expanded {
            self.set_expanded(env, id, true, now)?;
        }
        Ok(())
    }

    fn tree_contains_key(&self, children: &JsObject<LogFieldTree>, key: &JsString) -> bool {
        for tree in children.values() {
            if tree.full_key.as_ref() == Some(key) || self.tree_contains_key(&tree.children, key) {
                return true;
            }
        }
        false
    }

    fn set_expanded(
        &mut self,
        env: &mut SidebarEnv<'_>,
        id: usize,
        expanded: bool,
        now: f64,
    ) -> Attempt<()> {
        self.nodes[id].expanded = expanded;
        if expanded {
            self.expanded_fields
                .insert(self.nodes[id].full_title.clone());
        } else {
            self.expanded_fields
                .shift_remove(&self.nodes[id].full_title.clone());
        }
        if !self.nodes[id].children_materialized {
            self.nodes[id].children_materialized = true;
            let node = &self.nodes[id];
            let mut child_keys: Vec<JsString> = node
                .tree_children
                .keys()
                .filter(|key| !is_hidden_key(key))
                .cloned()
                .collect();
            let full_title = &node.full_title;
            let use_known = full_title == &js("/AdvantageKit")
                || full_title == &js("/NT")
                || starts_with(full_title, "/Log");
            array_sort(&mut child_keys, |a, b| sort_keys(a, b, use_known));
            for key in child_keys {
                let child = self.nodes[id]
                    .tree_children
                    .get_key(&key)
                    .expect("key came from tree_children")
                    .clone();
                let mut full_title = self.nodes[id].full_title.clone();
                full_title.push_str(&js("/"));
                full_title.push_str(&key);
                self.add_fields(
                    env,
                    FieldEntry {
                        title: key,
                        full_title,
                        field: &child,
                    },
                    Some(id),
                    self.nodes[id].indent + INDENT_SIZE_PX,
                    self.nodes[id].children_generated,
                    now,
                )?;
            }
        }
        Ok(())
    }

    /// `closedIcon`/`openIcon` clicks. `slot` 0 expands, 1 collapses, 2 (neutral) does nothing.
    pub fn icon_click(
        &mut self,
        env: &mut SidebarEnv<'_>,
        flat_index: usize,
        slot: usize,
        now: f64,
    ) -> EventOutcome {
        let Some(id) = self.flat_id(flat_index) else {
            return EventOutcome::Ok;
        };
        if !self.nodes[id].expand_listeners {
            return EventOutcome::Ok;
        }
        let result = match slot {
            0 => self.set_expanded(env, id, true, now),
            1 => self.set_expanded(env, id, false, now),
            _ => Ok(()),
        };
        match result {
            Ok(()) => EventOutcome::Ok,
            Err(outcome) => outcome,
        }
    }

    /// `field-item-label` mousedown (only registered on full-key rows upstream).
    pub fn label_mouse_down(&mut self, flat_index: usize, x: f64, y: f64, ox: f64, oy: f64) {
        if let Some(id) = self.flat_id(flat_index)
            && self.nodes[id].full_key.is_some()
        {
            self.nodes[id].mouse_down = Some([x, y, ox, oy]);
        }
    }

    /// `field-item-label` mouseup (only registered on full-key rows upstream).
    pub fn label_mouse_up(&mut self, flat_index: usize, x: f64, y: f64, ctrl_or_meta: bool) {
        let Some(id) = self.flat_id(flat_index) else {
            return;
        };
        if self.nodes[id].full_key.is_none() {
            return;
        }
        let Some(down) = self.nodes[id].mouse_down else {
            return;
        };
        if ctrl_or_meta
            && abs(x - down[0]) < FIELD_DRAG_THRESHOLD_PX
            && abs(y - down[1]) < FIELD_DRAG_THRESHOLD_PX
        {
            let full_key = self.nodes[id].full_key.clone().unwrap_or_default();
            match self.select_group.iter().position(|k| k == &full_key) {
                None => {
                    self.select_group.push(full_key);
                    self.nodes[id].label_bold = Some(true);
                }
                Some(index) => {
                    self.select_group.remove(index);
                    self.nodes[id].label_bold = Some(false);
                }
            }
        }
        self.nodes[id].mouse_down = None;
    }

    /// `field-item-label` contextmenu.
    pub fn label_context_menu(&mut self, flat_index: usize, x: f64, y: f64) {
        let Some(id) = self.flat_id(flat_index) else {
            return;
        };
        self.nodes[id].mouse_down = None;
        let mut data = JsObject::new();
        data.insert("title", JsValue::String(self.nodes[id].title.clone()));
        data.insert(
            "fullTitle",
            JsValue::String(self.nodes[id].full_title.clone()),
        );
        data.insert(
            "position",
            JsValue::Array(vec![JsValue::Number(x), JsValue::Number(y)]),
        );
        self.send_main("ask-open-sidebar-context-menu", JsValue::Object(data));
    }

    /// `field-item-label` touchstart, which reads the label's rect once per coordinate.
    pub fn label_touch_start(
        &mut self,
        geometry: &mut dyn SidebarGeometry,
        drag: &mut Drag,
        flat_index: usize,
        x: f64,
        y: f64,
    ) {
        if let Some(id) = self.flat_id(flat_index) {
            let offset_x = x - geometry.label_rect(self.row_ref(id)).x;
            let offset_y = y - geometry.label_rect(self.row_ref(id)).y;
            self.drag_event(drag, id, x, y, offset_x, offset_y);
        }
    }

    /// The label `dragEvent`: fills `#dragItem` and calls `window.startDrag`.
    fn drag_event(
        &mut self,
        drag: &mut Drag,
        id: usize,
        x: f64,
        y: f64,
        offset_x: f64,
        offset_y: f64,
    ) {
        let node = &self.nodes[id];
        let Some(full_key) = node.full_key.clone() else {
            return;
        };
        let is_group = self.select_group.contains(&full_key);
        let mut text = node.title.clone();
        if is_group {
            text.push_str(&js("\u{2026}"));
        }
        let children = if is_group {
            Vec::new()
        } else {
            node.tree_children
                .values()
                .filter_map(|tree| tree.full_key.clone())
                .filter(|key| !ends_with(key, "/length"))
                .collect()
        };
        let payload = DragPayload {
            fields: if is_group {
                self.select_group.clone()
            } else {
                vec![full_key]
            },
            children,
        };
        drag.start(
            x,
            y,
            offset_x,
            offset_y,
            DragItem {
                text,
                bold: is_group,
            },
            payload,
        );
        if is_group {
            self.select_group.clear();
            for node in &mut self.nodes {
                if node.generation == self.generation && node.full_key.is_some() {
                    node.label_bold = Some(false);
                }
            }
        }
    }

    /// Type-warning span click.
    pub fn warning_click(&mut self, flat_index: usize) {
        if self.flat_id(flat_index).is_none() {
            return;
        }
        let mut data = JsObject::new();
        data.insert("title", JsValue::String(js("Conflicting types")));
        data.insert("content", JsValue::String(js(TYPE_WARNING_TEXT)));
        self.send_main("alert", JsValue::Object(data));
    }

    /// `tuning-button` click.
    pub fn tuning_button_click(&mut self, env: &mut SidebarEnv<'_>) {
        self.set_tuning_mode_active(env, !self.is_tuning_mode);
    }

    /// `tuning-button` contextmenu.
    pub fn tuning_button_context_menu(&mut self, x: f64, y: f64) {
        let mut keys: Vec<JsString> = self.tuning_value_cache.keys().cloned().collect();
        array_sort(&mut keys, |a, b| code_unit_cmp(a.units(), b.units()));
        let mut lines = JsString::new();
        for (index, key) in keys.iter().enumerate() {
            if index > 0 {
                lines.push_str(&js("\n"));
            }
            lines.push_str(key);
            lines.push_str(&js(" = "));
            lines.push_str(self.tuning_value_cache.get_key(key).unwrap());
        }
        let mut data = JsObject::new();
        data.insert("isTuningMode", JsValue::Bool(self.is_tuning_mode));
        data.insert("values", JsValue::String(lines));
        data.insert(
            "position",
            JsValue::Array(vec![JsValue::Number(x), JsValue::Number(y)]),
        );
        self.send_main("ask-open-tuning-context-menu", JsValue::Object(data));
    }

    fn set_tuning_mode_active(&mut self, env: &mut SidebarEnv<'_>, active: bool) {
        self.is_tuning_mode = active;
        for id in 0..self.nodes.len() {
            if self.nodes[id].generation != self.generation {
                continue;
            }
            let Some(full_key) = self.nodes[id].full_key.clone() else {
                continue;
            };
            let tunable = active && env.tuner.is_some_and(|t| t.is_tunable(&full_key));
            match &mut self.nodes[id].value {
                ValueState::Bool(state) => {
                    state.tunable = tunable;
                    if !tunable {
                        self.tuning_value_cache.remove_key(&full_key);
                    }
                }
                ValueState::Number(state) => {
                    state.input_hidden = !tunable;
                    state.span_hidden = tunable;
                    if !tunable {
                        self.tuning_value_cache.remove_key(&full_key);
                    }
                    self.measure_value_width(env, id);
                }
                _ => {}
            }
        }
        self.tuning_button_active = active;
    }

    fn update_tuning_button(&mut self, env: &mut SidebarEnv<'_>) {
        let tuner_available = env.tuner.is_some_and(|t| t.has_tunable_fields());
        if self.tuning_button_hidden == tuner_available {
            self.tuning_button_hidden = !tuner_available;
            if !tuner_available {
                self.set_tuning_mode_active(env, false);
            }
        }
    }

    /// Boolean svg click (tuning toggle).
    pub fn svg_click(&mut self, env: &mut SidebarEnv<'_>, flat_index: usize) {
        let Some(id) = self.flat_id(flat_index) else {
            return;
        };
        let ValueState::Bool(state) = &mut self.nodes[id].value else {
            return;
        };
        if !state.tunable {
            return;
        }
        let value = state.fill.as_ref() == Some(&js("red"));
        let Some(live_time) = env.selection.get_current_live_time() else {
            return;
        };
        let full_key = self.nodes[id].full_key.clone().unwrap();
        self.tuning_value_cache
            .insert(full_key.clone(), js(if value { "true" } else { "false" }));
        if let Some(tuner) = env.tuner {
            tuner.publish(&full_key, &JsValue::Bool(value));
        }
        env.log
            .borrow_mut()
            .put_boolean(&full_key, live_time, value);
    }

    /// Number tuning input focus/blur/value/keydown (the DOM element's own state).
    pub fn number_input_focus(&mut self, flat_index: usize, focused: bool) {
        if let Some(id) = self.flat_id(flat_index)
            && let ValueState::Number(state) = &mut self.nodes[id].value
        {
            state.focused = focused;
        }
    }

    pub fn number_input_value(&mut self, flat_index: usize, value: JsString) {
        if let Some(id) = self.flat_id(flat_index)
            && let ValueState::Number(state) = &mut self.nodes[id].value
        {
            state.input_value = value;
        }
    }

    pub fn number_input_key_down(&mut self, flat_index: usize, key: &str) {
        if key == "Enter" {
            self.number_input_focus(flat_index, false);
        }
    }

    /// The 250 ms tuning-publish interval.
    fn tuning_publish_tick(&mut self, env: &mut SidebarEnv<'_>) {
        if !self.is_tuning_mode {
            return;
        }
        for node in &mut self.nodes {
            if node.generation != self.generation {
                continue;
            }
            let ValueState::Number(state) = &mut node.value else {
                continue;
            };
            if state.focused {
                continue;
            }
            let value = number::to_number(state.input_value.units());
            let has_value = !state.input_value.is_empty() && value.is_finite();
            let Some(full_key) = node.full_key.clone() else {
                continue;
            };
            if has_value {
                self.tuning_value_cache
                    .insert(full_key.clone(), state.input_value.clone());
                if let Some(live_time) = env.selection.get_current_live_time() {
                    if let Some(tuner) = env.tuner {
                        tuner.publish(&full_key, &JsValue::Number(value));
                    }
                    env.log.borrow_mut().put_number(&full_key, live_time, value);
                }
            } else if state.last_has_value {
                self.tuning_value_cache.remove_key(&full_key);
                if let Some(tuner) = env.tuner {
                    tuner.unpublish(&full_key);
                }
            }
            state.last_has_value = has_value;
        }
    }

    /// `periodic`: `searchPeriodic`, `updateTitle`, `updateValues`, `updateTuningButton`.
    pub fn periodic(&mut self, env: &mut SidebarEnv<'_>) {
        self.search_periodic(env.geometry);
        self.update_title(env);
        self.update_values(env);
        self.update_tuning_button(env);
    }

    fn update_title(&mut self, env: &SidebarEnv<'_>) {
        let mut range = env.log.borrow().get_timestamp_range();
        if let Some(live_time) = env.selection.get_current_live_time() {
            range[1] = live_time;
        }
        let title = if self.field_count == 0 {
            js(OPEN_LOG_TITLE)
        } else {
            let mut runtime = range[1] - range[0];
            let mut unit = "s";
            if runtime > 120.0 {
                runtime /= 60.0;
                unit = "m";
            }
            if runtime > 120.0 {
                runtime /= 60.0;
                unit = "h";
            }
            let mut title = JsString::from(number::to_string(self.field_count as f64));
            title.push_str(&js(" field"));
            if self.field_count != 1 {
                title.push_str(&js("s"));
            }
            title.push_str(&js(", "));
            title.push_str(&JsString::from(number::to_string(floor(runtime))));
            title.push_str(&js(unit));
            title.push_str(&js(" runtime"));
            title
        };
        if title != self.title_text {
            self.title_text = title;
        }
    }

    /// `updateValueCallbacks` run in registration order: the value rows of the current
    /// rebuild, rows built since under removed parents included.
    fn update_values(&mut self, env: &mut SidebarEnv<'_>) {
        let mode = env.selection.get_mode();
        let hovered = env.selection.get_hovered_time();
        let selected = env.selection.get_selected_time();
        let time = if mode == SelectionMode::Playback || mode == SelectionMode::Locked {
            selected
        } else if hovered.is_some() {
            hovered
        } else {
            selected
        };
        let display = self.dfs_index_map();
        for id in 0..self.nodes.len() {
            if self.nodes[id].generation != self.generation
                || matches!(self.nodes[id].value, ValueState::None)
            {
                continue;
            }
            let row = self.row_ref_in(&display, id);
            let rect = env.geometry.field_item_rect(row);
            self.update_value(env, id, time, rect);
        }
    }

    fn update_value(
        &mut self,
        env: &mut SidebarEnv<'_>,
        id: usize,
        time: Option<f64>,
        rect: RowRect,
    ) {
        if !rect.on_screen(env.inner_height) {
            return;
        }
        let Some(full_key) = self.nodes[id].full_key.clone() else {
            return;
        };
        let remeasure = match &mut self.nodes[id].value {
            ValueState::Bool(state) => {
                let value =
                    time.and_then(|t| get_value_at::<bool>(&env.log.borrow(), &full_key, t, None));
                if !state.first_update && value == state.last_value {
                    return;
                }
                state.first_update = false;
                state.last_value = value;
                if let Some(v) = value {
                    state.fill = Some(js(if v {
                        if env.dark_theme {
                            "lightgreen"
                        } else {
                            "green"
                        }
                    } else {
                        "red"
                    }));
                    state.line_hidden = Some(v);
                }
                let hidden = value.is_none();
                let changed = state.last_hidden != Some(hidden);
                state.last_hidden = Some(hidden);
                self.nodes[id].value_element_hidden = hidden;
                changed
            }
            ValueState::Number(state) => {
                let value =
                    time.and_then(|t| get_value_at::<f64>(&env.log.borrow(), &full_key, t, None));
                if !state.first_update && value == state.last_value {
                    return;
                }
                state.first_update = false;
                state.last_value = value;
                let value_str = match value {
                    None => JsString::new(),
                    Some(v) if abs(v) < 1e-9 => js("0"),
                    Some(v) if abs(v) >= 1e5 || abs(v) < 1e-3 => {
                        number::to_exponential(v, Some(1.0))
                            .map(|text| JsString::from(text).replace(&js("+"), &js("")).unwrap())
                            .unwrap_or_default()
                    }
                    Some(v) if rem(v, 1.0) == 0.0 => JsString::from(number::to_string(v)),
                    Some(v) => JsString::from(number::to_fixed(v, 3.0).unwrap()),
                };
                let char_count = value_str.len();
                state.placeholder = Some(value_str.clone());
                state.span_text = value_str;
                let changed = state.last_char_count != Some(char_count);
                state.last_char_count = Some(char_count);
                changed
            }
            ValueState::Text(state) => {
                let value = time
                    .and_then(|t| get_value_at::<JsString>(&env.log.borrow(), &full_key, t, None));
                if !state.first_update
                    && value == state.last_value
                    && self.sidebar_width == state.last_sidebar_width
                {
                    return;
                }
                state.first_update = false;
                state.last_value = value.clone();
                state.last_sidebar_width = self.sidebar_width;
                let value_str = match value {
                    None => JsString::new(),
                    Some(v) => {
                        let max_characters = round(self.sidebar_width * 0.03);
                        if (v.len() as f64) > max_characters {
                            let mut truncated = v.substring(0.0, Some(max_characters - 1.0));
                            truncated.push_str(&js("\u{2026}"));
                            truncated
                        } else {
                            v
                        }
                    }
                };
                let char_count = value_str.len();
                state.text = value_str;
                let changed = state.last_char_count != Some(char_count);
                state.last_char_count = Some(char_count);
                changed
            }
            ValueState::None => false,
        };
        if remeasure {
            self.measure_value_width(env, id);
        }
    }

    /// `--value-width`, which upstream re-measures only when the value element's visibility,
    /// character count or tuning mode changes, so a same-length value keeps the old width.
    fn measure_value_width(&mut self, env: &mut SidebarEnv<'_>, id: usize) {
        let node = &self.nodes[id];
        let value = node.value.to_row();
        let hidden = node.value_element_hidden;
        let display = self.dfs_index_map();
        let row = self.row_ref_in(&display, id);
        // `clientWidth === 0 ? 0 : clientWidth + margin` reads a nonzero width twice.
        let value_width = if env.geometry.value_client_width(row, &value, hidden) == 0.0 {
            0.0
        } else {
            env.geometry.value_client_width(row, &value, hidden) + VALUE_WIDTH_MARGIN_PX
        };
        self.nodes[id].value_width = value_width;
    }

    /// `getActiveFields`: `activeFieldCallbacks` in registration order, each reading its row's
    /// rect.
    pub fn get_active_fields(&mut self, env: &mut SidebarEnv<'_>) -> IndexSet<JsString> {
        if self.sidebar_width <= 0.0 {
            return IndexSet::new();
        }
        let display = self.dfs_index_map();
        for id in 0..self.nodes.len() {
            if self.nodes[id].generation != self.generation {
                continue;
            }
            let rect = env.geometry.field_item_rect(self.row_ref_in(&display, id));
            let expanded = rect.height > 0.0;
            let on_screen = rect.on_screen(env.inner_height);
            let node = &self.nodes[id];
            if let Some(full_key) = node.full_key.clone() {
                let mut is_active = false;
                let log = env.log.borrow();
                let r#type = log.get_type(&full_key);
                let structured_type = log.get_structured_type(&full_key);
                let wpilib_type = log.get_wpilib_type(&full_key);
                if expanded
                    && (r#type == Some(LoggableType::BooleanArray)
                        || r#type == Some(LoggableType::NumberArray)
                        || r#type == Some(LoggableType::StringArray)
                        || (r#type != Some(LoggableType::Empty) && structured_type.is_some())
                        || (r#type == Some(LoggableType::Raw)
                            && wpilib_type.is_some_and(custom_schemas::has)))
                {
                    is_active = true;
                }
                if on_screen
                    && (r#type == Some(LoggableType::Number)
                        || r#type == Some(LoggableType::Boolean)
                        || r#type == Some(LoggableType::String))
                {
                    is_active = true;
                }
                if is_active {
                    self.active_fields.insert(full_key);
                } else {
                    self.active_fields.shift_remove(&full_key);
                }
            }
            if let Some(type_child) = self.nodes[id].tree_children.get(TYPE_KEY)
                && let Some(type_key) = type_child.full_key.clone()
            {
                if expanded {
                    self.active_fields.insert(type_key);
                } else {
                    self.active_fields.shift_remove(&type_key);
                }
            }
        }
        self.active_fields.clone()
    }

    /// `refresh`.
    pub fn refresh(&mut self, env: &mut SidebarEnv<'_>, force: bool) -> Attempt<()> {
        let field_keys = env.log.borrow().get_field_keys();
        let fields_changed = force || !arrays_equal(&field_keys, &self.last_field_keys);
        self.last_field_keys = field_keys;
        if !fields_changed {
            for id in 0..self.nodes.len() {
                if self.nodes[id]
                    .full_key
                    .as_ref()
                    .is_some_and(|key| !key.is_empty())
                    && self.nodes[id].generation == self.generation
                {
                    self.update_type_warning(env, id);
                }
            }
            for id in 0..self.nodes.len() {
                if self.nodes[id].full_key_built && self.nodes[id].generation == self.generation {
                    self.update_metadata(env, id);
                }
            }
            for id in 0..self.nodes.len() {
                if self.nodes[id].full_key_built {
                    self.update_loading(env, id);
                }
            }
            return Ok(());
        }
        self.field_count = env.log.borrow().get_field_count();
        let original_scroll = env.geometry.scroll_top();
        self.generation += 1;
        for node in &mut self.nodes {
            node.detached = true;
        }
        self.roots.clear();
        self.active_fields.clear();
        let mut tree = env.log.borrow().get_field_tree(true, &JsString::new());
        let root_keys: Vec<JsString> = tree
            .keys()
            .filter(|key| !starts_with(key, "."))
            .cloned()
            .collect();
        if root_keys.len() == 1 && tree.get_key(&root_keys[0]).unwrap().full_key.is_none() {
            tree = tree.get_key(&root_keys[0]).unwrap().children.clone();
        }
        let secondary_keys: Vec<JsString> = tree
            .keys()
            .filter(|key| !starts_with(key, "."))
            .cloned()
            .collect();
        if secondary_keys.len() == 1
            && secondary_keys[0] == js("Dscomm")
            && tree.get_key(&secondary_keys[0]).unwrap().full_key.is_none()
        {
            tree = tree.get_key(&secondary_keys[0]).unwrap().children.clone();
        }
        let mut keys: Vec<JsString> = tree
            .keys()
            .filter(|key| !is_hidden_key(key))
            .cloned()
            .collect();
        array_sort(&mut keys, |a, b| sort_keys(a, b, true));
        for key in keys {
            let node = tree.get_key(&key).unwrap().clone();
            let mut full_title = js("/");
            full_title.push_str(&key);
            self.add_fields(
                env,
                FieldEntry {
                    title: key,
                    full_title,
                    field: &node,
                },
                None,
                0.0,
                false,
                0.0,
            )?;
        }
        env.geometry.set_scroll_top(original_scroll);
        self.update_search_results(env);
        Ok(())
    }

    fn update_type_warning(&mut self, env: &SidebarEnv<'_>, id: usize) {
        let Some(full_key) = self.nodes[id].full_key.clone() else {
            return;
        };
        let hidden = !env.log.borrow().get_type_warning(&full_key);
        let node = &mut self.nodes[id];
        if hidden != node.warning_last_hidden {
            node.warning_last_hidden = hidden;
            node.warning_hidden = hidden;
        }
    }

    fn update_metadata(&mut self, env: &SidebarEnv<'_>, id: usize) {
        let Some(full_key) = self.nodes[id].full_key.clone() else {
            return;
        };
        let metadata = env.log.borrow().get_metadata_string(&full_key);
        if Some(&metadata) == self.nodes[id].metadata_last.as_ref() {
            return;
        }
        self.nodes[id].metadata_last = Some(metadata.clone());
        // `Object.keys(parsed)` throws for null, enumerates string/array indices, and is
        // empty for numbers and booleans.
        let title = match json::parse(&metadata) {
            Ok(parsed) => {
                let mut entries: Vec<(JsString, JsValue)> = match &parsed {
                    JsValue::Object(obj) => obj
                        .keys()
                        .map(|key| {
                            (
                                key.clone(),
                                obj.get_key(key).cloned().unwrap_or(JsValue::Undefined),
                            )
                        })
                        .collect(),
                    JsValue::String(text) => (0..text.len())
                        .map(|index| {
                            (
                                JsString::from(number::to_string(index as f64)),
                                JsValue::String(
                                    text.substring(index as f64, Some(index as f64 + 1.0)),
                                ),
                            )
                        })
                        .collect(),
                    JsValue::Array(items) => items
                        .iter()
                        .enumerate()
                        .map(|(index, value)| {
                            (
                                JsString::from(number::to_string(index as f64)),
                                value.clone(),
                            )
                        })
                        .collect(),
                    JsValue::Null | JsValue::Undefined => {
                        self.nodes[id].label_title = Some(metadata);
                        return;
                    }
                    _ => Vec::new(),
                };
                array_sort(&mut entries, |a, b| code_unit_cmp(a.0.units(), b.0.units()));
                let mut text = JsString::new();
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index > 0 {
                        text.push_str(&js("\n"));
                    }
                    text.push_str(key);
                    text.push_str(&js(": "));
                    text.push_str(&js_to_string(value));
                }
                text
            }
            Err(_) => metadata,
        };
        self.nodes[id].label_title = Some(title);
    }

    fn update_loading(&mut self, env: &SidebarEnv<'_>, id: usize) {
        let Some(full_key) = &self.nodes[id].full_key else {
            return;
        };
        let is_loading = env.loading_fields.contains(full_key);
        let was_loading = self.nodes[id].label_loading;
        if is_loading && !was_loading {
            self.nodes[id].label_loading = true;
        } else if !is_loading && was_loading {
            self.nodes[id].label_loading = false;
        }
    }

    /// `addFields` (recursive child creation included).
    fn add_fields(
        &mut self,
        env: &mut SidebarEnv<'_>,
        entry: FieldEntry<'_>,
        parent: Option<usize>,
        indent: f64,
        generated: bool,
        now: f64,
    ) -> Attempt<usize> {
        let FieldEntry {
            title,
            full_title,
            field,
        } = entry;
        let detached = parent.is_some_and(|p| self.nodes[p].detached);
        let has_children = !field.children.is_empty();
        let children_generated = generated
            || (field
                .full_key
                .as_ref()
                .is_some_and(|key| env.log.borrow().is_generated_parent(key)));
        let id = self.nodes.len();
        let node_id = self.next_node_id;
        self.next_node_id += 1;
        let node = Node {
            id: node_id,
            title: title.clone(),
            full_title: full_title.clone(),
            full_key: field.full_key.clone(),
            tree_children: field.children.clone(),
            detached,
            generation: self.generation,
            full_key_built: false,
            has_children,
            child_span: false,
            expand_listeners: false,
            children_generated,
            indent,
            generated,
            known_label: (indent == 0.0
                || (indent == INDENT_SIZE_PX
                    && (starts_with(&full_title, "/AdvantageKit")
                        || starts_with(&full_title, "/Log"))))
                && (KNOWN_KEYS.iter().any(|k| title == js(k)) || starts_with(&title, MERGED_KEY)),
            label_loading: false,
            label_bold: None,
            label_title: None,
            type_label: None,
            warning_hidden: false,
            warning_last_hidden: false,
            highlighted: false,
            mouse_down: None,
            metadata_last: None,
            expanded: false,
            children_materialized: false,
            children: Vec::new(),
            parent,
            value_element_hidden: false,
            value_width: 0.0,
            value: ValueState::None,
        };
        self.nodes.push(node);
        match parent {
            Some(parent) => self.nodes[parent].children.push(id),
            None => self.roots.push(id),
        }
        if let Some(full_key) = field.full_key.clone() {
            if !full_key.is_empty() {
                self.update_type_warning(env, id);
                if let Some(structured_type) = env.log.borrow().get_structured_type(&full_key) {
                    let mut label = js(" \u{2013} ");
                    label.push_str(structured_type);
                    self.nodes[id].type_label = Some(label);
                }
            }
            self.search_callbacks.push(SearchCallback::Highlight(id));
            self.highlight_for_search(env, id, now);
            match env.log.borrow().get_type(&full_key) {
                Some(LoggableType::Boolean) => {
                    let mut state = BoolState {
                        first_update: true,
                        ..Default::default()
                    };
                    state.tunable =
                        self.is_tuning_mode && env.tuner.is_some_and(|t| t.is_tunable(&full_key));
                    if !state.tunable {
                        self.tuning_value_cache.remove_key(&full_key);
                    }
                    self.nodes[id].value = ValueState::Bool(state);
                }
                Some(LoggableType::Number) => {
                    let mut state = NumberState {
                        input_hidden: true,
                        first_update: true,
                        ..Default::default()
                    };
                    let tunable =
                        self.is_tuning_mode && env.tuner.is_some_and(|t| t.is_tunable(&full_key));
                    state.input_hidden = !tunable;
                    state.span_hidden = tunable;
                    if !tunable {
                        self.tuning_value_cache.remove_key(&full_key);
                    }
                    self.nodes[id].value = ValueState::Number(Box::new(state));
                    self.measure_value_width(env, id);
                    if let Some(cached) = self.tuning_value_cache.get_key(&full_key).cloned()
                        && let ValueState::Number(state) = &mut self.nodes[id].value
                    {
                        state.input_value = cached;
                    }
                }
                Some(LoggableType::String) => {
                    self.nodes[id].value = ValueState::Text(TextState {
                        first_update: true,
                        ..Default::default()
                    });
                }
                _ => {}
            }
            self.update_metadata(env, id);
            self.update_loading(env, id);
            self.nodes[id].full_key_built = true;
        } else if starts_with(&title, MERGED_KEY) && indent == 0.0 {
            let merge_index = number::to_number(title.slice(MERGED_KEY.len() as f64, None).units());
            let filenames = (self.get_filenames)();
            if merge_index < filenames.len() as f64 {
                if merge_index < 0.0 || merge_index.fract() != 0.0 {
                    self.nodes[id].type_label = Some(JsString::new());
                    return Err(EventOutcome::MergedFilenameMissing);
                }
                let mut label = js(" \u{2013} ");
                label.push_str(&filenames[merge_index as usize]);
                self.nodes[id].type_label = Some(label);
            }
        }
        if has_children {
            self.nodes[id].child_span = true;
            self.search_callbacks.push(SearchCallback::Expand(id));
            self.expand_for_search(env, id, now)?;
            self.nodes[id].expand_listeners = true;
            if self.expanded_fields.contains(&self.nodes[id].full_title) {
                self.set_expanded(env, id, true, now)?;
            }
        }
        Ok(id)
    }

    /// `(arena id, sibling position)` pairs in display order (DFS pre-order).
    fn dfs_ids(&self) -> Vec<(usize, usize)> {
        let mut order = Vec::new();
        let mut stack: Vec<(usize, usize)> = self
            .roots
            .iter()
            .enumerate()
            .rev()
            .map(|(i, id)| (*id, i + 1))
            .collect();
        while let Some((id, pos)) = stack.pop() {
            order.push((id, pos));
            for (i, &child) in self.nodes[id].children.iter().enumerate().rev() {
                stack.push((child, i + 1));
            }
        }
        order
    }

    /// Arena id to display index, for the rows a view shows: the attached rows under expanded
    /// parents.
    fn dfs_index_map(&self) -> HashMap<usize, usize> {
        let mut map = HashMap::new();
        let mut stack: Vec<usize> = self.roots.iter().rev().copied().collect();
        while let Some(id) = stack.pop() {
            map.insert(id, map.len());
            if self.nodes[id].expanded {
                stack.extend(self.nodes[id].children.iter().rev());
            }
        }
        map
    }

    fn row_ref(&self, id: usize) -> RowRef<'_> {
        let display = self.dfs_index_map();
        self.row_ref_in(&display, id)
    }

    fn row_ref_in(&self, display: &HashMap<usize, usize>, id: usize) -> RowRef<'_> {
        RowRef {
            nodes: &self.nodes,
            id,
            display_index: display.get(&id).copied(),
        }
    }

    fn flat_id(&self, flat_index: usize) -> Option<usize> {
        self.dfs_ids().get(flat_index).map(|(id, _)| *id)
    }

    /// Every row in the DOM, in document order, collapsed subtrees included.
    pub fn rows(&self) -> Vec<SidebarRow> {
        self.dfs_ids()
            .into_iter()
            .enumerate()
            .map(|(index, (id, position))| self.row(index, id, position))
            .collect()
    }

    /// The rows a view lays out, one per display index: those under expanded parents.
    pub fn displayed_rows(&self) -> Vec<SidebarRow> {
        let display = self.dfs_index_map();
        self.dfs_ids()
            .into_iter()
            .enumerate()
            .filter(|(_, (id, _))| display.contains_key(id))
            .map(|(index, (id, position))| self.row(index, id, position))
            .collect()
    }

    fn row(&self, index: usize, id: usize, position: usize) -> SidebarRow {
        let node = &self.nodes[id];
        SidebarRow {
            index,
            title: node.title.clone(),
            position,
            full_title: node.full_title.clone(),
            full_key: node.full_key.clone(),
            indent: node.indent,
            generated: node.generated,
            known_label: node.known_label,
            expanded: node.expanded,
            has_children: node.has_children,
            child_span: node.child_span,
            label_loading: node.label_loading,
            label_bold: node.label_bold,
            label_title: node.label_title.clone(),
            type_label: node.type_label.clone(),
            warning_hidden: node.warning_hidden,
            warning_title: js(TYPE_WARNING_TEXT),
            highlighted: node.highlighted,
            value_hidden: node.value_element_hidden,
            value_width: node.value_width,
            value: node.value.to_row(),
        }
    }

    pub fn title_text(&self) -> &JsString {
        &self.title_text
    }

    pub fn search_results_hidden(&self) -> bool {
        self.search_results_hidden
    }

    /// `.search-results`' inline `top` and `min-width` in px, unset before the first frame.
    pub fn search_results_position(&self) -> (Option<f64>, Option<f64>) {
        (self.search_results_top, self.search_results_min_width)
    }

    /// `.search-results-item` rows: `(key, hovered)`.
    pub fn search_items(&self) -> Vec<(JsString, bool)> {
        self.search_results
            .iter()
            .cloned()
            .zip(self.search_items_hovered.iter().copied())
            .collect()
    }

    pub fn search_value(&self) -> &JsString {
        &self.search_value
    }

    pub fn search_focused(&self) -> bool {
        self.search_focused
    }

    pub fn select_group(&self) -> &[JsString] {
        &self.select_group
    }

    pub fn tuning_button_hidden(&self) -> bool {
        self.tuning_button_hidden
    }

    pub fn tuning_button_active(&self) -> bool {
        self.tuning_button_active
    }

    pub fn is_tuning_mode(&self) -> bool {
        self.is_tuning_mode
    }

    pub fn tuning_value_cache(&self) -> &JsObject<JsString> {
        &self.tuning_value_cache
    }

    pub fn expanded_fields(&self) -> &IndexSet<JsString> {
        &self.expanded_fields
    }

    pub fn field_count(&self) -> usize {
        self.field_count
    }

    pub fn sidebar_width(&self) -> f64 {
        self.sidebar_width
    }

    pub fn shadow_shown(&self) -> bool {
        self.shadow_shown
    }
}

/// `sortKeys`.
pub fn sort_keys(a: &JsString, b: &JsString, use_known: bool) -> Ordering {
    if use_known {
        let merged_a = starts_with(a, MERGED_KEY);
        let merged_b = starts_with(b, MERGED_KEY);
        if merged_a && !merged_b {
            return Ordering::Greater;
        }
        if !merged_a && merged_b {
            return Ordering::Less;
        }
        let known_a = KNOWN_KEYS.iter().any(|k| a == &js(k));
        let known_b = KNOWN_KEYS.iter().any(|k| b == &js(k));
        if known_a && !known_b {
            return Ordering::Greater;
        }
        if !known_a && known_b {
            return Ordering::Less;
        }
    }
    locale_compare_numeric(a, b)
}

/// `String(value)` for JSON-shaped values (metadata tooltips). `Array.prototype.join` writes
/// nothing for `null` items.
fn js_to_string(value: &JsValue) -> JsString {
    match value {
        JsValue::Undefined => js("undefined"),
        JsValue::Null => js("null"),
        JsValue::Bool(v) => js(if *v { "true" } else { "false" }),
        JsValue::Number(v) => JsString::from(number::to_string(*v)),
        JsValue::String(v) => v.clone(),
        JsValue::Array(items) => {
            let mut text = JsString::new();
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    text.push_str(&js(","));
                }
                if !matches!(item, JsValue::Null | JsValue::Undefined) {
                    text.push_str(&js_to_string(item));
                }
            }
            text
        }
        _ => js("[object Object]"),
    }
}
