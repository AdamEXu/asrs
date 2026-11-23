//! The console tab's renderer element (`#renderer5` in `www/hub.html`, styled by
//! `www/renderers.css`), drawn from R11's `ConsoleView` over the boxes of [`ConsoleLayout`],
//! which answers the renderer's and the controller's layout reads.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;

use asrs_canvas::text::FontSpec;
use asrs_hub::hub::{HubModel, HubPage};
use asrs_js::string::{JsError, JsString};
use asrs_model::tab_type::TabType;
use asrs_tabs::console::controller::{ConsoleController, ConsoleDom, ConsoleRenderer};
use asrs_tabs::console::view::{ConsoleRow, ConsoleView};
use asrs_ui::button::IconButton;
use asrs_ui::canvas_view::geometry::mouse_client;
use asrs_ui::gpui::{
    AnyElement, App, AppContext, Context, Div, Entity, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, ScrollEvent, Styled,
    Subscription, Window, div, px, rgb, rgba,
};
use asrs_ui::input::{FieldEvent, FieldKind, FieldStyle, PlainText, TextField};
use asrs_ui::parity::probed;
use asrs_ui::text::{self, TextStyle, snap_remainder};
use asrs_ui::theme::{Appearance, FONT_FAMILY, IconTint, chains};

use super::console_layout::{
    CELL_PADDING, Cell, CellNode, ConsoleLayout, HEADER_HEIGHT, LINE_HEIGHT, TIME_WIDTH, cell_font,
    cell_style,
};
use super::frame::{FrameHost, HubFrame, wheel_of};
use super::large_table::{
    BUTTON, Grid, HandFade, HeaderPalette, hand_icon, header_cell, line_through, plain, symbol,
};
use super::sidebar::NumberValue;

const JUMP_INPUT: [f64; 4] = [4.0, 4.5, 64.0, 21.0];
const JUMP_BUTTON_LEFT: f64 = 70.0;
const BUTTON_TOP: f64 = 2.5;
/// `div` around the key: `right: 246px`, `height: 30px`, `padding-left: 5px`.
const KEY_RIGHT: f64 = 246.0;
const KEY_HEIGHT: f64 = 30.0;
const KEY_PADDING: f64 = 5.0;
const DELETE_MARGIN: f64 = 5.0;
const EXPORT_RIGHT: f64 = 216.0;
const HIGHLIGHT_RIGHT: f64 = 188.0;
const FILTER: [f64; 3] = [5.0, 180.0, 22.0];
/// `td span.highlight`.
const HIGHLIGHT: u32 = 0xffff00;
const FAINT: f64 = 0.7;
/// The thickness of an underline of 12 px text, and its top above the text box's bottom.
const UNDERLINE: f64 = 1.0;
const UNDERLINE_FROM_BOTTOM: f64 = 2.0;

/// `table.console-table button.highlight-button.active img`, which outranks `button.blurred`.
const HIGHLIGHT_ACTIVE: IconTint = IconTint {
    normal: [chains::CONSOLE_WARNING; 2],
    blurred: [chains::CONSOLE_WARNING; 2],
    disabled: [chains::CONSOLE_WARNING; 2],
};

fn key_style() -> TextStyle {
    let font = format!("bold 14px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "30px"),
        ("white-space", "nowrap"),
    ])
    .expect("console key style")
}

/// A `td`'s background by the cascade of `renderers.css`, as `0xRRGGBBAA`.
fn cell_background(row: &ConsoleRow, first: bool, appearance: Appearance) -> Option<u32> {
    if row.error || row.warning {
        let base = if row.error { 0xff000000 } else { 0xffaa0000 };
        let alpha = match (row.selected, row.hovered) {
            (true, _) => 0xaa,
            (false, true) => 0x88,
            (false, false) => 0x55,
        };
        return Some(base | alpha);
    }
    match (row.selected, row.hovered) {
        (true, _) => Some(appearance.pick(0xe3e3e3ff, 0x3e3e3eff)),
        (false, true) => Some(appearance.pick(0xeeeeeeff, 0x333333ff)),
        (false, false) => first
            .then(|| appearance.pick(None, Some(0x222222ff)))
            .flatten(),
    }
}

