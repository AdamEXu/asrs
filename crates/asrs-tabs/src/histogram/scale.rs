// Ported from Chart.js 4.4.0 (MIT, Copyright (c) 2014-2022 Chart.js Contributors). See THIRD_PARTY_NOTICES.md.

//! `Scale` and `LinearScale` as the histogram's two axes use them: `x` at the bottom with a
//! `stepSize`, `y` at the left with `beginAtZero` and a grace.

use std::rc::Rc;

use asrs_canvas::Property;
use asrs_canvas::text::TextBackend;
use asrs_js::math::{self, max2, min2};
use asrs_js::string::JsString;

use super::chart::{Area, FONT, LINE_HEIGHT, Meta, ScaleColors};
use super::surface::Surface;
use super::ticks::{self, Tick, limit};

const TICK_LENGTH: f64 = 8.0;
const TICK_PADDING: f64 = 3.0;
const AUTO_SKIP_PADDING: f64 = 3.0;
const MAX_ROTATION: f64 = 50.0;
const Y_GRACE: f64 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Margins {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

/// What a scale reads from its chart.
pub struct ChartView<'a> {
    pub width: f64,
    pub height: f64,
    pub device_pixel_ratio: f64,
    pub metas: &'a [Rc<Meta>],
    pub colors: &'a ScaleColors,
    pub step_size: f64,
}

#[derive(Clone, Copy, Default)]
struct Size {
    width: f64,
    height: f64,
}

#[derive(Clone)]
struct LabelSizes {
    first: Size,
    last: Size,
    widest: Size,
    highest: Size,
}

#[derive(Clone, Copy)]
struct GridLine {
    tick: [f64; 4],
    line: [f64; 4],
}

struct Label {
    text: JsString,
    x: f64,
    y: f64,
    rotation: f64,
    offset: f64,
    align: &'static str,
}

pub struct Scale {
    axis: Axis,
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub width: f64,
    pub height: f64,
    pub min: f64,
    pub max: f64,
    pub label_rotation: f64,
    pub ticks: Vec<Tick>,
    max_width: f64,
    max_height: f64,
    margins: Margins,
    padding: Margins,
    range: (f64, f64),
    data_limits_cached: bool,
    length: f64,
    max_length: f64,
    start_pixel: f64,
    pub end_pixel: f64,
    start_value: f64,
    value_range: f64,
    label_sizes: Option<LabelSizes>,
    grid_lines: Option<Vec<GridLine>>,
    labels: Option<Vec<Label>>,
    border_value: f64,
}

impl Scale {
    pub fn new(axis: Axis) -> Scale {
        Scale {
            axis,
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            width: 0.0,
            height: 0.0,
            min: 0.0,
            max: 0.0,
            label_rotation: 0.0,
            ticks: Vec::new(),
            max_width: 0.0,
            max_height: 0.0,
            margins: Margins::default(),
            padding: Margins::default(),
            range: (0.0, 0.0),
            data_limits_cached: false,
            length: 0.0,
            max_length: 0.0,
            start_pixel: 0.0,
            end_pixel: 0.0,
            start_value: 0.0,
            value_range: 0.0,
            label_sizes: None,
            grid_lines: None,
            labels: None,
            border_value: 0.0,
        }
    }

    fn is_horizontal(&self) -> bool {
        self.axis == Axis::X
    }

    pub fn length(&self) -> f64 {
        self.length
    }

    /// `getPadding`, whose `|| 0` drops NaN.
    pub fn padding(&self) -> Margins {
        let or_zero = |v: f64| if v.is_nan() { 0.0 } else { v };
        Margins {
            left: or_zero(self.padding.left),
            right: or_zero(self.padding.right),
            top: or_zero(self.padding.top),
            bottom: or_zero(self.padding.bottom),
        }
    }

    pub fn before_layout(&mut self) {
        self.data_limits_cached = false;
    }

