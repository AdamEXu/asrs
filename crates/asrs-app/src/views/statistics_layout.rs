//! Layout of `div.stats-values-container` and its automatic table, as Blink lays them out, and
//! of `div.stats-histogram-container` beside it. Answers `StatisticsRenderer`'s layout read.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_canvas::text::dom::Lu;
use asrs_canvas::text::{FixedAdvance, TextBackend, platform_backend};
use asrs_model::scroll_sensor::{Wheel, WheelScroll, assign_scroll_offset};
use asrs_tabs::statistics::renderer::StatisticsGeometry;
use asrs_tabs::statistics::values::StatisticsRow;
use asrs_ui::text::TextStyle;
use asrs_ui::theme::{FONT_FAMILY, MONOSPACE_FAMILY};

use super::cell_text::{CellText, border, css, length, px};

/// `table.stats-values`: `margin: 10px`; also what the histogram keeps to its right and below.
const MARGIN: f64 = 10.0;
const PADDING: [f64; 2] = [6.0, 2.0];
/// `tr.values td:first-child`.
const TITLE_WIDTH: f64 = 100.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    Section,
    Title,
    Value,
}

pub fn cell_style(kind: CellKind) -> TextStyle {
    let family = FONT_FAMILY.join(", ");
    let (font, align) = match kind {
        CellKind::Section => (format!("600 14px {family}"), "center"),
        CellKind::Title => (format!("12px {family}"), "right"),
        CellKind::Value => (format!("14px {MONOSPACE_FAMILY}"), "start"),
    };
    TextStyle::from_css([
        ("font", font.as_str()),
        ("text-align", align),
        ("white-space", "normal"),
        ("overflow-wrap", "break-word"),
    ])
    .expect("statistics cell style")
}

/// A cell's border box from the table's left edge, in device units.
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedCell {
    pub kind: CellKind,
    pub left: Lu,
    pub width: Lu,
    pub text: CellText,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlacedRow {
    /// From the table's top edge.
    pub top: Lu,
    pub height: Lu,
    pub cells: Vec<PlacedCell>,
}

/// The table's layout in device units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placed {
    pub scale: f32,
    /// The container's border box in the renderer element, whose left and top are 0.
    pub container: [Lu; 2],
    pub table: [Lu; 2],
    pub margin: Lu,
    pub border: Lu,
    pub padding: [Lu; 2],
    pub rows: Vec<PlacedRow>,
}

impl Placed {
    /// Where a cell's content box starts, from the table's top left corner.
    pub fn content(&self, row: &PlacedRow, cell: &PlacedCell) -> [Lu; 2] {
        let free = row.height - 2 * (self.padding[1] + self.border) - cell.text.height;
        [
            cell.left + self.border + self.padding[0],
            row.top + self.border + self.padding[1] + free / 2,
        ]
    }

    fn scroll_size(&self) -> [Lu; 2] {
        [0, 1].map(|axis| (self.table[axis] + 2 * self.margin).max(self.container[axis]))
    }

    /// The largest scroll offset per axis, in CSS px: Blink's is whole device px.
    fn max_scroll(&self) -> [f64; 2] {
        let scroll = self.scroll_size();
        [0, 1].map(|axis| css(round(scroll[axis]) - round(self.container[axis]), self.scale))
    }
}

/// The nearest whole device px.
pub fn round(lu: Lu) -> Lu {
    (lu + px(1) / 2).div_euclid(px(1)) * px(1)
}

struct Fonts {
    section: TextStyle,
    title: TextStyle,
    value: TextStyle,
}

impl Default for Fonts {
    fn default() -> Self {
        Fonts {
            section: cell_style(CellKind::Section),
            title: cell_style(CellKind::Title),
            value: cell_style(CellKind::Value),
        }
    }
}

struct State {
    backend: Box<dyn TextBackend>,
    fonts: Fonts,
    /// The renderer element's size in CSS px and the device scale.
    size: [f64; 2],
    scale: f32,
    rows: Vec<StatisticsRow>,
    placed: Option<Rc<Placed>>,
    /// The scroll offset before it is snapped to the device pixel grid.
    offset: [f64; 2],
    wheel: WheelScroll,
}

