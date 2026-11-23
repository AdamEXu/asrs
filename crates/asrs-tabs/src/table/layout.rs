//! Blink's layout of `div.data-table-container` under `www/renderers.css`, in device px: row
//! heights are fixed; the view supplies the container's box and the column widths, which follow
//! the text.

use asrs_js::math;
use asrs_model::source_list::Rect;

use super::{TableGeometry, TableRows};

/// `th { height: 30px }` and `td { height: 16px; padding: 4px }`, without their bottom border.
const HEADER_CONTENT: f64 = 30.0;
const ROW_CONTENT: f64 = 24.0;
/// `div.data-table-key-container.has-close-button`'s `left` and `right`.
const KEY_INSET: f64 = 6.0 + 30.0;

#[derive(Clone, Debug)]
pub struct TableLayout {
    /// `TABLE_CONTAINER`'s box, which has no borders. Scrollbars overlay its content.
    pub container: Rect,
    pub device_pixel_ratio: f64,
    /// The width of every column in CSS px, the jump input's first.
    pub columns: Vec<f64>,
    /// `offsetWidth` of every field's key text.
    pub key_text: Vec<f64>,
    rows: TableRows,
    /// The scroll offset in device px, which wheel input leaves between device pixels.
    scroll: [f64; 2],
}

impl Default for TableLayout {
    fn default() -> Self {
        TableLayout {
            container: Rect::default(),
            device_pixel_ratio: 1.0,
            columns: Vec::new(),
            key_text: Vec::new(),
            rows: TableRows::default(),
            scroll: [0.0, 0.0],
        }
    }
}

impl TableLayout {
    /// A 1 px border, floored to whole device pixels.
    fn border(&self) -> f64 {
        math::max2(math::floor(self.device_pixel_ratio), 1.0)
    }

    /// `AdjustForAbsoluteZoom`: Blink multiplies by the inverse zoom in single precision.
    fn css(&self, device: f64) -> f64 {
        f64::from(device as f32 * (1.0 / self.device_pixel_ratio) as f32)
    }

    fn lu(&self, css: f64) -> f64 {
        math::floor(css * self.device_pixel_ratio * 64.0) / 64.0
    }

    fn header_device(&self) -> f64 {
        self.lu(HEADER_CONTENT) + self.border()
    }

    fn row_device(&self) -> f64 {
        self.lu(ROW_CONTENT) + self.border()
    }

    pub fn border_px(&self) -> f64 {
        self.border() / self.device_pixel_ratio
    }

    pub fn header_height(&self) -> f64 {
        self.css(self.header_device())
    }

    pub fn row_height(&self) -> f64 {
        self.css(self.row_device())
    }

    pub fn table_width(&self) -> f64 {
        self.columns.iter().sum()
    }

    fn rows_device(&self, count: usize) -> f64 {
        self.lu(self.rows.filler_upper_px) + count as f64 * self.row_device()
    }

    fn table_device(&self) -> [f64; 2] {
        [
            self.table_width() * self.device_pixel_ratio,
            self.header_device()
                + self.rows_device(self.rows.shown)
                + self.lu(self.rows.filler_lower_px),
        ]
    }

    fn scroll_range_device(&self) -> [f64; 2] {
        let content = self.table_device();
        let size = [self.container.width, self.container.height];
        [0, 1].map(|axis| math::max2(content[axis] - size[axis] * self.device_pixel_ratio, 0.0))
    }

    fn set_offset_device(&mut self, offset: [f64; 2]) {
        let range = self.scroll_range_device();
        self.scroll = [0, 1].map(|axis| math::min2(math::max2(offset[axis], 0.0), range[axis]));
    }

    /// Assigns `scrollLeft` and `scrollTop`, which land on whole device pixels.
    pub fn scroll_to(&mut self, offset: [f64; 2]) {
        self.set_offset_device(offset.map(|v| v * self.device_pixel_ratio));
        self.scroll = self.scroll.map(math::round);
    }

    /// The scroll offset wheel input moves, in CSS px, not on the pixel grid.
    pub fn wheel_offset(&mut self) -> [f64; 2] {
        self.set_offset_device(self.scroll);
        self.scroll.map(|v| v / self.device_pixel_ratio)
    }

    pub fn set_wheel_offset(&mut self, offset: [f64; 2]) {
        self.set_offset_device(offset.map(|v| v * self.device_pixel_ratio));
    }