/// `top` with opacity `alpha` over the opaque `under`.
fn over(top: u32, alpha: f64, under: u32) -> u32 {
    let channel = |shift: u32| {
        let t = f64::from(top >> shift & 0xff);
        let u = f64::from(under >> shift & 0xff);
        (((t * alpha + u * (1.0 - alpha)).round() as u32) & 0xff) << shift
    };
    channel(16) | channel(8) | channel(0)
}

struct Fields {
    jump: Entity<TextField<NumberValue>>,
    filter: Entity<TextField<PlainText>>,
    _subscriptions: [Subscription; 2],
}

struct TabView {
    /// Keeps the tab's key, which is this allocation's address, from being reused.
    _dom: Rc<RefCell<ConsoleDom>>,
    layout: ConsoleLayout,
    fields: Option<Fields>,
    hand: HandFade,
}

fn inside([left, top, w, h]: [f64; 4], x: f64, y: f64) -> bool {
    x >= left && x < left + w && y >= top && y < top + h
}

impl TabView {
    fn new(dom: Rc<RefCell<ConsoleDom>>) -> Self {
        TabView {
            _dom: dom,
            layout: ConsoleLayout::default(),
            fields: None,
            hand: HandFade::default(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Shown {
    key: usize,
    index: usize,
    container: [f64; 4],
}

impl Shown {
    fn contains(&self, x: f64, y: f64) -> bool {
        inside(self.container, x, y)
    }

    fn jump_rect(&self) -> [f64; 4] {
        let [x, y, w, h] = JUMP_INPUT;
        [self.container[0] + x, self.container[1] + y, w, h]
    }

    /// `input.filter`: `top: 50%` with `translateY(-50%)`.
    fn filter_rect(&self) -> [f64; 4] {
        let [right, width, height] = FILTER;
        let [x, y, w, _] = self.container;
        [
            x + w - right - width,
            y + KEY_HEIGHT / 2.0 - height / 2.0,
            width,
            height,
        ]
    }
}

#[derive(Default)]
pub struct ConsoleViews {
    tabs: HashMap<usize, TabView>,
    shown: Option<Shown>,
    hovered: Option<usize>,
    pressed_row: Option<usize>,
    editing: bool,
}

impl ConsoleViews {
    /// Whether one of the displayed table's inputs has focus.
    pub fn editing(&self) -> bool {
        self.editing
    }
}

fn renderer_key(renderer: &ConsoleRenderer) -> usize {
    Rc::as_ptr(renderer.dom()) as *const () as usize
}

fn displayed(hub: &HubModel) -> Option<(usize, &ConsoleRenderer)> {
    hub.tabs().tabs().enumerate().find_map(|(i, tab)| {
        if tab.content_hidden() || tab.tab_type() != Some(TabType::Console) {
            return None;
        }
        tab.renderer()
            .downcast_ref::<ConsoleRenderer>()
            .map(|r| (i, r))
    })
}

fn with_renderer<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut ConsoleRenderer, &mut HubPage) -> R,
) -> Option<R> {
    let (tabs, page) = hub.tabs_mut();
    let tab = tabs.tab_mut(index)?;
    let renderer = tab.renderer.downcast_mut::<ConsoleRenderer>()?;
    Some(f(renderer, page))
}

fn with_controller<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut ConsoleController) -> R,
) -> Option<R> {
    let (tabs, _) = hub.tabs_mut();
    let tab = tabs.tab_mut(index)?;
    tab.controller.downcast_mut::<ConsoleController>().map(f)
}

fn report(result: Option<Result<(), JsError>>) {
    if let Some(Err(e)) = result {
        eprintln!("asrs: {}: {}", e.name, e.message);
    }
}

/// What the rows are drawn with.
struct Rows<'a> {
    layout: &'a ConsoleLayout,
    grid: Grid,
    appearance: Appearance,
    /// `div.console-table-container`.
    container: [f64; 4],
    scroll_top: f64,
    text_box: (f64, f64),
}

