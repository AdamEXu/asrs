//! The table tab's renderer element (`#renderer4` in `www/hub.html`, styled by
//! `www/renderers.css`), drawn from R11's `TableView` over the boxes of
//! [`TableLayout`](asrs_tabs::table::layout::TableLayout), which answers the renderer's and the
//! controller's layout reads.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use asrs_canvas::text::TextBackend;
use asrs_canvas::text::dom;
use asrs_hub::frame::{DragContent, DragData};
use asrs_hub::hub::{HubModel, HubPage};
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::scroll_sensor::{Wheel, WheelScroll};
use asrs_model::source_list::Rect;
use asrs_model::tab_controller::TabRenderer;
use asrs_model::tab_type::TabType;
use asrs_tabs::table::layout::TableLayout;
use asrs_tabs::table::view::{TableRow, TableView};
use asrs_tabs::table::{TableController, TableDom, TableRenderer};
use asrs_ui::button::IconButton;
use asrs_ui::canvas_view::geometry::mouse_client;
use asrs_ui::focus::{self, ScrollAxis};
use asrs_ui::gpui::{
    AnyElement, App, AppContext, Context, Div, Entity, FocusHandle, Focusable, InteractiveElement,
    IntoElement, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, ScrollEvent,
    SharedString, Styled, Subscription, Window, div, point, px, rgb,
};
use asrs_ui::input::{FieldEvent, FieldKind, FieldStyle, TextField};
use asrs_ui::parity::probed;
use asrs_ui::selection::geometry::{Block, Line, Piece};
use asrs_ui::selection::{Caret, Document, Hit, Place, Position};
use asrs_ui::text::{self, Text, TextStyle, WhiteSpace, snap_remainder};
use asrs_ui::theme::{Appearance, FONT_FAMILY};
use asrs_ui::tooltip::{titled, untitled};

use super::cell_text::following;
use super::frame::{FrameHost, HubFrame, wheel_of};
use super::large_table::{
    BUTTON, Grid, HandFade, HeaderPalette, hand_icon, header_cell, line_through, plain, symbol,
};
use super::number::FormNumber;
use super::page_text::{
    Owner, PageText, PlacedBlock, Selected, beside_table, device, unselectable,
};
use super::scrollers::{Area, smooth};
use asrs_ui::scroll::{self, Extent};

const FIRST_COLUMN_MIN: f64 = 97.0;
const COLUMN_MIN: f64 = 175.0;
const CELL_PADDING: f64 = 4.0;
const KEY_LEFT: f64 = 6.0;
const KEY_RIGHT: f64 = 6.0;
const KEY_RIGHT_WITH_CLOSE: f64 = 30.0;
const KEY_HEIGHT: f64 = 30.0;
const BUTTON_TOP: f64 = 2.5;
const JUMP_INPUT: [f64; 4] = [4.0, 4.5, 64.0, 21.0];
const JUMP_BUTTON_LEFT: f64 = 70.0;
const CLOSE_RIGHT: f64 = 2.0;
const HIGHLIGHT_WIDTH: f64 = 25.0;
const CACHED_CELLS: usize = 8192;
const CACHED_BYTES: usize = 1 << 24;
const PIECE_CHARS: usize = 200;
const PIECES_MEASURED: usize = 16;

fn cell_style(bold: bool) -> TextStyle {
    let font = if bold {
        "bold 14px Courier, monospace"
    } else {
        "14px Courier, monospace"
    };
    TextStyle::from_css([
        ("font", font),
        ("line-height", "16px"),
        ("white-space", "nowrap"),
    ])
    .expect("table cell style")
}

fn key_style(align: &str) -> TextStyle {
    let font = format!("bold 14px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "30px"),
        ("text-align", align),
        ("direction", "rtl"),
        ("unicode-bidi", "plaintext"),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ])
    .expect("table key style")
}

struct Palette {
    line: u32,
    hovered: u32,
    selected: u32,
}

impl Palette {
    fn of(appearance: Appearance) -> Palette {
        appearance.pick(
            Palette {
                line: 0xeeeeee,
                hovered: 0xdddddd,
                selected: 0xaaaaaa,
            },
            Palette {
                line: 0x333333,
                hovered: 0x444444,
                selected: 0x777777,
            },
        )
    }
}

/// The pieces one cell's text is painted in, each with its offset. One glyph run image would
/// outgrow the GPU's textures, so long text is split, between two visible ASCII characters where
/// there are any: those neither join nor kern, and no space at an end is there to collapse.
fn pieces(text: &str, mut measure: impl FnMut(&str) -> f64) -> Vec<(String, f64)> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.len() <= PIECE_CHARS {
        return vec![(text.to_owned(), 0.0)];
    }
    let mut out = Vec::new();
    let (mut start, mut last) = (0, 0);
    for i in 1..chars.len() {
        let safe = chars[i - 1].1.is_ascii_graphic() && chars[i].1.is_ascii_graphic();
        if i - last >= 2 * PIECE_CHARS || (safe && i - last >= PIECE_CHARS) {
            let at = chars[i].0;
            out.push((text[start..at].to_owned(), start));
            (start, last) = (at, i);
        }
    }
    out.push((text[start..].to_owned(), start));
    // A piece's offset is the width before it, measured over a bounded stretch at a time: whole
    // prefixes would make long text quadratic.
    let (mut base, mut from) = (0.0, 0);
    out.into_iter()
        .enumerate()
        .map(|(i, (piece, at))| {
            if i == 0 {
                return (piece, 0.0);
            }
            let offset = base + measure(&text[from..at]);
            if i % PIECES_MEASURED == 0 {
                (base, from) = (offset, at);
            }
            (piece, offset)
        })
        .collect()
}

struct Cell {
    width: f64,
    pieces: Vec<(String, f64)>,
}

struct CachedCell {
    cell: Rc<Cell>,
    frame: u64,
}

/// Measured cell texts, by whether they are bold.
#[derive(Default)]
struct Cells {
    scale: f32,
    frame: u64,
    bytes: usize,
    texts: [HashMap<String, CachedCell>; 2],
}

