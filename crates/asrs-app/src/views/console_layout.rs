//! Layout of `table.console-table`: the cells' line boxes, the rows' heights and the scroll
//! position of `div.console-table-container`, answering `ConsoleRenderer`'s layout reads.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use asrs_canvas::text::dom::{self, LineHeight, Lu};
use asrs_canvas::text::inline::{self, Breaker, Item, Line};
use asrs_canvas::text::{FixedAdvance, FontSpec, TextBackend, platform_backend};
use asrs_js::string::JsString;
use asrs_model::scroll_sensor::{Wheel, WheelScroll, assign_scroll_offset};
use asrs_model::source_list::Rect;
use asrs_tabs::ansi::AnsiStyle;
use asrs_tabs::console::ConsoleNode;
use asrs_tabs::console::controller::ConsoleGeometry;
use asrs_tabs::console::view::ConsoleRow;
use asrs_ui::selection::geometry;
use asrs_ui::text::TextStyle;

/// The header row: `height: 30px` and its 1 px bottom border.
pub const HEADER_HEIGHT: f64 = 31.0;
/// `th:first-child`: `width: 97px` and its 1 px right border.
pub const TIME_WIDTH: f64 = 98.0;
pub const CELL_PADDING: f64 = 6.0;
pub const LINE_HEIGHT: f64 = 16.0;
/// `margin-bottom` of the table.
const TABLE_MARGIN: f64 = 6.0;
const TAB_SIZE: u8 = 4;
/// Coverage below this is a glyph's edge grazing a pixel, not ink in it.
const INK: u8 = 24;

/// The font of a `td`, or of a rendition inside one.
pub fn cell_font(bold: bool, italic: bool) -> FontSpec {
    let css = match (bold, italic) {
        (false, false) => "12px Courier, monospace",
        (true, false) => "bold 12px Courier, monospace",
        (false, true) => "italic 12px Courier, monospace",
        (true, true) => "italic bold 12px Courier, monospace",
    };
    FontSpec::parse(css).expect("console cell font")
}

pub fn cell_style(font: FontSpec) -> TextStyle {
    let mut style = TextStyle::new(font);
    style.line_height = LineHeight::Px(LINE_HEIGHT as f32);
    style.white_space = dom::WhiteSpace::Pre;
    style
}

#[derive(Clone, Debug, PartialEq)]
pub enum CellNode {
    Text {
        text: String,
        style: AnsiStyle,
        link: bool,
        highlight: bool,
    },
    Break,
}

impl CellNode {
    fn item(&self, bold: bool) -> Item<'_> {
        match self {
            CellNode::Break => Item::Break,
            CellNode::Text { text, style, .. } => Item::Text {
                text,
                font: cell_font(bold || style.bold, style.italic),
            },
        }
    }
}

/// A laid out `td`: piece positions are from its content box's left edge.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub nodes: Vec<CellNode>,
    pub lines: Vec<Line>,
}

/// What a value cell renders, with `\n` for each `<br>`.
pub fn value_text(row: &ConsoleRow) -> String {
    let nodes = row.nodes();
    nodes
        .into_iter()
        .map(|node| match node {
            ConsoleNode::LineBreak => "\n".to_owned(),
            ConsoleNode::Text { text, .. } => text.to_string_lossy(),
        })
        .collect()
}