impl Rows<'_> {
    fn row_top(&self, index: usize) -> f64 {
        self.container[1] + HEADER_HEIGHT + self.layout.row(index).0 - self.scroll_top
    }

    fn cell(&self, index: usize, row: &ConsoleRow, layer: Div) -> Div {
        let (x, width) = (self.container[0], self.container[2]);
        let top = self.row_top(index);
        let height = self.layout.row(index).1;
        let (value, time) = self.layout.cells(index, row);
        let mut layer = layer;
        for (first, left, w) in [
            (true, x, TIME_WIDTH),
            (false, x + TIME_WIDTH, width - TIME_WIDTH),
        ] {
            let background = cell_background(row, first, self.appearance);
            if let Some(color) = background {
                let place = self.grid.place([left, top, w, height]);
                layer = layer.child(place.bg(rgba(color)));
            }
            let page = self.appearance.theme().background;
            let under =
                background.map_or(page, |c| over(c >> 8, f64::from(c & 0xff) / 255.0, page));
            let cell = if first { &time } else { &value };
            layer = self.text(cell, first, [left, top, w], under, layer);
        }
        layer
    }

    /// `(node, rect)` of each piece of `cell` in client coordinates.
    fn pieces<'c>(
        &self,
        cell: &'c Cell,
        first: bool,
        [left, top, width]: [f64; 3],
    ) -> impl Iterator<Item = (usize, &'c str, bool, [f64; 4])> {
        let (box_top, box_height) = self.text_box;
        let scale = self.grid.scale;
        let device = move |lu: i32| f64::from(lu) / 64.0 / scale;
        cell.lines.iter().enumerate().flat_map(move |(i, line)| {
            let offset = if first {
                width - 2.0 * CELL_PADDING - device(line.width)
            } else {
                0.0
            };
            let origin = left + CELL_PADDING + offset;
            line.pieces.iter().map(move |piece| {
                let text = match &cell.nodes[piece.item] {
                    CellNode::Text { text, .. } => &text[piece.range.clone()],
                    CellNode::Break => "",
                };
                let rect = [
                    origin + device(piece.x),
                    top + i as f64 * LINE_HEIGHT + box_top,
                    device(piece.width),
                    box_height,
                ];
                (piece.item, text, piece.tabs, rect)
            })
        })
    }

    fn text(&self, cell: &Cell, first: bool, at: [f64; 3], under: u32, layer: Div) -> Div {
        let theme = self.appearance.theme();
        let mut layer = layer;
        for (item, text, tabs, rect) in self.pieces(cell, first, at) {
            let CellNode::Text {
                style,
                link,
                highlight,
                ..
            } = &cell.nodes[item]
            else {
                continue;
            };
            let blend = |color: u32| {
                if style.faint {
                    over(color, FAINT, under)
                } else {
                    color
                }
            };
            let of = |c: asrs_tabs::ansi::Rgb| rgb_of(c);
            let background = if *highlight {
                Some(HIGHLIGHT)
            } else {
                style.bg.map(of)
            };
            if let Some(color) = background {
                layer = layer.child(self.grid.place(rect).bg(rgb(blend(color))));
            }
            let color = match (highlight, self.appearance) {
                (true, Appearance::Dark) => 0x000000,
                _ if *link => theme.link,
                _ => style.fg.map_or(theme.text, of),
            };
            let [x, y, ..] = rect;
            if !tabs {
                let font = cell_font(first || style.bold, style.italic);
                let line_top = y - self.text_box.0;
                let mut piece = self.grid.text(
                    plain(text, cell_style(font), color).form_control(),
                    [x, line_top],
                );
                if style.faint && background.is_none() {
                    piece = piece.opacity(FAINT as f32);
                } else if style.faint {
                    piece = self.grid.text(
                        plain(text, cell_style(font), blend(color)).form_control(),
                        [x, line_top],
                    );
                }
                layer = layer.child(piece);
            }
            if style.underline || *link {
                let font = cell_font(first || style.bold, style.italic);
                let text = if tabs { "" } else { text };
                layer = layer.child(self.underline(font, text, rect, blend(color)));
            }
        }
        layer
    }

    /// `text-decoration: underline` with `text-decoration-skip-ink: auto`: the stripe less
    /// what the glyphs that cross it cover, a thickness wider each side.
    fn underline(&self, font: FontSpec, text: &str, [x, y, w, h]: [f64; 4], color: u32) -> Div {
        let top = y + h - UNDERLINE_FROM_BOTTOM;
        let mut layer = div().absolute().inset_0();
        let mut from = x;
        for [left, right] in self.layout.ink(font, text, x, top - y) {
            let (left, right) = (left - UNDERLINE, right + UNDERLINE);
            if left > from {
                let part = self.grid.place([from, top, left - from, UNDERLINE]);
                layer = layer.child(part.bg(rgb(color)));
            }
            from = from.max(right);
        }
        if x + w > from {
            let part = self.grid.place([from, top, x + w - from, UNDERLINE]);
            layer = layer.child(part.bg(rgb(color)));
        }
        layer
    }
}

