//! `src/hub/Tabs.ts`: the tab list, the tab bar's pointer handling, the controls panel height,
//! the per-frame loop that feeds renderers and satellites, and the tabs half of `HubState`.
//!
//! Upstream keeps `selectedTab` and each tab's `type`, `title`, `controller.UUID` and
//! `controlsHeight` as whatever a restored layout held, and later code applies JS operators to
//! them, so they are `JsValue`s here too. The DOM side (title elements, CSS variables, hidden
//! flags) is kept as the state the view draws from.
//!
//! Lite and XR are out of scope: `DISTRIBUTION` is never `LiteDS` and `activeXRUUID` stays
//! `null`, which upstream still compares every controller UUID against.

mod js;

use std::cell::{Ref, RefCell};
use std::fmt;
use std::rc::Rc;

use asrs_js::math;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::Log;
use asrs_log::log::robot_state::{get_autonomous_key, get_enabled_key, get_utility_key};
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::scroll_sensor::{ScrollSensor, Wheel};
use asrs_model::source_list::Rect;
use asrs_model::source_list::{ListInput, ListOutput};
use asrs_model::tab_controller::{Command, TabController, TabRenderer};

pub use crate::factory::CanvasEnv;
use asrs_model::tab_type::TabType;
use indexmap::IndexSet;

use crate::drag::DragUpdate;

use js::{num, read_error};

pub const TAB_DRAG_THRESHOLD_PX: f64 = 5.0;
pub const DEFAULT_CONTROLS_HEIGHT: f64 = 200.0;
pub const CONTROLS_DOUBLE_CLICK_MS: f64 = 400.0;

/// `FIXED_CONTROL_HEIGHTS`: `None` for tabs whose controls panel the user can resize.
pub fn fixed_controls_height(tab: TabType) -> Option<f64> {
    match tab {
        TabType::Documentation | TabType::Table | TabType::Console | TabType::Metadata => Some(0.0),
        TabType::Video | TabType::Joysticks => Some(85.0),
        TabType::LineGraph
        | TabType::Statistics
        | TabType::Field2d
        | TabType::Field3d
        | TabType::Swerve
        | TabType::Mechanism
        | TabType::Points => None,
    }
}

/// `FIXED_CONTROL_HEIGHTS.get(value)`.
fn fixed_for_value(value: &JsValue) -> Option<f64> {
    match value {
        JsValue::Number(n) => TabType::from_number(*n).and_then(fixed_controls_height),
        _ => None,
    }
}

/// The member of `TabType` a raw `type` value selects in `addTab`'s switch.
fn kind_of(value: &JsValue) -> Option<TabType> {
    match value {
        JsValue::Number(n) => TabType::from_number(*n),
        _ => None,
    }
}

fn default_title(kind: Option<TabType>) -> &'static str {
    kind.map_or("", TabType::default_title)
}

fn icon(kind: Option<TabType>) -> &'static str {
    kind.map_or("", TabType::icon)
}

fn title_text(kind: Option<TabType>, title: &JsValue) -> JsString {
    let mut text = JsString::from(icon(kind));
    text.push_str(&JsString::from(" "));
    if js::strict_eq(title, &JsValue::string("")) {
        text.push_str(&JsString::from(default_title(kind)));
    } else {
        text.push_str(&js::to_string(title));
    }
    text
}

fn get(value: &JsValue, key: &str) -> JsValue {
    match value {
        JsValue::Object(o) => o.get(key).cloned().unwrap_or_default(),
        _ => JsValue::Undefined,
    }
}

/// `value[key]`, which throws on `undefined` and `null`.
fn read(value: &JsValue, key: &str) -> Result<JsValue> {
    match value {
        JsValue::Undefined | JsValue::Null => Err(type_error(read_error(value, key))),
        _ => Ok(get(value, key)),
    }
}

fn type_error(message: impl Into<String>) -> TabsError {
    TabsError::Thrown(JsError::type_error(message))
}

fn missing(property: &str) -> TabsError {
    type_error(read_error(&JsValue::Undefined, property))
}

fn contains(rect: &Rect, x: f64, y: f64) -> bool {
    x >= rect.left() && x <= rect.right() && y >= rect.top() && y <= rect.bottom()
}

fn rect_js(rect: &Rect) -> JsValue {
    let mut o = JsObject::new();
    o.insert("x", num(rect.x));
    o.insert("y", num(rect.y));
    o.insert("width", num(rect.width));
    o.insert("height", num(rect.height));
    JsValue::Object(o)
}

/// What stopped an upstream call part-way.
#[derive(Clone, Debug, PartialEq)]
pub enum TabsError {
    /// Upstream throws this.
    Thrown(JsError),
    /// Upstream goes on in a state the port does not model (an `undefined` entry spliced into
    /// the tab list); the port leaves the list unchanged.
    Unmodeled(&'static str),
}

impl From<JsError> for TabsError {
    fn from(e: JsError) -> Self {
        TabsError::Thrown(e)
    }
}

impl fmt::Display for TabsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TabsError::Thrown(e) => write!(f, "{}: {}", e.name, e.message),
            TabsError::Unmodeled(what) => write!(f, "unmodeled: {what}"),
        }
    }
}

pub type Result<T = ()> = std::result::Result<T, TabsError>;

