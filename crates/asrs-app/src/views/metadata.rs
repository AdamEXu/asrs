//! The metadata tab's renderer element (`#renderer12` in `www/hub.html`, styled by
//! `www/renderers.css`), drawn from R11's `MetadataTable` over the boxes of [`MetadataLayout`].

use std::borrow::Cow;

use asrs_canvas::text::TextBackend;
use asrs_hub::hub::HubModel;
use asrs_model::tab_type::TabType;
use asrs_tabs::metadata::MetadataRenderer;
use asrs_ui::gpui::{
    AnyElement, App, Context, IntoElement, ParentElement, Styled, Window, div, rgb,
};
use asrs_ui::parity::probed;
use asrs_ui::selection::{Document, Hit, Place};
use asrs_ui::text::opaque_background;
use asrs_ui::theme::Appearance;

use super::cell_text::css;
use super::frame::{FrameHost, HubFrame};
use super::large_table::Grid;
use super::metadata_layout::{
    CellKind, MetadataLayout, Placed, PlacedCell, alert_style, cell_style, satellite_alert_style,
};
use super::page_text::{
    Owner, PageText, PlacedBlock, Selected, beside_table, device, floor_device, unselectable,
};
use super::statistics::{selected_lines, text_lines};

#[derive(Default)]
pub struct MetadataView {
    layout: MetadataLayout,
    rect: [f64; 4],
}

/// The displayed tab's renderer when it is a metadata tab. The hub has at most one.
fn displayed(hub: &HubModel) -> Option<&MetadataRenderer> {
    hub.tabs().tabs().find_map(|tab| {
        if tab.content_hidden() || tab.tab_type() != Some(TabType::Metadata) {
            return None;
        }
        tab.renderer().downcast_ref::<MetadataRenderer>()
    })
}

/// The value cells in document order.
struct MetadataText<'a> {
    placed: &'a Placed,
    columns: usize,
    builds: u64,
}

impl<'a> MetadataText<'a> {
    fn new(placed: &'a Placed, builds: u64) -> MetadataText<'a> {
        let header = placed.rows.first();
        MetadataText {
            placed,
            columns: header.map_or(0, |row| row.cells.len().saturating_sub(1)),
            builds,
        }
    }

    fn cell(&self, block: usize) -> &'a PlacedCell {
        let row = &self.placed.rows[1 + block / self.columns];
        &row.cells[1 + block % self.columns]
    }
}

impl Document for MetadataText<'_> {
    fn len(&self) -> usize {
        self.placed.rows.len().saturating_sub(1) * self.columns
    }

    fn text(&self, block: usize) -> Cow<'_, str> {
        Cow::Borrowed(&self.cell(block).text.text)
    }

    fn place(&self, block: usize) -> Place {
        let column = block % self.columns;
        let last = column + 1 == self.columns;
        let key = &self.placed.rows[1 + block / self.columns].cells[0]
            .text
            .text;
        Place {
            cell: true,
            tab: true,
            newline_after: last,
            closed: last,
            after_other: column == 0,
            gap: if column == 0 {
                2 + key.encode_utf16().count()
            } else {
                1
            },
            ..Place::default()
        }
    }

    fn generation(&self) -> u64 {
        self.builds
    }
}

struct MetadataPage<'a> {
    doc: MetadataText<'a>,
    rect: [f64; 4],
}

impl PageText for MetadataPage<'_> {
    fn document(&self) -> &dyn Document {
        &self.doc
    }

    fn scroller(&self) -> [f64; 4] {
        self.rect
    }

    fn hit(&self, at @ [x, y]: [f64; 2], backend: &mut dyn TextBackend) -> Hit {
        let placed = self.doc.placed;
        let scale = f64::from(placed.scale);
        let [left, top, width, _] = self.rect;
        let at_table = [floor_device(x - left, scale), floor_device(y - top, scale)];
        let outside = x < left || x >= left + width || at_table[1] < 0;
        if outside || at_table[1] >= placed.height || self.doc.is_empty() {
            return beside_table(&self.doc, x, [left, width]);
        }
        let row = placed
            .rows
            .partition_point(|row| row.top + row.height <= at_table[1])
            .min(placed.rows.len() - 1);
        if row == 0 {
            return unselectable(&self.doc, 0);
        }
        let cells = &placed.rows[row].cells;
        let column = cells
            .partition_point(|cell| cell.left + cell.width <= at_table[0])
            .min(cells.len() - 1);
        let first = (row - 1) * self.doc.columns;
        if column == 0 {
            return unselectable(&self.doc, first);
        }
        let cell = &cells[column];
        let content = placed.content(&placed.rows[row], cell);
        let block = cell.text.block(&cell_style(cell.kind));
        let text = PlacedBlock {
            index: first + column - 1,
            block: &block,
            text: &cell.text.text,
            origin: [
                device(left, scale) + content[0],
                device(top, scale) + content[1],
            ],
            scale,
        };
        text.hit(at, backend)
    }
}