fn rgb_of(color: asrs_tabs::ansi::Rgb) -> u32 {
    u32::from_be_bytes([0, color.0, color.1, color.2])
}

impl<H: FrameHost> HubFrame<H> {
    /// The displayed console's renderer element over `rect` (`div.renderer-content`), if the
    /// displayed tab is a console.
    pub(super) fn console_element(
        &mut self,
        rect: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let hub = self.host().hub(cx);
        let live: Vec<usize> = hub
            .tabs()
            .tabs()
            .filter_map(|t| t.renderer().downcast_ref::<ConsoleRenderer>())
            .map(renderer_key)
            .collect();
        let shown = displayed(hub).map(|(index, renderer)| {
            (
                Shown {
                    key: renderer_key(renderer),
                    index,
                    container: rect,
                },
                renderer.dom().clone(),
            )
        });
        let blurred = !hub.is_focused();
        let key = shown.as_ref().map(|(s, ..)| s.key);
        let left = self.console.shown.filter(|s| Some(s.key) != key);
        if let Some(left) = left {
            self.console_hidden(left, window, cx);
        }
        self.console.tabs.retain(|key, _| live.contains(key));
        self.console.shown = shown.as_ref().map(|(s, _)| *s);
        let (shown, dom) = shown?;
        self.console.editing = self.console_field_focused(shown, window, cx);

        let now = self.host().frame_time(cx);
        let layout = match self.console.tabs.get(&shown.key) {
            Some(tab) => tab.layout.clone(),
            None => {
                let tab = TabView::new(dom);
                let layout = tab.layout.clone();
                self.console.tabs.insert(shown.key, tab);
                self.host_mut().input(cx, |hub| {
                    with_renderer(hub, shown.index, |renderer, _| {
                        renderer.set_geometry(Box::new(layout.clone()))
                    });
                    with_controller(hub, shown.index, |controller| {
                        controller.set_geometry(Box::new(layout.clone()))
                    });
                });
                layout
            }
        };
        let scrolling = layout.animate(now);
        let (_, renderer) = displayed(self.host().hub(cx))?;
        let view = renderer.view();
        layout.sync(rect, scale, &view.rows);
        let top = layout.scroll_top() - HEADER_HEIGHT;
        let visible = layout.rows_between(top, top + rect[3]);
        let recorded = record_rows(view, &layout, shown, scale);
        // Only the rows in view: a log can have many thousands.
        let view = ConsoleView {
            rows: view.rows[visible.clone()].to_vec(),
            has_controller: view.has_controller,
            field_text: view.field_text.clone(),
            field_line_through: view.field_line_through,
            field_delete_hidden: view.field_delete_hidden,
            show_hand: view.show_hand,
            highlight_active: view.highlight_active,
            jump_placeholder: view.jump_placeholder.clone(),
        };
        let tab = self.console.tabs.get_mut(&shown.key)?;
        let (hand, fading) = tab.hand.opacity(view.show_hand, now);
        if scrolling || fading {
            window.request_animation_frame();
        }

        let grid = Grid {
            origin: [rect[0], rect[1]],
            scale: f64::from(scale),
        };
        let dark = appearance == Appearance::Dark;
        let mut root = div()
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .child(hand_icon(rect, hand, dark, grid))
            .child(self.console_table(shown, &view, visible, &layout, appearance, grid, cx))
            .children(recorded)
            .child(self.console_header(shown, &view, appearance, blurred, grid, window, cx));
        let drag_hidden = displayed(self.host().hub(cx))
            .is_none_or(|(_, r)| r.dom().borrow().drag_highlight_hidden);
        if !drag_hidden {
            root = root.child(div().absolute().inset_0().bg(rgb(0x90ee90)).opacity(0.25));
        }
        Some(root.into_any_element())
    }

