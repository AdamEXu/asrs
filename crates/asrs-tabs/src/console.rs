//! Port of the console-line formatting and row filtering in
//! `src/shared/renderers/ConsoleRenderer.ts`.

use asrs_js::regex::sites;
use asrs_js::string::{JsError, JsString};

use crate::ansi::{AnsiStyle, AnsiUp, Rgb};

fn js(s: &str) -> JsString {
    JsString::from(s)
}

/// `ConsoleRenderer.formatValue`: normalizes spellings like `\x1b`, renders ANSI to HTML,
/// highlights a positive filter, then turns newlines into `<br />`.
pub fn format_value(value: &JsString, filter: &JsString) -> Result<JsString, JsError> {
    let normalized = sites::console_newlines(&sites::console_escapes(value));
    let mut ansi_up = AnsiUp::new();
    let mut html = ansi_up.ansi_to_html(&normalized);
    if !filter.is_empty() && !filter.starts_with(&js("!"), None) {
        let parts = sites::split_tags(&html);
        let filter_lower = filter.to_lower_case();
        let filter_len = filter.len() as i64;
        let mut formatted = Vec::with_capacity(html.len());
        for part in parts {
            if part.starts_with(&js("<"), None) && part.ends_with(&js(">"), None) {
                formatted.extend_from_slice(part.units());
                continue;
            }
            let part_lower = part.to_lower_case();
            let mut buf = Vec::with_capacity(part.len());
            let mut last_position: i64 = -1;
            let mut position: i64 = -1;
            while position + filter_len < part.len() as i64 && {
                position = part_lower.index_of(
                    &filter_lower,
                    Some(if position == -1 {
                        0.0
                    } else {
                        (position + filter_len) as f64
                    }),
                );
                position > -1
            } {
                let from = if last_position == -1 {
                    0.0
                } else {
                    (last_position + filter_len) as f64
                };
                buf.extend_from_slice(part.substring(from, Some(position as f64)).units());
                buf.extend("<span class=\"highlight\">".encode_utf16());
                buf.extend_from_slice(
                    part.substring(position as f64, Some((position + filter_len) as f64))
                        .units(),
                );
                buf.extend("</span>".encode_utf16());
                last_position = position;
            }
            if last_position != -1 {
                buf.extend_from_slice(
                    part.substring((last_position + filter_len) as f64, None)
                        .units(),
                );
                formatted.extend_from_slice(&buf);
            } else {
                formatted.extend_from_slice(part.units());
            }
        }
        html = JsString::from_units(formatted);
    }
    html.replace_all(&js("\n"), &js("<br />"))
}

/// The row predicate in `updateValueAvailability`: `filter` here is the raw FILTER_INPUT
/// value, exactly as upstream lowers it itself before matching.
pub fn line_visible(value: &JsString, filter_input: &JsString) -> bool {
    let filter = filter_input.to_lower_case();
    let negated = filter.starts_with(&js("!"), None);
    if !(if negated {
        filter.len() > 1
    } else {
        !filter.is_empty()
    }) {
        return true;
    }
    if negated {
        !value
            .to_lower_case()
            .includes(&filter.slice(1.0, None).to_lower_case(), None)
    } else {
        value
            .to_lower_case()
            .includes(&filter.to_lower_case(), None)
    }
}

/// What the console export button asks the main process to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsoleExport {
    Error {
        title: &'static str,
        content: &'static str,
    },
    /// The bytes main writes to the chosen file: every unfiltered line joined with `\n`,
    /// as UTF-8 with lone surrogates replaced.
    Save(Vec<u8>),
}

pub fn export(values: &[JsString]) -> ConsoleExport {
    if values.is_empty() {
        return ConsoleExport::Error {
            title: "Cannot export console log",
            content: "Please add a field with console data, then try again.",
        };
    }
    let mut text = JsString::new();
    for (i, value) in values.iter().enumerate() {
        if i > 0 {
            text.push_str(&js("\n"));
        }
        text.push_str(value);
    }
    ConsoleExport::Save(text.to_string_lossy().into_bytes())
}

/// One node of the DOM a console value cell holds after `innerHTML = formatValue(...)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsoleNode {
    Text {
        text: JsString,
        style: AnsiStyle,
        link: Option<JsString>,
        highlight: bool,
    },
    LineBreak,
}