    pub fn update(
        &mut self,
        max_width: f64,
        max_height: f64,
        margins: Margins,
        chart: &ChartView<'_>,
        ctx: &mut Surface,
        text: &mut dyn TextBackend,
    ) {
        self.max_width = max_width;
        self.max_height = max_height;
        self.margins = margins;
        self.label_sizes = None;
        self.grid_lines = None;
        self.labels = None;
        if self.is_horizontal() {
            self.width = max_width;
            self.left = 0.0;
            self.right = max_width;
            self.max_length = self.width + margins.left + margins.right;
        } else {
            self.height = max_height;
            self.top = 0.0;
            self.bottom = max_height;
            self.max_length = self.height + margins.top + margins.bottom;
        }
        self.padding = Margins::default();
        if !self.data_limits_cached {
            self.determine_data_limits(chart.metas);
            self.range = match self.axis {
                Axis::X => (self.min + -0.0, self.max + 0.0),
                Axis::Y => (
                    if self.min == 0.0 {
                        0.0
                    } else {
                        self.min + -Y_GRACE
                    },
                    if self.max == 0.0 {
                        0.0
                    } else {
                        self.max + Y_GRACE
                    },
                ),
            };
            self.data_limits_cached = true;
        }
        self.build_ticks(chart.step_size);
        self.configure();
        self.calculate_label_rotation(chart, ctx, text);
        let limit = self.max_ticks(ctx, text);
        self.ticks = ticks::auto_skip(&self.ticks, limit);
        self.label_sizes = None;
        self.fit(chart, ctx, text);
    }

    fn determine_data_limits(&mut self, metas: &[Rc<Meta>]) {
        let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
        for (position, meta) in metas.iter().enumerate() {
            let (lo, hi) = match self.axis {
                Axis::X => meta.index_min_max(),
                Axis::Y => meta.value_min_max(&metas[..position]),
            };
            min = min2(min, lo);
            max = max2(max, hi);
        }
        let finite = |v: f64, or: f64| if v.is_finite() { v } else { or };
        self.min = finite(finite(min, finite(max, min)), 0.0);
        self.max = finite(finite(max, finite(min, max)), 1.0);
        self.handle_tick_range_options();
    }

    fn handle_tick_range_options(&mut self) {
        let begin_at_zero = self.axis == Axis::Y;
        if begin_at_zero {
            let (min_sign, max_sign) = (math::sign(self.min), math::sign(self.max));
            if min_sign < 0.0 && max_sign < 0.0 {
                self.max = 0.0;
            } else if min_sign > 0.0 && max_sign > 0.0 {
                self.min = 0.0;
            }
        }
        if self.min == self.max {
            let offset = if self.max == 0.0 {
                1.0
            } else {
                math::abs(self.max * 0.05)
            };
            self.max += offset;
            if !begin_at_zero {
                self.min -= offset;
            }
        }
    }

    fn tick_limit(&self, step_size: f64) -> f64 {
        if self.axis == Axis::X && step_size != 0.0 && !step_size.is_nan() {
            let ticks = math::ceil(self.max / step_size) - math::floor(self.min / step_size) + 1.0;
            return if ticks > 1000.0 { 1000.0 } else { ticks };
        }
        let length = if self.is_horizontal() {
            self.width
        } else {
            self.height
        };
        let rotation = 0.0;
        let ratio = if self.is_horizontal() {
            math::sin(rotation)
        } else {
            math::cos(rotation)
        };
        let ratio = if ratio == 0.0 { 0.001 } else { ratio };
        min2(11.0, math::ceil(length / min2(40.0, LINE_HEIGHT / ratio)))
    }

    fn build_ticks(&mut self, step_size: f64) {
        let max_ticks = max2(2.0, self.tick_limit(step_size));
        let step = if self.axis == Axis::X { step_size } else { 0.0 };
        let values = ticks::generate(step, max_ticks, self.range.0, self.range.1);
        for &value in values.iter().filter(|v| !v.is_nan()) {
            self.min = min2(self.min, value);
            self.max = max2(self.max, value);
        }
        self.ticks = values
            .iter()
            .map(|&value| Tick {
                value,
                label: ticks::format(value, &values),
            })
            .collect();
    }