impl Cells {
    /// Drops what the last frame left unused once the cache is full. Nothing goes during a frame,
    /// which measures its cells before it draws them.
    fn start_frame(&mut self, scale: f32) {
        if self.scale != scale {
            *self = Cells {
                scale,
                ..Cells::default()
            };
        }
        let count: usize = self.texts.iter().map(HashMap::len).sum();
        if count > CACHED_CELLS || self.bytes > CACHED_BYTES {
            let last = self.frame;
            for texts in &mut self.texts {
                texts.retain(|_, cached| cached.frame == last);
            }
            self.bytes = self
                .texts
                .iter()
                .flat_map(HashMap::keys)
                .map(String::len)
                .sum();
        }
        self.frame += 1;
    }

    fn get(&mut self, text: &str, bold: bool, cx: &mut App) -> Rc<Cell> {
        let scale = self.scale;
        self.get_with(text, bold, |text| {
            text::measure(cx, &cell_style(bold), text, scale).0
        })
    }

    fn get_with(
        &mut self,
        text: &str,
        bold: bool,
        mut measure: impl FnMut(&str) -> f64,
    ) -> Rc<Cell> {
        let texts = &mut self.texts[usize::from(bold)];
        if let Some(cached) = texts.get_mut(text) {
            cached.frame = self.frame;
            return cached.cell.clone();
        }
        let cell = Rc::new(Cell {
            width: measure(text),
            pieces: pieces(text, measure),
        });
        self.bytes += text.len();
        let cached = CachedCell {
            cell: cell.clone(),
            frame: self.frame,
        };
        texts.insert(text.to_owned(), cached);
        cell
    }
}

/// The automatic table layout: every cell has `white-space: nowrap`, so a column is as wide as
/// its widest cell, its header cell's `min-width` included.
fn column_widths(
    view: &TableView,
    border: f64,
    mut cell_width: impl FnMut(&str, bool) -> f64,
) -> Vec<f64> {
    let count = 1 + view.headers.len();
    (0..count)
        .map(|column| {
            let border = if column + 1 == count { 0.0 } else { border };
            let min = if column == 0 {
                FIRST_COLUMN_MIN
            } else {
                COLUMN_MIN
            };
            view.rows
                .iter()
                .filter(|row| !row.hidden)
                .filter_map(|row| row.cells.get(column))
                .map(|text| cell_width(text, column == 0) + 2.0 * CELL_PADDING)
                .fold(min, math::max2)
                + border
        })
        .collect()
}

/// The shown rows' cells in document order.
struct TableText<'a> {
    rows: Vec<&'a TableRow>,
    columns: usize,
}

impl<'a> TableText<'a> {
    fn new(view: &'a TableView) -> TableText<'a> {
        TableText {
            rows: view.rows.iter().filter(|row| !row.hidden).collect(),
            columns: 1 + view.headers.len(),
        }
    }
}

impl Document for TableText<'_> {
    fn len(&self) -> usize {
        self.rows.len() * self.columns
    }

    fn text(&self, block: usize) -> Cow<'_, str> {
        let cell = self.rows[block / self.columns]
            .cells
            .get(block % self.columns);
        collapsed(cell.map_or("", String::as_str))
    }

    fn place(&self, block: usize) -> Place {
        let last = (block + 1).is_multiple_of(self.columns);
        Place {
            cell: true,
            tab: block > 0,
            newline_after: last,
            closed: last,
            gap: 1,
            ..Place::default()
        }
    }
}

/// What `white-space: nowrap` renders of a text node.
fn collapsed(text: &str) -> Cow<'_, str> {
    let plain = !text.contains([' ', '\t', '\n', '\r', '\u{c}']);
    if plain {
        Cow::Borrowed(text)
    } else {
        Cow::Owned(dom::collapse(text, WhiteSpace::NoWrap).swap_remove(0))
    }
}

/// A cell's one line.
fn cell_block(
    cell: &Cell,
    text: &str,
    bold: bool,
    scale: f64,
    backend: &mut dyn TextBackend,
) -> Block {
    let style = cell_style(bold);
    let (ascent, descent, height) = dom::line_metrics(backend, &style, scale as f32);
    let half = dom::LU_PER_PX * 2;
    let leading = (height - ascent - descent).div_euclid(half) * dom::LU_PER_PX;
    let mut from = 0;
    let pieces: Vec<Piece> = cell
        .pieces
        .iter()
        .enumerate()
        .map(|(i, (piece, dx))| {
            let range = following(text, from, collapsed(piece).as_ref());
            from = range.end;
            let to = cell.pieces.get(i + 1).map_or(cell.width, |next| next.1);
            let x = device(*dx, scale);
            Piece {
                range,
                node: 0,
                x,
                width: device(to, scale) - x,
                font: style.font,
                tabs: false,
            }
        })
        .collect();
    Block {
        lines: vec![Line {
            top: 0,
            height,
            box_top: leading,
            box_height: ascent + descent,
            pieces: if text.is_empty() { Vec::new() } else { pieces },
            newline: None,
            start: 0,
        }],
        font: style.font,
        truncates: false,
    }
}

/// Where the cells' text is.
struct CellPlaces {
    /// Client left and width of each column.
    columns: Vec<[f64; 2]>,
    rows_top: f64,
    row_height: f64,
    border: f64,
}

impl CellPlaces {
    fn new(layout: &mut TableLayout) -> CellPlaces {
        let columns = (0..layout.columns.len())
            .map(|i| [layout.column_left(i), layout.columns[i]])
            .collect();
        CellPlaces {
            columns,
            rows_top: layout.row_top(0),
            row_height: layout.row_height(),
            border: layout.border_px(),
        }
    }

    /// Where a cell's text of a width starts.
    fn text_origin(&self, row: usize, column: usize, width: f64) -> [f64; 2] {
        let [left, w] = self.columns[column];
        let border = if column + 1 == self.columns.len() {
            0.0
        } else {
            self.border
        };
        let x = if column == 0 {
            left + w - border - CELL_PADDING - width
        } else {
            left + CELL_PADDING
        };
        let top = self.rows_top + row as f64 * self.row_height;
        [x, top + CELL_PADDING]
    }

    /// The column at a client x; the first is over the others.
    fn column_at(&self, x: f64) -> usize {
        let over = |c: &[f64; 2]| x < c[0] + c[1];
        if self.columns.first().is_some_and(over) {
            return 0;
        }
        let column = self.columns.iter().position(over);
        column.unwrap_or(self.columns.len().saturating_sub(1))
    }
}