impl Cell {
    /// The cell's lines, for selecting. `text` is its [`value_text`] and `text_box` the top and
    /// height of a line's text boxes in CSS px.
    pub fn block(&self, text: &str, text_box: (f64, f64), scale: f64) -> geometry::Block {
        let device = |css: f64| dom::lu_round((css * scale) as f32);
        let mut starts = Vec::with_capacity(self.nodes.len());
        let mut at = 0;
        for node in &self.nodes {
            starts.push(at);
            at += match node {
                CellNode::Text { text, .. } => text.len(),
                CellNode::Break => 1,
            };
        }
        let font = cell_font(false, false);
        let mut next = 0;
        let lines = self.lines.iter().enumerate().map(|(i, line)| {
            let top = device(i as f64 * LINE_HEIGHT);
            let pieces: Vec<_> = line
                .pieces
                .iter()
                .map(|piece| {
                    let start = starts[piece.item];
                    let font = match &self.nodes[piece.item] {
                        CellNode::Text { style, .. } => cell_font(style.bold, style.italic),
                        CellNode::Break => font,
                    };
                    geometry::Piece {
                        range: start + piece.range.start..start + piece.range.end,
                        node: piece.item,
                        x: piece.x,
                        width: piece.width,
                        font,
                        tabs: piece.tabs,
                    }
                })
                .collect();
            let end = pieces.last().map_or(next, |p| p.range.end);
            let newline = (text.as_bytes().get(end) == Some(&b'\n')).then_some(end);
            next = newline.map_or(end, |n| n + 1);
            geometry::Line {
                top,
                height: device(LINE_HEIGHT),
                box_top: top + device(text_box.0),
                box_height: device(text_box.1),
                pieces,
                newline,
                start: 0,
            }
        });
        geometry::Block {
            lines: lines.collect(),
            font,
        }
    }
}

fn value_nodes(row: &ConsoleRow) -> Vec<CellNode> {
    let nodes = row.nodes();
    nodes
        .into_iter()
        .map(|node| match node {
            ConsoleNode::LineBreak => CellNode::Break,
            ConsoleNode::Text {
                text,
                style,
                link,
                highlight,
            } => CellNode::Text {
                text: text.to_string_lossy(),
                style,
                link: link.is_some(),
                highlight,
            },
        })
        .collect()
}

struct RowLayout {
    html: Rc<JsString>,
    timestamp: String,
    /// Line boxes in the taller of its cells.
    lines: usize,
}

struct Cells {
    value: Rc<Cell>,
    time: Rc<Cell>,
}

/// What the rows were laid out for; anything else lays all of them out again.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Block {
    width: f64,
    scale: f32,
}

struct State {
    backend: Box<dyn TextBackend>,
    breaker: Breaker,
    /// `div.console-table-container` in client px.
    container: [f64; 4],
    scale: f32,
    block: Block,
    rows: Vec<RowLayout>,
    /// Each row's top below the header, and the last one's bottom, in lines.
    tops: Vec<usize>,
    cells: HashMap<usize, Cells>,
    /// The scroll offset before it is snapped to the device pixel grid.
    offset: f64,
    wheel: WheelScroll,
    /// The scroll anchor: a row and its top relative to the scroll position.
    anchor: Option<(usize, f64)>,
}

/// One console tab's layout, shared by the view, the renderer and the controller.
#[derive(Clone)]
pub struct ConsoleLayout(Rc<RefCell<State>>);

impl Default for ConsoleLayout {
    fn default() -> Self {
        ConsoleLayout(Rc::new(RefCell::new(State {
            backend: platform_backend().unwrap_or_else(|| Box::new(FixedAdvance)),
            breaker: Breaker::default(),
            container: [0.0; 4],
            scale: 1.0,
            block: Block::default(),
            rows: Vec::new(),
            tops: vec![0],
            cells: HashMap::new(),
            offset: 0.0,
            wheel: WheelScroll::default(),
            anchor: None,
        })))
    }
}

impl State {
    fn layout(&mut self, nodes: Vec<CellNode>, bold: bool, width: f64) -> Cell {
        let items: Vec<Item<'_>> = nodes.iter().map(|n| n.item(bold)).collect();
        let available = dom::lu_round((width - 2.0 * CELL_PADDING) as f32 * self.scale);
        let lines = self.breaker.layout(
            self.backend.as_mut(),
            &items,
            available.max(0),
            TAB_SIZE,
            self.scale,
        );
        drop(items);
        Cell { nodes, lines }
    }