    pub fn configure(&mut self) {
        (self.start_pixel, self.end_pixel) = if self.is_horizontal() {
            (self.left, self.right)
        } else {
            (self.top, self.bottom)
        };
        self.length = self.end_pixel - self.start_pixel;
        self.start_value = self.min;
        self.value_range = self.max - self.min;
    }

    fn label_sizes(&mut self, ctx: &mut Surface, text: &mut dyn TextBackend) -> LabelSizes {
        if let Some(sizes) = &self.label_sizes {
            return sizes.clone();
        }
        let mut widths = Vec::with_capacity(self.ticks.len());
        for tick in &self.ticks {
            ctx.font(FONT);
            widths.push(ctx.measure(text, &JsString::from(tick.label.as_str())));
        }
        let widest = widths.iter().fold(0.0, |widest, &w| max2(w, widest));
        let widest = widths.iter().position(|&w| w == widest);
        let at = |index: Option<usize>| {
            index
                .and_then(|i| widths.get(i))
                .map_or(Size::default(), |&width| Size {
                    width,
                    height: LINE_HEIGHT,
                })
        };
        let sizes = LabelSizes {
            first: at(Some(0)),
            last: at(widths.len().checked_sub(1)),
            widest: at(widest),
            highest: at(Some(0)),
        };
        self.label_sizes = Some(sizes.clone());
        sizes
    }

    fn calculate_label_rotation(
        &mut self,
        chart: &ChartView<'_>,
        ctx: &mut Surface,
        text: &mut dyn TextBackend,
    ) {
        self.label_rotation = 0.0;
        let count = self.ticks.len() as f64;
        if count <= 1.0 || !self.is_horizontal() {
            return;
        }
        let sizes = self.label_sizes(ctx, text);
        let (label_width, label_height) = (sizes.widest.width, sizes.highest.height);
        let max_width = limit(chart.width - label_width, 0.0, self.max_width);
        if label_width + 6.0 > max_width / (count - 1.0) {
            let tick_width = max_width / (count - 1.0);
            let max_height = self.max_height - TICK_LENGTH - TICK_PADDING;
            let diagonal = math::sqrt(label_width * label_width + label_height * label_height);
            let asin = |v: f64| math::asin(limit(v, -1.0, 1.0));
            let rotation = min2(
                asin((label_height + 6.0) / tick_width),
                asin(max_height / diagonal) - asin(label_height / diagonal),
            ) * (180.0 / math::PI);
            self.label_rotation = max2(0.0, min2(MAX_ROTATION, rotation));
        }
    }

    fn radians(&self) -> f64 {
        self.label_rotation * (math::PI / 180.0)
    }

    /// `determineMaxTicks`.
    fn max_ticks(&mut self, ctx: &mut Surface, text: &mut dyn TextBackend) -> f64 {
        let rotation = self.radians();
        let cos = math::abs(math::cos(rotation));
        let sin = math::abs(math::sin(rotation));
        let sizes = self.label_sizes(ctx, text);
        let w = sizes.widest.width + AUTO_SKIP_PADDING;
        let h = sizes.highest.height + AUTO_SKIP_PADDING;
        let tick_length = if self.is_horizontal() {
            if h * cos > w * sin { w / cos } else { h / sin }
        } else if h * sin < w * cos {
            h / cos
        } else {
            w / sin
        };
        math::floor(min2(
            self.length / tick_length + 1.0,
            self.max_length / tick_length,
        ))
    }

