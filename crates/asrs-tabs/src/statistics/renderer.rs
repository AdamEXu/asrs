//! `src/shared/renderers/StatisticsRenderer.ts`: the values table beside the histogram.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_canvas::text::TextBackend;
use asrs_js::string::JsError;
use asrs_js::time::Clock;
use asrs_js::value::JsValue;
use asrs_model::tab_controller::{TabCommand, TabRenderer};

use super::values::{StatisticsRow, StatisticsTable};
use super::{StatisticsCommand, StatisticsCx};
use crate::histogram::{Env, Histogram, HistogramCommand, HistogramField, Work};

const HISTOGRAM_MARGIN: f64 = 10.0;

/// The layout `render` reads.
pub trait StatisticsGeometry {
    /// `VALUES_TABLE_CONTAINER.offsetWidth` with `rows` in the table.
    fn values_offset_width(&mut self, rows: &[StatisticsRow]) -> f64;
}

impl From<&StatisticsCommand> for HistogramCommand {
    fn from(command: &StatisticsCommand) -> HistogramCommand {
        HistogramCommand {
            bins: command.bins.clone(),
            step_size: command.step_size,
            fields: command
                .fields
                .iter()
                .map(|field| HistogramField {
                    title: field.title.clone(),
                    color: match &field.color {
                        JsValue::String(color) => color.to_string_lossy(),
                        _ => String::new(),
                    },
                    counts: field.histogram_counts.clone(),
                })
                .collect(),
        }
    }
}

pub struct StatisticsRenderer {
    clock: Rc<dyn Clock>,
    text: Rc<RefCell<dyn TextBackend>>,
    geometry: Option<Box<dyn StatisticsGeometry>>,
    table: StatisticsTable,
    histogram: Option<Histogram>,
    histogram_left: Option<f64>,
    /// How many of the chart's animation frame callbacks the next frame runs before the hub's
    /// own, and how many this frame still has to run after it, once it rendered.
    first: usize,
    rest: Option<usize>,
    errors: Vec<JsError>,
    rendered_in_frames: bool,
    /// Whether the last frame changed the table or the chart.
    changed: bool,
}

impl StatisticsRenderer {
    /// `new StatisticsRenderer(root)`.
    pub fn new(clock: Rc<dyn Clock>, text: Rc<RefCell<dyn TextBackend>>) -> Self {
        StatisticsRenderer {
            clock,
            text,
            geometry: None,
            table: StatisticsTable::new(),
            histogram: None,
            histogram_left: None,
            first: 0,
            rest: None,
            errors: Vec::new(),
            rendered_in_frames: true,
            changed: false,
        }
    }

    /// Whether `render` is called in an animation frame, as the hub does, or by a message, as in
    /// a satellite.
    pub fn set_rendered_in_frames(&mut self, in_frames: bool) {
        self.rendered_in_frames = in_frames;
    }

    pub fn table(&self) -> &StatisticsTable {
        &self.table
    }

    pub fn histogram(&self) -> Option<&Histogram> {
        self.histogram.as_ref()
    }

    pub fn histogram_mut(&mut self) -> Option<&mut Histogram> {
        self.histogram.as_mut()
    }

    /// `HISTOGRAM_CONTAINER.style.left`, in px.
    pub fn histogram_left(&self) -> Option<f64> {
        self.histogram_left
    }

    /// Gives the renderer its layout and the chart its canvas. Upstream's constructor makes the
    /// chart; here nothing is drawn before the window's `devicePixelRatio` is known.
    pub fn mount(&mut self, device_pixel_ratio: f64, geometry: Box<dyn StatisticsGeometry>) {
        self.geometry = Some(geometry);
        self.histogram
            .get_or_insert_with(|| Histogram::new(device_pixel_ratio));
    }