struct TablePage<'a> {
    doc: TableText<'a>,
    dom: &'a Rc<RefCell<TableDom>>,
    cells: &'a Cells,
    container: [f64; 4],
}

impl PageText for TablePage<'_> {
    fn document(&self) -> &dyn Document {
        &self.doc
    }

    fn scroller(&self) -> [f64; 4] {
        self.container
    }

    fn hit(&self, at @ [x, y]: [f64; 2], backend: &mut dyn TextBackend) -> Hit {
        let mut dom = self.dom.borrow_mut();
        let layout = &mut dom.layout;
        let table = layout.table_rect();
        if y < self.container[1] + layout.header_height() && x < table.right() {
            return unselectable(&self.doc, 0);
        }
        if x < table.left() || x >= table.right() || y >= table.bottom() || self.doc.is_empty() {
            return beside_table(&self.doc, x, [table.x, table.width]);
        }
        let places = CellPlaces::new(layout);
        let scale = layout.device_pixel_ratio;
        let row = math::floor((y - places.rows_top) / places.row_height);
        let row = (row.max(0.0) as usize).min(self.doc.rows.len() - 1);
        let column = places.column_at(x);
        let index = row * self.doc.columns + column;
        let text = self.doc.text(index);
        let cached = |text: &String| self.cells.texts[usize::from(column == 0)].get(text);
        let cell = self.doc.rows[row].cells.get(column).and_then(cached);
        let Some(cell) = cell.filter(|_| !text.is_empty()) else {
            return Hit {
                caret: Some(Caret::new(Position {
                    block: index,
                    offset: 0,
                })),
                selectable: true,
                text: false,
                link: None,
            };
        };
        let block = cell_block(&cell.cell, &text, column == 0, scale, backend);
        let origin = places.text_origin(row, column, cell.cell.width);
        let placed = PlacedBlock {
            index,
            block: &block,
            text: &text,
            origin: origin.map(|v| device(v, scale)),
            scale,
        };
        placed.hit(at, backend)
    }
}

#[derive(Default)]
struct TabView {
    jump: Option<(Entity<TextField<FormNumber>>, Subscription)>,
    wheel: WheelScroll,
    hand: HandFade,
    cells: Cells,
}

#[derive(Clone, Debug)]
struct Shown {
    key: JsString,
    index: usize,
    container: [f64; 4],
}

impl Shown {
    fn contains(&self, x: f64, y: f64) -> bool {
        let [left, top, w, h] = self.container;
        x >= left && x < left + w && y >= top && y < top + h
    }
}

#[derive(Default)]
pub struct TableViews {
    tabs: HashMap<JsString, TabView>,
    shown: Option<Shown>,
    pointer: Option<[f64; 2]>,
    /// Whether the pointer is over `TABLE_BODY`.
    over_body: bool,
    button_pressed: bool,
    pressed_row: Option<usize>,
    /// The header cells' buttons, with their column's left and width.
    deletes: Vec<(FocusHandle, [f64; 2])>,
    /// Where Tab goes on from in the header cells.
    cell_places: Vec<FocusHandle>,
}

fn renderer_key(renderer: &TableRenderer) -> JsString {
    TabRenderer::<HubPage>::renderer_uuid(renderer)
        .cloned()
        .unwrap_or_default()
}

fn displayed(hub: &HubModel) -> Option<(usize, &TableRenderer)> {
    hub.tabs().tabs().enumerate().find_map(|(i, tab)| {
        if tab.content_hidden() || tab.tab_type() != Some(TabType::Table) {
            return None;
        }
        tab.renderer()
            .downcast_ref::<TableRenderer>()
            .map(|r| (i, r))
    })
}

fn with_renderer<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut TableRenderer, &mut HubPage) -> R,
) -> Option<R> {
    let (tabs, page) = hub.tabs_mut();
    let tab = tabs.tab_mut(index)?;
    let renderer = tab.renderer.downcast_mut::<TableRenderer>()?;
    Some(f(renderer, page))
}

fn with_controller<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut TableController) -> R,
) -> Option<R> {
    let (tabs, _) = hub.tabs_mut();
    let tab = tabs.tab_mut(index)?;
    tab.controller.downcast_mut::<TableController>().map(f)
}

impl<H: FrameHost> HubFrame<H> {
    /// The displayed table's renderer element over `rect` (`div.renderer-content`), if the
    /// displayed tab is a table.
    pub(super) fn table_element(
        &mut self,
        rect: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let hub = self.host().hub(cx);
        let live: Vec<JsString> = hub
            .tabs()
            .tabs()
            .filter_map(|t| t.renderer().downcast_ref::<TableRenderer>())
            .map(renderer_key)
            .collect();
        let shown = displayed(hub).map(|(index, renderer)| {
            (
                Shown {
                    key: renderer_key(renderer),
                    index,
                    container: rect,
                },
                renderer.view().clone(),
                renderer.dom().clone(),
            )
        });
        let key = shown.as_ref().map(|(s, ..)| s.key.clone());
        let left = self
            .table
            .shown
            .take()
            .filter(|s| Some(&s.key) != key.as_ref());
        if let Some(left) = left {
            self.table_hidden(&left, window, cx);
        }
        self.table.tabs.retain(|key, _| live.contains(key));
        self.table.shown = shown.as_ref().map(|(s, ..)| s.clone());
        let (shown, view, dom) = shown?;

        let now = self.host().frame_time(cx);
        let tab = self.table.tabs.entry(shown.key.clone()).or_default();
        let (hand, fading) = tab.hand.opacity(view.show_hand, now);
        let [x, y, width, height] = rect;
        {
            let mut dom = dom.borrow_mut();
            let layout = &mut dom.layout;
            layout.scrollbar = scroll::style(cx).reserved();
            layout.container = Rect {
                x,
                y,
                width,
                height,
            };
            layout.device_pixel_ratio = f64::from(scale);
            tab.cells.start_frame(scale);
            layout.columns = column_widths(&view, layout.border_px(), |text, bold| {
                tab.cells.get(text, bold, cx).width
            });
            layout.key_text = view
                .headers
                .iter()
                .map(|h| {
                    let key = h.text.to_string_lossy();
                    math::round(text::measure(cx, &key_style("left"), &key, scale).0)
                })
                .collect();
            if let Some(offset) = tab.wheel.animate(now) {
                layout.set_wheel_offset(offset);
            }
        }
        if fading || tab.wheel.animating() {
            window.request_animation_frame();
        }
        let moved_away = self.table.over_body
            && !self.table.button_pressed
            && self
                .table
                .pointer
                .is_some_and(|[x, y]| !dom.borrow_mut().layout.body_contains(x, y));
        if moved_away {
            self.table_leave(shown.index, cx);
        }

        let grid = Grid {
            origin: [x, y],
            scale: f64::from(scale),
        };
        let owner = Owner::Table(shown.key.clone());
        let selected = (self.text).selected(&owner, &TableText::new(&view), window, cx);
        let jump = self.jump_field(&shown, &view, window, cx);
        let dark = appearance == Appearance::Dark;
        let mut root = div().absolute().left_0().top_0().size_full();
        root = root.child(hand_icon(rect, hand, dark, grid));
        root = root.child(self.table_body(
            &view, &dom, jump, rect, appearance, grid, selected, window, cx,
        ));
        if let Some(extent) = self.table_extent(cx) {
            root = root.child(self.area_bars(Area::Table(shown.key.clone()), extent, cx));
        }
        let dom = dom.borrow();
        if let (false, Some(left)) = (dom.drag_highlight_hidden, dom.drag_highlight_left) {
            root = root.child(
                grid.place([x + left, y, HIGHLIGHT_WIDTH, height])
                    .bg(rgb(0x90ee90))
                    .opacity(0.25),
            );
        }
        Some(root.into_any_element())
    }