    fn value_width(&self) -> f64 {
        (self.block.width - TIME_WIDTH).max(0.0)
    }

    fn lay_out(&mut self, row: &ConsoleRow) -> Cells {
        let time = vec![CellNode::Text {
            text: row.timestamp.clone(),
            style: AnsiStyle::default(),
            link: false,
            highlight: false,
        }];
        let width = self.value_width();
        Cells {
            value: Rc::new(self.layout(value_nodes(row), false, width)),
            time: Rc::new(self.layout(time, true, TIME_WIDTH)),
        }
    }

    /// Lays out the rows that changed. Scroll anchoring keeps the row that was first in view
    /// where it was; Blink keeps the anchor, and where it belongs, until something scrolls.
    fn sync(&mut self, rows: &[ConsoleRow]) {
        let scroll_top = self.scroll_top();
        if scroll_top == 0.0 {
            self.anchor = None;
        } else if self.anchor.is_none() {
            self.anchor = self
                .first_row_below(scroll_top)
                .map(|index| (index, self.row_top(index) - scroll_top));
        }
        let block = Block {
            width: self.container[2],
            scale: self.scale,
        };
        if block != self.block {
            self.block = block;
            self.rows.clear();
            self.cells.clear();
        }
        let mut changed = rows.len().min(self.rows.len());
        self.rows.truncate(rows.len());
        for (i, row) in rows.iter().enumerate() {
            if let Some(known) = self.rows.get(i)
                && Rc::ptr_eq(&known.html, row.html())
                && known.timestamp == row.timestamp
            {
                continue;
            }
            let cells = self.lay_out(row);
            let layout = RowLayout {
                html: row.html().clone(),
                timestamp: row.timestamp.clone(),
                lines: cells.value.lines.len().max(cells.time.lines.len()),
            };
            self.cells.remove(&i);
            changed = changed.min(i);
            match self.rows.get_mut(i) {
                Some(slot) => *slot = layout,
                None => self.rows.push(layout),
            }
        }
        self.tops.truncate(changed + 1);
        for row in &self.rows[changed..] {
            let top = self.tops.last().copied().unwrap_or(0);
            self.tops.push(top + row.lines);
        }
        self.anchor = self.anchor.filter(|(index, _)| *index < self.rows.len());
        if let Some((index, position)) = self.anchor {
            let moved = self.row_top(index) - position;
            if moved != self.scroll_top() {
                self.offset = moved;
            }
        }
        self.offset = self.clamp(self.offset);
    }

    fn row_top(&self, index: usize) -> f64 {
        HEADER_HEIGHT + self.tops[index] as f64 * LINE_HEIGHT
    }

    /// The first row whose bottom is past `y` px from the scrolled content's top.
    fn first_row_below(&self, y: f64) -> Option<usize> {
        let index = self
            .tops
            .partition_point(|&top| HEADER_HEIGHT + top as f64 * LINE_HEIGHT <= y);
        let index = index.saturating_sub(1);
        (index < self.rows.len()).then_some(index)
    }

    fn scroll_height(&self) -> f64 {
        let lines = self.tops.last().copied().unwrap_or(0) as f64;
        (HEADER_HEIGHT + lines * LINE_HEIGHT + TABLE_MARGIN).max(self.container[3])
    }

    fn max_offset(&self) -> f64 {
        (self.scroll_height() - self.container[3]).max(0.0)
    }

    fn clamp(&self, offset: f64) -> f64 {
        if offset.is_nan() {
            return 0.0;
        }
        offset.clamp(0.0, self.max_offset())
    }

    fn scroll_top(&self) -> f64 {
        assign_scroll_offset(self.offset, self.max_offset(), f64::from(self.scale))
    }

    fn set_scroll_top(&mut self, top: f64) {
        self.wheel.stop();
        self.scroll_to(top);
        self.offset = self.scroll_top();
    }