    fn fit(&mut self, chart: &ChartView<'_>, ctx: &mut Surface, text: &mut dyn TextBackend) {
        let mut min_size = if self.is_horizontal() {
            Size {
                width: self.max_width,
                height: TICK_LENGTH,
            }
        } else {
            Size {
                width: TICK_LENGTH,
                height: self.max_height,
            }
        };
        if !self.ticks.is_empty() {
            let sizes = self.label_sizes(ctx, text);
            let padding = TICK_PADDING * 2.0;
            let angle = self.radians();
            let (cos, sin) = (math::cos(angle), math::sin(angle));
            if self.is_horizontal() {
                let height = sin * sizes.widest.width + cos * sizes.highest.height;
                min_size.height = min2(self.max_height, min_size.height + height + padding);
            } else {
                let width = cos * sizes.widest.width + sin * sizes.highest.height;
                min_size.width = min2(self.max_width, min_size.width + width + padding);
            }
            self.calculate_padding(&sizes, sin, cos);
        }
        self.margins.left = max2(self.padding.left, self.margins.left);
        self.margins.top = max2(self.padding.top, self.margins.top);
        self.margins.right = max2(self.padding.right, self.margins.right);
        self.margins.bottom = max2(self.padding.bottom, self.margins.bottom);
        if self.is_horizontal() {
            self.length = chart.width - self.margins.left - self.margins.right;
            self.width = self.length;
            self.height = min_size.height;
        } else {
            self.width = min_size.width;
            self.length = chart.height - self.margins.top - self.margins.bottom;
            self.height = self.length;
        }
    }

    fn calculate_padding(&mut self, sizes: &LabelSizes, sin: f64, cos: f64) {
        if !self.is_horizontal() {
            self.padding.top = sizes.last.height / 2.0 + TICK_PADDING;
            self.padding.bottom = sizes.first.height / 2.0 + TICK_PADDING;
            return;
        }
        let offset_left = self.pixel_for_tick(0) - self.left;
        let offset_right = self.right - self.pixel_for_tick(self.ticks.len() - 1);
        let (left, right) = if self.label_rotation != 0.0 {
            (cos * sizes.first.width, sin * sizes.last.height)
        } else {
            (sizes.first.width / 2.0, sizes.last.width / 2.0)
        };
        let width = self.width;
        self.padding.left = max2(
            (left - offset_left + TICK_PADDING) * width / (width - offset_left),
            0.0,
        );
        self.padding.right = max2(
            (right - offset_right + TICK_PADDING) * width / (width - offset_right),
            0.0,
        );
    }

    pub fn pixel_for_tick(&self, index: usize) -> f64 {
        self.pixel_for_value(Some(self.ticks[index].value))
    }

    fn pixel_for_decimal(&self, decimal: f64) -> f64 {
        let decimal = if self.is_horizontal() {
            decimal
        } else {
            1.0 - decimal
        };
        limit(self.start_pixel + decimal * self.length, -32768.0, 32767.0)
    }

    pub fn pixel_for_value(&self, value: Option<f64>) -> f64 {
        value.map_or(f64::NAN, |v| {
            self.pixel_for_decimal((v - self.start_value) / self.value_range)
        })
    }

    /// `getLineWidthForValue`.
    pub fn line_width_for_value(&self, value: f64) -> f64 {
        if self.ticks.iter().any(|t| t.value == value) {
            1.0
        } else {
            0.0
        }
    }

    fn compute_grid_lines(&mut self, area: &Area, dpr: f64) -> Vec<GridLine> {
        let align = |pixel: f64| align_pixel(dpr, pixel, 1.0);
        let half = 1.0 / 2.0;
        let mut out = Vec::with_capacity(self.ticks.len());
        self.border_value = align(if self.is_horizontal() {
            self.top
        } else {
            self.right
        });
        for i in 0..self.ticks.len() {
            let at = align(self.pixel_for_tick(i));
            out.push(if self.is_horizontal() {
                GridLine {
                    tick: [at, self.border_value + half, at, self.top + TICK_LENGTH],
                    line: [at, area.top, at, align(area.bottom) - half],
                }
            } else {
                GridLine {
                    tick: [self.right - TICK_LENGTH, at, self.border_value - half, at],
                    line: [align(area.left) + half, at, area.right, at],
                }
            });
        }
        out
    }

