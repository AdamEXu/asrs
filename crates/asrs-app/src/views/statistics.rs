//! The statistics tab: its renderer element (`#renderer6` in `www/hub.html`, styled by
//! `www/renderers.css`), the values table over the boxes of [`StatisticsLayout`] beside the
//! histogram's canvas; and the controller's `div.stats-settings` (`www/hub.css`). Its source
//! list is the frame's.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use asrs_canvas::text::TextBackend;
use asrs_canvas::{Pixmap, RasterState, ShadeCache};
use asrs_hub::hub::HubModel;
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_model::scroll_sensor::{WheelScroll, assign_scroll_offset};
use asrs_model::tab_type::TabType;
use asrs_tabs::form::{self, NumberInput};
use asrs_tabs::histogram::EventKind;
use asrs_tabs::statistics::StatisticsController;
use asrs_tabs::statistics::renderer::StatisticsRenderer;
use asrs_tabs::statistics::values::{StatisticsCell, StatisticsRow};
use asrs_ui::canvas_view::geometry::mouse_client;
use asrs_ui::canvas_view::{FramePresenter, rasterize};
use asrs_ui::gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, Div, Entity, Focusable, InteractiveElement,
    IntoElement, ParentElement, ScrollEvent, StatefulInteractiveElement, Styled, Subscription,
    Window, canvas, div, px, rgb,
};
use asrs_ui::input::style::CONTROL_FONT;
use asrs_ui::input::{FieldEvent, FieldKind, FieldStyle, Select, SelectEvent, TextField};
use asrs_ui::parity::probed;
use asrs_ui::selection::{Document, Hit, Place};
use asrs_ui::text::{self, Text, TextStyle, WhiteSpace, snap_remainder};
use asrs_ui::theme::{Appearance, FONT_FAMILY, Theme};

use super::cell_text::{CellText, css};
use super::frame::{FrameHost, HubFrame, wheel_of};
use super::large_table::{Grid, mean};
use super::number::FormNumber;
use super::page_text::{
    Owner, PageText, PlacedBlock, Selected, beside_table, device, floor_device, unselectable,
};
use super::select::FormSelect;
use super::source_list::{centred_offset, place_in};
use super::statistics_layout::{CellKind, Placed, StatisticsLayout, cell_style};

const SETTINGS_MAX_WIDTH: f64 = 200.0;
const SETTINGS_TOP: f64 = 5.0;
const TITLE_HEIGHT: f64 = 20.0;
const INPUT_HEIGHT: f64 = 40.0;
/// `fix-second`'s third input: `height: calc(100% - 145px); max-height: 120px` over the 40 px
/// minimum.
const LAST_INSET: f64 = 145.0;
const LAST_MAX: f64 = 120.0;
const CONTROL_MARGIN: f64 = 5.0;
const CONTROL_WIDTH: f64 = 0.7;
const SELECT_HEIGHT: f64 = 25.0;
/// A number input's padding and borders, across and down.
const FIELD_CHROME: [f64; 2] = [8.0, 6.0];
const TITLES: [&str; 3] = ["Time Range", "Data Range", "Step Size"];
const FIELD_PROBES: [&str; 3] = ["input.range-min", "input.range-max", "input.step-size"];

fn lu(v: f64) -> f64 {
    math::floor(v * 64.0) / 64.0
}

fn title_style() -> TextStyle {
    let font = format!("bold 16px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "20px"),
        ("white-space", "nowrap"),
    ])
    .expect("statistics title style")
}

/// Where `div.stats-settings` puts its parts inside `div.controls-content`, in client
/// coordinates before its own scrolling.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SettingsLayout {
    /// The border box; its first pixel column is the `border-left`.
    panel: [f64; 4],
    titles: [[f64; 4]; 3],
    select: [f64; 4],
    /// `range-min`, `range-max` and `step-size`.
    fields: [[f64; 4]; 3],
    max_scroll: f64,
}

impl SettingsLayout {
    /// `line` is the height of a number input's line of text.
    fn new([x, y, w, h]: [f64; 4], line: f64) -> SettingsLayout {
        let width = math::min2(lu(w * 0.4), SETTINGS_MAX_WIDTH);
        let panel = [x + w - width - 1.0, y, width + 1.0, h];
        let left = panel[0] + 1.0;
        let half = lu(width * 0.5);
        let title_top = |i: f64| y + SETTINGS_TOP + i * (TITLE_HEIGHT + INPUT_HEIGHT);
        let control_top = |i: f64| title_top(i) + TITLE_HEIGHT + CONTROL_MARGIN;
        let control = |left: f64, available: f64, chrome: f64, i: f64, height: f64| {
            let width = lu(available * CONTROL_WIDTH) + chrome;
            let free = available - width - 2.0 * CONTROL_MARGIN;
            let left = left + math::max2(lu(free / 2.0), 0.0) + CONTROL_MARGIN;
            [left, control_top(i), width, height]
        };
        let field = |left: f64, available: f64, i: f64| {
            control(left, available, FIELD_CHROME[0], i, line + FIELD_CHROME[1])
        };
        let last = math::max2(math::min2(h - LAST_INSET, LAST_MAX), INPUT_HEIGHT);
        let content = title_top(2.0) - y + TITLE_HEIGHT + last;
        SettingsLayout {
            panel,
            titles: [0.0, 1.0, 2.0].map(|i| [left, title_top(i), width, TITLE_HEIGHT]),
            select: control(left, width, 0.0, 0.0, SELECT_HEIGHT),
            fields: [
                field(left, half, 1.0),
                field(left + half, half, 1.0),
                field(left, width, 2.0),
            ],
            max_scroll: math::max2(content - h, 0.0),
        }
    }
}

