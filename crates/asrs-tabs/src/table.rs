//! `src/hub/controllers/TableController.ts` and `src/shared/renderers/TableRenderer.ts`.
//!
//! The two halves share `TableDom` the way upstream shares the renderer element: the renderer
//! writes the header cell count and `scrollTop`, the controller reads those rects during
//! drags. The view layer keeps the rect and hidden fields current.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::math;
use asrs_js::ops;
use asrs_js::sort::array_sort;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::{LogValueSetAny, LoggableType};
use asrs_log::util::{arrays_equal, check_array_type, lower_bound, upper_bound};
use asrs_model::selection::SelectionMode;
use asrs_model::source_list::Rect;
use asrs_model::tab_controller::{Command, TabCommand, TabController, TabRenderer};

use crate::cx::{TabCx, display_offset};

const DRAG_THRESHOLD_PX: f64 = 5.0;
const DEFAULT_ROW_HEIGHT_PX: f64 = 25.0;
const DATA_ROW_BUFFER: usize = 15;
const DRAG_HIGHLIGHT_OFFSET_PX: f64 = 12.5;

/// The DOM facts of the shared renderer element. `header_cells` has one entry per `th` in the
/// header row: cell 0 is the jump-input cell, the rest follow `lastFields`.
#[derive(Clone, Debug, Default)]
pub struct TableDom {
    /// `ROOT.hidden`.
    pub hidden: bool,
    /// `TABLE_CONTAINER.getBoundingClientRect()`.
    pub container: Rect,
    /// `HEADER.children[i].getBoundingClientRect()` for every `th`.
    pub header_cells: Vec<Rect>,
    /// `HEADER.getBoundingClientRect()` (the header row).
    pub header_row: Rect,
    /// `dataRows[i].getBoundingClientRect()` for every visible row. The DOM oracle's layout
    /// resolves all rows alike, so one rect stands for them; hidden rows are skipped.
    pub data_row: Rect,
    /// `TABLE_CONTAINER.scrollTop`, persisted across frames.
    pub scroll_top: f64,
    /// `HEADER.childElementCount`, written by the renderer's `updateFields`.
    pub header_count: usize,
    /// `DRAG_HIGHLIGHT.hidden` and its `style.left` px value.
    pub drag_highlight_hidden: bool,
    pub drag_highlight_left: Option<f64>,
    /// `INPUT_FIELD.value`.
    pub jump_value: JsString,
    /// The last `mousemove` `clientY` on `TABLE_BODY`; `None` after `mouseleave`.
    pub hover_cursor_y: Option<f64>,
}

/// One entry of the command's `fields` array.
#[derive(Clone, Debug)]
pub struct TableCommandField {
    pub key: JsValue,
    pub is_available: bool,
    /// `null` when unavailable; otherwise the merged range data.
    pub data: Option<LogValueSetAny>,
    pub field_type: Option<LoggableType>,
}

impl ToJs for TableCommandField {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("key", self.key.clone());
        o.insert("isAvailable", JsValue::Bool(self.is_available));
        o.insert(
            "data",
            self.data.as_ref().map_or(JsValue::Null, value_set_js),
        );
        o.insert(
            "type",
            self.field_type
                .map_or(JsValue::Null, |t| JsValue::Number(t as u8 as f64)),
        );
        JsValue::Object(o)
    }
}

/// `TableRendererCommand`.
#[derive(Clone, Debug)]
pub struct TableCommand {
    pub timestamps: Vec<f64>,
    pub display_offset: f64,
    pub fields: Vec<TableCommandField>,
    pub selection_mode: SelectionMode,
    pub selected_time: Option<f64>,
    pub hovered_time: Option<f64>,
}

impl ToJs for TableCommand {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("timestamps", JsValue::Array(numbers(&self.timestamps)));
        o.insert("displayOffset", JsValue::Number(self.display_offset));
        o.insert(
            "fields",
            JsValue::Array(self.fields.iter().map(ToJs::to_js).collect()),
        );
        o.insert("selectionMode", ToJs::to_js(&self.selection_mode));
        o.insert("selectedTime", opt_num(self.selected_time));
        o.insert("hoveredTime", opt_num(self.hovered_time));
        JsValue::Object(o)
    }
}

fn numbers(values: &[f64]) -> Vec<JsValue> {
    values.iter().map(|v| JsValue::Number(*v)).collect()
}