/// The cell content [`format_value`] describes, as the HTML parser builds it: text nodes with
/// the ansi_up rendition, enclosing link and filter highlight they inherit, and `<br />`.
///
/// Highlighting runs on escaped text, so a match can cut a character reference in two. The
/// parser then decodes the fragments on their own: `&amp` before a tag is a legacy reference
/// and reads as `&`, `&#x2` reads as U+0002, and `&am` stays literal. `text` carries those
/// decoded characters, not the source line.
pub fn format_value_nodes(
    value: &JsString,
    filter: &JsString,
) -> Result<Vec<ConsoleNode>, JsError> {
    Ok(parse_cell_html(&format_value(value, filter)?))
}

fn parse_cell_html(html: &JsString) -> Vec<ConsoleNode> {
    #[derive(Clone)]
    enum Open {
        Style(AnsiStyle),
        Highlight,
        Link(JsString),
    }
    let mut stack: Vec<Open> = Vec::new();
    let mut nodes = Vec::new();
    for part in sites::split_tags(html) {
        let u = part.units();
        if u.first() == Some(&u16::from(b'<')) && u.last() == Some(&u16::from(b'>')) {
            let tag = String::from_utf16_lossy(u);
            if tag == "<br />" {
                nodes.push(ConsoleNode::LineBreak);
            } else if tag == "</span>" || tag == "</a>" {
                stack.pop();
            } else if tag == "<span class=\"highlight\">" {
                stack.push(Open::Highlight);
            } else if let Some(css) = attribute(&tag, "<span style=\"") {
                stack.push(Open::Style(parse_css(&css)));
            } else if let Some(href) = attribute(&tag, "<a href=\"") {
                stack.push(Open::Link(JsString::from_units(decode_references(&href))));
            }
            continue;
        }
        let text = text_content(u);
        if text.is_empty() {
            continue;
        }
        let mut style = AnsiStyle::default();
        let mut link = None;
        let mut highlight = false;
        for open in &stack {
            match open {
                Open::Style(s) => style = *s,
                Open::Highlight => highlight = true,
                Open::Link(href) => link = Some(href.clone()),
            }
        }
        nodes.push(ConsoleNode::Text {
            text: JsString::from_units(text),
            style,
            link,
            highlight,
        });
    }
    nodes
}

fn attribute(tag: &str, prefix: &str) -> Option<Vec<u16>> {
    let value = tag.strip_prefix(prefix)?.strip_suffix("\">")?;
    Some(value.encode_utf16().collect())
}

fn parse_css(css: &[u16]) -> AnsiStyle {
    let mut style = AnsiStyle::default();
    for decl in String::from_utf16_lossy(css).split(';') {
        match decl {
            "font-weight:bold" => style.bold = true,
            "opacity:0.7" => style.faint = true,
            "font-style:italic" => style.italic = true,
            "text-decoration:underline" => style.underline = true,
            _ => {
                if let Some(rgb) = decl.strip_prefix("color:rgb(") {
                    style.fg = parse_rgb(rgb);
                } else if let Some(rgb) = decl.strip_prefix("background-color:rgb(") {
                    style.bg = parse_rgb(rgb);
                }
            }
        }
    }
    style
}

fn parse_rgb(s: &str) -> Option<Rgb> {
    let channel = |c: &str| {
        c.bytes().try_fold(0u8, |acc, d| {
            acc.checked_mul(10)?
                .checked_add(d.checked_sub(b'0').filter(|&d| d < 10)?)
        })
    };
    let mut it = s.strip_suffix(')')?.split(',').map(channel);
    Some(Rgb(it.next()??, it.next()??, it.next()??))
}

/// The characters the HTML parser puts in the DOM for one run of cell text: CR and CRLF
/// become LF, character references are decoded, and NUL is dropped as body content.
fn text_content(u: &[u16]) -> Vec<u16> {
    let mut normalized = Vec::with_capacity(u.len());
    let mut i = 0;
    while i < u.len() {
        if u[i] == 0x0d {
            normalized.push(0x0a);
            i += 1 + usize::from(u.get(i + 1) == Some(&0x0a));
        } else {
            normalized.push(u[i]);
            i += 1;
        }
    }
    let mut text = decode_references(&normalized);
    text.retain(|&c| c != 0);
    text
}