type Field = Entity<TextField<FormNumber>>;

struct Controls {
    select: Entity<Select<FormSelect>>,
    synced: Option<form::Select>,
    fields: [Field; 3],
    scroll: f64,
    wheel: WheelScroll,
    _subscriptions: Vec<Subscription>,
}

struct TabView {
    layout: StatisticsLayout,
    /// What the chart's `ResizeObserver` reported last.
    observed: (f64, f64),
    /// The window size and scale of its last `resize` event.
    window: Option<[f32; 3]>,
    drawn: Option<(u64, f64)>,
    bitmap: Option<Arc<Pixmap>>,
    presenter: FramePresenter,
    controls: Option<Controls>,
}

impl TabView {
    fn new(layout: StatisticsLayout) -> TabView {
        TabView {
            layout,
            observed: (0.0, 0.0),
            window: None,
            drawn: None,
            bitmap: None,
            presenter: FramePresenter::default(),
            controls: None,
        }
    }
}

/// The displayed tab's boxes in client coordinates.
#[derive(Clone, Copy, Debug)]
struct Shown {
    key: usize,
    renderer: [f64; 4],
    values: [f64; 4],
    canvas: [f64; 4],
    settings: Option<SettingsLayout>,
}

#[derive(Default)]
pub struct StatisticsViews {
    tabs: HashMap<usize, TabView>,
    shown: Option<Shown>,
    /// The timeline's box, which sits above the renderer.
    covered: Option<[f64; 4]>,
    hovered: bool,
    pointer: Option<[f64; 2]>,
    pressed: bool,
    /// A satellite's next frame runs the chart's callbacks.
    frame_requested: bool,
}

fn inside([left, top, w, h]: [f64; 4], x: f64, y: f64) -> bool {
    x >= left && x < left + w && y >= top && y < top + h
}

impl StatisticsViews {
    fn over_canvas(&self, x: f64, y: f64) -> bool {
        let covered = self.covered.is_some_and(|c| inside(c, x, y));
        !covered && self.shown.is_some_and(|s| inside(s.canvas, x, y))
    }
}

fn renderer_key(renderer: &StatisticsRenderer) -> usize {
    std::ptr::from_ref(renderer) as usize
}

fn tab_index(hub: &HubModel, key: usize) -> Option<usize> {
    hub.tabs().tabs().position(|t| {
        t.renderer()
            .downcast_ref::<StatisticsRenderer>()
            .is_some_and(|r| renderer_key(r) == key)
    })
}

fn with_renderer<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut StatisticsRenderer) -> R,
) -> Option<R> {
    let tab = hub.tabs_mut().0.tab_mut(index)?;
    tab.renderer.downcast_mut::<StatisticsRenderer>().map(f)
}

fn with_controller<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut StatisticsController) -> R,
) -> Option<R> {
    let tab = hub.tabs_mut().0.tab_mut(index)?;
    tab.controller.downcast_mut::<StatisticsController>().map(f)
}

/// The displayed statistics tab, whose controller a satellite does not have.
fn displayed(hub: &HubModel) -> Option<(&StatisticsRenderer, Option<&StatisticsController>)> {
    hub.tabs().tabs().find_map(|tab| {
        if tab.content_hidden() || tab.tab_type() != Some(TabType::Statistics) {
            return None;
        }
        let renderer = tab.renderer().downcast_ref::<StatisticsRenderer>()?;
        let controller = tab.controller().downcast_ref::<StatisticsController>();
        Some((renderer, controller))
    })
}

struct Palette {
    table: u32,
    cell: u32,
    /// `border-top` of the first section and of the others.
    sections: [u32; 2],
}

impl Palette {
    fn of(appearance: Appearance) -> Palette {
        appearance.pick(
            Palette {
                table: 0x555555,
                cell: 0xeeeeee,
                sections: [0xeeeeee, 0x555555],
            },
            Palette {
                table: 0x999999,
                cell: 0x333333,
                sections: [0x999999, 0x555555],
            },
        )
    }
}

/// `style.color` as `ensureThemeContrast` wrote it.
fn cell_color(cell: Option<&StatisticsCell>, theme: &Theme) -> u32 {
    cell.and_then(|c| c.color.as_deref())
        .and_then(|c| c.strip_prefix('#'))
        .filter(|hex| hex.len() == 6)
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .unwrap_or(theme.text)
}