    #[expect(clippy::too_many_arguments)]
    fn console_table(
        &self,
        shown: Shown,
        view: &ConsoleView,
        visible: Range<usize>,
        layout: &ConsoleLayout,
        appearance: Appearance,
        grid: Grid,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let [x, y, width, height] = shown.container;
        let scroll_top = layout.scroll_top();
        let rows = Rows {
            layout,
            grid,
            appearance,
            container: shown.container,
            scroll_top,
            text_box: layout.text_box(),
        };
        layout.keep_cells(&visible);
        let mut layer = div().absolute().inset_0();
        for (index, row) in visible.zip(&view.rows) {
            layer = rows.cell(index, row, layer);
        }
        div()
            .id("console-table-container")
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .overflow_hidden()
            .on_any_mouse_down(cx.listener(|frame, event: &MouseDownEvent, window, cx| {
                frame.console_mouse_down(event, window, cx)
            }))
            .on_scroll(
                cx.listener(|frame, event: &ScrollEvent, _, cx| frame.console_wheel(event, cx)),
            )
            .child(layer)
            .child(probed(
                ".console-table-container",
                grid.place([x, y, width, height]),
            ))
            .into_any_element()
    }

    #[expect(clippy::too_many_arguments)]
    fn console_header(
        &mut self,
        shown: Shown,
        view: &ConsoleView,
        appearance: Appearance,
        blurred: bool,
        grid: Grid,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let [x, y, width, _] = shown.container;
        let palette = HeaderPalette::of(appearance);
        let text_color = appearance.theme().text;
        let Some((jump, filter)) = self.console_fields(shown, view, window, cx) else {
            return div().into_any_element();
        };
        let field_cell = [x + TIME_WIDTH, y, width - TIME_WIDTH, HEADER_HEIGHT];
        let right = x + width;
        let scale = grid.scale as f32;

        let key_left = field_cell[0] + KEY_PADDING;
        let key = view.field_text.to_string_lossy();
        let mut key_box = grid
            .place([
                field_cell[0],
                y,
                (field_cell[2] - KEY_RIGHT).max(0.0),
                KEY_HEIGHT,
            ])
            .overflow_hidden();
        let mut key_end = 0.0;
        for (i, line) in key.split('\n').enumerate() {
            let (w, _) = text::measure(cx, &key_style(), line, scale);
            key_end = w;
            let top = i as f64 * KEY_HEIGHT;
            key_box = key_box.child(
                div()
                    .absolute()
                    .left(px((grid.snap(key_left) - grid.snap(field_cell[0])) as f32))
                    .top(px(top as f32))
                    .child(
                        plain(line, key_style(), text_color)
                            .subpixel(grid.remainder(key_left), 0.0),
                    ),
            );
            if view.field_line_through {
                key_box = key_box.child(
                    line_through(
                        grid.snap(key_left) - grid.snap(field_cell[0]),
                        w,
                        text_color,
                    )
                    .mt(px(top as f32)),
                );
            }
        }
        let key_box = if view.field_delete_hidden {
            key_box
        } else {
            let at = [key_left + key_end + DELETE_MARGIN, y + BUTTON_TOP];
            let inner = Grid {
                origin: [grid.snap(field_cell[0]), grid.snap(y)],
                ..grid
            };
            let button = IconButton::new("console-delete", symbol("close.svg"))
                .blurred(blurred)
                .on_click(cx.listener(|frame, _, _, cx| frame.console_close(cx)));
            key_box.child(probed(
                "button.console-table-key-delete",
                inner.button(button, at),
            ))
        };

        let button = |button: IconButton, id: &'static str, left: f64| {
            probed(
                id,
                grid.button(button.blurred(blurred), [left, y + BUTTON_TOP])
                    .w(px(BUTTON as f32))
                    .h(px(BUTTON as f32)),
            )
        };
        // `top: 50%; transform: translateY(-50%)`
        let centered = |b: IconButton, id, left| button(b.translate(0.0, -BUTTON / 2.0), id, left);
        let jump_button = IconButton::new("console-jump", symbol("verticalalignbottom.svg"))
            .on_click(cx.listener(|frame, _, _, cx| frame.console_jump(cx)));
        let highlight = IconButton::new("console-highlight", symbol("palette.svg"))
            .tint(if view.highlight_active {
                HIGHLIGHT_ACTIVE
            } else {
                IconTint::BUTTON
            })
            .on_click(cx.listener(|frame, _, _, cx| {
                report(frame.console_input(cx, |renderer, page| renderer.highlight_click(page)));
                cx.notify();
            }));
        let mut header = div()
            .absolute()
            .left_0()
            .top_0()
            .w(px(width as f32))
            .h(px(HEADER_HEIGHT as f32))
            .child(probed(
                "table.console-table th",
                grid.place([x, y, TIME_WIDTH, HEADER_HEIGHT]),
            ))
            .child(probed("table.console-table th", grid.place(field_cell)))
            .child(header_cell(grid, field_cell, false, &palette))
            .child(header_cell(
                grid,
                [x, y, TIME_WIDTH, HEADER_HEIGHT],
                true,
                &palette,
            ))
            .child(probed(
                "input.console-table-jump-input",
                grid.place(shown.jump_rect()).child(jump),
            ))
            .child(button(
                jump_button,
                "button.console-table-jump-button",
                x + JUMP_BUTTON_LEFT,
            ))
            .child(key_box);
        if view.has_controller {
            let export = IconButton::new("console-export", symbol("download.svg")).on_click(
                cx.listener(|frame, _, _, cx| {
                    frame.console_input(cx, |renderer, page| renderer.export_click(page));
                }),
            );
            header = header.child(centered(
                export,
                "button.export-button",
                right - EXPORT_RIGHT - BUTTON,
            ));
        }
        header
            .child(centered(
                highlight,
                "button.highlight-button",
                right - HIGHLIGHT_RIGHT - BUTTON,
            ))
            .child(probed(
                "input.filter",
                grid.place(shown.filter_rect()).child(filter),
            ))
            .into_any_element()
    }
}