    #[expect(clippy::too_many_arguments)]
    fn table_body(
        &mut self,
        view: &TableView,
        dom: &Rc<RefCell<TableDom>>,
        jump: Entity<TextField<FormNumber>>,
        rect: [f64; 4],
        appearance: Appearance,
        grid: Grid,
        selected: Option<Selected>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let shown_key = self.table.shown.as_ref().map(|s| s.key.clone());
        let place = |part: &str, window: &mut Window, cx: &mut Context<'_, Self>| {
            let id = SharedString::from(format!("table-{part}-{shown_key:?}"));
            focus::place(id, window, cx)
        };
        let container_place = place("container", window, cx);
        let rows_place = place("rows", window, cx);
        let palette = Palette::of(appearance);
        let header_palette = HeaderPalette::of(appearance);
        let text_color = appearance.theme().text;
        let [x, y, width, height] = rect;
        let mut dom = dom.borrow_mut();
        let layout = &mut dom.layout;
        let count = layout.columns.len();
        let columns: Vec<[f64; 2]> = (0..count)
            .map(|i| [layout.column_left(i), layout.columns[i]])
            .collect();
        let border = layout.border_px();
        let (header_height, row_height) = (layout.header_height(), layout.row_height());
        let doc = TableText::new(view);
        let rows = &doc.rows;
        let places = CellPlaces::new(layout);
        let rows_top = layout.row_top(0);
        let rows_height = rows.len() as f64 * row_height;
        let first = columns[0];
        let measured = (self.table.shown.as_ref()).and_then(|s| self.table.tabs.get(&s.key));
        let measured = |text: &str, bold: bool| {
            Some(&measured?.cells.texts[usize::from(bold)].get(text)?.cell)
        };

        let mut cells = div().absolute().inset_0();
        let mut first_cells = div().absolute().inset_0().child(
            grid.place([first[0], rows_top, first[1], rows_height])
                .bg(rgb(header_palette.background)),
        );
        for (i, row) in rows.iter().enumerate() {
            let top = rows_top + i as f64 * row_height;
            let visible = top < y + height && top + row_height > y;
            let line = [top + row_height - border, border];
            if visible {
                let background = match (row.selected, row.hovered) {
                    (true, _) => Some(palette.selected),
                    (false, true) => Some(palette.hovered),
                    (false, false) => None,
                };
                let [end, end_width] = columns[count - 1];
                let right = end + end_width;
                for (layer, from, to) in [
                    (&mut cells, first[0], right),
                    (&mut first_cells, first[0], first[0] + first[1]),
                ] {
                    let mut out = std::mem::replace(layer, div());
                    if let Some(color) = background {
                        out = out.child(
                            grid.place([from, top, to - from, row_height])
                                .bg(rgb(color)),
                        );
                    }
                    out = out.child(
                        grid.place([from, line[0], to - from, line[1]])
                            .bg(rgb(palette.line)),
                    );
                    *layer = out;
                }
            }
        }
        if rows_height > 0.0 {
            for (column, [left, w]) in columns.iter().enumerate().take(count - 1) {
                let right = grid.snap(left + w);
                let line = grid
                    .place([right - border, rows_top, border, rows_height])
                    .bg(rgb(palette.line));
                if column == 0 {
                    first_cells = first_cells.child(line);
                } else {
                    cells = cells.child(line);
                }
            }
        }
        for (i, row) in rows.iter().enumerate() {
            let top = rows_top + i as f64 * row_height;
            let visible = top < y + height && top + row_height > y;
            if !visible && !cfg!(feature = "parity") {
                continue;
            }
            for (column, text) in row.cells.iter().take(count).enumerate() {
                let bold = column == 0;
                let Some(cell) = measured(text, bold).filter(|_| !text.is_empty()) else {
                    continue;
                };
                let origin = places.text_origin(i, column, cell.width);
                let text_left = origin[0];
                let index = i * count + column;
                let covered = |s: &&Selected| {
                    (s.range.start.block..=s.range.end.block).contains(&index) && visible
                };
                let mut over = Vec::new();
                if let Some(selected) = selected.as_ref().filter(covered) {
                    let backend = &mut **self.text.backend();
                    let text = doc.text(index);
                    let block = cell_block(cell, &text, bold, grid.scale, backend);
                    let placed = PlacedBlock {
                        index,
                        block: &block,
                        text: &text,
                        origin: origin.map(|v| device(v, grid.scale)),
                        scale: grid.scale,
                    };
                    let layer = if bold { &mut first_cells } else { &mut cells };
                    let drawn = std::mem::replace(layer, div());
                    *layer = selected.paint(grid, &placed, backend, drawn);
                    for (_, piece, rect) in placed.pieces(&selected.range, backend) {
                        let in_view = rect[0] + rect[2] >= x && rect[0] < x + width;
                        let Some(k) = piece.filter(|_| in_view) else {
                            continue;
                        };
                        let (piece, dx) = &cell.pieces[k];
                        over.extend(selected.recoloured(grid, rect, |color| {
                            let text = plain(piece, cell_style(bold), color).unrecorded();
                            vec![grid.text(text, [text_left + dx, top + CELL_PADDING])]
                        }));
                    }
                }
                for (k, (piece, dx)) in cell.pieces.iter().enumerate() {
                    let from = text_left + dx;
                    let to = cell.pieces.get(k + 1).map_or(cell.width, |next| next.1);
                    let cut = cell.pieces.len() > 1;
                    if cut && (text_left + to < x || from >= x + width) {
                        continue;
                    }
                    let at = [from, top + CELL_PADDING];
                    let element = grid.text(plain(piece, cell_style(bold), text_color), at);
                    if bold {
                        first_cells = first_cells.child(element);
                    } else {
                        cells = cells.child(element);
                    }
                }
                if bold {
                    first_cells = first_cells.children(over);
                } else {
                    cells = cells.children(over);
                }
            }
        }