/// Character references in a text run of the cell HTML. Every `&` there starts one of the five
/// escapes ansi_up writes, possibly cut short by a highlight tag, so the HTML tokenizer's rules
/// reduce to: a full escape, a legacy name without `;` (`amp`, `lt`, `gt`, `quot`), a hex
/// reference with at least one digit, or else a literal `&`.
fn decode_references(u: &[u16]) -> Vec<u16> {
    const NAMED: [(&str, u16); 4] = [("amp", 0x26), ("lt", 0x3c), ("gt", 0x3e), ("quot", 0x22)];
    let mut out = Vec::with_capacity(u.len());
    let mut i = 0;
    while i < u.len() {
        if u[i] != u16::from(b'&') {
            out.push(u[i]);
            i += 1;
            continue;
        }
        let rest = &u[i + 1..];
        let semicolon = |n: usize| usize::from(rest.get(n) == Some(&u16::from(b';')));
        if let Some((name, ch)) = NAMED
            .iter()
            .find(|(name, _)| rest.starts_with(&name.encode_utf16().collect::<Vec<_>>()))
        {
            out.push(*ch);
            i += 1 + name.len() + semicolon(name.len());
            continue;
        }
        if rest.starts_with(&[u16::from(b'#'), u16::from(b'x')]) {
            let digits = rest[2..]
                .iter()
                .take_while(|&&c| {
                    char::from_u32(u32::from(c)).is_some_and(|c| c.is_ascii_hexdigit())
                })
                .count();
            if digits > 0 {
                let code = rest[2..2 + digits].iter().fold(0u32, |acc, &c| {
                    acc * 16
                        + char::from_u32(u32::from(c))
                            .and_then(|c| c.to_digit(16))
                            .unwrap_or(0)
                });
                out.push(if code == 0 { 0xfffd } else { code as u16 });
                i += 3 + digits + semicolon(2 + digits);
                continue;
            }
        }
        out.push(u[i]);
        i += 1;
    }
    out
}

pub mod controller {
    //! `src/hub/controllers/ConsoleController.ts` and
    //! `src/shared/renderers/ConsoleRenderer.ts` halves.

    use std::cell::RefCell;
    use std::rc::Rc;

    use asrs_js::ops;
    use asrs_js::sort::array_sort;
    use asrs_js::string::{JsError, JsString};
    use asrs_js::value::{JsObject, JsValue, ToJs};
    use asrs_log::log::LoggableType;
    use asrs_log::log::log_util::LogFieldIterator;
    use asrs_model::selection::SelectionMode;
    use asrs_model::source_list::Rect;
    use asrs_model::tab_controller::{Command, TabCommand, TabController, TabRenderer};

    use crate::cx::{TabCx, display_offset};

    /// `["Console", "ConsoleLineTimestamp", "ErrorInfoTimestamp", "ProgramCrashInfoTimestamp"]`.
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
        /// `TABLE_CONTAINER.scrollHeight`; the harness resolves it like `clientHeight`.
        pub scroll_height: f64,
        /// A `tbody tr` rect for `jump`'s `row.clientHeight`.
        pub data_row: Rect,
        /// `JUMP_INPUT.value`, set by event `props` before the keydown or click.
        pub jump_value: JsString,
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