/// Records every row's text runs for a parity capture, also those scrolled out of view, as the
/// DOM dump lists them.
fn record_rows(
    view: &ConsoleView,
    layout: &ConsoleLayout,
    shown: Shown,
    scale: f32,
) -> Option<impl IntoElement + use<>> {
    if !cfg!(feature = "parity") {
        return None;
    }
    let (rows, layout) = (view.rows.clone(), layout.clone());
    let grid = Grid {
        origin: [shown.container[0], shown.container[1]],
        scale: f64::from(scale),
    };
    let record = asrs_ui::gpui::canvas(
        move |_, _, cx| {
            if !cx.has_global::<asrs_ui::parity::Recorder>() {
                return;
            }
            let drawn = Rows {
                layout: &layout,
                grid,
                appearance: Appearance::Light,
                container: shown.container,
                scroll_top: layout.scroll_top(),
                text_box: layout.text_box(),
            };
            let (x, width) = (shown.container[0], shown.container[2]);
            for (index, row) in rows.iter().enumerate() {
                let (value, time) = layout.cells(index, row);
                let top = drawn.row_top(index);
                let cells = [
                    (&time, true, [x, top, TIME_WIDTH]),
                    (&value, false, [x + TIME_WIDTH, top, width - TIME_WIDTH]),
                ];
                for (cell, first, at) in cells {
                    let mut runs: Vec<asrs_ui::parity::TextRun> = cell
                        .nodes
                        .iter()
                        .map(|node| asrs_ui::parity::TextRun {
                            text: match node {
                                CellNode::Text { text, .. } => text.clone(),
                                CellNode::Break => String::new(),
                            },
                            rects: Vec::new(),
                        })
                        .collect();
                    for (item, _, _, rect) in drawn.pieces(cell, first, at) {
                        runs[item].rects.push(rect);
                    }
                    for run in runs {
                        if !run.rects.is_empty() && !run.text.trim().is_empty() {
                            asrs_ui::parity::record_text(cx, run);
                        }
                    }
                }
            }
        },
        |_, _, _, _| {},
    );
    Some(record.absolute().size_0())
}

