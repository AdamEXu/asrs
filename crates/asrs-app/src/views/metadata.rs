//! The metadata tab's renderer element (`#renderer12` in `www/hub.html`, styled by
//! `www/renderers.css`), drawn from R11's `MetadataTable` over the boxes of [`MetadataLayout`].

use asrs_hub::hub::HubModel;
use asrs_model::tab_type::TabType;
use asrs_tabs::metadata::{MetadataRenderer, MetadataTable};
use asrs_ui::gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, div, rgb};
use asrs_ui::parity::probed;
use asrs_ui::theme::Appearance;

use super::cell_text::css;
use super::frame::{FrameHost, HubFrame};
use super::large_table::Grid;
use super::metadata_layout::{CellKind, MetadataLayout, Placed, alert_style, cell_style};
use super::statistics::text_lines;

#[derive(Default)]
pub struct MetadataView {
    layout: MetadataLayout,
}

/// The displayed tab's table when it is a metadata tab. The hub has at most one.
fn displayed(hub: &HubModel) -> Option<&MetadataTable> {
    hub.tabs().tabs().find_map(|tab| {
        if tab.content_hidden() || tab.tab_type() != Some(TabType::Metadata) {
            return None;
        }
        let renderer = tab.renderer().downcast_ref::<MetadataRenderer>()?;
        Some(renderer.table())
    })
}

fn table_element(
    grid: Grid,
    placed: &Placed,
    rect: [f64; 4],
    appearance: Appearance,
) -> AnyElement {
    let theme = appearance.theme();
    let [x, y, _, height] = rect;
    let px = |lu| css(lu, placed.scale);
    let border = px(placed.border);
    let line = appearance.pick(0x222222, 0xeeeeee);
    let mut layer = div().absolute().inset_0();
    for row in &placed.rows {
        // A parity capture also records the text below the tab, as the DOM dump lists it.
        if px(row.top) > height && !cfg!(feature = "parity") {
            break;
        }
        for cell in &row.cells {
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
            layer = text_lines(
                grid,
                &cell.text,
                &style,
                [x + dx, y + dy],
                theme.text,
                layer,
            );
        }
    }
    let container = [x, y, rect[2], px(placed.height)];
    layer
        .child(probed(".metadata-table-container", grid.place(container)))
        .into_any_element()
}

impl<H: FrameHost> HubFrame<H> {
    /// The displayed metadata tab's renderer element over `rect` (`div.renderer-content`).
    pub(super) fn metadata_element(
        &mut self,
        rect: [f64; 4],
        appearance: Appearance,
        scale: f32,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let table = displayed(self.host().hub(cx))?.clone();
        let grid = Grid {
            origin: [rect[0], rect[1]],
            scale: f64::from(scale),
        };
        let layout = &mut self.metadata.layout;
        if table.show_table {
            let placed = layout.place(&table, rect[2], scale);
            return Some(table_element(grid, placed, rect, appearance));
        }
        let alert = layout.alert([rect[2], rect[3]], scale);
        let at = [rect[0] + alert.origin[0], rect[1] + alert.origin[1]];
        let [w, h] = alert.size.map(|v| css(v, scale));
        let color = appearance.theme().text;
        let layer = div().absolute().inset_0();
        Some(
            text_lines(grid, &alert.text, &alert_style(), at, color, layer)
                .child(probed("div.tab-centered", grid.place([at[0], at[1], w, h])))
                .into_any_element(),
        )
    }
}