    fn scroll_to(&mut self, offset: f64) {
        let before = self.scroll_top();
        self.offset = self.clamp(offset);
        if self.scroll_top() != before {
            self.anchor = None;
        }
    }
}

impl ConsoleLayout {
    /// Places the container and lays out the rows that changed.
    pub fn sync(&self, container: [f64; 4], scale: f32, rows: &[ConsoleRow]) {
        let mut state = self.0.borrow_mut();
        state.scale = scale;
        state.container = container;
        state.sync(rows);
    }

    pub fn scroll_top(&self) -> f64 {
        self.0.borrow().scroll_top()
    }

    pub fn scale(&self) -> f64 {
        f64::from(self.0.borrow().scale)
    }

    /// `scrollTop = top`.
    pub fn set_scroll_top(&self, top: f64) {
        self.0.borrow_mut().set_scroll_top(top);
    }

    /// The bottom of the table in the scrolled content.
    pub fn table_height(&self) -> f64 {
        let lines = self.0.borrow().tops.last().copied().unwrap_or(0);
        HEADER_HEIGHT + lines as f64 * LINE_HEIGHT
    }

    /// A wheel event at `now` (ms).
    pub fn wheel(&self, wheel: Wheel, now: f64) {
        let mut state = self.0.borrow_mut();
        let offset = [0.0, state.offset];
        let max = [0.0, state.max_offset()];
        let client = [state.container[2], state.container[3]];
        if let Some([_, to]) = state.wheel.wheel(wheel, now, offset, max, client) {
            state.scroll_to(to);
        }
    }

    /// Advances a notched wheel animation to the frame at `now` (ms); whether it goes on.
    pub fn animate(&self, now: f64) -> bool {
        let mut state = self.0.borrow_mut();
        if let Some([_, to]) = state.wheel.animate(now) {
            state.scroll_to(to);
        }
        state.wheel.animating()
    }

    pub fn stop_wheel(&self) {
        self.0.borrow_mut().wheel.stop();
    }

    /// Row `index`'s top and height below the header, in px.
    pub fn row(&self, index: usize) -> (f64, f64) {
        let state = self.0.borrow();
        let top = state.tops[index] as f64 * LINE_HEIGHT;
        (top, state.tops[index + 1] as f64 * LINE_HEIGHT - top)
    }

    /// The row at `y` px below the header.
    pub fn row_at(&self, y: f64) -> Option<usize> {
        if y < 0.0 {
            return None;
        }
        let state = self.0.borrow();
        let line = asrs_js::math::floor(y / LINE_HEIGHT) as usize;
        let index = state.tops.partition_point(|&top| top <= line);
        (index > 0 && index < state.tops.len()).then(|| index - 1)
    }

    /// The rows overlapping `from..to` px below the header.
    pub fn rows_between(&self, from: f64, to: f64) -> std::ops::Range<usize> {
        let state = self.0.borrow();
        let count = state.rows.len();
        let line = |y: f64| asrs_js::math::floor(y.max(0.0) / LINE_HEIGHT) as usize;
        let first = state
            .tops
            .partition_point(|&top| top <= line(from))
            .saturating_sub(1)
            .min(count);
        let last = state
            .tops
            .partition_point(|&top| (top as f64) * LINE_HEIGHT < to);
        first..last.clamp(first, count)
    }

    /// Row `index`'s value and timestamp cells. `row` is the one last synced at that index.
    pub fn cells(&self, index: usize, row: &ConsoleRow) -> (Rc<Cell>, Rc<Cell>) {
        let mut state = self.0.borrow_mut();
        if !state.cells.contains_key(&index) {
            let cells = state.lay_out(row);
            state.cells.insert(index, cells);
        }
        let cells = &state.cells[&index];
        (cells.value.clone(), cells.time.clone())
    }

    /// Forgets the cells outside `rows`.
    pub fn keep_cells(&self, rows: &std::ops::Range<usize>) {
        self.0.borrow_mut().cells.retain(|i, _| rows.contains(i));
    }