/// 1 px borders inside `cell`, `top` along the top and `side` along the rest, with the top
/// corners' diagonal pixels taking half of each.
fn cell_borders(
    grid: Grid,
    [x, y, w, h]: [f64; 4],
    border: f64,
    (top, side): (u32, u32),
    layer: Div,
) -> Div {
    let (left, up) = (grid.snap(x), grid.snap(y));
    let (right, down) = (grid.snap(x + w), grid.snap(y + h));
    let fill = |rect: [f64; 4], color: u32| grid.place(rect).bg(rgb(color));
    let mut layer = layer
        .child(fill([left, up, border, down - up], side))
        .child(fill([right - border, up, border, down - up], side))
        .child(fill([left, down - border, right - left, border], side))
        .child(fill([left, up, right - left, border], top));
    if top == side {
        return layer;
    }
    let pixel = 1.0 / grid.scale;
    let half = mean(top, side);
    for step in 0..(border * grid.scale).round() as usize {
        let at = step as f64 * pixel;
        layer = layer
            .child(fill([left, up + at, at, pixel], side))
            .child(fill([left + at, up + at, pixel, pixel], half))
            .child(fill([right - at, up + at, at, pixel], side))
            .child(fill([right - at - pixel, up + at, pixel, pixel], half));
    }
    layer
}

/// The lines of `text` from client (`x`, `y`).
pub(super) fn text_lines(
    grid: Grid,
    text: &CellText,
    style: &TextStyle,
    [x, y]: [f64; 2],
    color: u32,
    layer: Div,
) -> Div {
    let mut style = *style;
    style.white_space = WhiteSpace::Pre;
    style.align = text::Align::Start;
    let scale = grid.scale as f32;
    text.lines.iter().fold(layer, |layer, line| {
        let at = [x + css(line.left, scale), y + css(line.top, scale)];
        layer.child(grid.text(Text::new(line.text.clone(), style).color(color), at))
    })
}

/// The value cells in document order.
struct StatisticsText {
    placed: Rc<Placed>,
    /// Each value cell's row and column.
    cells: Vec<[usize; 2]>,
    builds: u64,
}

impl StatisticsText {
    fn new(placed: Rc<Placed>, builds: u64) -> StatisticsText {
        let mut cells = Vec::new();
        for (r, row) in placed.rows.iter().enumerate() {
            let values = row.cells.iter().enumerate();
            let values = values.filter(|(_, cell)| cell.kind == CellKind::Value);
            cells.extend(values.map(|(c, _)| [r, c]));
        }
        StatisticsText {
            placed,
            cells,
            builds,
        }
    }

    fn cell(&self, block: usize) -> &CellText {
        let [row, column] = self.cells[block];
        &self.placed.rows[row].cells[column].text
    }

    /// The first value cell in or after a cell.
    fn block(&self, row: usize, column: usize) -> usize {
        self.cells.partition_point(|&at| at < [row, column])
    }
}

impl Document for StatisticsText {
    fn len(&self) -> usize {
        self.cells.len()
    }

    fn text(&self, block: usize) -> Cow<'_, str> {
        Cow::Borrowed(&self.cell(block).text)
    }

    fn place(&self, block: usize) -> Place {
        let [row, column] = self.cells[block];
        let cells = &self.placed.rows[row].cells;
        let last = column + 1 == cells.len();
        let title = if column == 1 {
            2 + cells[0].text.text.encode_utf16().count()
        } else {
            1
        };
        Place {
            cell: true,
            tab: true,
            newline_after: last,
            closed: last,
            after_other: column == 1,
            gap: title,
            ..Place::default()
        }
    }

    fn generation(&self) -> u64 {
        self.builds
    }
}

struct StatisticsPage {
    doc: StatisticsText,
    style: TextStyle,
    renderer: [f64; 4],
    values: [f64; 4],
    /// The table's client position.
    origin: [f64; 2],
}

impl PageText for StatisticsPage {
    fn document(&self) -> &dyn Document {
        &self.doc
    }

    fn scroller(&self) -> [f64; 4] {
        self.values
    }

    fn renderer(&self) -> [f64; 4] {
        self.renderer
    }

    fn hit(&self, at @ [x, y]: [f64; 2], backend: &mut dyn TextBackend) -> Hit {
        let placed = &*self.doc.placed;
        let scale = f64::from(placed.scale);
        let [left, _, width, _] = self.values;
        if (left + width..).contains(&x) {
            return unselectable(&self.doc, self.doc.len());
        }
        let lu = |css: f64| floor_device(css, scale);
        let table = placed.table;
        let at_table = [lu(x - self.origin[0]), lu(y - self.origin[1])];
        let outside = [0, 1]
            .into_iter()
            .any(|axis| at_table[axis] < 0 || at_table[axis] >= table[axis]);
        if outside {
            let width = css(table[0], placed.scale);
            return beside_table(&self.doc, x, [self.origin[0], width]);
        }
        let rows = &placed.rows;
        let row = rows
            .partition_point(|row| row.top + row.height <= at_table[1])
            .min(rows.len() - 1);
        let cells = &rows[row].cells;
        let column = cells
            .partition_point(|cell| cell.left + cell.width <= at_table[0])
            .min(cells.len() - 1);
        let (placed_row, cell) = (&rows[row], &cells[column]);
        let index = self.doc.block(row, column);
        if cell.kind != CellKind::Value {
            return unselectable(&self.doc, index);
        }
        let [dx, dy] = placed.content(placed_row, cell);
        let block = cell.text.block(&self.style);
        let text = PlacedBlock {
            index,
            block: &block,
            text: &cell.text.text,
            origin: [
                device(self.origin[0], scale) + dx,
                device(self.origin[1], scale) + dy,
            ],
            scale,
        };
        let inside = at_table[0] >= cell.left
            && at_table[1] >= placed_row.top
            && at_table[0] < cell.left + cell.width
            && at_table[1] < placed_row.top + placed_row.height;
        let hit = text.hit(at, backend);
        Hit {
            selectable: inside,
            text: hit.text && inside,
            ..hit
        }
    }
}

