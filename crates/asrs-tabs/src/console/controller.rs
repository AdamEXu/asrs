//! `src/hub/controllers/ConsoleController.ts` and `src/shared/renderers/ConsoleRenderer.ts`. Line
//! formatting and filtering are [`super::format_value`] and [`super::line_visible`].

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::ops;
use asrs_js::sort::array_sort;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::log_util::LogFieldIterator;
use asrs_log::log::{Log, LoggableType};
use asrs_log::util::upper_bound;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::Timestamps;
use asrs_model::selection::SelectionMode;
use asrs_model::source_list::Rect;
use asrs_model::tab_controller::{Command, TabCommand, TabController, TabRenderer};

use super::view::{ConsoleRow, ConsoleView};
use super::{EXPORT_ERROR, export_text, format_value, line_visible};
use crate::cx::{TabCx, display_offset, display_time};
use crate::dom_tree::inner_text;
use crate::form::NumberInput;

const CONSOLE_TYPES: [&str; 4] = [
    "Console",
    "ConsoleLineTimestamp",
    "ErrorInfoTimestamp",
    "ProgramCrashInfoTimestamp",
];

/// The DOM facts of the shared renderer element.
#[derive(Clone, Debug, Default)]
pub struct ConsoleDom {
    /// `ROOT.hidden`.
    pub hidden: bool,
    /// `TABLE_CONTAINER.getBoundingClientRect()`.
    pub container: Rect,
    /// `TABLE_CONTAINER.scrollTop`, persisted across frames.
    pub scroll_top: f64,
    /// `TABLE_CONTAINER.scrollHeight`.
    pub scroll_height: f64,
    /// `DRAG_HIGHLIGHT.hidden`.
    pub drag_highlight_hidden: bool,
    /// `JUMP_INPUT`, an `<input type="number">`.
    pub jump_input: NumberInput,
    /// `clientHeight` of every row after the header.
    pub row_heights: Vec<f64>,
}

/// One entry of the command's `lines` array.
#[derive(Clone, Debug, PartialEq)]
pub struct ConsoleLine {
    pub timestamp: f64,
    pub value: JsString,
    pub is_error: bool,
    pub is_warning: bool,
}

impl ToJs for ConsoleLine {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("timestamp", JsValue::Number(self.timestamp));
        o.insert("value", JsValue::String(self.value.clone()));
        o.insert("isError", JsValue::Bool(self.is_error));
        o.insert("isWarning", JsValue::Bool(self.is_warning));
        JsValue::Object(o)
    }
}

/// `ConsoleRendererCommand`.
#[derive(Clone, Debug)]
pub struct ConsoleCommand {
    pub key: JsValue,
    pub key_available: bool,
    pub lines: Vec<ConsoleLine>,
    pub display_offset: f64,
    pub selection_mode: SelectionMode,
    pub selected_time: Option<f64>,
    pub hovered_time: Option<f64>,
}

impl ToJs for ConsoleCommand {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("key", self.key.clone());
        o.insert("keyAvailable", JsValue::Bool(self.key_available));
        o.insert(
            "lines",
            JsValue::Array(self.lines.iter().map(ToJs::to_js).collect()),
        );
        o.insert("displayOffset", JsValue::Number(self.display_offset));
        o.insert("selectionMode", ToJs::to_js(&self.selection_mode));
        o.insert(
            "selectedTime",
            self.selected_time.map_or(JsValue::Null, JsValue::Number),
        );
        o.insert(
            "hoveredTime",
            self.hovered_time.map_or(JsValue::Null, JsValue::Number),
        );
        JsValue::Object(o)
    }
}

fn is_type(structured: Option<&JsString>, name: &str) -> bool {
    structured.is_some_and(|s| *s == JsString::from(name))
}

fn concat_key(key: &JsString, suffix: &str) -> JsString {
    let mut out = key.clone();
    out.push_str(&JsString::from(suffix));
    out
}

fn console_children(field: &JsString) -> Vec<JsString> {
    ["/ConsoleLine", "/ErrorInfo", "/ProgramCrashInfo"]
        .map(|suffix| concat_key(field, suffix))
        .to_vec()
}