/// What `Tabs` reads from the window.
pub trait TabsHost {
    fn inner_height(&self) -> f64;
    /// `RENDERER_CONTENT.getBoundingClientRect().top` while `--show-timeline` is `show_timeline`.
    fn renderer_top(&self, show_timeline: bool) -> f64;
    /// `getBoundingClientRect()` of the `index`th `div.tab` in `div.tab-bar`.
    fn title_rect(&self, _index: usize) -> Rect {
        Rect::default()
    }
    /// `div.tab-bar-scroll`'s `getBoundingClientRect()`.
    fn overlay_rect(&self) -> Rect {
        Rect::default()
    }
    /// `button.add-tab`'s `getBoundingClientRect()`.
    fn add_button_rect(&self) -> Rect {
        Rect::default()
    }
    /// `div.tab-bar`'s `scrollLeft`.
    fn tab_bar_scroll_left(&self) -> f64 {
        0.0
    }
    /// Assigns `div.tab-bar`'s `scrollLeft`; the host clamps and snaps it as Blink does.
    fn set_tab_bar_scroll_left(&self, _value: f64) {}
    fn tab_bar_scroll_width(&self) -> f64 {
        0.0
    }
    fn tab_bar_client_width(&self) -> f64 {
        0.0
    }
    /// What canvas renderers are built with; `None` on a page without canvases, whose line
    /// graphs then draw nothing.
    fn canvas_env(&self) -> Option<CanvasEnv> {
        None
    }
    /// A renderer the page's view builds for a `kind` tab, in place of the factory's own.
    fn view_renderer(&mut self, _kind: TabType) -> Option<Box<dyn TabRenderer<Self>>> {
        None
    }
}

/// A new tab's halves, built in `addTab`'s constructor order: the controller's `UUID` field is
/// drawn first.
pub struct TabParts<Cx: ?Sized> {
    pub uuid: JsString,
    pub controller: Box<dyn TabController<Cx>>,
    pub renderer: Box<dyn TabRenderer<Cx>>,
}

/// The switch in `addTab`. `None` is its `default` branch (`NoopController` and
/// `NoopRenderer`), taken for a restored `type` such as `"1"` that names a template but is no
/// `TabType` member.
/// An `Err` is a constructor that threw: upstream adds no tab.
pub trait TabFactory<Cx: ?Sized> {
    fn create(
        &mut self,
        cx: &mut Cx,
        tab_type: Option<TabType>,
    ) -> std::result::Result<TabParts<Cx>, JsError>;
}

/// A tab's `div.tab` in the tab bar.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TitleElement {
    pub text: JsString,
    /// The `class` attribute's tokens in order (`tab`, then `tab-hovered` and `tab-selected`
    /// in the order they were last added).
    pub classes: Vec<&'static str>,
}

impl TitleElement {
    fn add(&mut self, class: &'static str) {
        if !self.classes.contains(&class) {
            self.classes.push(class);
        }
    }

    fn remove(&mut self, class: &'static str) {
        self.classes.retain(|c| *c != class);
    }

    pub fn class_name(&self) -> String {
        self.classes.join(" ")
    }

    /// The `innerText` getter: the setter turned line feeds into `<br>`, and each line's
    /// spaces and tabs collapse, with collapsible space at a line's ends removed.
    pub fn inner_text(&self) -> JsString {
        let units = self.text.units();
        let mut out: Vec<u16> = Vec::with_capacity(units.len());
        let mut lines = Vec::new();
        let mut start = 0;
        let mut i = 0;
        while i < units.len() {
            match units[i] {
                0x0d if units.get(i + 1) == Some(&0x0a) => {
                    lines.push(&units[start..i]);
                    i += 2;
                    start = i;
                }
                0x0a | 0x0d => {
                    lines.push(&units[start..i]);
                    i += 1;
                    start = i;
                }
                _ => i += 1,
            }
        }
        lines.push(&units[start..]);
        for (n, line) in lines.iter().enumerate() {
            if n > 0 {
                out.push(0x0a);
            }
            let mut pending_space = false;
            let mut wrote = false;
            for &u in line.iter() {
                if u == 0x20 || u == 0x09 {
                    pending_space = wrote;
                    continue;
                }
                if pending_space {
                    out.push(0x20);
                    pending_space = false;
                }
                out.push(u);
                wrote = true;
            }
        }
        JsString::from_units(out)
    }
}

struct Tab<Cx: ?Sized> {
    type_value: JsValue,
    kind: Option<TabType>,
    title: JsValue,
    element: Rc<RefCell<TitleElement>>,
    content_hidden: bool,
    controller: Box<dyn TabController<Cx>>,
    renderer: Box<dyn TabRenderer<Cx>>,
    uuid: JsValue,
    controls_height: JsValue,
}

/// The mutable halves of one tab and the `controller.UUID` `Tabs` owns: control events
/// that read the UUID (`VideoController`'s source buttons) route through this.
pub struct TabMut<'a, Cx: ?Sized> {
    pub uuid: &'a JsValue,
    pub controller: &'a mut dyn TabController<Cx>,
    pub renderer: &'a mut dyn TabRenderer<Cx>,
}

/// A read-only view of one tab.
pub struct TabRef<'a, Cx: ?Sized>(&'a Tab<Cx>);

impl<'a, Cx: ?Sized> TabRef<'a, Cx> {
    /// The `TabType` member, or `None` for a restored `type` that is none.
    pub fn tab_type(&self) -> Option<TabType> {
        self.0.kind
    }

    pub fn type_value(&self) -> &'a JsValue {
        &self.0.type_value
    }

    /// `title`: `""` for the default title.
    pub fn title(&self) -> &'a JsValue {
        &self.0.title
    }

    pub fn title_element(&self) -> Ref<'a, TitleElement> {
        self.0.element.borrow()
    }

    pub fn uuid(&self) -> &'a JsValue {
        &self.0.uuid
    }

    pub fn controls_height(&self) -> &'a JsValue {
        &self.0.controls_height
    }

    /// Whether this tab's renderer and controls elements are hidden.
    pub fn content_hidden(&self) -> bool {
        self.0.content_hidden
    }

    pub fn controller(&self) -> &'a dyn TabController<Cx> {
        &*self.0.controller
    }

    pub fn renderer(&self) -> &'a dyn TabRenderer<Cx> {
        &*self.0.renderer
    }
}

/// The controls panel as `updateControlsHeight` last left it. The CSS variables are `None`
/// until first set, when hub.css's defaults (`200px`, `1`) apply.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControlsView {
    pub tab_controls_height: Option<f64>,
    pub show_tab_controls: Option<bool>,
    pub handle_hidden: bool,
    pub content_hidden: bool,
}