impl<H: FrameHost> HubFrame<H> {
    fn console_fields(
        &mut self,
        shown: Shown,
        view: &ConsoleView,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<(Entity<TextField<NumberValue>>, Entity<TextField<PlainText>>)> {
        let tab = self.console.tabs.get_mut(&shown.key)?;
        let fields = tab.fields.get_or_insert_with(|| {
            let jump = cx.new(|cx| {
                TextField::new(
                    NumberValue::default(),
                    FieldKind::Number,
                    FieldStyle::native(),
                    window,
                    cx,
                )
            });
            let filter = cx.new(|cx| {
                let mut field = TextField::new(
                    PlainText::default(),
                    FieldKind::Search,
                    FieldStyle::console_filter(),
                    window,
                    cx,
                );
                field.set_placeholder("Filter");
                field
            });
            let subscriptions = [
                cx.subscribe(&jump, |frame, field, event: &FieldEvent, cx| match event {
                    FieldEvent::Input => {
                        let value = JsString::from(field.read(cx).value().as_str());
                        frame.console_input(cx, |renderer, _| {
                            renderer.dom().borrow_mut().jump_input.set_value(value)
                        });
                    }
                    FieldEvent::Enter => frame.console_jump(cx),
                    FieldEvent::Focus | FieldEvent::Blur => cx.notify(),
                    FieldEvent::Change => {}
                }),
                cx.subscribe(
                    &filter,
                    |frame, field, event: &FieldEvent, cx| match event {
                        FieldEvent::Input => {
                            let value = JsString::from(field.read(cx).value().as_str());
                            report(frame.console_input(cx, |renderer, page| {
                                renderer.filter_input(page, &value)
                            }));
                            cx.notify();
                        }
                        FieldEvent::Focus | FieldEvent::Blur => cx.notify(),
                        FieldEvent::Enter | FieldEvent::Change => {}
                    },
                ),
            ];
            Fields {
                jump,
                filter,
                _subscriptions: subscriptions,
            }
        });
        let (jump, filter) = (fields.jump.clone(), fields.filter.clone());
        let [_, top, width, height] = JUMP_INPUT;
        let dy = snap_remainder((shown.container[1] + top) as f32, window.scale_factor());
        let placeholder = view.jump_placeholder.clone();
        jump.update(cx, |field, _| {
            field.set_placeholder(placeholder);
            field.set_size(width as f32, height as f32);
            field.set_subpixel(0.0, dy);
        });
        filter.update(cx, |field, _| {
            field.set_size(FILTER[1] as f32, FILTER[2] as f32)
        });
        Some((jump, filter))
    }

    fn console_field_focused(&self, shown: Shown, window: &Window, cx: &App) -> bool {
        self.console
            .tabs
            .get(&shown.key)
            .and_then(|tab| tab.fields.as_ref())
            .is_some_and(|f| {
                f.jump.focus_handle(cx).is_focused(window)
                    || f.filter.focus_handle(cx).is_focused(window)
            })
    }

    fn console_blur(&mut self, key: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.console.editing = false;
        let Some(fields) = self.console.tabs.get(&key).and_then(|t| t.fields.as_ref()) else {
            return;
        };
        let (jump, filter) = (fields.jump.clone(), fields.filter.clone());
        jump.update(cx, |field, cx| field.blur(window, cx));
        filter.update(cx, |field, cx| field.blur(window, cx));
    }

    /// A console that stopped being displayed: its focused input blurs and its rows are left.
    fn console_hidden(&mut self, left: Shown, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.console_blur(left.key, window, cx);
        self.console.hovered = None;
        self.console.pressed_row = None;
        if let Some(tab) = self.console.tabs.get_mut(&left.key) {
            tab.hand = HandFade::default();
            tab.layout.stop_wheel();
        }
    }

    /// Runs `f` on the displayed console's renderer as one page task.
    fn console_input<R>(
        &mut self,
        cx: &mut App,
        f: impl FnOnce(&mut ConsoleRenderer, &mut HubPage) -> R,
    ) -> Option<R> {
        let shown = self.console.shown?;
        self.host_mut()
            .input(cx, |hub| with_renderer(hub, shown.index, f))
    }

    fn console_jump(&mut self, cx: &mut Context<'_, Self>) {
        self.console_input(cx, |renderer, _| renderer.jump());
        cx.notify();
    }

    /// The delete button's `close-field` event.
    fn console_close(&mut self, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.console.shown else {
            return;
        };
        self.host_mut().input(cx, |hub| {
            with_controller(hub, shown.index, ConsoleController::close_field)
        });
        cx.notify();
    }

    /// Whether `#dragItem` or the drag highlight is over the table.
    fn console_covered(&mut self) -> bool {
        let drag = self.shell_mut().drag();
        drag.active() && !drag.item_hidden()
    }

    /// The row at a client position, outside the header.
    fn console_row_at(&self, shown: Shown, x: f64, y: f64) -> Option<usize> {
        if !shown.contains(x, y) || y < shown.container[1] + HEADER_HEIGHT {
            return None;
        }
        let layout = &self.console.tabs.get(&shown.key)?.layout;
        layout.row_at(y - shown.container[1] - HEADER_HEIGHT + layout.scroll_top())
    }

    /// Moves the hover to `row`, with the rows' `mouseleave` and `mouseenter`.
    fn console_hover(&mut self, row: Option<usize>, cx: &mut App) {
        let was = std::mem::replace(&mut self.console.hovered, row);
        if was == row {
            return;
        }
        self.console_input(cx, |renderer, page| {
            if was.is_some_and(|index| index < renderer.view().rows.len()) {
                renderer.row_mouse_leave(page);
            }
            if let Some(index) = row {
                renderer.row_mouse_enter(page, index);
            }
        });
    }

    pub(super) fn console_mouse_move(&mut self, x: f64, y: f64, cx: &mut Context<'_, Self>) {
        self.console_point(x, y, cx);
        cx.notify();
    }

    /// The pointer is at a client position: every mouse event moves the hover there.
    fn console_point(&mut self, x: f64, y: f64, cx: &mut App) {
        let Some(shown) = self.console.shown else {
            return;
        };
        let row = if self.console_covered() {
            None
        } else {
            self.console_row_at(shown, x, y)
        };
        self.console_hover(row, cx);
    }

    fn console_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(shown) = self.console.shown else {
            return;
        };
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        self.console_point(x, y, cx);
        if !inside(shown.jump_rect(), x, y) && !inside(shown.filter_rect(), x, y) {
            self.console_blur(shown.key, window, cx);
        }
        let row = self.console_row_at(shown, x, y);
        match event.button {
            MouseButton::Left => self.console.pressed_row = row,
            MouseButton::Right if row.is_some() => {
                self.console_input(cx, |renderer, page| renderer.row_context_menu(page));
            }
            _ => {}
        }
        cx.notify();
    }