pub struct ConsoleController {
    dom: Rc<RefCell<ConsoleDom>>,
    geometry: Option<Box<dyn ConsoleGeometry>>,
    field: JsValue,
}

impl ConsoleController {
    /// `new ConsoleController(element)`. The view shares `dom` with the matching
    /// `ConsoleRenderer`.
    pub fn new(dom: Rc<RefCell<ConsoleDom>>) -> Self {
        Self {
            dom,
            geometry: None,
            field: JsValue::Null,
        }
    }

    /// Answers the controller's layout read from `geometry` instead of the shared `ConsoleDom`.
    pub fn set_geometry(&mut self, geometry: Box<dyn ConsoleGeometry>) {
        self.geometry = Some(geometry);
    }

    /// The `close-field` event on `ROOT`.
    pub fn close_field(&mut self) {
        self.field = JsValue::Null;
    }

    /// The `drag-update` handler; `detail` is the event's `detail`.
    pub fn handle_drag(
        &mut self,
        cx: &mut (impl TabCx + ?Sized),
        detail: &JsValue,
    ) -> Result<(), JsError> {
        if self.dom.borrow().hidden {
            return Ok(());
        }
        let data = ops::get(detail, "data")?;
        if !ops::has(&data, "fields")? {
            return Ok(());
        }
        let x = ops::value_to_number(&ops::get(detail, "x")?).unwrap_or(f64::NAN);
        let y = ops::value_to_number(&ops::get(detail, "y")?).unwrap_or(f64::NAN);
        let rect = match self.geometry.as_deref_mut() {
            Some(geometry) => geometry.container_rect(),
            None => self.dom.borrow_mut().container_rect(),
        };
        let active = x > rect.left() && x < rect.right() && y > rect.top() && y < rect.bottom();
        let first = ops::index(&ops::get(&data, "fields")?, 0)?;
        let valid_type = match &first {
            JsValue::String(key) => {
                let log = cx.tab_log();
                log.get_type(key) == Some(LoggableType::String)
                    || CONSOLE_TYPES
                        .iter()
                        .any(|t| is_type(log.get_structured_type(key), t))
            }
            _ => false,
        };
        let mut dom = self.dom.borrow_mut();
        dom.drag_highlight_hidden = true;
        if active && valid_type {
            if ops::to_boolean(&ops::get(detail, "end")?) {
                self.field = first;
            } else {
                dom.drag_highlight_hidden = false;
            }
        }
        Ok(())
    }

    fn lines(field: &JsString, log: &Log) -> Vec<ConsoleLine> {
        let mut lines = Vec::new();
        if log.get_type(field) == Some(LoggableType::String) {
            if let Some(data) = log.get_string(field, f64::NEG_INFINITY, f64::INFINITY, None, None)
            {
                for (timestamp, value) in data.timestamps.iter().zip(&data.values) {
                    let lower = value.to_lower_case();
                    lines.push(ConsoleLine {
                        timestamp: *timestamp,
                        value: value.clone(),
                        is_error: lower.includes(&JsString::from("error"), None),
                        is_warning: lower.includes(&JsString::from("warning"), None),
                    });
                }
            }
        } else {
            let structured = log.get_structured_type(field);
            let keys = if is_type(structured, "Console") {
                console_children(field)
            } else if CONSOLE_TYPES[1..].iter().any(|t| is_type(structured, t)) {
                vec![field.clone()]
            } else {
                Vec::new()
            };
            for key in keys {
                let Some(raw) = log.get_raw(&key, f64::NEG_INFINITY, f64::INFINITY, None, None)
                else {
                    continue;
                };
                let key_type = log.get_structured_type(&key);
                if is_type(key_type, "ConsoleLineTimestamp") {
                    let data = log.get_string(
                        &concat_key(&key, "/ConsoleLine"),
                        f64::NEG_INFINITY,
                        f64::INFINITY,
                        None,
                        None,
                    );
                    let mut iter = LogFieldIterator::new(data.as_ref());
                    for t in &raw.timestamps {
                        if let Some(value) = iter.get_at_time(*t) {
                            lines.push(ConsoleLine {
                                timestamp: *t,
                                value: value.clone(),
                                is_error: false,
                                is_warning: false,
                            });
                        }
                    }
                } else if is_type(key_type, "ErrorInfoTimestamp") {
                    error_lines(log, &key, "/ErrorInfo/", &raw.timestamps, true, &mut lines);
                } else if is_type(key_type, "ProgramCrashInfoTimestamp") {
                    error_lines(
                        log,
                        &key,
                        "/ProgramCrashInfo/",
                        &raw.timestamps,
                        false,
                        &mut lines,
                    );
                }
            }
        }
        array_sort(&mut lines, |a, b| a.timestamp - b.timestamp);
        lines
    }
}