fn opt_num(value: Option<f64>) -> JsValue {
    value.map_or(JsValue::Null, JsValue::Number)
}

/// A `LogValueSetAny` as `{timestamps, values}`.
pub fn value_set_js(set: &LogValueSetAny) -> JsValue {
    let (timestamps, values): (&[f64], Vec<JsValue>) = match set {
        LogValueSetAny::Raw(s) => (
            &s.timestamps,
            s.values
                .iter()
                .map(|v| JsValue::Bytes(v.to_vec()))
                .collect(),
        ),
        LogValueSetAny::Boolean(s) => (
            &s.timestamps,
            s.values.iter().map(|v| JsValue::Bool(*v)).collect(),
        ),
        LogValueSetAny::Number(s) => (&s.timestamps, numbers(&s.values)),
        LogValueSetAny::String(s) => (
            &s.timestamps,
            s.values
                .iter()
                .map(|v| JsValue::String(v.clone()))
                .collect(),
        ),
        LogValueSetAny::BooleanArray(s) => (
            &s.timestamps,
            s.values
                .iter()
                .map(|v| JsValue::Array(v.iter().map(|b| JsValue::Bool(*b)).collect()))
                .collect(),
        ),
        LogValueSetAny::NumberArray(s) => (
            &s.timestamps,
            s.values
                .iter()
                .map(|v| JsValue::Array(numbers(v)))
                .collect(),
        ),
        LogValueSetAny::StringArray(s) => (
            &s.timestamps,
            s.values
                .iter()
                .map(|v| JsValue::Array(v.iter().map(|s| JsValue::String(s.clone())).collect()))
                .collect(),
        ),
        LogValueSetAny::Empty(s) => (&s.timestamps, Vec::new()),
    };
    let mut o = JsObject::new();
    o.insert("timestamps", JsValue::Array(numbers(timestamps)));
    o.insert("values", JsValue::Array(values));
    JsValue::Object(o)
}

/// `timestamps.concat` and `values.concat` across `getRange` results.
fn push_range_data(acc: &mut Option<LogValueSetAny>, new: LogValueSetAny) {
    let Some(acc) = acc.as_mut() else {
        *acc = Some(new);
        return;
    };
    match (&mut *acc, new) {
        (LogValueSetAny::Raw(a), LogValueSetAny::Raw(b)) => {
            a.timestamps.extend(b.timestamps);
            a.values.extend(b.values);
        }
        (LogValueSetAny::Boolean(a), LogValueSetAny::Boolean(b)) => {
            a.timestamps.extend(b.timestamps);
            a.values.extend(b.values);
        }
        (LogValueSetAny::Number(a), LogValueSetAny::Number(b)) => {
            a.timestamps.extend(b.timestamps);
            a.values.extend(b.values);
        }
        (LogValueSetAny::String(a), LogValueSetAny::String(b)) => {
            a.timestamps.extend(b.timestamps);
            a.values.extend(b.values);
        }
        (LogValueSetAny::BooleanArray(a), LogValueSetAny::BooleanArray(b)) => {
            a.timestamps.extend(b.timestamps);
            a.values.extend(b.values);
        }
        (LogValueSetAny::NumberArray(a), LogValueSetAny::NumberArray(b)) => {
            a.timestamps.extend(b.timestamps);
            a.values.extend(b.values);
        }
        (LogValueSetAny::StringArray(a), LogValueSetAny::StringArray(b)) => {
            a.timestamps.extend(b.timestamps);
            a.values.extend(b.values);
        }
        (LogValueSetAny::Empty(a), LogValueSetAny::Empty(b)) => {
            a.timestamps.extend(b.timestamps);
        }
        _ => {}
    }
}

/// `Array.prototype.splice` start-index coercion.
fn splice_start(index: f64, len: usize) -> usize {
    let i = index.trunc();
    if i == f64::NEG_INFINITY || i.is_nan() {
        0
    } else if i < 0.0 {
        math::max2(len as f64 + i, 0.0) as usize
    } else {
        math::min2(i, len as f64) as usize
    }
}

/// `startDrag`'s `data` payload for a column drag: `{tableIndex: n}`.
pub struct ColumnDrag {
    pub x: f64,
    pub y: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    pub table_index: usize,
}

pub struct TableController {
    uuid: String,
    dom: Rc<RefCell<TableDom>>,
    /// The `fields` list; drags can splice non-string values in.
    fields: Vec<JsValue>,
    ranges: JsObject<[f64; 2]>,
    mouse_down: Option<[f64; 2]>,
}