        let Rect {
            x: table_x,
            y: table_y,
            width: table_width,
            height: table_height,
        } = layout.table_rect();
        let mut probes = div().absolute().inset_0().child(probed(
            "table.data-table",
            grid.place([table_x, table_y, table_width, table_height]),
        ));
        for [left, w] in &columns {
            probes = probes.child(probed(
                "table.data-table th",
                grid.place([*left, y, *w, header_height]),
            ));
        }
        let blurred = !self.host().hub(cx).is_focused();
        // `updateFields` replaces every header cell after the first, which drops the focus of one's
        // button. Tab goes on from before them.
        let mut headers = DefaultHasher::new();
        for field in &view.headers {
            (&field.text.to_string_lossy(), field.available).hash(&mut headers);
        }
        let headers = headers.finish();
        let columns_place = place("columns", window, cx);
        let mut header = div()
            .absolute()
            .inset_0()
            .child(focus::hidden(&columns_place).tab_group().tab_index(2));
        let mut deletes = Vec::new();
        let mut cell_places = Vec::new();
        for (index, field) in view.headers.iter().enumerate() {
            let column = index + 1;
            let [left, w] = columns[column];
            let last = column + 1 == count;
            let cell_border = if last { 0.0 } else { border };
            let cell = [left, y, w, header_height];
            let key_right = if view.has_controller {
                KEY_RIGHT_WITH_CLOSE
            } else {
                KEY_RIGHT
            };
            let key_width = math::max2(w - cell_border - KEY_LEFT - key_right, 0.0);
            let key_left = left + KEY_LEFT;
            let key = field.text.to_string_lossy();
            let place = format!("table-key-{shown_key:?}-{headers:x}-{index}");
            let place = focus::place(SharedString::from(place), window, cx);
            cell_places.push(place.clone());
            let mut key_box = grid
                .place([key_left, y, key_width, KEY_HEIGHT])
                .track_focus(&place)
                .overflow_hidden();
            key_box = key_box.child(
                Text::new(key, key_style("left"))
                    .color(text_color)
                    .subpixel(grid.remainder(key_left), grid.remainder(y))
                    .content_width(key_width)
                    .h(px(KEY_HEIGHT as f32)),
            );
            let mut th = div()
                .absolute()
                .inset_0()
                .tab_group()
                .tab_index(column as isize + 1)
                .child(header_cell(grid, cell, !last, border, &header_palette))
                .child(titled(field.title.to_string_lossy(), grid.place(cell)))
                .child(untitled(key_box));
            if !field.available {
                let key = field.text.to_string_lossy();
                let style = key_style("left");
                let at = [key_left, y];
                th = th.children(line_through(
                    &key,
                    &style,
                    at,
                    Some(key_width),
                    text_color,
                    grid,
                    cx,
                ));
            }
            if view.has_controller {
                let at = [
                    left + w - cell_border - CLOSE_RIGHT - BUTTON,
                    y + BUTTON_TOP,
                ];
                let id = format!("table-close-{shown_key:?}-{headers:x}-{index}");
                let delete = focus::place(SharedString::from(format!("{id}-focus")), window, cx);
                cell_places.push(delete.clone());
                deletes.push((delete.clone(), [left, w - cell_border]));
                let button = IconButton::new(SharedString::from(id), symbol("close.svg"))
                    .title("")
                    .blurred(blurred)
                    .track_focus(&delete)
                    .on_click(cx.listener(move |frame, _, _, cx| frame.table_close(index, cx)));
                th = th.child(grid.button(button, at));
                probes = probes.child(probed(
                    "button.data-table-key-delete",
                    grid.place([at[0], at[1], BUTTON, BUTTON]),
                ));
            }
            header = header.child(th);
        }
        let first_cell = [first[0], y, first[1], header_height];
        let [jx, jy, jw, jh] = JUMP_INPUT;
        let jump_at = [first[0] + jx, y + jy, jw, jh];
        let jump_id = SharedString::from(format!("table-jump-{shown_key:?}"));
        let button = IconButton::new(jump_id, symbol("verticalalignbottom.svg"))
            .blurred(blurred)
            .title("Go to timestamp")
            .on_click(cx.listener(|frame, _, _, cx| frame.table_jump(cx)));
        let button_at = [first[0] + JUMP_BUTTON_LEFT, y + BUTTON_TOP];
        header = header
            .child(header_cell(
                grid,
                first_cell,
                count > 1,
                border,
                &header_palette,
            ))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .tab_group()
                    .tab_index(1)
                    .child(titled("Go to timestamp", grid.place(jump_at).child(jump)))
                    .child(grid.button(button, button_at)),
            );
        self.table.deletes = deletes;
        let gone: Vec<FocusHandle> = std::mem::replace(&mut self.table.cell_places, cell_places)
            .into_iter()
            .filter(|place| !self.table.cell_places.contains(place))
            .collect();
        self.focus_removed(&gone, &columns_place);
        probes = probes
            .child(probed("input.data-table-jump-input", grid.place(jump_at)))
            .child(probed(
                "button.data-table-jump-button",
                grid.place([button_at[0], button_at[1], BUTTON, BUTTON]),
            ));

        let area = self
            .table
            .shown
            .as_ref()
            .map(|s| Area::Table(s.key.clone()));
        div()
            .id("data-table-container")
            .track_focus(&container_place)
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .overflow_hidden()
            .on_any_mouse_down(
                cx.listener(|frame, event: &MouseDownEvent, _, cx| {
                    frame.table_mouse_down(event, cx)
                }),
            )
            .capture_any_mouse_down(cx.listener(move |frame, _, _, _| {
                if let Some(area) = &area {
                    frame.press_area(area);
                }
            }))
            .on_scroll(
                cx.listener(|frame, event: &ScrollEvent, _, cx| frame.table_wheel(event, cx)),
            )
            .child(
                grid.place([x, y, width, rows_top + rows_height - y])
                    .track_focus(&focus::last(&rows_place)),
            )
            .child(probes)
            .child(cells)
            .child(first_cells)
            .child(header)
            .into_any_element()
    }
}

