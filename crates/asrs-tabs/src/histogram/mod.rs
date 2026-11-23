//! The chart half of `shared/renderers/StatisticsRenderer.ts`: a stacked Chart.js 4.4.0 bar
//! chart, ported for the one configuration upstream gives it.

mod animation;
mod chart;
mod color;
mod scale;
mod surface;
mod ticks;
mod tooltip;

use std::rc::Rc;

use asrs_canvas::{Call, Canvas2d};
use asrs_js::string::{JsError, JsString};
use asrs_log::colors::ensure_theme_contrast;

pub use chart::{Area, Env, EventKind, Work};
use chart::{Chart, Dataset};

const MAX_TITLE_LENGTH: usize = 20;

/// The parts of `StatisticsRendererCommand` the chart's data is made of.
#[derive(Clone, Debug, PartialEq)]
pub struct HistogramCommand {
    pub bins: Vec<f64>,
    pub step_size: f64,
    pub fields: Vec<HistogramField>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HistogramField {
    pub title: JsString,
    pub color: String,
    pub counts: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScaleLayout {
    /// Left, top, right, bottom, width and height.
    pub bounds: [f64; 6],
    pub range: [f64; 2],
    pub label_rotation: f64,
    pub ticks: Vec<(f64, String)>,
}

pub struct Histogram {
    chart: Chart,
    last_is_light: Option<bool>,
}

impl Histogram {
    /// The renderer's constructor, which runs before its elements join the document.
    pub fn new(device_pixel_ratio: f64) -> Histogram {
        Histogram {
            chart: Chart::new(device_pixel_ratio),
            last_is_light: None,
        }
    }

    /// The chart's container joined the document with this size.
    pub fn attach(&mut self, container: (f64, f64), env: &mut Env<'_>) -> Result<(), JsError> {
        self.chart.attach(container, env)
    }

    pub fn is_attached(&self) -> bool {
        self.chart.is_attached()
    }

    /// The container's content box changed; the chart follows in the next frame.
    pub fn container_resized(&mut self, container: (f64, f64)) {
        self.chart.container_resized(container);
    }

    /// The window resized, possibly with another `devicePixelRatio`.
    pub fn window_resized(&mut self, device_pixel_ratio: f64) {
        self.chart.window_resized(device_pixel_ratio);
    }

    /// A mouse event at `offsetX`, `offsetY` of the canvas; handled in the next frame.
    pub fn pointer(&mut self, kind: EventKind, x: f64, y: f64) {
        self.chart.pointer(kind, x, y);
    }

    pub fn callbacks(&self) -> usize {
        self.chart.callbacks()
    }

    pub fn run_callback(&mut self, env: &mut Env<'_>) -> Option<(Work, Result<(), JsError>)> {
        self.chart.run_callback(env)
    }

    /// The colours for a theme, when it is not the one set last.
    pub fn set_theme(&mut self, dark: bool, env: &mut Env<'_>) -> Result<(), JsError> {
        let light = !dark;
        if self.last_is_light == Some(light) {
            return Ok(());
        }
        self.last_is_light = Some(light);
        let (text, grid) = if light {
            ("#222", "#eee")
        } else {
            ("#eee", "#333")
        };
        self.chart.legend_color = text.to_owned();
        self.chart.scale_colors.ticks = text.to_owned();
        self.chart.scale_colors.border = text.to_owned();
        self.chart.scale_colors.grid = grid.to_owned();
        self.chart.update(env)
    }

    /// The data of a new command.
    pub fn set_data(
        &mut self,
        command: &HistogramCommand,
        dark: bool,
        env: &mut Env<'_>,
    ) -> Result<(), JsError> {
        let half = command.step_size / 2.0;
        self.chart.labels = command.bins.iter().map(|value| value + half).collect();
        self.chart.datasets = command
            .fields
            .iter()
            .map(|field| {
                Rc::new(Dataset {
                    label: label(&field.title),
                    data: field.counts.clone(),
                    background: ensure_theme_contrast(&field.color, dark),
                })
            })
            .collect();
        self.chart.step_size = command.step_size;
        self.chart.update(env)
    }

    /// The canvas's display list since its last full clear.
    pub fn canvas(&self) -> &Canvas2d {
        self.chart.surface.canvas()
    }

    pub fn size(&self) -> (f64, f64) {
        (self.chart.width, self.chart.height)
    }

    pub fn device_pixel_ratio(&self) -> f64 {
        self.chart.device_pixel_ratio.unwrap_or(f64::NAN)
    }

    pub fn area(&self) -> Option<Area> {
        self.chart.area
    }

    pub fn scales(&self) -> [ScaleLayout; 2] {
        [&self.chart.x, &self.chart.y].map(|s| ScaleLayout {
            bounds: [s.left, s.top, s.right, s.bottom, s.width, s.height],
            range: [s.min, s.max],
            label_rotation: s.label_rotation,
            ticks: s.ticks.iter().map(|t| (t.value, t.label.clone())).collect(),
        })
    }

    /// The legend's left, top, right and bottom.
    pub fn legend(&self) -> [f64; 4] {
        self.chart.legend_box()
    }

    /// Every dataset's bars as `[x, y, base, width, height]`.
    pub fn bars(&self) -> Vec<Vec<[f64; 5]>> {
        let bars = |meta| {
            self.chart
                .bars(meta)
                .iter()
                .map(|bar| {
                    let b = bar.borrow();
                    [b.x, b.y, b.base, b.width, b.height]
                })
                .collect()
        };
        self.chart.metas().iter().map(|m| bars(m)).collect()
    }

    pub fn record_calls(&mut self) {
        self.chart.surface.record_calls();
    }

    /// Whether the canvas was reset since the last take, and the calls since then.
    pub fn take_calls(&mut self) -> (bool, Vec<Call>) {
        self.chart.surface.take_calls()
    }
}

fn label(title: &JsString) -> JsString {
    let units = title.units();
    if units.len() <= MAX_TITLE_LENGTH {
        return title.clone();
    }
    let mut label = JsString::from("...");
    label.push_str(&JsString::from_units(
        units[units.len() - MAX_TITLE_LENGTH..].to_vec(),
    ));
    label
}