/// One statistics tab's layout, shared by the view and the renderer.
#[derive(Clone)]
pub struct StatisticsLayout(Rc<RefCell<State>>);

impl Default for StatisticsLayout {
    fn default() -> Self {
        StatisticsLayout(Rc::new(RefCell::new(State {
            backend: platform_backend().unwrap_or_else(|| Box::new(FixedAdvance)),
            fonts: Fonts::default(),
            size: [0.0; 2],
            scale: 1.0,
            rows: Vec::new(),
            placed: None,
            offset: [0.0; 2],
            wheel: WheelScroll::default(),
        })))
    }
}

/// Each column's min-content and max-content width, borders and padding included.
fn column_widths(state: &mut State, extra: Lu) -> Vec<[Lu; 2]> {
    let State {
        backend,
        fonts,
        scale,
        rows,
        ..
    } = state;
    let backend = backend.as_mut();
    let mut columns: Vec<[Lu; 2]> = vec![[0, length(TITLE_WIDTH, *scale) + extra]];
    let grow = |column: &mut [Lu; 2], min: Lu, max: Lu| {
        column[0] = column[0].max(min + extra);
        column[1] = column[1].max(max + extra);
    };
    for row in rows.iter() {
        let StatisticsRow::Values { title, cells } = row else {
            continue;
        };
        let min = CellText::min_content(backend, &fonts.title, title, *scale);
        grow(&mut columns[0], min, min);
        columns.resize(columns.len().max(1 + cells.len()), [0; 2]);
        for (column, cell) in columns[1..].iter_mut().zip(cells) {
            let min = CellText::min_content(backend, &fonts.value, &cell.text, *scale);
            let max = CellText::new(backend, &fonts.value, &cell.text, None, *scale).width;
            grow(column, min, max);
        }
    }
    columns
}

/// The automatic table layout's column widths for a table `inner` wide between its borders: the
/// title column, the only fixed one, grows to its width before the others grow to theirs.
fn distribute(columns: &[[Lu; 2]], inner: Lu) -> Vec<Lu> {
    let mut widths: Vec<Lu> = columns.iter().map(|c| c[0]).collect();
    let mut left = (inner - widths.iter().sum::<Lu>()).max(0);
    let title = (columns[0][1] - columns[0][0]).min(left);
    widths[0] += title;
    left -= title;
    let room: i64 = columns[1..].iter().map(|c| i64::from(c[1] - c[0])).sum();
    let Some(last) = columns[1..].iter().rposition(|c| c[1] > c[0]) else {
        return widths;
    };
    let mut rest = left;
    for (i, column) in columns[1..].iter().enumerate() {
        let share = if i == last {
            rest
        } else {
            (i64::from(left) * i64::from(column[1] - column[0]) / room) as Lu
        };
        widths[1 + i] += share;
        rest -= share;
    }
    widths
}

impl State {
    fn place(&mut self) -> Rc<Placed> {
        if let Some(placed) = &self.placed {
            return placed.clone();
        }
        let scale = self.scale;
        let [width, height] = self.size.map(|v| (v as f32 * scale * 64.0).round() as Lu);
        let border = border(scale);
        let padding = PADDING.map(|p| length(p, scale));
        let margin = length(MARGIN, scale);
        let extra = 2 * (border + padding[0]);
        let columns = if self.rows.is_empty() {
            Vec::new()
        } else {
            column_widths(self, extra)
        };
        let sum = |k: usize| columns.iter().map(|c| c[k]).sum::<Lu>() + 2 * border;
        let container = (sum(1) + 2 * margin).min(width / 2);
        let table_width = (container - 2 * margin).max(sum(0));
        let widths = if columns.is_empty() {
            Vec::new()
        } else {
            distribute(&columns, table_width - 2 * border)
        };

        let State {
            backend,
            fonts,
            rows,
            ..
        } = self;
        let backend = backend.as_mut();
        let mut top = border;
        let mut placed_rows = Vec::with_capacity(rows.len());
        for row in rows.iter() {
            let mut cells = Vec::new();
            let mut left = border;
            let mut cell = |kind: CellKind, text: &str, width: Lu| {
                let style = match kind {
                    CellKind::Section => &fonts.section,
                    CellKind::Title => &fonts.title,
                    CellKind::Value => &fonts.value,
                };
                let available = (width - extra).max(0);
                let text = CellText::new(backend, style, text, Some(available), scale);
                cells.push(PlacedCell {
                    kind,
                    left,
                    width,
                    text,
                });
                left += width;
            };
            match row {
                StatisticsRow::Section { title, col_span } => {
                    let width = widths.iter().take(*col_span).sum();
                    cell(CellKind::Section, title, width);
                }
                StatisticsRow::Values {
                    title,
                    cells: values,
                } => {
                    cell(CellKind::Title, title, widths[0]);
                    for (value, &width) in values.iter().zip(&widths[1..]) {
                        cell(CellKind::Value, &value.text, width);
                    }
                }
            }
            let content = cells.iter().map(|c| c.text.height).max().unwrap_or(0);
            let height = content + 2 * (border + padding[1]);
            placed_rows.push(PlacedRow { top, height, cells });
            top += height;
        }
        let placed = Rc::new(Placed {
            scale,
            container: [container, height],
            table: [table_width, top + border],
            margin,
            border,
            padding,
            rows: placed_rows,
        });
        self.placed = Some(placed.clone());
        placed
    }

