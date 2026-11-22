//! `src/hub/controllers/ConsoleController.ts` and the parts of
//! `src/shared/renderers/ConsoleRenderer.ts` that reach the selection or the saved state. Line
//! formatting and filtering are [`super::format_value`] and [`super::line_visible`].

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::ops;
use asrs_js::sort::array_sort;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::log_util::LogFieldIterator;
use asrs_log::log::{Log, LoggableType};
use asrs_model::selection::SelectionMode;
use asrs_model::source_list::Rect;
use asrs_model::tab_controller::{Command, TabCommand, TabController, TabRenderer};

use crate::cx::{TabCx, display_offset};

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
}

/// One entry of the command's `lines` array.
#[derive(Clone, Debug)]
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
    field: JsValue,
}

impl ConsoleController {
    /// `new ConsoleController(element)`. The view shares `dom` with the matching
    /// `ConsoleRenderer`.
    pub fn new(dom: Rc<RefCell<ConsoleDom>>) -> Self {
        Self {
            dom,
            field: JsValue::Null,
        }
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
        let rect = self.dom.borrow().container;
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

pub struct ConsoleRenderer {
    dom: Rc<RefCell<ConsoleDom>>,
    highlight: bool,
    last_scroll_position: Option<f64>,
}

impl ConsoleRenderer {
    /// `new ConsoleRenderer(element, hasController)`.
    pub fn new(dom: Rc<RefCell<ConsoleDom>>) -> Self {
        Self {
            dom,
            highlight: false,
            last_scroll_position: None,
        }
    }

    pub fn dom(&self) -> &Rc<RefCell<ConsoleDom>> {
        &self.dom
    }

    /// The `HIGHLIGHT_BUTTON` click.
    pub fn highlight_click(&mut self) {
        self.highlight = !self.highlight;
    }
}

impl<Cx: TabCx + ?Sized> TabRenderer<Cx> for ConsoleRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        let mut o = JsObject::new();
        o.insert("highlight", JsValue::Bool(self.highlight));
        JsValue::Object(o)
    }

    fn restore_state(&mut self, _cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        if matches!(state, JsValue::Object(_) | JsValue::Array(_))
            && let JsValue::Bool(b) = ops::get(state, "highlight")?
        {
            self.highlight = b;
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
        if command.selection_mode == SelectionMode::Locked {
            let scroll_top = self.dom.borrow().scroll_top;
            if self
                .last_scroll_position
                .is_some_and(|last| scroll_top < last)
            {
                cx.tab_selection().unlock();
            } else {
                let mut dom = self.dom.borrow_mut();
                dom.scroll_top = dom.scroll_height - asrs_js::math::round(dom.container.height);
            }
        }
        self.last_scroll_position = Some(self.dom.borrow().scroll_top);
        Ok(())
    }
}