impl TableController {
    /// `new TableController(element)`. The view shares `dom` with the matching
    /// `TableRenderer`.
    pub fn new(uuid: &JsString, dom: Rc<RefCell<TableDom>>) -> Self {
        Self {
            uuid: uuid.to_string_lossy(),
            dom,
            fields: Vec::new(),
            ranges: JsObject::new(),
            mouse_down: None,
        }
    }

    /// The `close-field` event on `ROOT`; `detail` is the raw event detail.
    pub fn close_field(&mut self, detail: &JsValue) {
        let index = ops::value_to_number(detail).unwrap_or(f64::NAN);
        if index < self.fields.len() as f64 {
            self.fields.remove(splice_start(index, self.fields.len()));
        }
    }

    /// `mousedown` on `ROOT`.
    pub fn root_mouse_down(&mut self, x: f64, y: f64) {
        let dom = self.dom.borrow();
        if y < dom.header_cells.first().map_or(0.0, Rect::bottom) {
            self.mouse_down = Some([x, y]);
        }
    }

    /// `mouseup` on `ROOT`.
    pub fn root_mouse_up(&mut self) {
        self.mouse_down = None;
    }

    /// `mousemove` on `ROOT`: starts a column drag once the pointer has moved past the
    /// threshold from a press inside the header.
    pub fn root_mouse_move(&mut self, x: f64, y: f64) -> Option<ColumnDrag> {
        let [down_x, down_y] = self.mouse_down?;
        if math::abs(x - down_x) < DRAG_THRESHOLD_PX && math::abs(y - down_y) < DRAG_THRESHOLD_PX {
            return None;
        }
        let mut index = None;
        let mut cell = Rect::default();
        {
            let dom = self.dom.borrow();
            for (i, rect) in dom.header_cells.iter().enumerate().skip(1) {
                if down_x >= rect.left()
                    && down_x <= rect.right()
                    && down_y >= rect.top()
                    && down_y <= rect.bottom()
                {
                    index = Some(i);
                    cell = *rect;
                }
            }
        }
        self.mouse_down = None;
        index.map(|i| ColumnDrag {
            x,
            y,
            offset_x: x - cell.left(),
            offset_y: y - cell.top(),
            table_index: i - 1,
        })
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
        let is_field = ops::has(&data, "fields")?;
        let is_column = ops::has(&data, "tableIndex")?;
        if !is_field && !is_column {
            return Ok(());
        }

        let mut drag_fields = Vec::new();
        if is_field {
            let fields = ops::get(&data, "fields")?;
            let JsValue::Array(items) = &fields else {
                return Err(JsError::type_error("dragFields.filter is not a function"));
            };
            drag_fields = items
                .iter()
                .filter(|v| {
                    let key = match v {
                        JsValue::String(s) => Some(s),
                        _ => None,
                    };
                    key.and_then(|k| cx.tab_log().get_type(k)) != Some(LoggableType::Empty)
                })
                .cloned()
                .collect();
            if drag_fields.is_empty() {
                return Ok(());
            }
        }

        let x = ops::value_to_number(&ops::get(detail, "x")?).unwrap_or(f64::NAN);
        let y = ops::value_to_number(&ops::get(detail, "y")?).unwrap_or(f64::NAN);
        let end = ops::to_boolean(&ops::get(detail, "end")?);

        let table_box = self.dom.borrow().container;
        let mut selected = None;
        let mut selected_x = 0.0;
        if y > table_box.y {
            let dom = self.dom.borrow();
            let count = dom.header_count;
            for i in 0..count {
                let cell = |i: usize| dom.header_cells.get(i).copied().unwrap_or_default();
                let target_x = if i == 0 && !self.fields.is_empty() {
                    cell(1).left()
                } else {
                    cell(i).right()
                };
                if target_x < cell(0).right() {
                    continue;
                }
                let left_bound = if i == 0 {
                    table_box.x
                } else {
                    target_x - cell(i).width / 2.0
                };
                let right_bound = if i == count - 1 {
                    f64::INFINITY
                } else {
                    target_x + cell(i + 1).width / 2.0
                };
                if left_bound < x && right_bound > x {
                    selected = Some(i);
                    selected_x = target_x;
                }
            }
        }

        if end {
            self.dom.borrow_mut().drag_highlight_hidden = true;
            if let Some(selected) = selected {
                if is_field {
                    for (offset, field) in drag_fields.into_iter().enumerate() {
                        let at = (selected + offset).min(self.fields.len());
                        self.fields.insert(at, field);
                    }
                } else if is_column {
                    let source =
                        ops::value_to_number(&ops::get(&data, "tableIndex")?).unwrap_or(f64::NAN);
                    let source_index = splice_start(source, self.fields.len());
                    if source_index < self.fields.len() {
                        let field = self.fields.remove(source_index);
                        let at = if (selected as f64) <= source {
                            selected as f64
                        } else {
                            selected as f64 - 1.0
                        };
                        self.fields
                            .insert(splice_start(at, self.fields.len()), field);
                    }
                }
            }
        } else {
            let mut dom = self.dom.borrow_mut();
            dom.drag_highlight_hidden = selected.is_none();
            if selected.is_some() {
                dom.drag_highlight_left = Some(selected_x - table_box.x - DRAG_HIGHLIGHT_OFFSET_PX);
            }
        }
        Ok(())
    }
}