    fn with_chart<R>(&mut self, f: impl FnOnce(&mut Histogram, &mut Env<'_>) -> R) -> Option<R> {
        let histogram = self.histogram.as_mut()?;
        let mut text = self.text.borrow_mut();
        let mut env = Env {
            text: &mut *text,
            now: self.clock.now(),
        };
        Some(f(histogram, &mut env))
    }

    /// The chart's container joined the document with this size.
    pub fn attach(&mut self, container: (f64, f64)) -> Result<(), JsError> {
        self.with_chart(|chart, env| chart.attach(container, env))
            .unwrap_or(Ok(()))
    }

    /// Runs the next of the chart's callbacks.
    pub fn run_callback(&mut self) -> Option<(Work, Result<(), JsError>)> {
        self.with_chart(|chart, env| chart.run_callback(env))?
    }

    fn run_callbacks(&mut self, count: usize) {
        for _ in 0..count {
            if let Some((_, Err(error))) = self.run_callback() {
                self.errors.push(error);
            }
        }
    }

    fn callbacks(&self) -> usize {
        self.histogram.as_ref().map_or(0, Histogram::callbacks)
    }

    /// What tells one drawing of the table and the chart from another.
    fn drawing(&self) -> (u64, Option<u64>, Option<f64>) {
        let chart = self.histogram.as_ref().map(Histogram::revision);
        (self.table.builds(), chart, self.histogram_left)
    }

    /// `render(command)` in the theme `dark`.
    pub fn render_command(
        &mut self,
        command: &StatisticsCommand,
        dark: bool,
    ) -> Result<(), JsError> {
        if let Some(geometry) = &mut self.geometry {
            let width = geometry.values_offset_width(self.table.rows());
            self.histogram_left = Some(width + HISTOGRAM_MARGIN);
        }
        self.with_chart(|chart, env| chart.set_theme(dark, env))
            .unwrap_or(Ok(()))?;
        if self.table.render(command, dark)? {
            let command = HistogramCommand::from(command);
            self.with_chart(|chart, env| chart.set_data(&command, dark, env))
                .unwrap_or(Ok(()))?;
        }
        Ok(())
    }

    /// The frame in which the hub renders the tab: the chart's callbacks requested before the
    /// hub's own, then `render`.
    pub fn frame(&mut self, command: &StatisticsCommand, dark: bool) -> Result<(), JsError> {
        if !self.histogram().is_some_and(Histogram::is_attached) {
            return Ok(());
        }
        let rest = self.callbacks() - self.first;
        let first = std::mem::take(&mut self.first);
        self.run_callbacks(first);
        let result = self.render_command(command, dark);
        self.first = self.callbacks() - rest;
        self.rest = Some(rest);
        result
    }

    /// The chart's callbacks behind the hub's, or all of them when the tab was not rendered.
    pub fn frame_rest(&mut self) {
        match self.rest.take() {
            Some(rest) => self.run_callbacks(rest),
            None => {
                self.first = 0;
                self.run_callbacks(self.callbacks());
            }
        }
    }
}

impl<Cx: StatisticsCx + ?Sized> TabRenderer<Cx> for StatisticsRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut Cx, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        None
    }

    fn render(&mut self, cx: &mut Cx, command: &dyn TabCommand) -> Result<(), JsError> {
        let Some(command) = command.downcast_ref::<StatisticsCommand>() else {
            return Err(JsError::type_error("not a statistics command"));
        };
        let dark = cx.tab_dark();
        let before = self.drawing();
        let result = if self.rendered_in_frames {
            self.frame(command, dark)
        } else if !self.histogram().is_some_and(Histogram::is_attached) {
            Ok(())
        } else {
            self.render_command(command, dark)
        };
        self.changed = self.drawing() != before;
        result
    }

    fn animation_frame(&mut self, _cx: &mut Cx) -> Vec<JsError> {
        let before = self.drawing();
        self.frame_rest();
        self.changed |= self.drawing() != before;
        std::mem::take(&mut self.errors)
    }

    fn needs_draw(&self) -> bool {
        self.callbacks() > 0 || self.changed
    }
}