    /// A row's `click`: the button went down and up on the same row.
    pub(super) fn console_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        x: f64,
        y: f64,
        cx: &mut Context<'_, Self>,
    ) {
        self.console_point(x, y, cx);
        if event.button != MouseButton::Left {
            return;
        }
        let Some(row) = self.console.pressed_row.take() else {
            return;
        };
        let Some(shown) = self.console.shown else {
            return;
        };
        if self.console_covered() || self.console_row_at(shown, x, y) != Some(row) {
            return;
        }
        self.console_input(cx, |renderer, page| renderer.row_click(page, row));
        cx.notify();
    }

    fn console_wheel(&mut self, event: &ScrollEvent, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.console.shown else {
            return;
        };
        let now = self.host().frame_time(cx);
        if let Some(tab) = self.console.tabs.get(&shown.key) {
            tab.layout.wheel(wheel_of(event), now);
        }
        cx.notify();
    }

    /// The renderer's window `keydown` listener: Cmd+F (Ctrl+F) selects the filter.
    pub(super) fn console_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let m = event.keystroke.modifiers;
        let command = if cfg!(target_os = "macos") {
            m.platform
        } else {
            m.control
        };
        if !command || event.keystroke.key != "f" || m.shift {
            return;
        }
        if self.console.editing || self.host().editable_focused(cx) {
            return;
        }
        let Some(shown) = self.console.shown else {
            return;
        };
        let filter = self
            .console
            .tabs
            .get(&shown.key)
            .and_then(|t| t.fields.as_ref())
            .map(|f| f.filter.clone());
        if let Some(filter) = filter {
            filter.update(cx, |field, cx| field.select(window, cx));
            cx.notify();
        }
    }
}
