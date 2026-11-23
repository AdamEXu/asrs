//! What `TableRenderer` leaves in its root element, for the hub and satellite views to draw.

use asrs_js::string::JsString;

/// A field's `th`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableHeader {
    /// `cell.title`.
    pub title: JsString,
    /// The key text, struck through unless `available`. Line breaks are `\n`.
    pub text: JsString,
    pub available: bool,
}

/// One of the pooled data rows. A hidden row keeps the cells and classes it last had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableRow {
    /// The timestamp, then one text per field.
    pub cells: Vec<String>,
    pub hidden: bool,
    pub selected: bool,
    pub hovered: bool,
}

impl TableRow {
    pub(super) fn new() -> TableRow {
        TableRow {
            cells: vec![String::new()],
            hidden: false,
            selected: false,
            hovered: false,
        }
    }
}

/// The table body is the header row (the jump input's cell, then `headers`), an upper filler,
/// `rows` and a lower filler.
#[derive(Clone, Debug, PartialEq)]
pub struct TableView {
    /// Whether header cells show their close button.
    pub has_controller: bool,
    /// Whether the hand icon is fading in (opacity 0.15 over 1 s after 1 s) rather than gone.
    pub show_hand: bool,
    pub headers: Vec<TableHeader>,
    pub filler_upper_px: f64,
    pub filler_lower_px: f64,
    pub rows: Vec<TableRow>,
    /// `INPUT_FIELD.placeholder`.
    pub jump_placeholder: String,
}

impl TableView {
    pub(super) fn new(has_controller: bool) -> TableView {
        TableView {
            has_controller,
            show_hand: has_controller,
            headers: Vec::new(),
            filler_upper_px: 500.0,
            filler_lower_px: 1000.0,
            rows: Vec::new(),
            jump_placeholder: "0.000".to_owned(),
        }
    }
}

/// The prefix upstream puts before a boolean cell's text.
pub fn boolean_icon(value: bool) -> &'static str {
    if value { "\u{1f7e9}" } else { "\u{1f7e5}" }
}