impl<H: FrameHost> HubFrame<H> {
    /// The displayed table's text that can be selected.
    pub(super) fn table_text<'a>(&'a self, cx: &'a App) -> Option<(Owner, Box<dyn PageText + 'a>)> {
        let (_, renderer) = displayed(self.host().hub(cx))?;
        let shown = self.table.shown.as_ref()?;
        let page = TablePage {
            doc: TableText::new(renderer.view()),
            dom: renderer.dom(),
            cells: &self.table.tabs.get(&shown.key)?.cells,
            container: shown.container,
        };
        Some((Owner::Table(shown.key.clone()), Box::new(page)))
    }

    /// `AutoscrollController`'s scrolling of the container.
    pub(super) fn table_scroll_by(&mut self, by: [f64; 2], cx: &mut App) {
        self.table_input(cx, |renderer, _| {
            let mut dom = renderer.dom().borrow_mut();
            let [left, top] = dom.layout.scroll();
            dom.layout.scroll_to([left + by[0], top + by[1]]);
        });
    }

    /// Scrolls the table to the focused header button.
    pub(super) fn scroll_table(&mut self, focused: &FocusHandle, cx: &mut App) {
        let mut deletes = self.table.deletes.iter();
        let Some((_, [left, width])) = deletes.find(|(delete, _)| delete == focused).cloned()
        else {
            return;
        };
        self.table_input(cx, |renderer, _| {
            let mut dom = renderer.dom().borrow_mut();
            let layout = &mut dom.layout;
            let button = left + width - CLOSE_RIGHT - BUTTON - layout.container.x;
            let [visible, _] = layout.client();
            let by = focus::reveal(ScrollAxis::Horizontal, visible, [button, BUTTON]);
            if by != 0.0 {
                let [x, y] = layout.scroll();
                layout.scroll_to([x + by, y]);
            }
        });
    }

    /// The table whose header holds the focused element.
    pub(super) fn focused_table(&self, focused: &FocusHandle) -> Option<JsString> {
        let mut deletes = self.table.deletes.iter();
        let shown = self.table.shown.as_ref()?;
        deletes
            .any(|(delete, _)| delete == focused)
            .then(|| shown.key.clone())
    }

    fn jump_field(
        &mut self,
        shown: &Shown,
        view: &TableView,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<TextField<FormNumber>> {
        let tab = self.table.tabs.entry(shown.key.clone()).or_default();
        if tab.jump.is_none() {
            let field = cx.new(|cx| {
                TextField::new(
                    FormNumber::default(),
                    FieldKind::Number,
                    FieldStyle::native(),
                    window,
                    cx,
                )
            });
            let subscription =
                cx.subscribe(&field, |frame, field, event: &FieldEvent, cx| match event {
                    FieldEvent::Input => {
                        let value = JsString::from(field.read(cx).value().as_str());
                        frame.table_input(cx, |renderer, _| {
                            renderer.dom().borrow_mut().jump_input.set_value(value)
                        });
                    }
                    FieldEvent::Enter => frame.table_jump(cx),
                    FieldEvent::Focus | FieldEvent::Blur | FieldEvent::Change => {}
                });
            tab.jump = Some((field, subscription));
        }
        let field = tab.jump.as_ref().expect("the jump input").0.clone();
        let placeholder = view.jump_placeholder.clone();
        let [_, top, width, height] = JUMP_INPUT;
        let y = shown.container[1] + top;
        let dy = snap_remainder(y as f32, window.scale_factor());
        field.update(cx, |field, _| {
            field.set_placeholder(placeholder);
            field.set_size(width as f32, height as f32);
            field.set_subpixel(0.0, dy);
        });
        field
    }

    /// A table that stopped being displayed: its focused input blurs and its body is left.
    fn table_hidden(&mut self, left: &Shown, window: &mut Window, cx: &mut Context<'_, Self>) {
        let index = self.host().hub(cx).tabs().tabs().position(|tab| {
            tab.renderer()
                .downcast_ref::<TableRenderer>()
                .is_some_and(|renderer| renderer_key(renderer) == left.key)
        });
        if let Some(index) = index.filter(|_| self.table.over_body) {
            self.table_leave(index, cx);
        }
        self.table.over_body = false;
        self.table.pressed_row = None;
        let Some(tab) = self.table.tabs.get_mut(&left.key) else {
            return;
        };
        tab.hand = HandFade::default();
        tab.wheel.stop();
        if let Some((field, _)) = &tab.jump
            && field.focus_handle(cx).is_focused(window)
        {
            let field = field.clone();
            field.update(cx, |field, cx| field.blur(window, cx));
        }
    }

    /// Runs `f` on the displayed table's renderer as one page task.
    fn table_input<R>(
        &mut self,
        cx: &mut App,
        f: impl FnOnce(&mut TableRenderer, &mut HubPage) -> R,
    ) -> Option<R> {
        let index = self.table.shown.as_ref()?.index;
        self.host_mut()
            .input(cx, |hub| with_renderer(hub, index, f))
    }

    fn table_jump(&mut self, cx: &mut Context<'_, Self>) {
        self.table_input(cx, |renderer, page| renderer.jump(page));
        cx.notify();
    }

    /// The close button's `close-field` event.
    fn table_close(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.table.shown.clone() else {
            return;
        };
        self.host_mut().input(cx, |hub| {
            with_controller(hub, shown.index, |controller| {
                controller.close_field(&JsValue::Number(index as f64))
            })
        });
        cx.notify();
    }

    fn table_leave(&mut self, index: usize, cx: &mut App) {
        self.table.over_body = false;
        self.host_mut().input(cx, |hub| {
            with_renderer(hub, index, |renderer, _| renderer.body_mouse_leave())
        });
    }

    /// Whether `#dragItem` or the drag highlight is over the table at a client position.
    fn table_covered(&mut self, shown: &Shown, x: f64, y: f64, cx: &App) -> bool {
        let shell = self.shell_mut();
        let drag = shell.drag();
        if drag.item_hidden() || !drag.active() {
            return false;
        }
        let Some((left, top)) = drag.item_position() else {
            return false;
        };
        let over_item = match drag.item() {
            Some(DragContent::Column { width, .. }) => {
                x >= left && x < left + KEY_LEFT + width && y >= top && y < top + KEY_HEIGHT
            }
            _ => true,
        };
        let highlight = displayed(self.host().hub(cx)).and_then(|(_, renderer)| {
            let dom = renderer.dom().borrow();
            dom.drag_highlight_left
                .filter(|_| !dom.drag_highlight_hidden)
        });
        let over_highlight = highlight.is_some_and(|left| {
            let left = shown.container[0] + left;
            x >= left && x < left + HIGHLIGHT_WIDTH
        });
        over_item || over_highlight
    }

    /// The table body's and the renderer element's `mousemove` listeners, which run before the
    /// window's, and the body's `mouseleave`.
    pub(super) fn table_mouse_move(
        &mut self,
        x: f64,
        y: f64,
        pressed: bool,
        cx: &mut Context<'_, Self>,
    ) {
        self.table.pointer = Some([x, y]);
        self.table.button_pressed = pressed;
        let Some(shown) = self.table.shown.clone() else {
            return;
        };
        let covered = self.table_covered(&shown, x, y, cx);
        let on_root = shown.contains(x, y) && !covered;
        let over = on_root
            && !self.on_scrollbar(point(px(x as f32), px(y as f32)))
            && self
                .table_input(cx, |renderer, _| {
                    renderer.dom().borrow_mut().layout.body_contains(x, y)
                })
                .unwrap_or(false);
        if self.table.over_body && !over {
            self.table_leave(shown.index, cx);
        }
        self.table.over_body = over;
        if over {
            self.table_input(cx, |renderer, _| renderer.body_mouse_move(y));
        }
        if !on_root {
            return;
        }
        let drag = self.host_mut().input(cx, |hub| {
            with_controller(hub, shown.index, |controller| {
                controller.root_mouse_move(x, y)
            })
        });
        if let Some(drag) = drag.flatten() {
            let header = displayed(self.host().hub(cx))
                .and_then(|(_, r)| r.view().headers.get(drag.table_index).cloned());
            if let Some(header) = header {
                let content = DragContent::Column {
                    text: header.text,
                    available: header.available,
                    width: drag.width,
                };
                self.shell_mut().start_drag(
                    drag.x,
                    drag.y,
                    drag.offset_x,
                    drag.offset_y,
                    content,
                    DragData::Column(drag.table_index),
                );
            }
        }
        cx.notify();
    }

    /// The pointer left the window.
    pub(super) fn table_mouse_exit(&mut self, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.table.shown.clone() else {
            return;
        };
        if self.table.over_body {
            self.table_leave(shown.index, cx);
            cx.notify();
        }
    }

    /// The pooled data row at a client position, outside the header.
    fn table_row_at(&mut self, shown: &Shown, x: f64, y: f64, cx: &mut App) -> Option<usize> {
        if !shown.contains(x, y) {
            return None;
        }
        self.table_input(cx, |renderer, _| {
            let mut dom = renderer.dom().borrow_mut();
            let layout = &mut dom.layout;
            let below_header = y >= shown.container[1] + layout.header_height();
            (below_header && layout.body_contains(x, y))
                .then(|| layout.row_at(y))
                .flatten()
        })
        .flatten()
    }

    fn table_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.table.shown.clone() else {
            return;
        };
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        self.table.button_pressed = true;
        self.host_mut().input(cx, |hub| {
            with_controller(hub, shown.index, |controller| {
                controller.root_mouse_down(x, y)
            })
        });
        let row = self.table_row_at(&shown, x, y, cx);
        match event.button {
            MouseButton::Left => self.table.pressed_row = row,
            MouseButton::Right if row.is_some() => {
                self.table_input(cx, |renderer, page| renderer.row_context_menu(page));
            }
            _ => {}
        }
        cx.notify();
    }

    /// The renderer element's `mouseup` listener and a data row's `click`.
    pub(super) fn table_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        x: f64,
        y: f64,
        cx: &mut Context<'_, Self>,
    ) {
        self.table.button_pressed = false;
        let pressed = match event.button {
            MouseButton::Left => self.table.pressed_row.take(),
            _ => None,
        };
        let Some(shown) = self.table.shown.clone() else {
            return;
        };
        if !shown.contains(x, y) || self.table_covered(&shown, x, y, cx) {
            return;
        }
        self.host_mut().input(cx, |hub| {
            with_controller(hub, shown.index, |controller| controller.root_mouse_up())
        });
        let Some(row) = pressed else {
            return;
        };
        if self.table_row_at(&shown, x, y, cx) == Some(row) {
            self.table_input(cx, |renderer, page| renderer.row_click(page, row));
        }
        cx.notify();
    }

    fn table_wheel(&mut self, event: &ScrollEvent, cx: &mut Context<'_, Self>) {
        let Some(shown) = &self.table.shown else {
            return;
        };
        let [_, _, width, height] = shown.container;
        self.table_scroll(wheel_of(event), [width, height], cx);
    }

    /// A smooth scroll by a key or a scrollbar.
    pub(super) fn table_smooth(&mut self, by: [f64; 2], cx: &mut Context<'_, Self>) {
        self.table_scroll(smooth(by), [f64::INFINITY; 2], cx);
    }

    pub(super) fn table_scroll_to(&mut self, to: [f64; 2], cx: &mut Context<'_, Self>) {
        let Some(shown) = self.table.shown.clone() else {
            return;
        };
        if let Some(tab) = self.table.tabs.get_mut(&shown.key) {
            tab.wheel.stop();
        }
        self.table_input(cx, |renderer, _| {
            renderer.dom().borrow_mut().layout.set_wheel_offset(to);
        });
        cx.notify();
    }

    pub(super) fn table_range(&self, key: &JsString, cx: &App) -> Option<[[f64; 2]; 2]> {
        let shown = self.table.shown.as_ref().filter(|s| s.key == *key);
        let (_, renderer) = shown.and_then(|_| displayed(self.host().hub(cx)))?;
        let layout = &renderer.dom().borrow().layout;
        Some([layout.client(), layout.scroll_range()])
    }

    fn table_extent(&self, cx: &App) -> Option<Extent> {
        let (_, renderer) = displayed(self.host().hub(cx))?;
        let mut dom = renderer.dom().borrow_mut();
        Some(Extent {
            content: dom.layout.content(),
            offset: dom.layout.scroll(),
            axes: [true; 2],
            dark: false,
        })
    }

    fn table_scroll(&mut self, wheel: Wheel, client: [f64; 2], cx: &mut Context<'_, Self>) {
        let Some(shown) = self.table.shown.clone() else {
            return;
        };
        let now = self.host().frame_time(cx);
        let Some(mut state) = self
            .table
            .tabs
            .get_mut(&shown.key)
            .map(|tab| std::mem::take(&mut tab.wheel))
        else {
            return;
        };
        self.table_input(cx, |renderer, _| {
            let mut dom = renderer.dom().borrow_mut();
            let offset = dom.layout.wheel_offset();
            let max = dom.layout.scroll_range();
            if let Some(to) = state.wheel(wheel, now, offset, max, client) {
                dom.layout.set_wheel_offset(to);
            }
        });
        if let Some(tab) = self.table.tabs.get_mut(&shown.key) {
            tab.wheel = state;
        }
        cx.notify();
    }
}