    fn type_of(value: &JsValue) -> Option<JsString> {
        match value {
            JsValue::String(s) => Some(s.clone()),
            _ => None,
        }
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
            let end = match ops::get(detail, "end")? {
                JsValue::Bool(b) => b,
                ref other => {
                    matches!(other, JsValue::Number(n) if *n != 0.0 && !n.is_nan())
                        || matches!(other, JsValue::String(s) if !s.is_empty())
                        || matches!(
                            other,
                            JsValue::Array(_)
                                | JsValue::Object(_)
                                | JsValue::Bytes(_)
                                | JsValue::BigInt(_)
                                | JsValue::Date(_)
                                | JsValue::ExtData(..)
                        )
                }
            };

            let rect = self.dom.borrow().container;
            let active = x > rect.left() && x < rect.right() && y > rect.top() && y < rect.bottom();
            let first = ops::index(&ops::get(&data, "fields")?, 0)?;
            let field_type = type_of(&first).and_then(|k| cx.tab_log().get_type(&k));
            let structured =
                type_of(&first).and_then(|k| cx.tab_log().get_structured_type(&k).cloned());
            let valid_type = field_type == Some(LoggableType::String)
                || structured
                    .as_ref()
                    .is_some_and(|s| CONSOLE_TYPES.iter().any(|t| s == &JsString::from(*t)));
            let mut dom = self.dom.borrow_mut();
            dom.drag_highlight_hidden = true;
            if active && valid_type {
                if end {
                    self.field = first;
                } else {
                    dom.drag_highlight_hidden = false;
                }
            }
            Ok(())
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
            let log = cx.tab_log();
            if log
                .get_structured_type(field)
                .is_some_and(|s| s == &JsString::from("Console"))
            {
                [
                    concat_key(field, "/ConsoleLine"),
                    concat_key(field, "/ErrorInfo"),
                    concat_key(field, "/ProgramCrashInfo"),
                ]
                .to_vec()
            } else {
                vec![field.clone()]
            }
        }

        fn show_timeline(&self, _cx: &Cx) -> bool {
            false
        }

