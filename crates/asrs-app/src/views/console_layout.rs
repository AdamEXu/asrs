//! Layout of `table.console-table`: the cells' line boxes, the rows' heights and the scroll
//! position of `div.console-table-container`, answering `ConsoleRenderer`'s layout reads.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use asrs_canvas::text::dom::{self, LineHeight, Lu};
use asrs_canvas::text::inline::{Breaker, Item, Line};
use asrs_canvas::text::{FixedAdvance, FontSpec, TextBackend, platform_backend};
use asrs_js::string::JsString;
use asrs_model::source_list::Rect;
use asrs_tabs::ansi::AnsiStyle;
use asrs_tabs::console::ConsoleNode;
use asrs_tabs::console::controller::ConsoleGeometry;
use asrs_tabs::console::view::ConsoleRow;
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
    scroll_top: f64,
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
            scroll_top: 0.0,
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

    fn sync(&mut self, rows: &[ConsoleRow]) {
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
        if rows.len() < self.rows.len() {
            self.rows.truncate(rows.len());
        }
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
        self.scroll_top = self.clamp(self.scroll_top);
    }

    fn scroll_height(&self) -> f64 {
        let lines = self.tops.last().copied().unwrap_or(0) as f64;
        (HEADER_HEIGHT + lines * LINE_HEIGHT + TABLE_MARGIN).max(self.container[3])
    }

    /// The range of `scrollTop`, which Chromium keeps on whole px.
    fn clamp(&self, top: f64) -> f64 {
        let max = (self.scroll_height() - self.container[3]).max(0.0);
        if top.is_nan() {
            return 0.0;
        }
        top.clamp(0.0, max).round()
    }
}

impl ConsoleLayout {
    /// Places the container and lays out the rows that changed.
    pub fn sync(&self, container: [f64; 4], scale: f32, rows: &[ConsoleRow]) {
        let mut state = self.0.borrow_mut();
        state.container = container;
        state.scale = scale;
        state.sync(rows);
    }

    pub fn scroll_top(&self) -> f64 {
        self.0.borrow().scroll_top
    }

    pub fn scroll_height(&self) -> f64 {
        self.0.borrow().scroll_height()
    }

    /// A user scroll to `top`; returns whether the position changed.
    pub fn scroll_to(&self, top: f64) -> bool {
        let mut state = self.0.borrow_mut();
        let top = state.clamp(top);
        std::mem::replace(&mut state.scroll_top, top) != top
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
        state.scroll_top
    }

    fn set_scroll_top(&mut self, rows: &[ConsoleRow], value: f64) {
        let mut state = self.0.borrow_mut();
        state.sync(rows);
        state.scroll_top = state.clamp(value);
    }

    fn scroll_height(&mut self, rows: &[ConsoleRow]) -> f64 {
        let mut state = self.0.borrow_mut();
        state.sync(rows);
        asrs_js::math::round(state.scroll_height())
    }

    fn container_client_height(&mut self) -> f64 {
        asrs_js::math::round(self.0.borrow().container[3])
    }

    fn row_client_height(&mut self, rows: &[ConsoleRow], index: usize) -> f64 {
        let mut state = self.0.borrow_mut();
        state.sync(rows);
        (state.tops[index + 1] - state.tops[index]) as f64 * LINE_HEIGHT
    }
}