fn table_layer(
    grid: Grid,
    doc: &StatisticsText,
    rows: &[StatisticsRow],
    [x, y]: [f64; 2],
    appearance: Appearance,
    selected: Option<(&Selected, &RefCell<Box<dyn TextBackend>>)>,
) -> Div {
    let placed = &*doc.placed;
    let palette = Palette::of(appearance);
    let theme = appearance.theme();
    let px = |lu| css(lu, placed.scale);
    let border = px(placed.border);
    let [width, height] = placed.table.map(px);
    let styles = [CellKind::Section, CellKind::Title, CellKind::Value].map(cell_style);
    let mut layer = cell_borders(
        grid,
        [x, y, width, height],
        border,
        (palette.table, palette.table),
        div().absolute().inset_0(),
    );
    for (r, (row, data)) in placed.rows.iter().zip(rows).enumerate() {
        for (c, cell) in row.cells.iter().enumerate() {
            let bounds = [
                x + px(cell.left),
                y + px(row.top),
                px(cell.width),
                px(row.height),
            ];
            let top = match cell.kind {
                CellKind::Section => palette.sections[usize::from(r > 0)],
                CellKind::Title | CellKind::Value => palette.cell,
            };
            layer = cell_borders(grid, bounds, border, (top, palette.cell), layer);
            let value = match data {
                StatisticsRow::Values { cells, .. } if c > 0 => cells.get(c - 1),
                _ => None,
            };
            let [dx, dy] = placed.content(row, cell).map(px);
            if cell.kind == CellKind::Value
                && let Some((selected, backend)) = selected
            {
                let index = doc.block(r, c);
                let block = cell.text.block(&styles[cell.kind as usize]);
                let content = placed.content(row, cell);
                let block = PlacedBlock {
                    index,
                    block: &block,
                    text: &cell.text.text,
                    origin: [
                        device(x, grid.scale) + content[0],
                        device(y, grid.scale) + content[1],
                    ],
                    scale: grid.scale,
                };
                // `overflow: hidden` clips at the padding box.
                let inner = [
                    x + px(cell.left) + border,
                    y + px(row.top) + border,
                    px(cell.width) - 2.0 * border,
                    px(row.height) - 2.0 * border,
                ];
                let clip = Grid {
                    origin: [grid.snap(inner[0]), grid.snap(inner[1])],
                    ..grid
                };
                let drawn = grid.place(inner).overflow_hidden();
                let backend = &mut **backend.borrow_mut();
                layer = layer.child(selected.paint(clip, &block, backend, drawn));
            }
            layer = text_lines(
                grid,
                &cell.text,
                &styles[cell.kind as usize],
                [x + dx, y + dy],
                cell_color(value, theme),
                layer,
            );
        }
    }
    layer
}

impl<H: FrameHost> HubFrame<H> {
    /// Gives every statistics renderer its layout and its chart its window, as the document does
    /// whether or not the tab is shown.
    fn statistics_mount(
        &mut self,
        shown: Option<usize>,
        size: [f64; 2],
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let viewport = window.viewport_size();
        let viewport = [f32::from(viewport.width), f32::from(viewport.height), scale];
        let mut tabs = std::mem::take(&mut self.statistics.tabs);
        let mut dropped = Vec::new();
        let mut live = Vec::new();
        let result = self.host_mut().input(cx, |hub| {
            let mut result = Ok(());
            for index in 0..hub.tabs().len() {
                let attached = with_renderer(hub, index, |renderer| {
                    let key = renderer_key(renderer);
                    live.push(key);
                    if renderer.histogram().is_none() {
                        let layout = StatisticsLayout::default();
                        renderer.mount(f64::from(scale), Box::new(layout.clone()));
                        let replaced = tabs.insert(key, TabView::new(layout));
                        dropped.extend(replaced.map(|tab| tab.presenter));
                    }
                    let tab = tabs.get_mut(&key)?;
                    let displayed = shown == Some(key);
                    if displayed {
                        tab.layout.resize(size, scale);
                    }
                    let left = renderer.histogram_left();
                    let chart = renderer.histogram_mut()?;
                    if tab.window.is_some_and(|w| w != viewport) {
                        chart.window_resized(f64::from(scale));
                    }
                    tab.window = Some(viewport);
                    let content = if displayed {
                        let [_, _, w, h] = tab.layout.histogram(left, chart.size().0);
                        (w, h)
                    } else {
                        (0.0, 0.0)
                    };
                    if !chart.is_attached() {
                        return Some(renderer.attach(content));
                    }
                    if content != tab.observed {
                        tab.observed = content;
                        chart.container_resized(content);
                    }
                    None
                });
                result = result.and(attached.flatten().unwrap_or(Ok(())));
            }
            result
        });
        if let Err(e) = result {
            eprintln!("asrs: {}: {}", e.name, e.message);
        }
        tabs.retain(|key, tab| {
            let keep = live.contains(key);
            if !keep {
                dropped.push(std::mem::take(&mut tab.presenter));
            }
            keep
        });
        self.statistics.tabs = tabs;
        for mut presenter in dropped {
            presenter.release(cx);
        }
    }