impl<Cx: TabCx + ?Sized> TabController<Cx> for TableController {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Array(self.fields.clone())
    }

    fn restore_state(&mut self, _cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        if check_array_type(state, "string") {
            self.fields = state.as_array().unwrap_or_default().to_vec();
        }
        Ok(())
    }

    fn refresh(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    fn new_assets(&mut self, _cx: &mut Cx) -> Result<(), JsError> {
        Ok(())
    }

    /// `getActiveFields()`; string members only, the rest cannot name a field.
    fn active_fields(&self, _cx: &Cx) -> Vec<JsString> {
        self.fields
            .iter()
            .filter_map(|v| match v {
                JsValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect()
    }

    fn show_timeline(&self, _cx: &Cx) -> bool {
        false
    }

    fn command(&mut self, cx: &mut Cx, _uuid: &JsValue) -> Result<Command, JsError> {
        let uuid = self.uuid.clone();
        let (timestamps, offset, fields): (Vec<f64>, f64, Vec<TableCommandField>);
        {
            let log = cx.tab_log();
            let available_keys = log.get_field_keys();

            let mut ranges: Vec<(JsString, [f64; 2])> =
                self.ranges.iter().map(|(k, r)| (k.clone(), *r)).collect();
            array_sort(&mut ranges, |a, b| a.1[0] - b.1[0]);
            let mut merged: Vec<(JsString, [f64; 2])> = Vec::new();
            for (key, range) in ranges {
                match merged.last_mut() {
                    Some((owner, last)) if range[0] <= last[1] => {
                        last[1] = math::max2(last[1], range[1]);
                        // Upstream extends the stored range array in place, so a satellite's
                        // range stays extended until its next `add-table-range`.
                        if let Some(stored) = self.ranges.get_units_mut(owner.units()) {
                            stored[1] = last[1];
                        }
                    }
                    _ => merged.push((key, range)),
                }
            }

            fields = self
                .fields
                .iter()
                .map(|key| {
                    let key_str = match key {
                        JsValue::String(s) => Some(s),
                        _ => None,
                    };
                    let is_available = key_str.is_some_and(|k| available_keys.contains(k));
                    if !is_available {
                        TableCommandField {
                            key: key.clone(),
                            is_available: false,
                            data: None,
                            field_type: None,
                        }
                    } else {
                        let k = key_str.unwrap();
                        let mut data = None;
                        for (_, range) in &merged {
                            if let Some(new) = log.get_range(k, range[0], range[1], Some(&uuid)) {
                                push_range_data(&mut data, new);
                            }
                        }
                        TableCommandField {
                            key: key.clone(),
                            is_available: true,
                            data: data.or(Some(LogValueSetAny::Empty(Default::default()))),
                            field_type: log.get_type(k),
                        }
                    }
                })
                .collect();

            let string_fields: Vec<JsString> = self
                .fields
                .iter()
                .filter_map(|v| match v {
                    JsValue::String(s) => Some(s.clone()),
                    _ => None,
                })
                .collect();
            timestamps = log.get_timestamps(&string_fields, Some(&uuid));
            offset = display_offset(cx);
        }
        let selection = cx.tab_selection();
        Ok(Rc::new(TableCommand {
            timestamps,
            display_offset: offset,
            fields,
            selection_mode: selection.get_mode(),
            selected_time: selection.get_selected_time(),
            hovered_time: selection.get_hovered_time(),
        }))
    }

    fn add_renderer_range(
        &mut self,
        _cx: &mut Cx,
        renderer_uuid: &JsString,
        range: Option<[f64; 2]>,
    ) -> Result<(), JsError> {
        match range {
            None => {
                self.ranges.remove_units(renderer_uuid.units());
            }
            Some(range) => {
                self.ranges.insert(renderer_uuid.clone(), range);
            }
        }
        Ok(())
    }
}

pub struct TableRenderer {
    uuid: JsString,
    dom: Rc<RefCell<TableDom>>,
    row_height_px: f64,
    data_rows: usize,
    data_row_timestamps: Vec<f64>,
    timestamps: Vec<f64>,
    last_fields: Vec<JsString>,
    last_fields_available: Vec<bool>,
    last_scroll_position: Option<f64>,
    did_clear_hovered_time: bool,
    timestamp_range: Option<[f64; 2]>,
    selection_mode: SelectionMode,
    selected_time: Option<f64>,
    hovered_time: Option<f64>,
    display_offset: f64,
}

impl TableRenderer {
    /// `new TableRenderer(element, hasController)`; `uuid` is its `createUUID()`, drawn after
    /// the controller's. `updateFields([])` leaves the single jump-input header cell.
    pub fn new(uuid: JsString, dom: Rc<RefCell<TableDom>>) -> Self {
        dom.borrow_mut().header_count = 1;
        Self {
            uuid,
            dom,
            row_height_px: DEFAULT_ROW_HEIGHT_PX,
            data_rows: 0,
            data_row_timestamps: Vec::new(),
            timestamps: Vec::new(),
            last_fields: Vec::new(),
            last_fields_available: Vec::new(),
            last_scroll_position: None,
            did_clear_hovered_time: false,
            timestamp_range: None,
            selection_mode: SelectionMode::Idle,
            selected_time: None,
            hovered_time: None,
            display_offset: 0.0,
        }
    }

    pub fn dom(&self) -> &Rc<RefCell<TableDom>> {
        &self.dom
    }

    /// `TABLE_BODY`'s `mousemove`.
    pub fn body_mouse_move(&mut self, client_y: f64) {
        self.dom.borrow_mut().hover_cursor_y = Some(client_y);
    }

    /// `TABLE_BODY`'s `mouseleave`.
    pub fn body_mouse_leave(&mut self) {
        self.dom.borrow_mut().hover_cursor_y = None;
    }

    /// A data row's `click`.
    pub fn row_click(&mut self, cx: &mut impl TabCx, row_index: usize) {
        if row_index < self.data_row_timestamps.len() {
            cx.tab_selection()
                .set_selected_time(self.data_row_timestamps[row_index]);
        }
    }

    /// A data row's `contextmenu`.
    pub fn row_context_menu(&mut self, cx: &mut impl TabCx) {
        cx.tab_selection().go_idle();
    }

    /// The jump input's Enter handler and jump-button click.
    pub fn jump(&mut self, cx: &mut impl TabCx) {
        let value = self.dom.borrow().jump_value.clone();
        let target = if value.is_empty() {
            if self.selection_mode != SelectionMode::Idle {
                self.selected_time
            } else {
                cx.tab_selection().set_selected_time(0.0);
                Some(0.0)
            }
        } else {
            let t = asrs_js::number::to_number(value.units()) - self.display_offset;
            cx.tab_selection().set_selected_time(t);
            Some(t)
        };
        self.selected_time = target;
        self.scroll_to_selected();
    }

    fn scroll_to_selected(&mut self) {
        let Some(selected) = self.selected_time else {
            return;
        };
        let mut target_row = upper_bound(&self.timestamps, selected, 0) as f64 - 1.0;
        if target_row == -1.0 {
            target_row = 0.0;
        }
        let mut dom = self.dom.borrow_mut();
        let client_height = math::round(dom.container.height);
        let visible_height = client_height - math::round(dom.header_row.height);
        let first_visible = math::ceil(dom.scroll_top / self.row_height_px);
        let last_visible =
            math::ceil((dom.scroll_top + visible_height - self.row_height_px) / self.row_height_px);
        if target_row < first_visible {
            dom.scroll_top = target_row * self.row_height_px;
        } else if target_row > last_visible {
            dom.scroll_top = target_row * self.row_height_px - visible_height + self.row_height_px;
        }
    }
}

impl<Cx: TabCx + ?Sized> TabRenderer<Cx> for TableRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        None
    }

    fn renderer_uuid(&self) -> Option<&JsString> {
        Some(&self.uuid)
    }

    fn timestamp_range(&self, _cx: &Cx) -> Option<[f64; 2]> {
        self.timestamp_range
    }

    fn render(&mut self, cx: &mut Cx, command: &dyn TabCommand) -> Result<(), JsError> {
        let Some(command) = command.downcast_ref::<TableCommand>() else {
            return Err(JsError::type_error(
                "Cannot read properties of undefined (reading 'timestamps')",
            ));
        };
        let initial_scroll = self.dom.borrow().scroll_top;
        self.display_offset = command.display_offset;
        self.selection_mode = command.selection_mode;
        self.selected_time = command.selected_time;
        self.hovered_time = command.hovered_time;

        let fields: Vec<JsString> = command
            .fields
            .iter()
            .filter_map(|f| match &f.key {
                JsValue::String(s) => Some(s.clone()),
                _ => None,
            })
            .collect();
        let fields_available: Vec<bool> = command.fields.iter().map(|f| f.is_available).collect();
        if !arrays_equal(&fields, &self.last_fields)
            || !arrays_equal(&fields_available, &self.last_fields_available)
        {
            self.dom.borrow_mut().header_count = 1 + command.fields.len();
            self.last_fields = fields;
            self.last_fields_available = fields_available;
        }

        let client_height = math::round(self.dom.borrow().container.height);
        let data_row_count =
            math::ceil(client_height / self.row_height_px) as usize + DATA_ROW_BUFFER * 2;
        self.data_rows = data_row_count;
        if self.data_rows > 0 {
            let row_height = self.dom.borrow().data_row.height;
            if row_height > 0.0 && row_height != self.row_height_px {
                self.row_height_px = row_height;
            }
        }

        let match_index = if command.timestamps.len() > 1 {
            lower_bound(&self.timestamps, command.timestamps[1], 0) as f64
        } else {
            -1.0
        };
        let removed_rows = if match_index != -1.0
            && self.timestamps.get(match_index as usize).copied() == Some(command.timestamps[1])
        {
            match_index - 1.0
        } else {
            -2.0
        };
        if removed_rows > 0.0 {
            self.dom.borrow_mut().scroll_top = initial_scroll - removed_rows * self.row_height_px;
        }
        self.timestamps = command.timestamps.clone();

        let (data_row_start, data_row_end);
        if self.timestamps.len() < data_row_count {
            data_row_start = 0;
            data_row_end = self.timestamps.len();
        } else {
            let scroll_row = math::floor(self.dom.borrow().scroll_top / self.row_height_px);
            let mut filler_upper = 0.0;
            if scroll_row > DATA_ROW_BUFFER as f64 {
                filler_upper = scroll_row - DATA_ROW_BUFFER as f64;
            }
            if filler_upper + data_row_count as f64 > self.timestamps.len() as f64 {
                filler_upper = self.timestamps.len() as f64 - data_row_count as f64;
            }
            data_row_start = filler_upper as usize;
            data_row_end = data_row_start + data_row_count;
        }

        self.data_row_timestamps = self.timestamps[data_row_start..data_row_end].to_vec();
        self.timestamp_range = self
            .data_row_timestamps
            .first()
            .map(|first| [*first, *self.data_row_timestamps.last().unwrap()]);

        match self.selection_mode {
            SelectionMode::Playback => self.scroll_to_selected(),
            SelectionMode::Locked => {
                if self
                    .last_scroll_position
                    .is_some_and(|last| initial_scroll < last)
                {
                    cx.tab_selection().unlock();
                } else {
                    self.scroll_to_selected();
                }
            }
            _ => {}
        }
        self.last_scroll_position = Some(self.dom.borrow().scroll_top);

        let header_bottom = self.dom.borrow().header_row.bottom();
        let hover_y = self.dom.borrow().hover_cursor_y;
        match hover_y {
            Some(y) if y > header_bottom => {
                let rect = self.dom.borrow().data_row;
                for &time in &self.data_row_timestamps {
                    if y >= rect.top() && y < rect.bottom() {
                        cx.tab_selection().set_hovered_time(Some(time));
                        self.did_clear_hovered_time = false;
                    }
                }
            }
            _ if !self.did_clear_hovered_time => {
                cx.tab_selection().set_hovered_time(None);
                self.did_clear_hovered_time = true;
            }
            _ => {}
        }
        Ok(())
    }
}
