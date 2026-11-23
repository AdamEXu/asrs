//! The table tab's renderer element (`#renderer4` in `www/hub.html`, styled by
//! `www/renderers.css`), drawn from R11's `TableView` over the boxes of
//! [`TableLayout`](asrs_tabs::table::layout::TableLayout), which answers the renderer's and the
//! controller's layout reads.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use asrs_hub::frame::{DragContent, DragData};
use asrs_hub::hub::{HubModel, HubPage};
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::scroll_sensor::WheelScroll;
use asrs_model::source_list::Rect;
use asrs_model::tab_controller::TabRenderer;
use asrs_model::tab_type::TabType;
use asrs_tabs::table::view::TableView;
use asrs_tabs::table::{TableController, TableDom, TableRenderer};
use asrs_ui::button::IconButton;
use asrs_ui::canvas_view::geometry::mouse_client;
use asrs_ui::gpui::{
    AnyElement, App, AppContext, Context, Div, Entity, Focusable, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, ScrollEvent, Styled, Subscription,
    Window, div, px, rgb, rgba,
};
use asrs_ui::icon::{Icon, IconFit};
use asrs_ui::input::{FieldEvent, FieldKind, FieldStyle, TextField};
use asrs_ui::parity::probed;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, ELLIPSIS, Text, TextStyle, snap_remainder};
use asrs_ui::theme::{Appearance, FONT_FAMILY, chains};

use super::frame::{FrameHost, HubFrame, wheel_of};
use super::number::FormNumber;
use super::source_list::ratio_symbol;

const FIRST_COLUMN_MIN: f64 = 97.0;
const COLUMN_MIN: f64 = 175.0;
const CELL_PADDING: f64 = 4.0;
const KEY_LEFT: f64 = 6.0;
const KEY_RIGHT: f64 = 6.0;
const KEY_RIGHT_WITH_CLOSE: f64 = 30.0;
const KEY_HEIGHT: f64 = 30.0;
const KEY_FONT_SIZE: f64 = 14.0;
const BUTTON: f64 = 25.0;
const BUTTON_TOP: f64 = 2.5;
const JUMP_INPUT: [f64; 4] = [4.0, 4.5, 64.0, 21.0];
const JUMP_BUTTON_LEFT: f64 = 70.0;
const CLOSE_RIGHT: f64 = 2.0;
const HIGHLIGHT_WIDTH: f64 = 25.0;
const HAND_SIZE: f64 = 100.0;
const HAND_OPACITY: f64 = 0.15;
const HAND_DELAY_MS: f64 = 1000.0;
const HAND_FADE_MS: f64 = 1000.0;
const CELL_CACHE: usize = 8192;
const PIECE_CHARS: usize = 200;

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
    header_line: u32,
    line: u32,
    background: u32,
    hovered: u32,
    selected: u32,
}

impl Palette {
    fn of(appearance: Appearance) -> Palette {
        appearance.pick(
            Palette {
                header_line: 0x222222,
                line: 0xeeeeee,
                background: 0xffffff,
                hovered: 0xdddddd,
                selected: 0xaaaaaa,
            },
            Palette {
                header_line: 0xeeeeee,
                line: 0x333333,
                background: 0x222222,
                hovered: 0x444444,
                selected: 0x777777,
            },
        )
    }
}