    fn set_rows(&mut self, rows: &[StatisticsRow]) {
        if self.rows != rows {
            self.rows = rows.to_vec();
            self.placed = None;
        }
    }

    fn scroll_to(&mut self, offset: [f64; 2]) {
        let max = self.place().max_scroll();
        self.offset = [0, 1].map(|axis| offset[axis].clamp(0.0, max[axis]));
    }
}

impl StatisticsLayout {
    /// Sizes the renderer element.
    pub fn resize(&self, size: [f64; 2], scale: f32) {
        let mut state = self.0.borrow_mut();
        if (state.size, state.scale) != (size, scale) {
            state.size = size;
            state.scale = scale;
            state.placed = None;
        }
    }

    pub fn place(&self, rows: &[StatisticsRow]) -> Rc<Placed> {
        let mut state = self.0.borrow_mut();
        state.set_rows(rows);
        let placed = state.place();
        let offset = state.offset;
        state.scroll_to(offset);
        placed
    }

    /// `div.stats-histogram-container`'s box in the renderer element, in CSS px. Without an
    /// inline `left` it shrinks to its canvas, `canvas` wide.
    pub fn histogram(&self, left: Option<f64>, canvas: f64) -> [f64; 4] {
        let state = self.0.borrow();
        let [width, height] = state.size;
        let right = width - MARGIN;
        let left = left.unwrap_or((right - canvas).max(0.0));
        [
            left,
            0.0,
            (right - left).max(0.0),
            (height - MARGIN).max(0.0),
        ]
    }

    pub fn scroll(&self) -> [f64; 2] {
        let mut state = self.0.borrow_mut();
        let max = state.place().max_scroll();
        let scale = f64::from(state.scale);
        [0, 1].map(|axis| assign_scroll_offset(state.offset[axis], max[axis], scale))
    }

    /// A wheel event at `now` (ms).
    pub fn wheel(&self, wheel: Wheel, now: f64) {
        let mut state = self.0.borrow_mut();
        let placed = state.place();
        let client = placed.container.map(|v| css(v, placed.scale));
        let (offset, max) = (state.offset, placed.max_scroll());
        if let Some(to) = state.wheel.wheel(wheel, now, offset, max, client) {
            state.scroll_to(to);
        }
    }

    /// Advances a notched wheel animation to the frame at `now` (ms); whether it goes on.
    pub fn animate(&self, now: f64) -> bool {
        let mut state = self.0.borrow_mut();
        if let Some(to) = state.wheel.animate(now) {
            state.scroll_to(to);
        }
        state.wheel.animating()
    }

    pub fn stop_wheel(&self) {
        self.0.borrow_mut().wheel.stop();
    }
}

impl StatisticsGeometry for StatisticsLayout {
    fn values_offset_width(&mut self, rows: &[StatisticsRow]) -> f64 {
        let mut state = self.0.borrow_mut();
        state.set_rows(rows);
        let placed = state.place();
        asrs_js::math::floor(css(round(placed.container[0]), placed.scale))
    }
}