fn table_element(
    grid: Grid,
    placed: &Placed,
    rect: [f64; 4],
    appearance: Appearance,
    selected: Option<&Selected>,
    backend: &mut dyn TextBackend,
) -> AnyElement {
    let columns = MetadataText::new(placed, 0).columns;
    let theme = appearance.theme();
    let [x, y, _, height] = rect;
    let px = |lu| css(lu, placed.scale);
    let border = px(placed.border);
    let line = appearance.pick(0x222222, 0xeeeeee);
    let mut layer = div().absolute().inset_0();
    for (r, row) in placed.rows.iter().enumerate() {
        // A parity capture also records the text below the tab, as the DOM dump lists it.
        if px(row.top) > height && !cfg!(feature = "parity") {
            break;
        }
        for (c, cell) in row.cells.iter().enumerate() {
            let bounds = [
                x + px(cell.left),
                y + px(row.top),
                px(cell.width),
                px(row.height),
            ];
            let [left, top, w, h] = bounds;
            match cell.kind {
                CellKind::Header => {
                    let down = grid.snap(top + h);
                    layer = layer
                        .child(grid.place(bounds).bg(rgb(theme.background)))
                        .child(grid.place([left, down - border, w, border]).bg(rgb(line)))
                        .child(probed("table.metadata-table th", grid.place(bounds)));
                }
                CellKind::Key => {
                    let right = grid.snap(left + w);
                    layer = layer.child(grid.place([right - border, top, border, h]).bg(rgb(line)));
                }
                CellKind::Value | CellKind::NoData => {}
            }
            let [dx, dy] = placed.content(row, cell).map(px);
            let style = cell_style(cell.kind);
            let mut over = Vec::new();
            if let Some(selected) = selected
                && r > 0
                && c > 0
            {
                let block = cell.text.block(&style);
                let content = placed.content(row, cell);
                let block = PlacedBlock {
                    index: (r - 1) * columns + c - 1,
                    block: &block,
                    text: &cell.text.text,
                    origin: [
                        device(x, grid.scale) + content[0],
                        device(y, grid.scale) + content[1],
                    ],
                    scale: grid.scale,
                };
                layer = selected.paint(grid, &block, backend, layer);
                let text = (&cell.text, &style);
                let at = [x + dx, y + dy];
                over = selected_lines(grid, text, at, selected, &block, backend);
            }
            layer = text_lines(
                grid,
                &cell.text,
                &style,
                [x + dx, y + dy],
                theme.text,
                layer,
            )
            .children(over);
        }
    }
    let container = [x, y, rect[2], px(placed.height)];
    let scrolls = px(placed.height) > height;
    let layer = layer.child(probed(".metadata-table-container", grid.place(container)));
    opaque_background(scrolls, layer).into_any_element()
}

impl<H: FrameHost> HubFrame<H> {
    /// The displayed metadata tab's text that can be selected.
    pub(super) fn metadata_text<'a>(
        &'a self,
        cx: &'a App,
    ) -> Option<(Owner, Box<dyn PageText + 'a>)> {
        let renderer = displayed(self.host().hub(cx)).filter(|r| r.table().show_table)?;
        let page = MetadataPage {
            doc: MetadataText::new(self.metadata.layout.placed(), renderer.builds()),
            rect: self.metadata.rect,
        };
        Some((Owner::Metadata, Box::new(page)))
    }

    /// The displayed metadata tab's renderer element over `rect` (`div.renderer-content`).
    pub(super) fn metadata_element(
        &mut self,
        rect: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let hub = self.host().hub(cx);
        let live = (hub.tabs().tabs()).any(|tab| tab.tab_type() == Some(TabType::Metadata));
        self.text_retain(|owner| live || *owner != Owner::Metadata);
        let hub = self.host().hub(cx);
        let satellite = hub.page().is_satellite();
        let renderer = displayed(hub)?;
        let (table, builds) = (renderer.table().clone(), renderer.builds());
        self.metadata.rect = rect;
        let grid = Grid {
            origin: [rect[0], rect[1]],
            scale: f64::from(scale),
        };
        let layout = &mut self.metadata.layout;
        if table.show_table {
            let placed = layout.place(&table, rect[2], scale);
            let doc = MetadataText::new(placed, builds);
            let owner = Owner::Metadata;
            let selected = self.text.selected(&owner, &doc, window, cx);
            let backend = &mut **self.text.backend();
            return Some(table_element(
                grid,
                placed,
                rect,
                appearance,
                selected.as_ref(),
                backend,
            ));
        }
        let (alert, style) = if satellite {
            let alert = layout.satellite_alert(rect[2], scale);
            (alert, satellite_alert_style())
        } else {
            (layout.alert([rect[2], rect[3]], scale), alert_style())
        };
        let at = [rect[0] + alert.origin[0], rect[1] + alert.origin[1]];
        let [w, h] = alert.size.map(|v| css(v, scale));
        let color = appearance.theme().text;
        let layer = div().absolute().inset_0();
        Some(
            text_lines(grid, &alert.text, &style, at, color, layer)
                .child(probed("div.tab-centered", grid.place([at[0], at[1], w, h])))
                .into_any_element(),
        )
    }
}
