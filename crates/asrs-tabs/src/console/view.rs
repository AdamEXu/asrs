//! What `ConsoleRenderer` leaves in its root element, for the hub and satellite views to draw.

use std::rc::Rc;

use asrs_js::string::JsString;

use super::{ConsoleNode, parse_cell_html};

/// One line's `tr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleRow {
    pub timestamp: String,
    pub(super) html: Rc<JsString>,
    pub error: bool,
    pub warning: bool,
    pub selected: bool,
    pub hovered: bool,
}

impl ConsoleRow {
    pub(super) fn new() -> ConsoleRow {
        ConsoleRow {
            timestamp: String::new(),
            html: Rc::default(),
            error: false,
            warning: false,
            selected: false,
            hovered: false,
        }
    }

    /// The value cell's content, parsed from its HTML on every call.
    pub fn nodes(&self) -> Vec<ConsoleNode> {
        parse_cell_html(&self.html)
    }

    /// Identifies the value cell's content: the same key on two rows or frames means the same
    /// nodes, so a view can cache its layout by it.
    pub fn html(&self) -> &Rc<JsString> {
        &self.html
    }
}

/// The table body is the header row (the jump input's cell and the field cell), then `rows`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsoleView {
    /// Whether the export button exists (the hub's markup; satellites have none).
    pub has_controller: bool,
    /// The field key, line breaks as `\n`; empty without a field.
    pub field_text: JsString,
    /// Whether the key is struck through.
    pub field_line_through: bool,
    pub field_delete_hidden: bool,
    /// Whether the hand icon is fading in (opacity 0.15 over 1 s after 1 s) rather than gone.
    pub show_hand: bool,
    /// Whether `HIGHLIGHT_BUTTON` has the `active` class.
    pub highlight_active: bool,
    pub rows: Vec<ConsoleRow>,
    /// `JUMP_INPUT.placeholder`.
    pub jump_placeholder: String,
}

impl ConsoleView {
    pub(super) fn new(has_controller: bool) -> ConsoleView {
        ConsoleView {
            has_controller,
            field_text: JsString::new(),
            field_line_through: false,
            field_delete_hidden: true,
            show_hand: has_controller,
            highlight_active: false,
            rows: Vec::new(),
            jump_placeholder: "0.000".to_owned(),
        }
    }
}