/// The `ErrorInfoTimestamp` and `ProgramCrashInfoTimestamp` branches: crash lines have no
/// `IsError` child and are always errors.
fn error_lines(
    log: &Log,
    key: &JsString,
    prefix: &str,
    timestamps: &[f64],
    reads_is_error: bool,
    lines: &mut Vec<ConsoleLine>,
) {
    let child = |name: &str| concat_key(key, &format!("{prefix}{name}"));
    let string =
        |name: &str| log.get_string(&child(name), f64::NEG_INFINITY, f64::INFINITY, None, None);
    let details_data = string("Details");
    let location_data = string("Location");
    let call_stack_data = string("CallStack");
    let is_error_data = reads_is_error
        .then(|| {
            log.get_boolean(
                &child("IsError"),
                f64::NEG_INFINITY,
                f64::INFINITY,
                None,
                None,
            )
        })
        .flatten();
    let mut details = LogFieldIterator::new(details_data.as_ref());
    let mut location = LogFieldIterator::new(location_data.as_ref());
    let mut call_stack = LogFieldIterator::new(call_stack_data.as_ref());
    let mut is_error = LogFieldIterator::new(is_error_data.as_ref());
    for t in timestamps {
        let mut value = details.get_at_time(*t).cloned().unwrap_or_default();
        let location = location.get_at_time(*t).cloned().unwrap_or_default();
        let call_stack = call_stack.get_at_time(*t).cloned().unwrap_or_default();
        if !location.is_empty() || !call_stack.is_empty() {
            value.push_str(&JsString::from("\n"));
            value.push_str(&location);
            value.push_str(&JsString::from("\n"));
            value.push_str(&call_stack);
        }
        let is_error = !reads_is_error || is_error.get_at_time(*t).copied().unwrap_or(false);
        lines.push(ConsoleLine {
            timestamp: *t,
            value,
            is_error,
            is_warning: !is_error,
        });
    }
}