    /// The displayed statistics tab's renderer element over `rect` (`div.renderer-content`).
    /// `covered` is the timeline's box.
    pub(super) fn statistics_element(
        &mut self,
        rect: [f64; 4],
        covered: Option<[f64; 4]>,
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let key = displayed(self.host().hub(cx)).map(|(renderer, _)| renderer_key(renderer));
        if self.statistics.shown.map(|s| s.key) != key {
            self.statistics_hidden(window, cx);
        }
        self.statistics_mount(key, [rect[2], rect[3]], scale, window, cx);
        self.statistics.covered = covered;
        let (renderer, _) = displayed(self.host().hub(cx))?;
        let key = renderer_key(renderer);
        let layout = self.statistics.tabs.get(&key)?.layout.clone();
        let rows = renderer.table().rows().to_vec();
        let chart = renderer.histogram()?;
        let (chart_size, left) = (chart.size(), renderer.histogram_left());
        let drawn = (chart.revision(), chart.device_pixel_ratio());
        let picture = chart.canvas().clone();

        let now = self.host().frame_time(cx);
        let scrolling = layout.animate(now);
        let placed = layout.place(&rows);
        let [scroll_x, scroll_y] = layout.scroll();
        let container = layout.histogram(left, chart_size.0);
        let shown = Shown {
            key,
            renderer: rect,
            values: [
                rect[0],
                rect[1],
                css(placed.container[0], scale),
                css(placed.container[1], scale),
            ],
            canvas: [
                rect[0] + container[0],
                rect[1] + container[1],
                chart_size.0,
                chart_size.1,
            ],
            settings: self.statistics.shown.and_then(|s| s.settings),
        };
        self.statistics.shown = Some(shown);
        self.statistics_layout_changed(cx);
        // The hub's own frame loop runs the chart's callbacks, which a satellite does not have.
        let pending = displayed(self.host().hub(cx))
            .and_then(|(r, _)| r.histogram())
            .is_some_and(|chart| chart.callbacks() > 0);
        if scrolling || pending {
            window.request_animation_frame();
        }
        let satellite = self.host().hub(cx).page().is_satellite();
        if satellite && pending && !std::mem::replace(&mut self.statistics.frame_requested, true) {
            let frame = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = frame.update(cx, |frame, cx| {
                    frame.statistics.frame_requested = false;
                    frame.host_mut().animation_frame(cx);
                });
            });
        }

        let tab = self.statistics.tabs.get_mut(&key)?;
        if tab.drawn != Some(drawn) {
            tab.drawn = Some(drawn);
            let mut state = RasterState::default();
            let mut shades = ShadeCache::default();
            tab.bitmap = rasterize(&picture, &HashMap::new(), &mut state, &mut shades, cx)
                .map(|(pixmap, _)| Arc::new(pixmap));
            match &tab.bitmap {
                Some(bitmap) => tab.presenter.present((**bitmap).clone(), window, cx),
                None => tab.presenter.clear(window, cx),
            }
        }

        let grid = Grid {
            origin: [rect[0], rect[1]],
            scale: f64::from(scale),
        };
        let margin = css(placed.margin, scale);
        let origin = [rect[0] + margin - scroll_x, rect[1] + margin - scroll_y];
        let [width, height] = placed.table.map(|v| css(v, scale));
        let builds = displayed(self.host().hub(cx)).map_or(0, |(r, _)| r.table().builds());
        let doc = StatisticsText::new(placed.clone(), builds);
        let owner = Owner::Statistics(key);
        let selected = self.text.selected(&owner, &doc, appearance, window, cx);
        let selected = selected.as_ref().map(|s| (s, self.text.backend_cell()));
        let table = table_layer(grid, &doc, &rows, origin, appearance, selected);
        let values = grid
            .place(shown.values)
            .id("stats-values-container")
            .overflow_hidden()
            .on_scroll(
                cx.listener(|frame, event: &ScrollEvent, _, cx| frame.statistics_wheel(event, cx)),
            )
            .child(table)
            .child(probed(
                "table.stats-values",
                grid.place([origin[0], origin[1], width, height]),
            ));
        let dump = self.statistics.tabs.get(&key)?.bitmap.clone();
        let canvas_rect = shown.canvas;
        let entity = cx.entity().downgrade();
        let bitmap = canvas(
            |_, _, _| {},
            move |_, _, window, cx| {
                let _ = entity.update(cx, |frame, _| {
                    if let Some(tab) = frame.statistics.tabs.get(&key) {
                        let at = [canvas_rect[0] as f32, canvas_rect[1] as f32];
                        tab.presenter.paint(at, window);
                    }
                });
                #[cfg(feature = "parity")]
                if let Some(bitmap) = &dump {
                    asrs_ui::parity::record_canvas(cx, || asrs_ui::parity::CanvasDump {
                        rect: canvas_rect,
                        width: bitmap.width(),
                        height: bitmap.height(),
                        rgba: asrs_canvas::pixels::to_straight_rgba(bitmap),
                    });
                }
                #[cfg(not(feature = "parity"))]
                let _ = &dump;
            },
        )
        .absolute()
        .size_full();
        let client = |r: [f64; 4]| [rect[0] + r[0], rect[1] + r[1], r[2], r[3]];
        Some(
            div()
                .absolute()
                .left_0()
                .top_0()
                .size_full()
                .child(probed("div.stats-values-container", values))
                .child(probed(
                    "div.stats-histogram-container",
                    grid.place(client(container))
                        .overflow_hidden()
                        .child(bitmap),
                ))
                .child(probed(
                    "div.stats-histogram-container > canvas",
                    grid.place(canvas_rect)
                        .id("stats-histogram")
                        .on_click(cx.listener(|frame, event: &ClickEvent, _, cx| {
                            frame.statistics_click(event, cx)
                        })),
                ))
                .into_any_element(),
        )
    }

    /// The displayed statistics tab's text that can be selected.
    pub(super) fn statistics_text<'a>(
        &'a self,
        cx: &'a App,
    ) -> Option<(Owner, Box<dyn PageText + 'a>)> {
        let (renderer, _) = displayed(self.host().hub(cx))?;
        let key = renderer_key(renderer);
        let shown = self.statistics.shown.filter(|s| s.key == key)?;
        let layout = &self.statistics.tabs.get(&key)?.layout;
        let placed = layout.place(renderer.table().rows());
        let margin = css(placed.margin, placed.scale);
        let scroll = layout.scroll();
        let [left, top, ..] = shown.values;
        let page = StatisticsPage {
            doc: StatisticsText::new(placed, renderer.table().builds()),
            style: cell_style(CellKind::Value),
            renderer: shown.renderer,
            values: shown.values,
            origin: [left + margin - scroll[0], top + margin - scroll[1]],
        };
        Some((Owner::Statistics(key), Box::new(page)))
    }

    /// `AutoscrollController`'s scrolling of the values.
    pub(super) fn statistics_scroll_by(&mut self, by: [f64; 2]) {
        let Some(shown) = self.statistics.shown else {
            return;
        };
        if let Some(tab) = self.statistics.tabs.get(&shown.key) {
            tab.layout.scroll_by(by);
        }
    }

    /// The tab shown last is gone or hidden: its focused input blurs and the pointer leaves it.
    fn statistics_hidden(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.statistics_blur(window, cx);
        if std::mem::take(&mut self.statistics.hovered)
            && let Some([x, y]) = self.statistics.pointer
        {
            self.statistics_pointer(EventKind::MouseOut, x, y, cx);
        }
        if let Some(shown) = self.statistics.shown.take()
            && let Some(tab) = self.statistics.tabs.get(&shown.key)
        {
            tab.layout.stop_wheel();
        }
    }

    /// Chromium's fake mouse move after a layout change.
    fn statistics_layout_changed(&mut self, cx: &mut Context<'_, Self>) {
        let views = &self.statistics;
        if views.hovered
            && !views.pressed
            && let Some([x, y]) = views.pointer
            && !views.over_canvas(x, y)
        {
            self.statistics.hovered = false;
            self.statistics_pointer(EventKind::MouseOut, x, y, cx);
        }
    }

    fn statistics_pointer(&mut self, kind: EventKind, x: f64, y: f64, cx: &mut App) {
        let Some(shown) = self.statistics.shown else {
            return;
        };
        let [left, top, ..] = shown.canvas;
        let (x, y) = (math::floor(x - left), math::floor(y - top));
        self.host_mut().input(cx, |hub| {
            let index = tab_index(hub, shown.key)?;
            with_renderer(hub, index, |renderer| {
                if let Some(chart) = renderer.histogram_mut() {
                    chart.pointer(kind, x, y);
                }
            })
        });
    }

    pub(super) fn statistics_mouse_move(
        &mut self,
        x: f64,
        y: f64,
        pressed: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let drag = self.shell_mut().drag();
        let under_drag_item = drag.active() && !drag.item_hidden();
        let views = &mut self.statistics;
        views.pointer = Some([x, y]);
        views.pressed = pressed;
        let over = !under_drag_item && views.over_canvas(x, y);
        let was = std::mem::replace(&mut views.hovered, over);
        if over || was {
            let kind = if over {
                EventKind::MouseMove
            } else {
                EventKind::MouseOut
            };
            self.statistics_pointer(kind, x, y, cx);
            cx.notify();
        }
    }

    /// The pointer left the window.
    pub(super) fn statistics_mouse_exit(&mut self, cx: &mut Context<'_, Self>) {
        if std::mem::take(&mut self.statistics.hovered)
            && let Some([x, y]) = self.statistics.pointer
        {
            self.statistics_pointer(EventKind::MouseOut, x, y, cx);
            cx.notify();
        }
    }

    fn statistics_click(&mut self, event: &ClickEvent, cx: &mut Context<'_, Self>) {
        let position = event.position();
        let x = mouse_client(f32::from(position.x));
        let y = mouse_client(f32::from(position.y));
        if self.statistics.over_canvas(x, y) {
            self.statistics_pointer(EventKind::Click, x, y, cx);
            cx.notify();
        }
    }

    fn statistics_wheel(&mut self, event: &ScrollEvent, cx: &mut Context<'_, Self>) {
        let now = self.host().frame_time(cx);
        let tab = self.statistics.shown.map(|s| s.key);
        if let Some(tab) = tab.and_then(|key| self.statistics.tabs.get(&key)) {
            tab.layout.wheel(wheel_of(event), now);
        }
        cx.notify();
    }

    /// A press anywhere in the window: outside the focused input it takes the focus away.
    pub(super) fn statistics_window_mouse_down(
        &mut self,
        x: f64,
        y: f64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(shown) = self.statistics.shown else {
            return;
        };
        let scroll = self.statistics_settings_scroll(shown.key);
        let over_field = shown.settings.is_some_and(|settings| {
            let mut fields = settings.fields.iter();
            fields.any(|&[fx, fy, w, h]| inside([fx, fy - scroll, w, h], x, y))
        });
        if !over_field {
            self.statistics_blur(window, cx);
        }
    }

    fn statistics_blur(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let shown = self.statistics.shown.map(|s| s.key);
        let tab = shown.and_then(|key| self.statistics.tabs.get(&key));
        let Some(controls) = tab.and_then(|t| t.controls.as_ref()) else {
            return;
        };
        for field in controls.fields.clone() {
            if field.focus_handle(cx).is_focused(window) {
                field.update(cx, |field, cx| field.blur(window, cx));
            }
        }
    }

    fn statistics_settings_scroll(&self, key: usize) -> f64 {
        let tab = self.statistics.tabs.get(&key);
        tab.and_then(|t| t.controls.as_ref())
            .map_or(0.0, |c| c.scroll)
    }

    fn statistics_control<R>(
        &mut self,
        key: usize,
        cx: &mut App,
        f: impl FnOnce(&mut StatisticsController) -> R,
    ) -> Option<R> {
        self.host_mut().input(cx, |hub| {
            let index = tab_index(hub, key)?;
            with_controller(hub, index, f)
        })
    }

    fn statistics_field_event(
        &mut self,
        key: usize,
        field: usize,
        event: FieldEvent,
        value: JsString,
        cx: &mut Context<'_, Self>,
    ) {
        self.statistics_control(key, cx, |controller| match (event, field) {
            (FieldEvent::Input, 0) => controller.set_range_min(value),
            (FieldEvent::Input, 1) => controller.set_range_max(value),
            (FieldEvent::Input, _) => controller.set_step_size(value),
            (FieldEvent::Change, 0) => controller.range_min_input(value),
            (FieldEvent::Change, 1) => controller.range_max_input(value),
            (FieldEvent::Change, _) => controller.step_size_input(value),
            (FieldEvent::Enter | FieldEvent::Focus | FieldEvent::Blur, _) => {}
        });
        cx.notify();
    }

    fn statistics_make_controls(
        &mut self,
        key: usize,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Controls {
        let select = cx.new(|cx| Select::new(FormSelect::default(), window, cx));
        let mut subscriptions = vec![cx.subscribe(
            &select,
            move |frame: &mut Self, select, event: &SelectEvent, cx| {
                if *event == SelectEvent::Change {
                    let value = select.read(cx).model().select().value();
                    frame.statistics_control(key, cx, |controller| {
                        controller.time_range_input(value)
                    });
                    cx.notify();
                }
            },
        )];
        let fields = [0, 1, 2].map(|i| {
            let field = cx.new(|cx| {
                let style = FieldStyle::setting_block();
                TextField::new(FormNumber::default(), FieldKind::Number, style, window, cx)
            });
            subscriptions.push(cx.subscribe(
                &field,
                move |frame: &mut Self, field, event: &FieldEvent, cx| {
                    let value = JsString::from(field.read(cx).value().as_str());
                    frame.statistics_field_event(key, i, *event, value, cx);
                },
            ));
            field
        });
        Controls {
            select,
            synced: None,
            fields,
            scroll: 0.0,
            wheel: WheelScroll::default(),
            _subscriptions: subscriptions,
        }
    }

    /// `div.stats-settings` in `controls` (`div.controls-content`), if the displayed tab is a
    /// statistics tab.
    pub(super) fn statistics_controls(
        &mut self,
        controls: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let (renderer, controller) = displayed(self.host().hub(cx))?;
        let controller = controller?;
        let key = renderer_key(renderer);
        let time_range = controller.time_range().clone();
        let inputs: [NumberInput; 3] = [
            controller.range_min().clone(),
            controller.range_max().clone(),
            controller.step_size().clone(),
        ];
        let (_, line) = text::measure(cx, &TextStyle::new(CONTROL_FONT), "0", scale);
        let layout = SettingsLayout::new(controls, line);
        if let Some(shown) = &mut self.statistics.shown {
            shown.settings = Some(layout);
        }
        if self.statistics.tabs.get(&key)?.controls.is_none() {
            let made = self.statistics_make_controls(key, window, cx);
            self.statistics.tabs.get_mut(&key)?.controls = Some(made);
        }
        let now = self.host().frame_time(cx);
        let parts = self.statistics.tabs.get_mut(&key)?.controls.as_mut()?;
        if let Some([_, to]) = parts.wheel.animate(now) {
            parts.scroll = to;
        }
        parts.scroll = parts.scroll.clamp(0.0, layout.max_scroll);
        if parts.wheel.animating() {
            window.request_animation_frame();
        }
        let scroll = assign_scroll_offset(parts.scroll, layout.max_scroll, f64::from(scale));
        let changed = parts.synced.as_ref().is_none_or(|s| {
            s.items() != time_range.items() || s.selected_index() != time_range.selected_index()
        });
        if changed {
            parts.synced = Some(time_range.clone());
        }
        let (select, fields) = (parts.select.clone(), parts.fields.clone());
        if changed {
            select.update(cx, |s, cx| {
                s.update_model(|m| *m.select_mut() = time_range, cx);
            });
        }
        let scrolled = |[x, y, w, h]: [f64; 4]| [x, y - scroll, w, h];
        let subpixel = |v: f64| snap_remainder(v as f32, scale);
        let select_rect = scrolled(layout.select);
        select.update(cx, |s, _| {
            s.set_size(select_rect[2] as f32, select_rect[3] as f32);
            s.set_subpixel(subpixel(select_rect[0]), subpixel(select_rect[1]));
        });
        for ((field, input), rect) in fields.iter().zip(inputs).zip(layout.fields) {
            let [x, y, w, h] = scrolled(rect);
            field.update(cx, |field, cx| {
                field.set_size(w as f32, h as f32);
                field.set_subpixel(subpixel(x), subpixel(y));
                field.update_model(|model| model.0.set_step(input.step().clone()));
                let value = input.value().to_string_lossy();
                if field.value() != value {
                    field.set_value(&value, window, cx);
                }
            });
        }

        let theme = appearance.theme();
        let dark = appearance == Appearance::Dark;
        let panel = Grid {
            origin: [controls[0], controls[1]],
            scale: f64::from(scale),
        };
        let within = Grid {
            origin: [panel.snap(layout.panel[0]), panel.snap(layout.panel[1])],
            ..panel
        };
        let height = layout.panel[3];
        let mut out = panel
            .place(layout.panel)
            .id("stats-settings")
            .overflow_hidden()
            .on_scroll(cx.listener(move |frame, event: &ScrollEvent, _, cx| {
                frame.statistics_settings_wheel(key, layout, event, cx)
            }))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(1.0))
                    .h(px(height as f32))
                    .bg(rgb(if dark { 0x000000 } else { 0xdddddd })),
            );
        for (label, rect) in TITLES.into_iter().zip(layout.titles) {
            let r = scrolled(rect);
            let (w, _) = text::measure(cx, &title_style(), label, scale);
            let Some(offset) = centred_offset(r[2], w) else {
                continue;
            };
            let (x, dx) = place_in(r[0], offset, f64::from(scale));
            out = out.child(
                within.place(r).overflow_hidden().child(
                    div().absolute().left(px(x)).top_0().child(
                        Text::new(label, title_style())
                            .color(theme.text)
                            .subpixel(dx as f32, 0.0),
                    ),
                ),
            );
        }
        out = out.child(probed(
            "select.time-range",
            within.place(select_rect).child(select),
        ));
        for ((field, id), rect) in fields.into_iter().zip(FIELD_PROBES).zip(layout.fields) {
            out = out.child(probed(id, within.place(scrolled(rect)).child(field)));
        }
        Some(probed("div.stats-settings", out).into_any_element())
    }

    fn statistics_settings_wheel(
        &mut self,
        key: usize,
        layout: SettingsLayout,
        event: &ScrollEvent,
        cx: &mut Context<'_, Self>,
    ) {
        let now = self.host().frame_time(cx);
        let tab = self.statistics.tabs.get_mut(&key);
        let Some(parts) = tab.and_then(|t| t.controls.as_mut()) else {
            return;
        };
        let offset = [0.0, parts.scroll];
        let max = [0.0, layout.max_scroll];
        let client = [layout.panel[2], layout.panel[3]];
        if let Some([_, to]) = parts.wheel.wheel(wheel_of(event), now, offset, max, client) {
            parts.scroll = to.clamp(0.0, layout.max_scroll);
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_boxes_match_blink() {
        let layout = SettingsLayout::new([300.0, 450.0, 800.0, 200.0], 15.0);
        assert_eq!(layout.panel, [899.0, 450.0, 201.0, 200.0]);
        assert_eq!(layout.select, [930.0, 480.0, 140.0, 25.0]);
        assert_eq!(
            layout.fields,
            [
                [911.0, 540.0, 78.0, 21.0],
                [1011.0, 540.0, 78.0, 21.0],
                [926.0, 600.0, 148.0, 21.0]
            ]
        );
        assert_eq!(layout.max_scroll, 0.0);
        assert_eq!(
            SettingsLayout::new([300.0, 550.0, 800.0, 100.0], 15.0).max_scroll,
            85.0
        );
    }
}
