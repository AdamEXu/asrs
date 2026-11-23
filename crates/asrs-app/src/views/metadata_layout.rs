//! Layout of `table.metadata-table`, a fixed table as Blink lays it out. Upstream's
//! `height: 100%` on its container resolves to `auto`, so a table taller than the tab is cut off
//! and nothing scrolls.

use asrs_canvas::text::dom::Lu;
use asrs_canvas::text::{FixedAdvance, TextBackend, platform_backend};
use asrs_tabs::metadata::MetadataTable;
use asrs_ui::text::TextStyle;
use asrs_ui::theme::{FONT_FAMILY, MONOSPACE_FAMILY};

use super::cell_text::{CellText, border, length};

const KEY_WIDTH: f64 = 150.0;
const PADDING: f64 = 6.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    Header,
    Key,
    Value,
    NoData,
}

pub fn cell_style(kind: CellKind) -> TextStyle {
    let family = FONT_FAMILY.join(", ");
    let (font, align) = match kind {
        CellKind::Header => (format!("bold 16px {family}"), "left"),
        CellKind::Key => (format!("bold 16px {family}"), "right"),
        CellKind::Value => (format!("14px {MONOSPACE_FAMILY}"), "left"),
        CellKind::NoData => (format!("italic 14px {MONOSPACE_FAMILY}"), "left"),
    };
    TextStyle::from_css([
        ("font", font.as_str()),
        ("text-align", align),
        ("white-space", "normal"),
        ("overflow-wrap", "break-word"),
    ])
    .expect("metadata cell style")
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

/// The table's layout in device units. The first row is the header.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Placed {
    pub scale: f32,
    pub height: Lu,
    pub border: Lu,
    pub padding: Lu,
    pub rows: Vec<PlacedRow>,
}

impl Placed {
    /// Where a cell's content box starts, from the table's top left corner.
    pub fn content(&self, row: &PlacedRow, cell: &PlacedCell) -> [Lu; 2] {
        let bottom = if cell.kind == CellKind::Header {
            self.border
        } else {
            0
        };
        let free = row.height - 2 * self.padding - bottom - cell.text.height;
        [
            cell.left + self.padding,
            row.top + self.padding + free / 2,
        ]
    }
}

pub struct MetadataLayout {
    backend: Box<dyn TextBackend>,
    key: Option<(MetadataTable, f64, f32)>,
    placed: Placed,
}

impl Default for MetadataLayout {
    fn default() -> Self {
        MetadataLayout {
            backend: platform_backend().unwrap_or_else(|| Box::new(FixedAdvance)),
            key: None,
            placed: Placed::default(),
        }
    }
}

impl MetadataLayout {
    /// The layout of `table` in a renderer element `width` CSS px wide.
    pub fn place(&mut self, table: &MetadataTable, width: f64, scale: f32) -> &Placed {
        let known = self
            .key
            .as_ref()
            .is_some_and(|(t, w, s)| t == table && *w == width && *s == scale);
        if !known {
            self.placed = self.lay_out(table, width, scale);
            self.key = Some((table.clone(), width, scale));
        }
        &self.placed
    }

    fn lay_out(&mut self, table: &MetadataTable, width: f64, scale: f32) -> Placed {
        let border = border(scale);
        let padding = length(PADDING, scale);
        let table_width = (width as f32 * scale * 64.0).round() as Lu;
        let columns = column_lefts(table_width, table.columns.len(), padding, scale);
        let mut rows = Vec::with_capacity(1 + table.rows.len());
        let mut top = 0;
        let mut push = |texts: Vec<(CellKind, String)>, backend: &mut dyn TextBackend| {
            let cells: Vec<PlacedCell> = texts
                .into_iter()
                .enumerate()
                .map(|(i, (kind, text))| {
                    let (left, width) = (columns[i], columns[i + 1] - columns[i]);
                    let right = if kind == CellKind::Key { border } else { 0 };
                    let available = (width - 2 * padding - right).max(0);
                    let style = cell_style(kind);
                    PlacedCell {
                        kind,
                        left,
                        width,
                        text: CellText::new(backend, &style, &text, Some(available), scale),
                    }
                })
                .collect();
            let height = cells
                .iter()
                .map(|c| {
                    let bottom = if c.kind == CellKind::Header { border } else { 0 };
                    c.text.height + 2 * padding + bottom
                })
                .max()
                .unwrap_or(0);
            rows.push(PlacedRow { top, height, cells });
            top += height;
        };
        let mut header = vec![(CellKind::Header, String::new())];
        let titles = table.columns.iter().map(|c| c.title().to_owned());
        header.extend(titles.map(|title| (CellKind::Header, title)));
        push(header, self.backend.as_mut());
        for row in &table.rows {
            let mut texts = vec![(CellKind::Key, row.key.to_string_lossy())];
            texts.extend(row.cells.iter().map(|cell| {
                let kind = if cell.no_data {
                    CellKind::NoData
                } else {
                    CellKind::Value
                };
                (kind, cell.text.to_string_lossy())
            }));
            push(texts, self.backend.as_mut());
        }
        Placed {
            scale,
            height: top,
            border,
            padding,
            rows,
        }
    }
}

/// Each column's left edge and the last one's right edge. The key column's width comes from the
/// header cell, which has no right border; the value columns share the rest evenly.
fn column_lefts(table: Lu, values: usize, padding: Lu, scale: f32) -> Vec<Lu> {
    let key = length(KEY_WIDTH, scale) + 2 * padding;
    let mut lefts = vec![0];
    if values == 0 {
        lefts.push(table);
        return lefts;
    }
    let mut left = key;
    let rest = (table - key).max(0);
    let count = values as Lu;
    for i in 0..count {
        lefts.push(left);
        left += if i + 1 == count {
            rest - rest / count * (count - 1)
        } else {
            rest / count
        };
    }
    lefts.push(left);
    lefts
}