        fn command(&mut self, cx: &mut Cx, _uuid: &JsValue) -> Result<Command, JsError> {
            let (key_available, lines, offset): (bool, Vec<ConsoleLine>, f64);
            {
                let log = cx.tab_log();
                key_available = match &self.field {
                    JsValue::String(field) => log.get_field_keys().contains(field),
                    _ => false,
                };
                lines = if key_available
                    && matches!(&self.field, JsValue::String(s) if !s.is_empty())
                {
                    self.lines(cx)
                } else {
                    Vec::new()
                };
                offset = display_offset(cx);
            }
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

    impl ConsoleController {
        /// The `lines` array for `getCommand` when the field is available.
        fn lines(&self, cx: &(impl TabCx + ?Sized)) -> Vec<ConsoleLine> {
            let log = cx.tab_log();
            let field = match &self.field {
                JsValue::String(s) => s.clone(),
                _ => unreachable!(),
            };
            let structured = log.get_structured_type(&field).cloned();
            let field_type = log.get_type(&field);
            let mut lines = Vec::new();

            if field_type == Some(LoggableType::String) {
                if let Some(data) =
                    log.get_string(&field, f64::NEG_INFINITY, f64::INFINITY, None, None)
                {
                    for i in 0..data.timestamps.len() {
                        let value = data.values[i].clone();
                        let lower = value.to_lower_case();
                        lines.push(ConsoleLine {
                            timestamp: data.timestamps[i],
                            value,
                            is_error: lower.includes(&JsString::from("error"), None),
                            is_warning: lower.includes(&JsString::from("warning"), None),
                        });
                    }
                }
            } else {
                let keys_to_process: Vec<JsString> = if structured
                    .as_ref()
                    .is_some_and(|s| s == &JsString::from("Console"))
                {
                    vec![
                        concat_key(&field, "/ConsoleLine"),
                        concat_key(&field, "/ErrorInfo"),
                        concat_key(&field, "/ProgramCrashInfo"),
                    ]
                } else if structured.as_ref().is_some_and(|s| {
                    [
                        "ConsoleLineTimestamp",
                        "ErrorInfoTimestamp",
                        "ProgramCrashInfoTimestamp",
                    ]
                    .iter()
                    .any(|t| s == &JsString::from(*t))
                }) {
                    vec![field.clone()]
                } else {
                    Vec::new()
                };

                for key in keys_to_process {
                    let key_type = log.get_structured_type(&key).cloned();
                    let raw = log.get_raw(&key, f64::NEG_INFINITY, f64::INFINITY, None, None);
                    let Some(raw) = raw else { continue };

                    if key_type
                        .as_ref()
                        .is_some_and(|t| t == &JsString::from("ConsoleLineTimestamp"))
                    {
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
                    } else if key_type
                        .as_ref()
                        .is_some_and(|t| t == &JsString::from("ErrorInfoTimestamp"))
                    {
                        let child = |name: &str| {
                            log.get_string(
                                &concat_key(&key, &format!("/ErrorInfo/{name}")),
                                f64::NEG_INFINITY,
                                f64::INFINITY,
                                None,
                                None,
                            )
                        };
                        let details_data = child("Details");
                        let location_data = child("Location");
                        let call_stack_data = child("CallStack");
                        let is_error_data = log.get_boolean(
                            &concat_key(&key, "/ErrorInfo/IsError"),
                            f64::NEG_INFINITY,
                            f64::INFINITY,
                            None,
                            None,
                        );
                        let mut details = LogFieldIterator::new(details_data.as_ref());
                        let mut location = LogFieldIterator::new(location_data.as_ref());
                        let mut call_stack = LogFieldIterator::new(call_stack_data.as_ref());
                        let mut is_error = LogFieldIterator::new(is_error_data.as_ref());
                        for t in &raw.timestamps {
                            let details = details.get_at_time(*t).cloned().unwrap_or_default();
                            let location = location.get_at_time(*t).cloned().unwrap_or_default();
                            let call_stack =
                                call_stack.get_at_time(*t).cloned().unwrap_or_default();
                            let is_error = is_error.get_at_time(*t).copied().unwrap_or(false);
                            let mut value = details;
                            if !location.is_empty() || !call_stack.is_empty() {
                                value.push_str(&JsString::from("\n"));
                                value.push_str(&location);
                                value.push_str(&JsString::from("\n"));
                                value.push_str(&call_stack);
                            }
                            lines.push(ConsoleLine {
                                timestamp: *t,
                                value,
                                is_error,
                                is_warning: !is_error,
                            });
                        }
                    } else if key_type
                        .as_ref()
                        .is_some_and(|t| t == &JsString::from("ProgramCrashInfoTimestamp"))
                    {
                        let child = |name: &str| {
                            log.get_string(
                                &concat_key(&key, &format!("/ProgramCrashInfo/{name}")),
                                f64::NEG_INFINITY,
                                f64::INFINITY,
                                None,
                                None,
                            )
                        };
                        let details_data = child("Details");
                        let location_data = child("Location");
                        let call_stack_data = child("CallStack");
                        let mut details = LogFieldIterator::new(details_data.as_ref());
                        let mut location = LogFieldIterator::new(location_data.as_ref());
                        let mut call_stack = LogFieldIterator::new(call_stack_data.as_ref());
                        for t in &raw.timestamps {
                            let details = details.get_at_time(*t).cloned().unwrap_or_default();
                            let location = location.get_at_time(*t).cloned().unwrap_or_default();
                            let call_stack =
                                call_stack.get_at_time(*t).cloned().unwrap_or_default();
                            let mut value = details;
                            if !location.is_empty() || !call_stack.is_empty() {
                                value.push_str(&JsString::from("\n"));
                                value.push_str(&location);
                                value.push_str(&JsString::from("\n"));
                                value.push_str(&call_stack);
                            }
                            lines.push(ConsoleLine {
                                timestamp: *t,
                                value,
                                is_error: true,
                                is_warning: false,
                            });
                        }
                    }
                }
            }

            array_sort(&mut lines, |a, b| a.timestamp - b.timestamp);
            lines
        }
    }

    fn concat_key(key: &JsString, suffix: &str) -> JsString {
        let mut out = key.clone();
        out.push_str(&JsString::from(suffix));
        out
    }

    pub struct ConsoleRenderer {
        dom: Rc<RefCell<ConsoleDom>>,
        highlight: bool,
        last_scroll_position: Option<f64>,
        lines: Vec<f64>,
        display_offset: f64,
        selection_mode: SelectionMode,
        selected_time: Option<f64>,
    }

    impl ConsoleRenderer {
        /// `new ConsoleRenderer(element, hasController)`.
        pub fn new(dom: Rc<RefCell<ConsoleDom>>, _has_controller: bool) -> Self {
            Self {
                dom,
                highlight: false,
                last_scroll_position: None,
                lines: Vec::new(),
                display_offset: 0.0,
                selection_mode: SelectionMode::Idle,
                selected_time: None,
            }
        }

        /// The jump input's Enter handler and jump-button click. Console's `jump` never
        /// writes the selection.
        pub fn jump(&mut self) {
            let value = self.dom.borrow().jump_value.clone();
            let mut target = asrs_js::number::to_number(value.units());
            if value.is_empty() {
                target = if self.selection_mode != SelectionMode::Idle {
                    self.selected_time.unwrap_or(0.0)
                } else {
                    0.0
                };
            } else {
                target -= self.display_offset;
            }
            let mut target_row = self
                .lines
                .iter()
                .position(|t| *t > target)
                .map(|i| i as f64)
                .unwrap_or(-1.0);
            if target_row == -1.0 {
                target_row = self.lines.len() as f64;
            }
            if target_row < 1.0 {
                target_row = 1.0;
            }
            target_row -= 1.0;
            let mut dom = self.dom.borrow_mut();
            let row_height = asrs_js::math::round(dom.data_row.height);
            dom.scroll_top = row_height * target_row;
        }

        /// The `HIGHLIGHT_BUTTON` click: toggles `active`, then `updateData()`.
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
            match state {
                JsValue::Null => {}
                v if ops::has(v, "highlight").unwrap_or(false) => {
                    if let JsValue::Bool(b) = ops::get(v, "highlight").unwrap_or_default() {
                        self.highlight = b;
                    }
                }
                _ => {}
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
            self.display_offset = command.display_offset;
            self.selection_mode = command.selection_mode;
            self.selected_time = command.selected_time;
            self.lines = command.lines.iter().map(|line| line.timestamp).collect();
            if command.selection_mode == SelectionMode::Locked {
                let mut dom = self.dom.borrow_mut();
                if self
                    .last_scroll_position
                    .is_some_and(|last| dom.scroll_top < last)
                {
                    drop(dom);
                    cx.tab_selection().unlock();
                } else {
                    let client_height = asrs_js::math::round(dom.container.height);
                    dom.scroll_top = dom.scroll_height - client_height;
                }
            }
            self.last_scroll_position = Some(self.dom.borrow().scroll_top);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str, style: AnsiStyle, link: Option<&str>, highlight: bool) -> ConsoleNode {
        ConsoleNode::Text {
            text: js(s),
            style,
            link: link.map(js),
            highlight,
        }
    }

    fn nodes(value: &str, filter: &str) -> Vec<ConsoleNode> {
        format_value_nodes(&js(value), &js(filter)).unwrap()
    }

    // Expected nodes were read back from Electron's DOM after `innerHTML = formatValue(...)`.
    #[test]
    fn highlight_inside_escapes() {
        let plain = AnsiStyle::default();
        assert_eq!(
            nodes("a&b", ";"),
            [
                text("a&", plain, None, false),
                text(";", plain, None, true),
                text("b", plain, None, false)
            ]
        );
        assert_eq!(
            nodes("a&b", "am"),
            [
                text("a&", plain, None, false),
                text("am", plain, None, true),
                text("p;b", plain, None, false)
            ]
        );
        assert_eq!(
            nodes("'", "7"),
            [
                text("\u{2}", plain, None, false),
                text("7", plain, None, true),
                text(";", plain, None, false)
            ]
        );
        assert_eq!(
            nodes("<", "&"),
            [
                text("&", plain, None, true),
                text("lt;", plain, None, false)
            ]
        );
    }

    #[test]
    fn styles_links_and_breaks() {
        let red_bold = AnsiStyle {
            fg: Some(Rgb(187, 0, 0)),
            bold: true,
            ..AnsiStyle::default()
        };
        assert_eq!(
            nodes("\\x1b[1;31mx\\ny\r\n\0z", "y"),
            [
                text("x", red_bold, None, false),
                ConsoleNode::LineBreak,
                text("y", red_bold, None, true),
                text("\n", red_bold, None, false),
                ConsoleNode::LineBreak,
                text("z", red_bold, None, false),
            ]
        );
        assert_eq!(
            nodes("\x1b]8;;https://a.b/?x&y\x07go\x1b]8;;\x07", ""),
            [text(
                "go",
                AnsiStyle::default(),
                Some("https://a.b/?x&y"),
                false
            )]
        );
    }
}