    pub fn draw_grid(&mut self, area: &Area, chart: &ChartView<'_>, ctx: &mut Surface) {
        let lines = match self.grid_lines.take() {
            Some(lines) => lines,
            None => self.compute_grid_lines(area, chart.device_pixel_ratio),
        };
        for item in &lines {
            for ([x1, y1, x2, y2], offset) in [(item.line, Some(0.0)), (item.tick, None)] {
                ctx.save();
                ctx.line_width(1.0);
                ctx.stroke_style(&chart.colors.grid);
                ctx.line_dash();
                ctx.set(Property::LineDashOffset(offset));
                ctx.begin_path();
                ctx.move_to(x1, y1);
                ctx.line_to(x2, y2);
                ctx.stroke();
                ctx.restore();
            }
        }
        self.grid_lines = Some(lines);
    }

    pub fn draw_border(&self, chart: &ChartView<'_>, ctx: &mut Surface) {
        let align = |pixel: f64| align_pixel(chart.device_pixel_ratio, pixel, 1.0);
        let [x1, y1, x2, y2] = if self.is_horizontal() {
            [
                align(self.left) - 1.0 / 2.0,
                self.border_value,
                align(self.right) + 1.0 / 2.0,
                self.border_value,
            ]
        } else {
            [
                self.border_value,
                align(self.top) - 1.0 / 2.0,
                self.border_value,
                align(self.bottom) + 1.0 / 2.0,
            ]
        };
        ctx.save();
        ctx.line_width(1.0);
        ctx.stroke_style(&chart.colors.border);
        ctx.begin_path();
        ctx.move_to(x1, y1);
        ctx.line_to(x2, y2);
        ctx.stroke();
        ctx.restore();
    }

    fn compute_labels(&mut self, ctx: &mut Surface, text: &mut dyn TextBackend) -> Vec<Label> {
        let tick_and_padding = TICK_LENGTH + TICK_PADDING;
        let rotation = -self.radians();
        self.label_sizes(ctx, text);
        (0..self.ticks.len())
            .map(|i| {
                let label = JsString::from(self.ticks[i].label.as_str());
                let pixel = self.pixel_for_tick(i);
                if !self.is_horizontal() {
                    return Label {
                        text: label,
                        x: self.right - tick_and_padding,
                        y: pixel,
                        rotation,
                        offset: 0.0,
                        align: "right",
                    };
                }
                let mut x = pixel;
                if rotation != 0.0 {
                    x += LINE_HEIGHT / 2.0 * math::sin(rotation);
                }
                Label {
                    text: label,
                    x,
                    y: self.top + tick_and_padding,
                    rotation,
                    offset: LINE_HEIGHT / 2.0,
                    align: if rotation != 0.0 { "right" } else { "center" },
                }
            })
            .collect()
    }

    pub fn draw_labels(
        &mut self,
        chart: &ChartView<'_>,
        ctx: &mut Surface,
        text: &mut dyn TextBackend,
    ) {
        if self.is_horizontal() {
            ctx.clip_area(0.0, self.top, chart.width, self.bottom);
        } else {
            ctx.clip_area(self.left, 0.0, self.right, chart.height);
        }
        let labels = match self.labels.take() {
            Some(labels) => labels,
            None => self.compute_labels(ctx, text),
        };
        for label in &labels {
            ctx.save();
            ctx.font(FONT);
            ctx.translate(label.x, label.y);
            ctx.rotate(label.rotation);
            ctx.fill_style(&chart.colors.ticks);
            ctx.text_align(label.align);
            ctx.text_baseline("middle");
            ctx.fill_text(label.text.clone(), 0.0, label.offset);
            ctx.restore();
        }
        self.labels = Some(labels);
        ctx.restore();
    }
}

/// `_alignPixel`.
fn align_pixel(dpr: f64, pixel: f64, width: f64) -> f64 {
    let half = max2(width / 2.0, 0.5);
    math::round((pixel - half) * dpr) / dpr + half
}