    /// Where each glyph of `text`, drawn from client `x`, has ink in the 1 px stripe that starts
    /// `stripe` px below the text box's top: `[left, right]` in client px, in glyph order.
    pub fn ink(&self, font: FontSpec, text: &str, x: f64, stripe: f64) -> Vec<[f64; 2]> {
        let mut state = self.0.borrow_mut();
        let state = &mut *state;
        let scale = state.scale;
        let backend = state.backend.as_mut();
        let (ascent, ..) = dom::line_metrics(backend, &cell_style(font), scale);
        let top = (stripe as f32 * scale).round() as i32 - ascent / dom::LU_PER_PX;
        let rows = top..top + scale.round().max(1.0) as i32;
        let mut out = Vec::new();
        for glyph in inline::glyphs(backend, &font, text, scale) {
            let Some(mask) = backend.raw_mask(glyph.glyph, f64::from(scale), 1.0, 0) else {
                continue;
            };
            if mask.color || mask.width == 0 {
                continue;
            }
            let columns = rows
                .clone()
                .filter_map(|row| usize::try_from(row - mask.top).ok())
                .filter(|row| *row < mask.height as usize)
                .flat_map(|row| {
                    let width = mask.width as usize;
                    let line = &mask.data[row * width..(row + 1) * width];
                    (0..width).filter(move |&c| line[c] > INK)
                });
            let (mut first, mut last) = (None, 0);
            for column in columns {
                first = Some(first.map_or(column, |f: usize| f.min(column)));
                last = last.max(column);
            }
            if let Some(first) = first {
                let at = |column: usize| {
                    let device = f64::from(glyph.x * scale) + f64::from(mask.left) + column as f64;
                    x + device / f64::from(scale)
                };
                out.push([at(first), at(last + 1)]);
            }
        }
        out
    }

    /// The ascent and descent of the cells' text boxes and their top in a line, in px.
    pub fn text_box(&self) -> (f64, f64) {
        let mut state = self.0.borrow_mut();
        let scale = state.scale;
        let style = cell_style(cell_font(false, false));
        let (ascent, descent, line) = dom::line_metrics(state.backend.as_mut(), &style, scale);
        let device = |v: Lu| f64::from(v) / 64.0 / f64::from(scale);
        let half_leading =
            (line - ascent - descent).div_euclid(2 * dom::LU_PER_PX) * dom::LU_PER_PX;
        (device(half_leading), device(ascent + descent))
    }
}

impl ConsoleGeometry for ConsoleLayout {
    fn place(&mut self, container: Rect, device_pixel_ratio: f64) {
        let mut state = self.0.borrow_mut();
        state.container = [container.x, container.y, container.width, container.height];
        state.scale = device_pixel_ratio as f32;
    }

    fn container_rect(&mut self) -> Rect {
        let [x, y, width, height] = self.0.borrow().container;
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn scroll_top(&mut self, rows: &[ConsoleRow]) -> f64 {
        let mut state = self.0.borrow_mut();
        state.sync(rows);
        state.scroll_top()
    }

    fn set_scroll_top(&mut self, rows: &[ConsoleRow], value: f64) {
        let mut state = self.0.borrow_mut();
        state.sync(rows);
        state.set_scroll_top(value);
    }

    fn scroll_height(&mut self, rows: &[ConsoleRow]) -> f64 {
        let mut state = self.0.borrow_mut();
        state.sync(rows);
        asrs_js::math::round(state.scroll_height())
    }

    fn container_client_height(&mut self) -> f64 {
        asrs_js::math::round(self.0.borrow().container[3])
    }

    fn lay_out(&mut self, rows: &[ConsoleRow]) {
        self.0.borrow_mut().sync(rows);
    }

    fn row_client_height(&mut self, index: usize) -> f64 {
        let state = self.0.borrow();
        (state.tops[index + 1] - state.tops[index]) as f64 * LINE_HEIGHT
    }
}