impl<Cx: TabCx + ?Sized> TabController<Cx> for ConsoleController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        self.field.clone()
    }

    fn restore_state(&mut self, _cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        if matches!(state, JsValue::String(_) | JsValue::Null) {
            self.field = state.clone();
        }
        Ok(())
    }

    fn refresh(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn new_assets(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn active_fields(&self, cx: &Cx) -> Vec<JsString> {
        let JsValue::String(field) = &self.field else {
            return Vec::new();
        };
        if is_type(cx.tab_log().get_structured_type(field), "Console") {
            console_children(field)
        } else {
            vec![field.clone()]
        }
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        false
    }

    fn command(&mut self, cx: &mut Cx, _uuid: &JsValue) -> Result<Command, JsError> {
        let (key_available, lines) = {
            let log = cx.tab_log();
            match &self.field {
                JsValue::String(field) if log.get_field_keys().contains(field) => {
                    let lines = if field.is_empty() {
                        Vec::new()
                    } else {
                        Self::lines(field, &log)
                    };
                    (true, lines)
                }
                _ => (false, Vec::new()),
            }
        };
        let offset = display_offset(cx);
        let selection = cx.tab_selection();
        Ok(Rc::new(ConsoleCommand {
            key: self.field.clone(),
            key_available,
            lines,
            display_offset: offset,
            selection_mode: selection.get_mode(),
            selected_time: selection.get_selected_time(),
            hovered_time: selection.get_hovered_time(),
        }))
    }
}

/// The layout reads `ConsoleController` and `ConsoleRenderer` make and the scroll position the
/// renderer writes, one call per upstream access and in upstream's order.
pub trait ConsoleGeometry {
    /// `TABLE_CONTAINER.getBoundingClientRect()`.
    fn container_rect(&mut self) -> Rect;
    /// `TABLE_CONTAINER.scrollTop`.
    fn scroll_top(&mut self) -> f64;
    fn set_scroll_top(&mut self, value: f64);
    /// `TABLE_CONTAINER.scrollHeight`.
    fn scroll_height(&mut self) -> f64;
    /// `TABLE_CONTAINER.clientHeight`.
    fn container_client_height(&mut self) -> f64;
    /// `TABLE_BODY.children[index + 1].clientHeight`.
    fn row_client_height(&mut self, index: usize) -> f64;
}

impl ConsoleGeometry for ConsoleDom {
    fn container_rect(&mut self) -> Rect {
        self.container
    }

    fn scroll_top(&mut self) -> f64 {
        self.scroll_top
    }

    fn set_scroll_top(&mut self, value: f64) {
        self.scroll_top = value;
    }

    fn scroll_height(&mut self) -> f64 {
        self.scroll_height
    }

    fn container_client_height(&mut self) -> f64 {
        asrs_js::math::round(self.container.height)
    }

    fn row_client_height(&mut self, index: usize) -> f64 {
        self.row_heights.get(index).copied().unwrap_or(0.0)
    }
}

pub struct ConsoleRenderer {
    dom: Rc<RefCell<ConsoleDom>>,
    geometry: Option<Box<dyn ConsoleGeometry>>,
    view: ConsoleView,
    /// `FILTER_INPUT.value`.
    filter: JsString,
    key: JsValue,
    key_available: bool,
    lines: Vec<ConsoleLine>,
    rendered_timestamps: Vec<f64>,
    /// Never shortened: upstream's array keeps the values of rows a filter removed, and a row
    /// added later is pushed behind them.
    rendered_values: Vec<Rc<JsString>>,
    rendered_raw_values: Vec<JsString>,
    last_filter: JsString,
    last_scroll_position: Option<f64>,
    selection_mode: SelectionMode,
    selected_time: Option<f64>,
    hovered_time: Option<f64>,
    last_display_offset: Option<f64>,
    last_original: Option<bool>,
    display_offset: f64,
}

impl ConsoleRenderer {
    /// `new ConsoleRenderer(element, hasController)`.
    pub fn new(dom: Rc<RefCell<ConsoleDom>>, has_controller: bool) -> Self {
        Self {
            dom,
            geometry: None,
            view: ConsoleView::new(has_controller),
            filter: JsString::new(),
            key: JsValue::Null,
            key_available: false,
            lines: Vec::new(),
            rendered_timestamps: Vec::new(),
            rendered_values: Vec::new(),
            rendered_raw_values: Vec::new(),
            last_filter: JsString::new(),
            last_scroll_position: None,
            selection_mode: SelectionMode::Idle,
            selected_time: None,
            hovered_time: None,
            last_display_offset: None,
            last_original: None,
            display_offset: 0.0,
        }
    }

    pub fn dom(&self) -> &Rc<RefCell<ConsoleDom>> {
        &self.dom
    }

    pub fn view(&self) -> &ConsoleView {
        &self.view
    }

    /// `FILTER_INPUT.value`.
    pub fn filter(&self) -> &JsString {
        &self.filter
    }

    /// Answers the renderer's layout reads from `geometry` instead of the shared `ConsoleDom`.
    pub fn set_geometry(&mut self, geometry: Box<dyn ConsoleGeometry>) {
        self.geometry = Some(geometry);
    }

    fn geometry<R>(&mut self, f: impl FnOnce(&mut dyn ConsoleGeometry) -> R) -> R {
        match self.geometry.as_deref_mut() {
            Some(geometry) => f(geometry),
            None => f(&mut *self.dom.borrow_mut()),
        }
    }

    /// The `HIGHLIGHT_BUTTON` click.
    pub fn highlight_click(&mut self, cx: &(impl TabCx + ?Sized)) -> Result<(), JsError> {
        self.view.highlight_active = !self.view.highlight_active;
        self.update_data(cx)
    }

    /// `FILTER_INPUT`'s `input` event with `value` in the input, which strips line breaks from
    /// what it is given.
    pub fn filter_input(
        &mut self,
        cx: &(impl TabCx + ?Sized),
        value: &JsString,
    ) -> Result<(), JsError> {
        let units = value.units().iter().copied();
        self.filter = JsString::from_units(units.filter(|c| !matches!(c, 0x0a | 0x0d)).collect());
        self.update_data(cx)
    }

    /// Row `index`'s `mouseenter`.
    pub fn row_mouse_enter(&self, cx: &mut (impl TabCx + ?Sized), index: usize) {
        let time = self.rendered_timestamps.get(index).copied();
        cx.tab_selection().set_hovered_time(time);
    }

    /// A row's `mouseleave`.
    pub fn row_mouse_leave(&self, cx: &mut (impl TabCx + ?Sized)) {
        cx.tab_selection().set_hovered_time(None);
    }

    /// Row `index`'s `click`.
    pub fn row_click(&self, cx: &mut (impl TabCx + ?Sized), index: usize) {
        if let Some(time) = self.rendered_timestamps.get(index) {
            cx.tab_selection().set_selected_time(*time);
        }
    }

    /// A row's `contextmenu`.
    pub fn row_context_menu(&self, cx: &mut (impl TabCx + ?Sized)) {
        cx.tab_selection().go_idle();
    }

    /// The jump input's Enter handler and jump-button click. The target row is found among all
    /// lines, also the ones the filter hides.
    pub fn jump(&mut self) {
        let value = self.dom.borrow().jump_input.value().clone();
        let target = if !value.is_empty() {
            asrs_js::number::to_number(value.units()) - self.display_offset
        } else if self.selection_mode != SelectionMode::Idle {
            self.selected_time.unwrap_or(0.0)
        } else {
            0.0
        };
        let row = self
            .lines
            .iter()
            .position(|line| line.timestamp > target)
            .unwrap_or(self.lines.len());
        let above = row.saturating_sub(1).min(self.view.rows.len());
        self.geometry(|g| {
            let height = (0..above).fold(0.0, |sum, index| sum + g.row_client_height(index));
            g.set_scroll_top(height);
        });
    }

    /// The `EXPORT_BUTTON` click.
    pub fn export_click(&self, cx: &mut (impl TabCx + ?Sized)) {
        let message = match export_text(self.lines.iter().map(|line| &line.value)) {
            Some(text) => NamedMessage::new(HubToMain::ExportConsole, JsValue::String(text)),
            None => NamedMessage::new(
                HubToMain::Error,
                ops::object([
                    ("title", JsValue::string(EXPORT_ERROR.0)),
                    ("content", JsValue::string(EXPORT_ERROR.1)),
                ]),
            ),
        };
        cx.send_main_message(message);
    }

    /// `updateData()`.
    fn update_data(&mut self, cx: &(impl TabCx + ?Sized)) -> Result<(), JsError> {
        let has_controller = self.view.has_controller;
        self.view.field_text = match &self.key {
            JsValue::Null => JsString::new(),
            key => inner_text(&ops::to_string(key)),
        };
        let has_key = !matches!(self.key, JsValue::Null);
        self.view.field_line_through = has_key && !self.key_available;
        self.view.field_delete_hidden = !(has_key && has_controller);
        self.view.show_hand = has_controller && !self.key_available;

        let filter = self.filter.to_lower_case();
        let lines: Vec<&ConsoleLine> = self
            .lines
            .iter()
            .filter(|line| line_visible(&line.value, &self.filter))
            .collect();

        let rows = &mut self.view.rows;
        rows.resize_with(lines.len(), ConsoleRow::new);

        let display = display_time(cx, self.display_offset);
        for (i, (line, row)) in lines.iter().zip(rows.iter_mut()).enumerate() {
            let was_rendered = i < self.rendered_timestamps.len();
            let same_time = was_rendered && self.rendered_timestamps[i] == line.timestamp;
            let formatted = if self.last_filter == filter
                && same_time
                && self.rendered_raw_values.get(i) == Some(&line.value)
            {
                self.rendered_values[i].clone()
            } else {
                Rc::new(format_value(&line.value, &self.filter)?)
            };

            row.error = self.view.highlight_active && line.is_error;
            row.warning = self.view.highlight_active && line.is_warning;

            let mut has_changed = false;
            if !was_rendered {
                has_changed = true;
                self.rendered_values.push(formatted.clone());
            } else if !same_time || self.rendered_values[i] != formatted {
                has_changed = true;
                self.rendered_values[i] = formatted.clone();
            }

            row.timestamp = display(line.timestamp);
            if has_changed {
                row.html = formatted;
            }
        }
        self.rendered_timestamps = lines.iter().map(|line| line.timestamp).collect();
        self.rendered_raw_values = lines.iter().map(|line| line.value.clone()).collect();
        self.last_filter = filter;
        Ok(())
    }

    /// `updateHighlights()`.
    fn update_highlights(&mut self) {
        if self.rendered_timestamps.is_empty() {
            return;
        }
        let row_at = |time: Option<f64>| {
            let time = time.filter(|t| *t != 0.0 && !t.is_nan())?;
            Some(upper_bound(&self.rendered_timestamps, time, 0).saturating_sub(1))
        };
        let (selected, hovered) = match self.selection_mode {
            SelectionMode::Idle => (None, row_at(self.hovered_time)),
            SelectionMode::Static | SelectionMode::Playback => {
                (row_at(self.selected_time), row_at(self.hovered_time))
            }
            SelectionMode::Locked => (None, None),
        };
        for (index, row) in self.view.rows.iter_mut().enumerate() {
            row.selected = selected == Some(index);
            row.hovered = hovered == Some(index);
        }
    }
}

impl<Cx: TabCx + ?Sized> TabRenderer<Cx> for ConsoleRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        let mut o = JsObject::new();
        o.insert("highlight", JsValue::Bool(self.view.highlight_active));
        JsValue::Object(o)
    }

    fn restore_state(&mut self, cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        if matches!(state, JsValue::Object(_) | JsValue::Array(_))
            && let JsValue::Bool(b) = ops::get(state, "highlight")?
        {
            self.view.highlight_active = b;
            self.update_data(cx)?;
        }
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        None
    }

    fn render(&mut self, cx: &mut Cx, command: &dyn TabCommand) -> Result<(), JsError> {
        let Some(command) = command.downcast_ref::<ConsoleCommand>() else {
            return Err(JsError::type_error(
                "Cannot read properties of undefined (reading 'lines')",
            ));
        };
        self.selection_mode = command.selection_mode;
        self.selected_time = command.selected_time;
        self.hovered_time = command.hovered_time;

        let data_changed = command.lines != self.lines;
        let original = cx
            .tab_preferences()
            .is_some_and(|p| p.timestamps == Timestamps::Original);
        let format_changed = self.last_display_offset != Some(command.display_offset)
            || self.last_original != Some(original);
        self.last_display_offset = Some(command.display_offset);
        self.last_original = Some(original);

        if !ops::strict_equals(&command.key, &self.key)
            || command.key_available != self.key_available
            || data_changed
            || format_changed
        {
            self.key = command.key.clone();
            self.key_available = command.key_available;
            self.lines = command.lines.clone();
            self.display_offset = command.display_offset;
            self.update_data(cx)?;
        }

        self.update_highlights();
        self.view.jump_placeholder =
            display_time(cx, command.display_offset)(self.selected_time.unwrap_or(0.0));

        if command.selection_mode == SelectionMode::Locked {
            let last = self.last_scroll_position;
            if last.is_some_and(|last| self.geometry(|g| g.scroll_top()) < last) {
                cx.tab_selection().unlock();
            } else {
                self.geometry(|g| {
                    let bottom = g.scroll_height() - g.container_client_height();
                    g.set_scroll_top(bottom);
                });
            }
        }
        self.last_scroll_position = Some(self.geometry(|g| g.scroll_top()));
        Ok(())
    }
}