/// `#dragItem`'s copy of a column's key container.
pub fn drag_column(
    text: &JsString,
    available: bool,
    width: f64,
    appearance: Appearance,
    scale: f32,
    cx: &mut App,
) -> Div {
    let color = appearance.theme().text;
    let key = text.to_string_lossy();
    let style = key_style("start");
    let mut item = div().relative();
    if !available {
        let grid = Grid {
            origin: [0.0, 0.0],
            scale: f64::from(scale),
        };
        item = item.children(line_through(
            &key,
            &style,
            [KEY_LEFT, 0.0],
            Some(width),
            color,
            grid,
            cx,
        ));
    }
    item.child(
        div()
            .absolute()
            .left(px(KEY_LEFT as f32))
            .top_0()
            .w(px(width as f32))
            .h(px(KEY_HEIGHT as f32))
            .overflow_hidden()
            .child(
                Text::new(key, style)
                    .color(color)
                    .content_width(width)
                    .h(px(KEY_HEIGHT as f32)),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_text_is_cut_between_visible_ascii() {
        let text = format!("{} {}", "ab".repeat(150), "c".repeat(260));
        let cut = pieces(&text, |prefix| prefix.len() as f64);
        let joined: String = cut.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(joined, text);
        assert_eq!(
            cut.iter().map(|(p, at)| (p.len(), *at)).collect::<Vec<_>>(),
            [(200, 0.0), (200, 200.0), (161, 400.0)]
        );
        let long = "x".repeat(100_000);
        let mut measured = 0;
        let cut = pieces(&long, |part| {
            measured += part.len();
            part.len() as f64
        });
        assert_eq!(cut.last().unwrap().1, 99_800.0);
        assert!(measured < 10 * long.len(), "{measured} characters measured");
        let spaces = " ".repeat(1000);
        assert_eq!(pieces(&spaces, |p| p.len() as f64)[1].1, 400.0);
        assert_eq!(pieces("short", |_| unreachable!()).len(), 1);
    }

    #[test]
    fn a_frame_keeps_every_cell_it_measured() {
        let mut cells = Cells::default();
        let frame = |cells: &mut Cells, from: usize| {
            cells.start_frame(1.0);
            let texts: Vec<String> = (from..from + CACHED_CELLS + 1)
                .map(|i| i.to_string())
                .collect();
            for text in &texts {
                cells.get_with(text, false, |text| text.len() as f64);
            }
            assert!(texts.iter().all(|text| cells.texts[0].contains_key(text)));
        };
        frame(&mut cells, 0);
        frame(&mut cells, 0);
        frame(&mut cells, 20_000);
        frame(&mut cells, 40_000);
        assert_eq!(cells.texts[0].len(), 2 * (CACHED_CELLS + 1));
    }

    /// The columns of `dom/tabs/table-wide` before its first drop, recorded on macOS.
    #[test]
    #[cfg(target_os = "macos")]
    fn column_widths_are_blinks() {
        use asrs_parity::geometry::GeometryRecord;
        use asrs_tabs::table::view::{TableHeader, TableRow};

        let header = |key: &str| TableHeader {
            title: key.into(),
            text: key.into(),
            available: true,
        };
        let text = format!(
            "[\"{}\", \"{}\", \"10\"]",
            "alpha".repeat(6),
            "beta".repeat(8)
        );
        let cells = ["+1.000", "15", &text, "\u{1f7e9} true", "null"];
        let view = TableView {
            has_controller: true,
            show_hand: false,
            headers: ["/V", "/T", "/F", "/M"].map(header).to_vec(),
            filler_upper_px: 0.0,
            filler_lower_px: 0.0,
            rows: vec![TableRow {
                cells: cells.map(str::to_owned).to_vec(),
                hidden: false,
                selected: false,
                hovered: false,
            }],
            jump_placeholder: String::new(),
        };
        for (vector, scale) in [
            ("dom/tabs/table-wide", 1.0f32),
            ("dom/tabs/table-wide-2x", 2.0),
        ] {
            let mut measurer = text::Measurer::default();
            let widths = column_widths(&view, 1.0, |text, bold| {
                measurer.measure(&cell_style(bold), text, scale).0
            });
            let mut recorded = vec![None; widths.len()];
            for record in asrs_parity::vectors::load(vector).unwrap() {
                let Some(g) = GeometryRecord::parse(&record).unwrap() else {
                    continue;
                };
                let asrs_parity::Value::Array(rect) = &g.value else {
                    continue;
                };
                for (i, slot) in recorded.iter_mut().enumerate() {
                    if slot.is_none() && g.is_on(&format!("tbody:0>tr:0>th:{i}")) {
                        *slot = rect[2].as_f64();
                    }
                }
            }
            let widths: Vec<_> = widths.into_iter().map(Some).collect();
            assert_eq!(widths, recorded, "{vector}");
        }
    }
}