/// `div.tab-bar-drag-highlight`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragHighlight {
    pub hidden: bool,
    /// `style.left` in px, once set.
    pub left: Option<f64>,
}

/// A tab drag `startDrag` begins: the `#dragItem` contents and the drag data.
#[derive(Clone, Debug, PartialEq)]
pub struct TabDragStart {
    pub x: f64,
    pub y: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    pub text: JsString,
    pub selected: bool,
    pub tab_index: usize,
}

/// The data a tab drag carries (`{tabIndex}`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TabDragData {
    pub tab_index: f64,
}

/// One `update-satellite` message.
#[derive(Clone)]
pub struct SatelliteUpdate {
    pub uuid: JsValue,
    pub command: Command,
    pub title: JsValue,
}

impl fmt::Debug for SatelliteUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SatelliteUpdate")
            .field("uuid", &self.uuid)
            .field("title", &self.title)
            .finish_non_exhaustive()
    }
}

impl PartialEq for SatelliteUpdate {
    fn eq(&self, other: &Self) -> bool {
        self.uuid == other.uuid
            && self.title == other.title
            && Rc::ptr_eq(&self.command, &other.command)
    }
}

impl SatelliteUpdate {
    pub fn to_message(&self) -> NamedMessage<HubToMain> {
        let mut o = JsObject::new();
        o.insert("uuid", self.uuid.clone());
        o.insert("command", self.command.to_js());
        o.insert("title", self.title.clone());
        NamedMessage::new(HubToMain::UpdateSatellite, JsValue::Object(o))
    }
}

/// What one animation frame produced. An `error` ends the frame loop, as an exception in
/// upstream's `periodic` skips its `requestAnimationFrame`.
#[derive(Default)]
pub struct FrameOutput {
    pub satellite_updates: Vec<SatelliteUpdate>,
    pub error: Option<TabsError>,
}

pub struct Tabs<Cx: ?Sized> {
    factory: Box<dyn TabFactory<Cx>>,
    tabs: Vec<Tab<Cx>>,
    selected: JsValue,
    bar: Vec<Rc<RefCell<TitleElement>>>,
    active_satellites: Vec<JsValue>,
    controls_handle_active: bool,
    last_handle_click: f64,
    mouse_down: Option<(f64, f64)>,
    controls: ControlsView,
    show_timeline: Option<bool>,
    drag_highlight: DragHighlight,
    frame_loop_alive: bool,
    messages: Vec<NamedMessage<HubToMain>>,
    bar_sensor: ScrollSensor,
    shadows: [Option<&'static str>; 2],
}

impl<Cx: TabsHost + ?Sized> Tabs<Cx> {
    /// No tabs at all, which only a satellite's page starts with.
    pub(crate) fn empty(factory: Box<dyn TabFactory<Cx>>) -> Self {
        Tabs {
            factory,
            tabs: Vec::new(),
            selected: num(0.0),
            bar: Vec::new(),
            active_satellites: Vec::new(),
            controls_handle_active: false,
            last_handle_click: 0.0,
            mouse_down: None,
            controls: ControlsView::default(),
            show_timeline: None,
            drag_highlight: DragHighlight {
                hidden: true,
                left: None,
            },
            frame_loop_alive: true,
            messages: Vec::new(),
            bar_sensor: ScrollSensor::default(),
            shadows: [None, None],
        }
    }

    /// A satellite's only tab: the renderer for `kind` and no controller.
    pub(crate) fn set_satellite_tab(
        &mut self,
        cx: &mut Cx,
        type_value: &JsValue,
        kind: TabType,
    ) -> Result {
        let parts = self.factory.create(cx, Some(kind))?;
        self.tabs = vec![Tab {
            type_value: type_value.clone(),
            kind: Some(kind),
            title: JsValue::string(""),
            element: Rc::default(),
            content_hidden: false,
            controller: parts.controller,
            renderer: parts.renderer,
            uuid: JsValue::String(parts.uuid),
            controls_height: num(0.0),
        }];
        self.selected = num(0.0);
        Ok(())
    }

    /// The constructor: the default tabs, with Line Graph selected.
    pub fn new(factory: Box<dyn TabFactory<Cx>>, cx: &mut Cx) -> Self {
        let mut tabs = Tabs::empty(factory);
        let start = tabs
            .update_controls_height(cx)
            .and_then(|()| {
                [
                    TabType::Documentation,
                    TabType::LineGraph,
                    TabType::Field2d,
                    TabType::Field3d,
                ]
                .into_iter()
                .try_for_each(|t| tabs.add_tab(cx, &num(t.number())))
            })
            .and_then(|()| tabs.set_selected(cx, &num(1.0)));
        debug_assert!(start.is_ok());
        tabs
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn tab(&self, index: usize) -> Option<TabRef<'_, Cx>> {
        self.tabs.get(index).map(TabRef)
    }

    /// `tabList[i]` mutable, with the UUID `addTab` drew or a restore set. Control events
    /// dispatched through `Tabs` that reach for `this.UUID` use this.
    pub fn tab_mut(&mut self, index: usize) -> Option<TabMut<'_, Cx>> {
        let tab = self.tabs.get_mut(index)?;
        let uuid = &tab.uuid;
        let controller = &mut *tab.controller;
        let renderer = &mut *tab.renderer;
        Some(TabMut {
            uuid,
            controller,
            renderer,
        })
    }