/// `cubic-bezier(0.42, 0, 1, 1)` (`ease-in`) at `t`.
fn ease_in(t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    let (x1, x2) = (0.42, 1.0);
    let (y1, y2) = (0.0, 1.0);
    let bezier = |a: f64, b: f64, s: f64| {
        let r = 1.0 - s;
        3.0 * r * r * s * a + 3.0 * r * s * s * b + s * s * s
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..64 {
        let mid = (lo + hi) / 2.0;
        if bezier(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier(y1, y2, (lo + hi) / 2.0)
}

/// `img.large-table-hand-icon`'s opacity. `transition: opacity 1s ease-in 1s` runs only when the
/// icon was on display with opacity 0 before.
#[derive(Default)]
struct HandFade {
    shown: Option<bool>,
    since: Option<f64>,
}

impl HandFade {
    fn opacity(&mut self, show: bool, now: f64) -> (f64, bool) {
        let was = self.shown.replace(show);
        match (was, show) {
            (Some(false), true) => self.since = Some(now),
            (_, false) => self.since = None,
            _ => {}
        }
        if !show {
            return (0.0, false);
        }
        let Some(since) = self.since else {
            return (HAND_OPACITY, false);
        };
        let t = (now - since - HAND_DELAY_MS) / HAND_FADE_MS;
        if t >= 1.0 {
            self.since = None;
            return (HAND_OPACITY, false);
        }
        (HAND_OPACITY * ease_in(t), true)
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
    out.into_iter()
        .map(|(piece, at)| (piece, if at == 0 { 0.0 } else { measure(&text[..at]) }))
        .collect()
}

struct Cell {
    width: f64,
    pieces: Vec<(String, f64)>,
}

/// Measured cell texts, by whether they are bold.
#[derive(Default)]
struct Cells {
    scale: f32,
    texts: [HashMap<String, Rc<Cell>>; 2],
}

impl Cells {
    fn get(&mut self, text: &str, bold: bool, scale: f32, cx: &mut App) -> Rc<Cell> {
        if self.scale != scale || self.texts.iter().any(|c| c.len() > CELL_CACHE) {
            *self = Cells {
                scale,
                ..Cells::default()
            };
        }
        let texts = &mut self.texts[usize::from(bold)];
        if let Some(cell) = texts.get(text) {
            return cell.clone();
        }
        let mut measure = |text: &str| text::measure(cx, &cell_style(bold), text, scale).0;
        let cell = Rc::new(Cell {
            width: measure(text),
            pieces: pieces(text, measure),
        });
        texts.insert(text.to_owned(), cell.clone());
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

/// Positions in the renderer element for client coordinates, on the device pixel grid.
#[derive(Clone, Copy)]
struct Grid {
    origin: [f64; 2],
    scale: f64,
}

impl Grid {
    fn snap(&self, v: f64) -> f64 {
        math::floor(v * self.scale + 0.5) / self.scale
    }

    /// Blink's pixel-snapped rect of a box.
    fn place(&self, [x, y, w, h]: [f64; 4]) -> Div {
        let (left, top) = (self.snap(x), self.snap(y));
        div()
            .absolute()
            .left(px((left - self.origin[0]) as f32))
            .top(px((top - self.origin[1]) as f32))
            .w(px((self.snap(x + w) - left) as f32))
            .h(px((self.snap(y + h) - top) as f32))
    }

    fn remainder(&self, v: f64) -> f32 {
        snap_remainder(v as f32, self.scale as f32)
    }

    /// Text whose box starts at client (`x`, `y`).
    fn text(&self, text: &str, style: TextStyle, color: u32, [x, y]: [f64; 2]) -> Div {
        div()
            .absolute()
            .left(px((self.snap(x) - self.origin[0]) as f32))
            .top(px((self.snap(y) - self.origin[1]) as f32))
            .child(
                Text::new(text.to_owned(), style)
                    .color(color)
                    .subpixel(self.remainder(x), self.remainder(y)),
            )
    }

    /// One of the header's buttons at client (`x`, `y`).
    fn button(&self, button: IconButton, [x, y]: [f64; 2]) -> Div {
        div()
            .absolute()
            .left(px((self.snap(x) - self.origin[0]) as f32))
            .top(px((self.snap(y) - self.origin[1]) as f32))
            .child(
                button
                    .size(BUTTON as f32, BUTTON as f32)
                    .subpixel(x - self.snap(x), y - self.snap(y)),
            )
    }
}

fn symbol(name: &str) -> Symbol {
    Symbol::named(name).unwrap_or_else(|| panic!("bundled symbol {name}"))
}

fn mean(a: u32, b: u32) -> u32 {
    let channel = |shift: u32| (((a >> shift & 0xff) + (b >> shift & 0xff)).div_ceil(2)) << shift;
    channel(16) | channel(8) | channel(0)
}

/// A `th`: its background, and its borders meeting in a mitre at the bottom right corner, where
/// the pixels on the diagonal take half of each.
fn header_cell(grid: Grid, cell: [f64; 4], right: bool, border: f64, palette: &Palette) -> Div {
    let [x, y, w, h] = cell;
    let (right_edge, bottom_edge) = (grid.snap(x + w), grid.snap(y + h));
    let mut out = div().absolute().inset_0();
    out = out
        .child(grid.place(cell).bg(rgb(palette.background)))
        .child(
            grid.place([x, bottom_edge - border, right_edge - x, border])
                .bg(rgb(palette.header_line)),
        );
    if !right {
        return out;
    }
    let left = right_edge - border;
    out = out.child(
        grid.place([left, y, border, bottom_edge - border - y])
            .bg(rgb(palette.line)),
    );
    let pixel = 1.0 / grid.scale;
    let half = mean(palette.header_line, palette.line);
    for step in 0..(border * grid.scale).round() as usize {
        let at = step as f64 * pixel;
        let top = bottom_edge - border + at;
        out = out
            .child(grid.place([left + at, top, pixel, pixel]).bg(rgb(half)))
            .child(
                grid.place([left + at + pixel, top, border - at - pixel, pixel])
                    .bg(rgb(palette.line)),
            );
    }
    out
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
            layout.container = Rect {
                x,
                y,
                width,
                height,
            };
            layout.device_pixel_ratio = f64::from(scale);
            layout.columns = column_widths(&view, layout.border_px(), |text, bold| {
                tab.cells.get(text, bold, scale, cx).width
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
        let jump = self.jump_field(&shown, &view, window, cx);
        let dark = appearance == Appearance::Dark;
        let mut root = div().absolute().left_0().top_0().size_full();
        root = root.child(hand_icon(rect, hand, dark, grid));
        root = root.child(self.table_body(&view, &dom, jump, rect, appearance, grid, cx));
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
        &self,
        view: &TableView,
        dom: &Rc<RefCell<TableDom>>,
        jump: Entity<TextField<FormNumber>>,
        rect: [f64; 4],
        appearance: Appearance,
        grid: Grid,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let palette = Palette::of(appearance);
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
        let rows: Vec<_> = view.rows.iter().filter(|row| !row.hidden).collect();
        let rows_top = layout.row_top(0);
        let rows_height = rows.len() as f64 * row_height;
        let first = columns[0];
        let measured = (self.table.shown.as_ref()).and_then(|s| self.table.tabs.get(&s.key));
        let measured =
            |text: &str, bold: bool| measured?.cells.texts[usize::from(bold)].get(text).cloned();

        let mut cells = div().absolute().inset_0();
        let mut first_cells = div().absolute().inset_0().child(
            grid.place([first[0], rows_top, first[1], rows_height])
                .bg(rgb(palette.background)),
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
            if !visible && !cfg!(feature = "parity") {
                continue;
            }
            for (column, text) in row.cells.iter().take(count).enumerate() {
                let bold = column == 0;
                let Some(cell) = measured(text, bold).filter(|_| !text.is_empty()) else {
                    continue;
                };
                let [left, w] = columns[column];
                let cell_border = if column + 1 == count { 0.0 } else { border };
                let text_left = if bold {
                    left + w - cell_border - CELL_PADDING - cell.width
                } else {
                    left + CELL_PADDING
                };
                for (i, (piece, dx)) in cell.pieces.iter().enumerate() {
                    let from = text_left + dx;
                    let to = cell.pieces.get(i + 1).map_or(cell.width, |next| next.1);
                    let cut = cell.pieces.len() > 1;
                    if cut && (text_left + to < x || from >= x + width) {
                        continue;
                    }
                    let at = [from, top + CELL_PADDING];
                    let element = grid.text(piece, cell_style(bold), text_color, at);
                    if bold {
                        first_cells = first_cells.child(element);
                    } else {
                        cells = cells.child(element);
                    }
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
        let mut header = div().absolute().inset_0();
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
            let mut key_box = grid
                .place([key_left, y, key_width, KEY_HEIGHT])
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
                .child(header_cell(grid, cell, !last, border, &palette))
                .child(key_box);
            if !field.available {
                let key = field.text.to_string_lossy();
                let style = key_style("left");
                let at = [key_left, y];
                th = th.children(line_through(
                    &key, &style, at, key_width, text_color, grid, cx,
                ));
            }
            if view.has_controller {
                let at = [
                    left + w - cell_border - CLOSE_RIGHT - BUTTON,
                    y + BUTTON_TOP,
                ];
                let button = IconButton::new(("table-close", index), symbol("close.svg"))
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
        let button = IconButton::new("table-jump", symbol("verticalalignbottom.svg"))
            .on_click(cx.listener(|frame, _, _, cx| frame.table_jump(cx)));
        let button_at = [first[0] + JUMP_BUTTON_LEFT, y + BUTTON_TOP];
        header = header
            .child(header_cell(grid, first_cell, count > 1, border, &palette))
            .child(
                grid.place(jump_at)
                    .on_mouse_down_out({
                        let jump = jump.clone();
                        move |_, window, cx| jump.update(cx, |field, cx| field.blur(window, cx))
                    })
                    .child(jump),
            )
            .child(grid.button(button, button_at));
        probes = probes
            .child(probed("input.data-table-jump-input", grid.place(jump_at)))
            .child(probed(
                "button.data-table-jump-button",
                grid.place([button_at[0], button_at[1], BUTTON, BUTTON]),
            ));

        div()
            .id("data-table-container")
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
            .on_scroll(
                cx.listener(|frame, event: &ScrollEvent, _, cx| frame.table_wheel(event, cx)),
            )
            .child(probes)
            .child(cells)
            .child(first_cells)
            .child(header)
            .into_any_element()
    }
}

impl<H: FrameHost> HubFrame<H> {
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
        let Some(shown) = self.table.shown.clone() else {
            return;
        };
        let now = self.host().frame_time(cx);
        let wheel = wheel_of(event);
        let [_, _, width, height] = shown.container;
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
            if let Some(to) = state.wheel(wheel, now, offset, [width, height]) {
                dom.layout.set_wheel_offset(to);
            }
        });
        if let Some(tab) = self.table.tabs.get_mut(&shown.key) {
            tab.wheel = state;
        }
        cx.notify();
    }
}

/// `text-decoration: line-through` on a key whose line box starts at client (`x`, `y`), cut to
/// `width`: Blink decorates the text and not its ellipsis, rounds the line to whole device rows
/// and leaves its ends antialiased.
fn line_through(
    key: &str,
    style: &TextStyle,
    [x, y]: [f64; 2],
    width: f64,
    color: u32,
    grid: Grid,
    cx: &mut App,
) -> Vec<Div> {
    let scale = grid.scale;
    let layout = text::layout(cx, style, key, scale as f32, Some(width as f32));
    let Some(line) = layout.lines.first() else {
        return Vec::new();
    };
    let device = |lu: i32| f64::from(lu) / 64.0;
    let ascent = device(layout.ascent);
    let top = y * scale + device(line.baseline) - ascent;
    let row = math::floor(top + 2.0 * ascent / 3.0 + 0.5);
    let thickness = math::max2(math::floor(KEY_FONT_SIZE * scale / 10.0), 1.0);
    let mut out = Vec::new();
    let mut span = |from: f64, to: f64, alpha: f64| {
        if to > from {
            let alpha = (alpha * 255.0).round() as u32;
            out.push(
                grid.place([from, row, to - from, thickness].map(|v| v / scale))
                    .bg(rgba(color << 8 | alpha)),
            );
        }
    };
    for f in &line.fragments {
        if line.cut_at.is_some() && f.text == ELLIPSIS {
            continue;
        }
        let from = x * scale + device(f.x);
        let to = from + device(f.width);
        let (inner_from, inner_to) = (math::ceil(from), math::floor(to));
        span(math::floor(from), inner_from, inner_from - from);
        span(inner_from, inner_to, 1.0);
        span(inner_to, math::ceil(to), to - inner_to);
    }
    out
}

fn hand_icon(root: [f64; 4], opacity: f64, dark: bool, grid: Grid) -> impl IntoElement {
    let [x, y, w, h] = root;
    let half = HAND_SIZE / 2.0;
    let chain = if dark { chains::INVERT } else { "" };
    let at = [x + w / 2.0 - half, y + h / 2.0 - half];
    probed(
        "img.large-table-hand-icon",
        grid.place([at[0], at[1], HAND_SIZE, HAND_SIZE])
            .opacity(opacity as f32)
            .child(Icon::new(ratio_symbol("trackpadinput.svg"), chain).fit(IconFit::Natural)),
    )
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
            width,
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
    fn hand_fades_in_only_after_it_was_gone() {
        let mut hand = HandFade::default();
        assert_eq!(hand.opacity(true, 0.0), (HAND_OPACITY, false));
        assert_eq!(hand.opacity(false, 10.0), (0.0, false));
        assert_eq!(hand.opacity(true, 20.0), (0.0, true));
        assert_eq!(hand.opacity(true, 1020.0), (0.0, true));
        let (mid, _) = hand.opacity(true, 1520.0);
        assert!((0.3..0.33).contains(&(mid / HAND_OPACITY)));
        assert_eq!(hand.opacity(true, 2020.0), (HAND_OPACITY, false));
    }

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
        let spaces = " ".repeat(1000);
        assert_eq!(pieces(&spaces, |p| p.len() as f64)[1].1, 400.0);
        assert_eq!(pieces("short", |_| unreachable!()).len(), 1);
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

    #[test]
    fn corner_pixels_take_half_of_each_border() {
        assert_eq!(mean(0x222222, 0xeeeeee), 0x888888);
        assert_eq!(mean(0xeeeeee, 0x333333), 0x919191);
    }
}