    fn scroll_device(&mut self) -> [f64; 2] {
        self.set_offset_device(self.scroll);
        self.scroll.map(math::round)
    }

    /// `scrollLeft` and `scrollTop`, which layout clamps to the content.
    pub fn scroll(&mut self) -> [f64; 2] {
        let device = self.scroll_device();
        device.map(|v| self.css(v))
    }

    fn row_top_device(&mut self, index: usize) -> f64 {
        let top = self.scroll_device()[1];
        self.container.y * self.device_pixel_ratio + self.header_device() + self.rows_device(index)
            - top
    }

    /// The y of `dataRows[index]` in client coordinates.
    pub fn row_top(&mut self, index: usize) -> f64 {
        let device = self.row_top_device(index);
        self.css(device)
    }

    fn column_left_device(&mut self, index: usize) -> f64 {
        let x = self.container.x * self.device_pixel_ratio;
        if index == 0 {
            return x;
        }
        let left = self.scroll_device()[0];
        let before: f64 = self.columns.iter().take(index).sum();
        x + before * self.device_pixel_ratio - left
    }

    /// The x of column `index`: the first is sticky, the others scroll under it.
    pub fn column_left(&mut self, index: usize) -> f64 {
        let device = self.column_left_device(index);
        self.css(device)
    }

    /// `table.data-table`'s box.
    pub fn table_rect(&mut self) -> Rect {
        let [left, top] = self.scroll_device();
        let dpr = self.device_pixel_ratio;
        let [width, height] = self.table_device();
        Rect {
            x: self.css(self.container.x * dpr - left),
            y: self.css(self.container.y * dpr - top),
            width: self.css(width),
            height: self.css(height),
        }
    }

    /// The shown data row under client y `y`.
    pub fn row_at(&mut self, y: f64) -> Option<usize> {
        let top = self.row_top_device(0);
        let row = math::floor((y * self.device_pixel_ratio - top) / self.row_device());
        (row >= 0.0 && row < self.rows.shown as f64).then_some(row as usize)
    }

    /// Whether a client position is inside `TABLE_BODY` as the container clips it.
    pub fn body_contains(&mut self, x: f64, y: f64) -> bool {
        let inside = |r: Rect| x >= r.left() && x < r.right() && y >= r.top() && y < r.bottom();
        inside(self.container) && inside(self.table_rect())
    }
}

impl TableGeometry for TableLayout {
    fn scroll_top(&mut self) -> f64 {
        self.scroll()[1]
    }

    fn set_scroll_top(&mut self, value: f64) {
        let left = self.scroll()[0];
        self.scroll_to([left, value]);
    }

    fn container_client_height(&mut self) -> f64 {
        math::round(self.container.height)
    }

    fn header_client_height(&mut self) -> f64 {
        math::round(self.header_height())
    }

    /// The row scrolls with the body: only its cells are sticky.
    fn header_rect(&mut self) -> Rect {
        Rect {
            height: self.header_height(),
            ..self.table_rect()
        }
    }

    fn data_row_rect(&mut self, index: usize, hidden: bool) -> Rect {
        if hidden {
            return Rect::default();
        }
        let left = self.scroll_device()[0];
        Rect {
            x: self.css(self.container.x * self.device_pixel_ratio - left),
            y: self.row_top(index),
            width: self.table_width(),
            height: self.row_height(),
        }
    }

    fn container_rect(&mut self) -> Rect {
        self.container
    }

    fn header_cell_rect(&mut self, index: usize) -> Rect {
        Rect {
            x: self.column_left(index),
            y: self.container.y,
            width: self.columns.get(index).copied().unwrap_or_default(),
            height: self.header_height(),
        }
    }

    fn header_key_client_width(&mut self, index: usize) -> f64 {
        let border = if index + 1 == self.columns.len() {
            0.0
        } else {
            self.border_px()
        };
        let width = self.columns.get(index).copied().unwrap_or_default();
        math::round(math::max2(width - border - KEY_INSET, 0.0))
    }

    fn header_key_text_width(&mut self, index: usize) -> f64 {
        self.key_text
            .get(index.wrapping_sub(1))
            .copied()
            .unwrap_or_default()
    }

    fn rows_changed(&mut self, rows: TableRows) {
        self.rows = rows;
    }
}