    /// `tabList[selectedTab]` mutable, like `tab_mut`.
    pub fn selected_tab_mut(&mut self) -> Option<TabMut<'_, Cx>> {
        self.selected_index().and_then(|i| self.tab_mut(i))
    }

    /// `tabList[i].controller`, for `eval` steps that reach a tab by index.
    pub fn controller_mut(&mut self, index: &JsValue) -> Result<&mut dyn TabController<Cx>> {
        let i = js::index(index, self.tabs.len()).ok_or_else(|| missing("controller"))?;
        Ok(&mut *self.tabs[i].controller)
    }

    /// `tabList[i].renderer`.
    pub fn renderer_mut(&mut self, index: &JsValue) -> Result<&mut dyn TabRenderer<Cx>> {
        let i = js::index(index, self.tabs.len()).ok_or_else(|| missing("renderer"))?;
        Ok(&mut *self.tabs[i].renderer)
    }

    pub fn tabs(&self) -> impl ExactSizeIterator<Item = TabRef<'_, Cx>> + '_ {
        self.tabs.iter().map(TabRef)
    }

    /// `input` for every tab's source lists, in tab order. Each list's own listener runs
    /// whatever another one threw; their exceptions come back together.
    pub fn source_list_input(
        &mut self,
        cx: &mut Cx,
        input: ListInput<'_>,
    ) -> (ListOutput, Vec<JsError>) {
        let mut errors = Vec::new();
        let mut out = ListOutput::default();
        for tab in &mut self.tabs {
            let o = tab.controller.source_list_input(cx, input, &mut errors);
            out.drag = out.drag.or(o.drag);
            out.edit = out.edit.or(o.edit);
        }
        (out, errors)
    }

    /// `getSelectedTab()`.
    pub fn selected(&self) -> &JsValue {
        &self.selected
    }

    /// The tab `tabList[selectedTab]` reads.
    pub fn selected_index(&self) -> Option<usize> {
        js::index(&self.selected, self.tabs.len())
    }

    /// Whether `index === selectedTab`, which decides the highlighted tab and local rendering.
    pub fn is_selected(&self, index: usize) -> bool {
        js::strict_eq(&js::len(index), &self.selected)
    }

    /// `getSelectedTabType()`.
    pub fn selected_tab_type(&self) -> Result<&JsValue> {
        let i = self.selected_index().ok_or_else(|| missing("type"))?;
        Ok(&self.tabs[i].type_value)
    }

    /// The title elements in `div.tab-bar`, as the last `updateElements` placed them.
    pub fn bar(&self) -> impl ExactSizeIterator<Item = Ref<'_, TitleElement>> + '_ {
        self.bar.iter().map(|e| e.borrow())
    }

    pub fn controls(&self) -> ControlsView {
        self.controls
    }

    /// `--show-timeline` (and `!TIMELINE_CONTAINER.hidden`), `None` until first set.
    pub fn show_timeline(&self) -> Option<bool> {
        self.show_timeline
    }

    pub fn drag_highlight(&self) -> DragHighlight {
        self.drag_highlight
    }

    /// Whether the controls handle is being dragged (`body` cursor `row-resize`).
    pub fn controls_handle_active(&self) -> bool {
        self.controls_handle_active
    }

    pub fn frame_loop_alive(&self) -> bool {
        self.frame_loop_alive
    }

    /// Hub-to-main messages sent since the last call.
    pub fn take_messages(&mut self) -> Vec<NamedMessage<HubToMain>> {
        std::mem::take(&mut self.messages)
    }

    fn lookup_selected(&self, property: &str) -> Result<usize> {
        self.selected_index().ok_or_else(|| missing(property))
    }

    fn last_index(&self) -> JsValue {
        num(self.tabs.len() as f64 - 1.0)
    }

    fn update_controls_height(&mut self, cx: &Cx) -> Result {
        let mut available = cx.inner_height() - cx.renderer_top(self.show_timeline.unwrap_or(true));
        available -= 150.0;
        if js::lt(&self.selected, &num(0.0)) || js::ge(&self.selected, &js::len(self.tabs.len())) {
            return Ok(());
        }
        let i = self.lookup_selected("type")?;
        let tab = &mut self.tabs[i];
        let fixed = fixed_for_value(&tab.type_value);
        let mut applied = math::min2(
            math::max2(js::to_number(&tab.controls_height), 0.0),
            available,
        );
        match fixed {
            Some(height) => applied = if applied > 30.0 { height } else { 0.0 },
            None => {
                if (30.0..100.0).contains(&applied) {
                    applied = 100.0;
                }
                if applied > available {
                    applied = 0.0;
                }
                if applied < 30.0 {
                    applied = 0.0;
                }
            }
        }
        tab.controls_height = num(applied);
        self.controls = ControlsView {
            tab_controls_height: Some(applied),
            show_tab_controls: Some(applied != 0.0),
            handle_hidden: fixed == Some(0.0),
            content_hidden: applied == 0.0,
        };
        Ok(())
    }

    fn update_elements(&mut self, cx: &Cx) -> Result {
        self.bar = self.tabs.iter().map(|t| t.element.clone()).collect();
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            let selected = js::strict_eq(&js::len(i), &self.selected);
            let mut element = tab.element.borrow_mut();
            if selected {
                element.add("tab-selected");
            } else {
                element.remove("tab-selected");
            }
            tab.content_hidden = !selected;
        }
        self.update_timeline_visibility(cx)
    }

    fn update_timeline_visibility(&mut self, cx: &Cx) -> Result {
        let i = self.lookup_selected("controller")?;
        self.show_timeline = Some(self.tabs[i].controller.show_timeline(cx));
        Ok(())
    }

    /// `saveState()`: the `TabsState` value, in upstream's key order.
    pub fn save_state(&self, cx: &Cx) -> JsValue {
        let tabs = self
            .tabs
            .iter()
            .map(|tab| {
                let mut o = JsObject::new();
                o.insert("type", tab.type_value.clone());
                o.insert("title", tab.title.clone());
                o.insert("controller", tab.controller.save_state(cx));
                o.insert("controllerUUID", tab.uuid.clone());
                o.insert("renderer", tab.renderer.save_state(cx));
                o.insert("controlsHeight", tab.controls_height.clone());
                JsValue::Object(o)
            })
            .collect();
        let mut o = JsObject::new();
        o.insert("selected", self.selected.clone());
        o.insert("tabs", JsValue::Array(tabs));
        JsValue::Object(o)
    }

    /// `restoreState(state)` with the raw `tabs` member of a `HubState`. On an error the tabs
    /// rebuilt so far stay, as upstream leaves them.
    pub fn restore_state(&mut self, cx: &mut Cx, state: &JsValue) -> Result {
        self.tabs.clear();
        self.selected = num(0.0);
        let list = match read(state, "tabs")? {
            v @ JsValue::Array(_) => v.into_array().unwrap(),
            v @ (JsValue::Undefined | JsValue::Null) => {
                return Err(type_error(read_error(&v, "forEach")));
            }
            _ => return Err(type_error("state.tabs.forEach is not a function")),
        };
        for (index, tab_state) in list.iter().enumerate() {
            let type_value = read(tab_state, "type")?;
            self.add_tab(cx, &type_value)?;
            let title = get(tab_state, "title");
            if js::truthy(&title) {
                self.rename_tab(&js::len(index), title)?;
            }
            let tab = self
                .tabs
                .get_mut(index)
                .ok_or_else(|| missing("controller"))?;
            tab.uuid = get(tab_state, "controllerUUID");
            tab.controller
                .restore_state(cx, &get(tab_state, "controller"))?;
            tab.renderer
                .restore_state(cx, &get(tab_state, "renderer"))?;
            let controls_height = get(tab_state, "controlsHeight");
            if js::strict_eq(&controls_height, &num(0.0)) || fixed_for_value(&type_value).is_none()
            {
                tab.controls_height = controls_height;
            }
        }
        let selected = get(state, "selected");
        self.selected = if js::ge(&selected, &js::len(self.tabs.len())) {
            self.last_index()
        } else {
            selected
        };
        self.update_elements(cx)?;
        self.update_controls_height(cx)
    }

    pub fn refresh(&mut self, cx: &mut Cx) -> Result {
        for tab in &mut self.tabs {
            tab.controller.refresh(cx)?;
        }
        Ok(())
    }

    pub fn new_assets(&mut self, cx: &mut Cx) -> Result {
        for tab in &mut self.tabs {
            tab.controller.new_assets(cx)?;
        }
        Ok(())
    }

    /// `getActiveFields()`, in the `Set`'s insertion order.
    pub fn active_fields(&self, cx: &Cx, log: &Log) -> Vec<JsString> {
        let mut fields: IndexSet<JsString> = IndexSet::new();
        for tab in &self.tabs {
            fields.extend(tab.controller.active_fields(cx));
        }
        fields.extend(get_enabled_key(log));
        fields.extend(get_autonomous_key(log));
        fields.extend(get_utility_key(log));
        fields.into_iter().collect()
    }

    pub fn toggle_controls_visible(&mut self, cx: &Cx) -> Result {
        let i = self.lookup_selected("type")?;
        let tab = &mut self.tabs[i];
        let fixed = fixed_for_value(&tab.type_value);
        tab.controls_height = if js::strict_eq(&tab.controls_height, &num(0.0)) {
            num(fixed.unwrap_or(DEFAULT_CONTROLS_HEIGHT))
        } else {
            #[allow(clippy::neg_multiply)]
            let flipped = js::to_number(&tab.controls_height) * -1.0;
            num(flipped)
        };
        self.update_controls_height(cx)
    }

    /// `addTab(type)`, with the raw type value.
    pub fn add_tab(&mut self, cx: &mut Cx, type_value: &JsValue) -> Result {
        let metadata = num(TabType::Metadata.number());
        if js::strict_eq(type_value, &metadata)
            && let Some(existing) = self
                .tabs
                .iter()
                .position(|t| js::strict_eq(&t.type_value, &metadata))
        {
            return self.set_selected(cx, &js::len(existing));
        }
        if matches!(type_value, JsValue::Undefined | JsValue::Null) {
            return Err(type_error(read_error(type_value, "toString")));
        }
        let key = js::to_string(type_value);
        let names_template = TabType::ALL
            .iter()
            .any(|t| key == JsString::from(asrs_js::number::to_string(t.number())));
        if !names_template {
            return Err(missing("removeAttribute"));
        }
        let kind = kind_of(type_value);
        let parts = self.factory.create(cx, kind)?;
        let element = Rc::new(RefCell::new(TitleElement {
            text: title_text(kind, &JsValue::string("")),
            classes: vec!["tab"],
        }));
        if self.tabs.is_empty() {
            self.selected = num(-1.0);
        }
        let controls_height = fixed_for_value(type_value).unwrap_or(DEFAULT_CONTROLS_HEIGHT);
        let at = js::splice_start(&js::add(&self.selected, &num(1.0)), self.tabs.len());
        self.tabs.insert(
            at,
            Tab {
                type_value: type_value.clone(),
                kind,
                title: JsValue::string(""),
                element,
                content_hidden: false,
                controller: parts.controller,
                renderer: parts.renderer,
                uuid: JsValue::String(parts.uuid),
                controls_height: num(controls_height),
            },
        );
        self.selected = js::add(&self.selected, &num(1.0));
        self.update_elements(cx)
    }

    /// `newSatellite()`: asks main for a satellite showing the selected tab.
    pub fn new_satellite(&mut self) -> Result {
        if js::ge(&self.selected, &num(0.0)) {
            let i = self.lookup_selected("controller")?;
            let mut o = JsObject::new();
            o.insert("uuid", self.tabs[i].uuid.clone());
            o.insert("type", self.tabs[i].type_value.clone());
            self.messages.push(NamedMessage::new(
                HubToMain::CreateSatellite,
                JsValue::Object(o),
            ));
        }
        Ok(())
    }

    /// `close(index, force)`. A tab whose UUID is `null` matches the unset `activeXRUUID`, so
    /// without `force` upstream asks main to confirm an XR close (out of scope) instead.
    pub fn close(&mut self, cx: &Cx, index: &JsValue, force: &JsValue) -> Result {
        if js::lt(index, &num(1.0)) || js::gt(index, &self.last_index()) {
            return Ok(());
        }
        let force = js::truthy(force);
        let i = js::index(index, self.tabs.len()).ok_or_else(|| {
            missing(if force {
                "rendererElement"
            } else {
                "controller"
            })
        })?;
        if !force && self.tabs[i].uuid.is_null() {
            return Ok(());
        }
        self.tabs.remove(i);
        if js::gt(&self.selected, index) {
            self.selected = num(js::to_number(&self.selected) - 1.0);
        }
        if js::gt(&self.selected, &self.last_index()) {
            self.selected = self.last_index();
        }
        self.update_elements(cx)
    }

    pub fn set_selected(&mut self, cx: &Cx, index: &JsValue) -> Result {
        if js::lt(index, &num(0.0)) || js::gt(index, &self.last_index()) {
            return Ok(());
        }
        self.selected = index.clone();
        self.update_elements(cx)
    }

    /// The `move-tab` message: `setSelected(getSelectedTab() + delta)`.
    pub fn move_selection(&mut self, cx: &Cx, delta: &JsValue) -> Result {
        let index = js::add(&self.selected, delta);
        self.set_selected(cx, &index)
    }

    pub fn shift(&mut self, cx: &Cx, index: &JsValue, shift: &JsValue) -> Result {
        let min_index = num(1.0);
        if js::lt(index, &min_index) {
            return Ok(());
        }
        let mut shift = shift.clone();
        if js::lt(&js::add(index, &shift), &min_index) {
            shift = num(1.0 - js::to_number(index));
        }
        if js::gt(&js::add(index, &shift), &self.last_index()) {
            shift = num(self.tabs.len() as f64 - 1.0 - js::to_number(index));
        }
        let offset = num(js::to_number(&self.selected) - js::to_number(index));
        if js::strict_eq(&self.selected, index) {
            self.selected = js::add(&self.selected, &shift);
        } else if js::gt(index, &self.selected) && js::le(&shift, &offset) {
            self.selected = num(js::to_number(&self.selected) + 1.0);
        } else if js::lt(index, &self.selected) && js::ge(&shift, &offset) {
            self.selected = num(js::to_number(&self.selected) - 1.0);
        }
        let start = js::splice_start(index, self.tabs.len());
        if start >= self.tabs.len() {
            return Err(TabsError::Unmodeled(
                "shift spliced undefined into the tab list",
            ));
        }
        let tab = self.tabs.remove(start);
        let at = js::splice_start(&js::add(index, &shift), self.tabs.len());
        self.tabs.insert(at, tab);
        self.update_elements(cx)
    }

    /// The `shift-tab` message: `shift(getSelectedTab(), delta)`.
    pub fn shift_selected(&mut self, cx: &Cx, delta: &JsValue) -> Result {
        let index = self.selected.clone();
        self.shift(cx, &index, delta)
    }

    pub fn set_active_satellites(&mut self, uuids: Vec<JsValue>) {
        self.active_satellites = uuids;
    }

    /// The last `set-active-satellites`.
    pub fn active_satellites(&self) -> &[JsValue] {
        &self.active_satellites
    }

    pub fn is_valid_uuid(&self, uuid: &JsValue) -> bool {
        self.tabs.iter().any(|t| js::strict_eq(uuid, &t.uuid))
    }

    /// `renameTab(index, name)`. A name equal to the default title is stored as `""`.
    pub fn rename_tab(&mut self, index: &JsValue, name: JsValue) -> Result {
        let i = js::index(index, self.tabs.len()).ok_or_else(|| missing("type"))?;
        let tab = &mut self.tabs[i];
        let name = if js::strict_eq(&name, &JsValue::string(default_title(tab.kind))) {
            JsValue::string("")
        } else {
            name
        };
        tab.element.borrow_mut().text = title_text(tab.kind, &name);
        tab.title = name;
        Ok(())
    }

    /// The controller of the selected tab, when that tab is a `kind` tab (`editAxis`,
    /// `clearAxis`, `setRobotModeVisible`, `isUnlockedVideoSelected`).
    pub fn selected_controller_as<T: 'static>(&mut self, kind: TabType) -> Result<Option<&mut T>> {
        let i = self.lookup_selected("type")?;
        let tab = &mut self.tabs[i];
        Ok(if tab.kind == Some(kind) {
            tab.controller.downcast_mut::<T>()
        } else {
            None
        })
    }

    /// The renderer of the selected tab, when that tab is a `kind` tab (`set3DCamera`,
    /// `set2DCamera`, `setFov`).
    pub fn selected_renderer_as<T: 'static>(&mut self, kind: TabType) -> Result<Option<&mut T>> {
        let i = self.lookup_selected("type")?;
        let tab = &mut self.tabs[i];
        Ok(if tab.kind == Some(kind) {
            tab.renderer.downcast_mut::<T>()
        } else {
            None
        })
    }

    /// The selected tab's renderer when that tab is a `kind` tab, as `Tabs.set3DCamera` and
    /// `Tabs.setFov` reach it.
    pub fn selected_renderer(&mut self, kind: TabType) -> Result<Option<&mut dyn TabRenderer<Cx>>> {
        let i = self.lookup_selected("type")?;
        let tab = &mut self.tabs[i];
        Ok(if tab.kind == Some(kind) {
            Some(tab.renderer.as_mut())
        } else {
            None
        })
    }

    /// Every `kind` tab's `(controllerUUID, controller)` pair, in tab order
    /// (`processVideoData` filters on `data.uuid !== this.UUID`).
    pub fn controllers_with_uuid<T: 'static>(
        &mut self,
        kind: TabType,
    ) -> impl Iterator<Item = (&JsValue, &mut T)> + '_ {
        self.tabs
            .iter_mut()
            .filter(move |t| t.kind == Some(kind))
            .filter_map(|t| {
                let uuid = &t.uuid;
                t.controller
                    .downcast_mut::<T>()
                    .map(|controller| (uuid, controller))
            })
    }

    /// `addTableRange(controllerUUID, rendererUUID, range)`. Upstream hands the range to the
    /// selected tab's controller once per matching Table tab, and throws when the selected tab
    /// is no Table.
    pub fn add_table_range(
        &mut self,
        cx: &mut Cx,
        controller_uuid: &JsValue,
        renderer_uuid: &JsString,
        range: Option<[f64; 2]>,
    ) -> Result {
        for i in 0..self.tabs.len() {
            let tab = &self.tabs[i];
            if tab.kind == Some(TabType::Table) && js::strict_eq(&tab.uuid, controller_uuid) {
                let s = self.lookup_selected("controller")?;
                if self.tabs[s].kind != Some(TabType::Table) {
                    return Err(type_error(
                        "this.tabList[this.selectedTab].controller.addRendererRange is not a function",
                    ));
                }
                self.tabs[s]
                    .controller
                    .add_renderer_range(cx, renderer_uuid, range)?;
            }
        }
        Ok(())
    }

    /// A wheel event over `div.tab-bar-scroll` at `now` (ms).
    pub fn tab_bar_wheel(&mut self, wheel: Wheel, now: f64) {
        self.bar_sensor.wheel(wheel, now);
    }

    /// The tab bar's scroll sensor, for hosts that set its layout or watch its animation.
    pub fn tab_bar_sensor_mut(&mut self) -> &mut ScrollSensor {
        &mut self.bar_sensor
    }

    pub fn tab_bar_sensor(&self) -> &ScrollSensor {
        &self.bar_sensor
    }

    /// `style.opacity` of `div.tab-bar-shadow-left` and `-right`, `None` until first set.
    pub fn shadows(&self) -> [Option<&'static str>; 2] {
        self.shadows
    }

    /// The frame's scroll step for the tab bar's overlay, which runs before the frame's
    /// animation callbacks and keeps running after the frame loop dies: its `scroll` event
    /// moves `div.tab-bar` by the scroll sensor's `dx + dy`.
    pub fn scroll_step(&mut self, cx: &Cx, now: f64) {
        self.bar_sensor.animate(now);
        if let Some(d) = self.bar_sensor.scroll_step(now, true) {
            let left = cx.tab_bar_scroll_left();
            cx.set_tab_bar_scroll_left(left + (d.dx + d.dy));
        }
    }

    /// One pass of upstream's `periodic` at `now` (ms), except the timeline's: the tab bar
    /// shadows and scroll sensor, the controls height, the timeline visibility, then a command
    /// for every tab shown in the hub, a satellite or XR. Once a frame fails the loop is dead
    /// and this does nothing.
    pub fn periodic(&mut self, cx: &mut Cx, now: f64) -> FrameOutput {
        let mut out = FrameOutput::default();
        if !self.frame_loop_alive {
            return out;
        }
        let left = cx.tab_bar_scroll_left();
        self.shadows[0] = Some(if math::floor(left) <= 0.0 { "0" } else { "1" });
        let left = cx.tab_bar_scroll_left();
        let max = cx.tab_bar_scroll_width() - cx.tab_bar_client_width();
        self.shadows[1] = Some(if math::ceil(left) >= max { "0" } else { "1" });
        self.bar_sensor.periodic(now);
        if let Err(e) = self.frame(cx, &mut out.satellite_updates) {
            self.frame_loop_alive = false;
            out.error = Some(e);
        }
        out
    }

    fn title_rect(&self, cx: &Cx, index: usize) -> Rect {
        let element = &self.tabs[index].element;
        let position = self
            .bar
            .iter()
            .position(|e| Rc::ptr_eq(e, element))
            .unwrap_or(index);
        cx.title_rect(position)
    }

    fn frame(&mut self, cx: &mut Cx, updates: &mut Vec<SatelliteUpdate>) -> Result {
        self.update_controls_height(cx)?;
        self.update_timeline_visibility(cx)?;
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            let active_local = js::strict_eq(&js::len(i), &self.selected);
            let active_satellite = self
                .active_satellites
                .iter()
                .any(|u| js::same_value_zero(u, &tab.uuid));
            let active_xr = tab.uuid.is_null();
            if !(active_local || active_satellite || active_xr) {
                continue;
            }
            if tab.kind == Some(TabType::Table) {
                let renderer_uuid = tab.renderer.renderer_uuid().cloned().unwrap_or_default();
                let range = if active_local {
                    tab.renderer.timestamp_range(cx)
                } else {
                    None
                };
                tab.controller
                    .add_renderer_range(cx, &renderer_uuid, range)?;
            }
            let command = tab.controller.command(cx, &tab.uuid)?;
            if active_local {
                tab.renderer.render(cx, &*command)?;
            }
            if active_satellite {
                let title = if tab.kind == Some(TabType::Documentation) {
                    JsValue::string("Documentation")
                } else if js::strict_eq(&tab.title, &JsValue::string("")) {
                    JsValue::string(default_title(tab.kind))
                } else {
                    tab.title.clone()
                };
                updates.push(SatelliteUpdate {
                    uuid: tab.uuid.clone(),
                    command,
                    title,
                });
            }
        }
        Ok(())
    }

    /// `mousedown` on `div.tab-bar-scroll`.
    pub fn overlay_mouse_down(&mut self, x: f64, y: f64) {
        self.mouse_down = Some((x, y));
    }

    /// `mouseup` on `div.tab-bar-scroll`: a click selects every tab under the pointer in turn.
    pub fn overlay_mouse_up(&mut self, cx: &Cx, x: f64, y: f64) -> Result {
        let Some((down_x, down_y)) = self.mouse_down else {
            return Ok(());
        };
        if math::abs(x - down_x) < TAB_DRAG_THRESHOLD_PX
            && math::abs(y - down_y) < TAB_DRAG_THRESHOLD_PX
        {
            for index in 0..self.tabs.len() {
                if contains(&self.title_rect(cx, index), x, y) {
                    self.set_selected(cx, &js::len(index))?;
                }
            }
        }
        self.mouse_down = None;
        Ok(())
    }

    /// `mousemove` on `div.tab-bar-scroll`: hover classes, then a drag once the pointer has
    /// moved past the threshold from a press on a tab other than the first.
    pub fn overlay_mouse_move(&mut self, cx: &Cx, x: f64, y: f64) -> Option<TabDragStart> {
        for index in 0..self.tabs.len() {
            let hovered = contains(&self.title_rect(cx, index), x, y);
            let mut element = self.tabs[index].element.borrow_mut();
            if hovered {
                element.add("tab-hovered");
            } else {
                element.remove("tab-hovered");
            }
        }
        let (down_x, down_y) = self.mouse_down?;
        if math::abs(x - down_x) < TAB_DRAG_THRESHOLD_PX
            && math::abs(y - down_y) < TAB_DRAG_THRESHOLD_PX
        {
            return None;
        }
        let mut tab_index = 0;
        for index in 0..self.tabs.len() {
            if contains(&self.title_rect(cx, index), x, y) {
                tab_index = index;
            }
        }
        if tab_index == 0 {
            return None;
        }
        let text = self.tabs[tab_index].element.borrow().inner_text();
        let rect = self.title_rect(cx, tab_index);
        self.mouse_down = None;
        Some(TabDragStart {
            x,
            y,
            offset_x: x - rect.left(),
            offset_y: y - rect.top(),
            text,
            selected: self.is_selected(tab_index),
            tab_index,
        })
    }

    /// `mouseout` on `div.tab-bar-scroll`.
    pub fn overlay_mouse_out(&mut self) {
        for tab in &self.tabs {
            tab.element.borrow_mut().remove("tab-hovered");
        }
    }

    /// `contextmenu` on `div.tab-bar-scroll`: `ask-rename-tab` for every tab under the pointer
    /// except the first.
    pub fn overlay_context_menu(&mut self, cx: &Cx, x: f64, y: f64) {
        self.mouse_down = None;
        for (index, tab) in self.tabs.iter().enumerate().skip(1) {
            let rect = self.title_rect(cx, index);
            if !contains(&rect, x, y) {
                continue;
            }
            let name = if js::strict_eq(&tab.title, &JsValue::string("")) {
                JsValue::string(default_title(tab.kind))
            } else {
                tab.title.clone()
            };
            let mut o = JsObject::new();
            o.insert("index", js::len(index));
            o.insert("name", name);
            o.insert("rect", rect_js(&rect));
            self.messages.push(NamedMessage::new(
                HubToMain::AskRenameTab,
                JsValue::Object(o),
            ));
        }
    }

    /// `keydown` on the title element at `bar_index` in the tab bar. Returns whether upstream
    /// calls `preventDefault`.
    pub fn title_key_down(&mut self, cx: &Cx, bar_index: usize, key: &str) -> Result<bool> {
        if key != "Enter" && key != " " {
            return Ok(false);
        }
        if let Some(element) = self.bar.get(bar_index).cloned()
            && let Some(index) = self
                .tabs
                .iter()
                .position(|t| Rc::ptr_eq(&t.element, &element))
        {
            self.set_selected(cx, &js::len(index))?;
        }
        Ok(true)
    }

    /// `mousedown` on `div.controls-handle`.
    pub fn controls_handle_mouse_down(&mut self) {
        self.controls_handle_active = true;
    }

    /// `mouseup` on the window.
    pub fn window_mouse_up(&mut self) {
        self.controls_handle_active = false;
    }

    /// `mousemove` on the window: resizes the controls while the handle is held.
    pub fn window_mouse_move(&mut self, cx: &Cx, y: f64) -> Result {
        if !self.controls_handle_active {
            return Ok(());
        }
        let i = self.selected_index().ok_or_else(|| {
            type_error("Cannot set properties of undefined (setting 'controlsHeight')")
        })?;
        self.tabs[i].controls_height = num(cx.inner_height() - y);
        self.update_controls_height(cx)
    }

    /// `click` on `div.controls-handle` at time `now` (ms): a second click within 400 ms
    /// toggles the controls.
    pub fn controls_handle_click(&mut self, cx: &Cx, now: f64) -> Result {
        if now - self.last_handle_click < CONTROLS_DOUBLE_CLICK_MS {
            self.toggle_controls_visible(cx)?;
            self.last_handle_click = 0.0;
        } else {
            self.last_handle_click = now;
        }
        Ok(())
    }

    /// The close button.
    pub fn close_button(&mut self, cx: &Cx) -> Result {
        let index = self.selected.clone();
        self.close(cx, &index, &JsValue::Bool(false))
    }

    /// The pop-out button.
    pub fn popup_button(&mut self) -> Result {
        self.new_satellite()
    }

    /// The add button.
    pub fn add_button(&mut self, cx: &Cx) {
        let rect = cx.add_button_rect();
        self.messages
            .push(NamedMessage::new(HubToMain::AskNewTab, rect_js(&rect)));
    }

    /// A `drag-update` event for a tab drag started by `overlay_mouse_move`.
    pub fn drag_update(&mut self, cx: &Cx, update: &DragUpdate<TabDragData>) -> Result {
        let (x, y, end, tab_index) = (update.x, update.y, update.end, update.data.tab_index);
        if y > cx.overlay_rect().bottom() + 100.0 {
            self.drag_highlight.hidden = true;
            return Ok(());
        }
        let mut closest_dist = f64::INFINITY;
        let mut closest = 0usize;
        for index in 0..self.tabs.len() {
            let dist = math::abs(x - self.title_rect(cx, index).right());
            if dist < closest_dist {
                closest_dist = dist;
                closest = index;
            }
        }
        let closest_f = closest as f64;
        if end {
            self.drag_highlight.hidden = true;
            let shift = if closest_f >= tab_index {
                closest_f - tab_index
            } else {
                closest_f - tab_index + 1.0
            };
            self.shift(cx, &num(tab_index), &num(shift))
        } else {
            self.drag_highlight.hidden = false;
            if closest >= self.tabs.len() {
                return Err(missing("titleElement"));
            }
            let right = self.title_rect(cx, closest).right();
            self.drag_highlight.left = Some(right - cx.overlay_rect().left() + 10.0);
            Ok(())
        }
    }
}
