//! `src/shared/renderers/LineGraphRenderer.ts`: the graph painter, the `lastRenderState`
//! gate, the hovered time it derives from the cursor, the pointer and scroll listeners and the
//! help overlay. Of `ScrollSensor`, only the drag pan lives here, as in the timeline (R03);
//! wheel deltas arrive through [`LineGraphRenderer::scroll`] from the host's wheel mapping.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use asrs_canvas::Canvas2d;
use asrs_canvas::text::TextBackend;
use asrs_js::json;
use asrs_js::math;
use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::Clock;
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::colors::ensure_theme_contrast;
use asrs_log::line_graph_filter::LineGraphFilter;
use asrs_log::units::get_suffix_for_filter;
use asrs_log::util::{
    ValueScaler, calc_axis_step_size, clamp_value, clean_float, scale_value, shift_color,
};
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::{Preferences, Timestamps};
use asrs_model::selection::{Selection, SelectionMode};
use asrs_model::tab_controller::TabRenderer;

use super::command::{Axis, AxisUnits, DiscreteField, LineGraphCommand, NumericField};
use super::str_is;
use crate::cx::TabCx;

const Y_STEP_TARGET_PX: f64 = 50.0;
const X_STEP_TARGET_PX: f64 = 100.0;
const MAX_DECIMAL_VALUE: f64 = 1e9;
const FONT: &str = "12px ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";
const HELP_TIMER_MS: f64 = 1000.0;

/// What one `render` call reads from the DOM and `window`.
#[derive(Clone, Copy, Debug)]
pub struct LineGraphFrame<'a> {
    /// `CANVAS.clientWidth`.
    pub width: f64,
    /// `CANVAS.clientHeight`.
    pub height: f64,
    pub device_pixel_ratio: f64,
    /// `!matchMedia("(prefers-color-scheme: dark)").matches`.
    pub light: bool,
    /// `window.preferences`, `None` before `set-preferences` arrives.
    pub preferences: Option<&'a Preferences>,
}

/// Where a pointer event lands, relative to one renderer's elements.
#[derive(Clone, Copy, Debug)]
pub struct Pointer {
    pub client_x: f64,
    pub shift_key: bool,
    /// `ROOT.getBoundingClientRect().x`.
    pub root_x: f64,
    /// `SCROLL_OVERLAY.getBoundingClientRect().x`.
    pub overlay_x: f64,
    /// `SCROLL_OVERLAY.clientWidth`.
    pub overlay_client_width: f64,
    /// `ROOT.hidden`.
    pub root_hidden: bool,
}

/// The inline styles `render` and the scroll listener write. `None` is a style never set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayStyle {
    /// `SCROLL_OVERLAY.style.top`, `left` and `right` in px.
    pub scroll_top: Option<f64>,
    pub scroll_left: Option<f64>,
    pub scroll_right: Option<f64>,
    /// `HELP_OVERLAY.style.display`: `"flex"` when true, `"none"` when false.
    pub help_visible: Option<bool>,
    /// `HELP_OVERLAY.classList.contains("fading-out")`.
    pub help_fading_out: bool,
}

/// A `render` that threw: the canvas as far as it was painted, and the error.
#[derive(Clone, Debug)]
pub struct RenderError {
    pub canvas: Canvas2d,
    pub error: JsError,
}

/// The DOM inputs [`TabRenderer::render`] reads and the frame it leaves for the host.
#[derive(Debug, Default)]
pub struct LineGraphDom {
    pub width: f64,
    pub height: f64,
    pub device_pixel_ratio: f64,
    pub light: bool,
    /// The last painted frame, until the host takes it.
    pub painted: Option<Canvas2d>,
}

pub struct LineGraphRenderer {
    has_controller: bool,
    clock: Rc<dyn Clock>,
    is_fading_out: bool,
    zoom_in_accumulator: f64,
    last_render_state: String,
    mouse_down_x: f64,
    grab_zoom_active: bool,
    grab_zoom_start_time: f64,
    last_cursor_x: Option<f64>,
    last_hovered_time: Option<f64>,
    last_cursor_in_rect: bool,
    did_clear_hovered_time: bool,
    pan_active: bool,
    pan_last_cursor_x: f64,
    help_timers: VecDeque<f64>,
    style: OverlayStyle,
    dom: Rc<RefCell<LineGraphDom>>,
    text: Rc<RefCell<dyn TextBackend>>,
}

fn ink(light: bool) -> &'static str {
    if light { "#222" } else { "#eee" }
}

fn at(values: &[f64], i: isize) -> f64 {
    usize::try_from(i)
        .ok()
        .and_then(|i| values.get(i))
        .copied()
        .unwrap_or(f64::NAN)
}

/// `units.suffix`, or the TypeError reading it from `undefined`.
fn axis_value_text(value: f64, units: AxisUnits, filter: &JsValue) -> Result<String, JsError> {
    let value_clean = if math::abs(value) < MAX_DECIMAL_VALUE {
        clean_float(value)
    } else {
        math::round(value)
    };
    let mut output = number::to_string(value_clean);
    match units {
        AxisUnits::None => {}
        AxisUnits::Unknown => {
            return Err(JsError::new(
                "TypeError",
                "Cannot read properties of undefined (reading 'suffix')",
            ));
        }
        AxisUnits::Unit(config) => {
            let suffix = JsString::from(config.suffix);
            let suffix = match filter {
                JsValue::Number(n) if *n == 1.0 => {
                    get_suffix_for_filter(&suffix, LineGraphFilter::Differentiate)
                }
                JsValue::Number(n) if *n == 2.0 => {
                    get_suffix_for_filter(&suffix, LineGraphFilter::Integrate)
                }
                _ => suffix,
            };
            if suffix.len() > 1 {
                output.push(' ');
            }
            output.push_str(&suffix.to_string_lossy());
            let is_none = matches!(filter, JsValue::Number(n) if *n == 0.0);
            if is_none && config.pluralize_suffix && value != 1.0 {
                output.push('s');
            }
        }
    }
    Ok(output)
}

fn nullable(value: Option<f64>) -> JsValue {
    value.map_or(JsValue::Null, JsValue::Number)
}

fn numeric_state(fields: &[NumericField]) -> JsValue {
    JsValue::Array(
        fields
            .iter()
            .map(|f| {
                JsValue::Array(vec![
                    (f.values.len() as f64).to_js(),
                    f.color.to_js(),
                    f.kind.to_js(),
                    f.size.to_js(),
                ])
            })
            .collect(),
    )
}

fn discrete_state(fields: &[DiscreteField]) -> JsValue {
    JsValue::Array(
        fields
            .iter()
            .map(|f| {
                JsValue::Array(vec![
                    (f.values.len() as f64).to_js(),
                    f.color.to_js(),
                    f.kind.to_js(),
                    f.toggle_reference.to_js(),
                    if f.color_map.is_some() {
                        f.values.to_js()
                    } else {
                        JsValue::Null
                    },
                ])
            })
            .collect(),
    )
}

/// `field.colorMap[value]`; the map's own keys are the four robot modes.
fn mapped_color(field: &DiscreteField, value: Option<&String>) -> Option<&'static str> {
    let map = field.color_map.as_ref()?;
    match value?.as_str() {
        "Disabled" => Some(map.disabled),
        "Teleop" => Some(map.teleop),
        "Auto" => Some(map.auto),
        "Utility" => Some(map.utility),
        _ => None,
    }
}

impl LineGraphRenderer {
    /// `new LineGraphRenderer(root, hasController)`.
    pub fn new(
        has_controller: bool,
        clock: Rc<dyn Clock>,
        dom: Rc<RefCell<LineGraphDom>>,
        text: Rc<RefCell<dyn TextBackend>>,
    ) -> Self {
        LineGraphRenderer {
            has_controller,
            clock,
            is_fading_out: false,
            zoom_in_accumulator: 0.0,
            last_render_state: String::new(),
            mouse_down_x: 0.0,
            grab_zoom_active: false,
            grab_zoom_start_time: 0.0,
            last_cursor_x: None,
            last_hovered_time: None,
            last_cursor_in_rect: false,
            did_clear_hovered_time: false,
            pan_active: false,
            pan_last_cursor_x: 0.0,
            help_timers: VecDeque::new(),
            style: OverlayStyle::default(),
            dom,
            text,
        }
    }

    /// What the host writes before each `render` and takes the painted frame from.
    pub fn dom(&self) -> &Rc<RefCell<LineGraphDom>> {
        &self.dom
    }

    pub fn style(&self) -> &OverlayStyle {
        &self.style
    }

    /// `mousemove` on the overlay: its own listener, then its scroll sensor's.
    pub fn overlay_mouse_move(
        &mut self,
        pointer: Pointer,
        preferences: Option<&Preferences>,
        selection: &mut dyn Selection,
    ) {
        self.last_cursor_x = Some(pointer.client_x - pointer.root_x);
        self.last_cursor_in_rect = true;
        let cursor_x = pointer.client_x - pointer.overlay_x;
        if self.pan_active {
            let dx = self.pan_last_cursor_x - cursor_x;
            self.scroll(dx, 0.0, true, pointer, preferences, selection);
            self.pan_last_cursor_x = cursor_x;
        }
    }

    /// `mousemove` on `window`: both of the renderer's listeners there. Runs after the
    /// overlay's listeners when the event targets the overlay.
    pub fn window_mouse_move(&mut self, pointer: Pointer, selection: &mut dyn Selection) {
        if pointer.root_hidden {
            return;
        }
        if self.grab_zoom_active {
            self.last_cursor_x = Some(pointer.client_x - pointer.root_x);
            if let Some(hovered_time) = self.last_hovered_time {
                selection.set_grab_zoom_range(Some([self.grab_zoom_start_time, hovered_time]));
            }
        }
    }

    /// `mouseleave` on the overlay.
    pub fn mouse_leave(&mut self, selection: &mut dyn Selection) {
        self.last_cursor_x = None;
        self.last_cursor_in_rect = false;
        selection.set_hovered_time(None);
        self.pan_active = false;
    }

    /// `mousedown` on the overlay.
    pub fn mouse_down(&mut self, pointer: Pointer) {
        self.mouse_down_x = pointer.client_x - pointer.overlay_x;
        if let (true, Some(hovered_time)) = (pointer.shift_key, self.last_hovered_time) {
            self.grab_zoom_active = true;
            self.grab_zoom_start_time = hovered_time;
        }
        if !pointer.shift_key {
            self.pan_active = true;
            self.pan_last_cursor_x = pointer.client_x - pointer.overlay_x;
        }
    }

    /// `mouseup` on the overlay (its scroll sensor's listener).
    pub fn overlay_mouse_up(&mut self) {
        self.pan_active = false;
    }

    /// `mouseup` on `window`.
    pub fn window_mouse_up(&mut self, pointer: Pointer, selection: &mut dyn Selection) {
        if pointer.root_hidden {
            return;
        }
        if self.grab_zoom_active {
            selection.finish_grab_zoom();
            self.grab_zoom_active = false;
            if !self.last_cursor_in_rect {
                self.last_cursor_x = None;
                selection.set_hovered_time(None);
            }
        }
    }

    /// `click` on the overlay.
    pub fn click(&mut self, pointer: Pointer, selection: &mut dyn Selection) {
        if math::abs(pointer.client_x - pointer.overlay_x - self.mouse_down_x) <= 5.0
            && let Some(time) = self.last_hovered_time.filter(|t| *t != 0.0 && !t.is_nan())
        {
            selection.set_selected_time(time);
        }
    }

    /// `contextmenu` on the overlay.
    pub fn context_menu(&mut self, selection: &mut dyn Selection) {
        selection.go_idle();
    }

    /// The scroll sensor's callback: wheel deltas, or a drag pan.
    pub fn scroll(
        &mut self,
        dx: f64,
        dy: f64,
        is_pan: bool,
        pointer: Pointer,
        preferences: Option<&Preferences>,
        selection: &mut dyn Selection,
    ) {
        if pointer.root_hidden {
            return;
        }
        selection.apply_timeline_scroll(
            dx,
            if is_pan { 0.0 } else { dy },
            pointer.overlay_client_width,
        );
        if let Some(preferences) = preferences
            && !preferences.has_scrolled_line_graph
            && !self.is_fading_out
            && dy < 0.0
        {
            self.zoom_in_accumulator += dy;
            if self.zoom_in_accumulator <= -200.0 {
                self.is_fading_out = true;
                self.style.help_fading_out = true;
                self.help_timers.push_back(self.clock.now() + HELP_TIMER_MS);
            }
        }
    }

    /// The clock time the earliest pending help-overlay timer is due.
    pub fn next_timer(&self) -> Option<f64> {
        self.help_timers.front().copied()
    }

    /// Runs the help-overlay timers due by the clock's now and returns the
    /// `update-preferences` messages they send.
    pub fn run_timers(&mut self) -> Vec<NamedMessage<HubToMain>> {
        let now = self.clock.now();
        let mut out = Vec::new();
        while self.help_timers.front().is_some_and(|due| *due <= now) {
            self.help_timers.pop_front();
            let mut data = JsObject::new();
            data.insert("hasScrolledLineGraph", JsValue::Bool(true));
            out.push(NamedMessage::new(
                HubToMain::UpdatePreferences,
                JsValue::Object(data),
            ));
        }
        out
    }

    fn render_state(&self, frame: &LineGraphFrame<'_>, command: &LineGraphCommand) -> String {
        let state = JsValue::Array(vec![
            frame.width.to_js(),
            frame.height.to_js(),
            frame.light.to_js(),
            frame.device_pixel_ratio.to_js(),
            command.time_range.to_js(),
            command.selection_mode.to_js(),
            nullable(command.selected_time),
            nullable(command.hovered_time),
            command
                .grab_zoom_range
                .as_ref()
                .map_or(JsValue::Null, ToJs::to_js),
            command.left_range.to_js(),
            command.right_range.to_js(),
            command.show_left_axis.to_js(),
            command.show_right_axis.to_js(),
            command.left_units.to_js(),
            command.right_units.to_js(),
            command.priority_axis.name().to_js(),
            nullable(self.last_cursor_x),
            frame
                .preferences
                .map_or(JsValue::Undefined, |p| p.timestamps.to_js()),
            command.display_offset.to_js(),
            numeric_state(&command.left_fields),
            discrete_state(&command.discrete_fields),
            numeric_state(&command.right_fields),
            JsValue::Array(
                command
                    .alerts
                    .iter()
                    .map(|row| {
                        JsValue::Array(
                            row.iter()
                                .map(|a| {
                                    JsValue::Array(vec![
                                        a.kind.name().to_js(),
                                        a.text.to_js(),
                                        a.range.to_js(),
                                    ])
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            ),
        ]);
        json::stringify(&state).ok().flatten().unwrap_or_default()
    }

    /// `render(command)`. `Ok(None)` when the render state is unchanged and the canvas keeps
    /// its last frame.
    pub fn render_frame(
        &mut self,
        frame: &LineGraphFrame<'_>,
        command: &LineGraphCommand,
        selection: &mut dyn Selection,
        text: &mut dyn TextBackend,
    ) -> Result<Option<Canvas2d>, RenderError> {
        let has_scrolled = frame.preferences.is_some_and(|p| p.has_scrolled_line_graph);
        self.style.help_visible = Some(!has_scrolled);
        if has_scrolled {
            self.is_fading_out = false;
            self.zoom_in_accumulator = 0.0;
            self.style.help_fading_out = false;
        }

        let render_state = self.render_state(frame, command);
        if render_state == self.last_render_state {
            return Ok(None);
        }
        self.last_render_state = render_state;

        let dpr = frame.device_pixel_ratio;
        let width = frame.width;
        let height = frame.height;
        let mut ctx = Canvas2d::new(math::to_uint32(width * dpr), math::to_uint32(height * dpr));
        ctx.scale(dpr, dpr);
        ctx.clear_rect(0.0, 0.0, width, height);
        ctx.set_font(FONT);
        match self.paint(&mut ctx, frame, command, selection, text) {
            Ok(()) => Ok(Some(ctx)),
            Err(error) => Err(RenderError { canvas: ctx, error }),
        }
    }

    fn paint(
        &mut self,
        ctx: &mut Canvas2d,
        frame: &LineGraphFrame<'_>,
        command: &LineGraphCommand,
        selection: &mut dyn Selection,
        text: &mut dyn TextBackend,
    ) -> Result<(), JsError> {
        let time_range = command.time_range;
        let dpr = frame.device_pixel_ratio;
        let width = frame.width;
        let height = frame.height;
        let light = frame.light;

        let graph_top = if self.has_controller { 8.0 } else { 20.0 };
        self.style.scroll_top = Some(graph_top);
        let mut graph_height = height - graph_top - 35.0;
        if graph_height < 1.0 {
            graph_height = 1.0;
        }
        let discrete_row_count = (command.discrete_fields.len() + command.alerts.len()) as f64;
        let mut graph_height_open = graph_height
            - discrete_row_count * 20.0
            - if discrete_row_count > 0.0 { 5.0 } else { 0.0 };
        if graph_height_open < 1.0 {
            graph_height_open = 1.0;
        }

        let left_step_size =
            calc_axis_step_size(command.left_range, graph_height_open, Y_STEP_TARGET_PX);
        let right_step_size =
            calc_axis_step_size(command.right_range, graph_height_open, Y_STEP_TARGET_PX);

        let get_text_width = |ctx: &Canvas2d,
                              text: &mut dyn TextBackend,
                              range: [f64; 2],
                              step_size: f64,
                              units: AxisUnits,
                              filter: &JsValue|
         -> Result<f64, JsError> {
            let mut length: f64 = 0.0;
            let mut value = math::floor(range[1] / step_size) * step_size;
            while value > range[0] {
                let label = axis_value_text(value, units, filter)?;
                length = math::max2(length, ctx.measure_text(text, &label).width);
                value -= step_size;
            }
            Ok(math::ceil(length / 10.0) * 10.0)
        };
        let graph_left = 25.0
            + if command.show_left_axis {
                get_text_width(
                    ctx,
                    text,
                    command.left_range,
                    left_step_size,
                    command.left_units,
                    &command.left_filter,
                )?
            } else {
                0.0
            };
        let graph_right = 25.0
            + if command.show_right_axis {
                get_text_width(
                    ctx,
                    text,
                    command.right_range,
                    right_step_size,
                    command.right_units,
                    &command.right_filter,
                )?
            } else {
                0.0
            };
        let mut graph_width = width - graph_left - graph_right;
        if graph_width < 1.0 {
            graph_width = 1.0;
        }
        let x_range = [graph_left, graph_left + graph_width];

        let display_offset = command.display_offset;
        let display_time_range = [
            command.time_range[0] + display_offset,
            command.time_range[1] + display_offset,
        ];
        let max_time = math::max2(
            math::abs(display_time_range[0]),
            math::abs(display_time_range[1]),
        );
        let int_digits = if max_time > 0.0 {
            math::log10(max_time) + 1.0
        } else {
            1.0
        };
        let time_span = display_time_range[1] - display_time_range[0];
        let dec_digits = if time_span < 1.0 && time_span > 0.0 {
            -math::log10(time_span)
        } else {
            0.0
        };
        let extra_digits = math::max2(0.0, int_digits + dec_digits - 3.0);
        let time_step_size = calc_axis_step_size(
            display_time_range,
            graph_width,
            X_STEP_TARGET_PX + extra_digits * 12.0,
        );

        self.style.scroll_left = Some(graph_left);
        self.style.scroll_right = Some(graph_right);

        let mut discrete_borders: Vec<f64> = Vec::new();
        ctx.set_global_alpha(1.0);
        ctx.set_text_align("left");
        ctx.set_text_baseline("middle");
        ctx.set_line_width(1.0);
        ctx.set_line_cap("round");
        for (render_index, field) in command.discrete_fields.iter().enumerate() {
            ctx.begin_path();
            let mut toggle = field.toggle_reference;
            let mut skipped_samples = 0usize;
            discrete_borders.extend_from_slice(&field.timestamps);
            let count = field.timestamps.len();
            let mut i = 0usize;
            while i < count {
                i += skipped_samples;
                if i >= count {
                    break;
                }

                let start_x = scale_value(field.timestamps[i], time_range, x_range);
                let mut end_x;
                skipped_samples = 0;
                loop {
                    end_x = scale_value(
                        at(&field.timestamps, (i + skipped_samples + 1) as isize),
                        time_range,
                        x_range,
                    );
                    if end_x - start_x < 1.0 / dpr && i + skipped_samples + 1 < count {
                        skipped_samples += 1;
                        toggle = !toggle;
                    } else {
                        break;
                    }
                }
                if i + skipped_samples == count - 1 {
                    end_x = graph_left + graph_width;
                }
                if end_x > graph_left + graph_width {
                    end_x = graph_left + graph_width;
                }
                let top_y = graph_top + graph_height - 20.0 - render_index as f64 * 20.0;

                toggle = !toggle;
                let value = field.values.get(i + skipped_samples);
                let base_color = mapped_color(field, value).unwrap_or(field.color.as_str());
                if str_is(&field.kind, "stripes") {
                    if field.color_map.is_some() {
                        ctx.set_fill_style(base_color);
                    } else {
                        ctx.set_fill_style(
                            shift_color(
                                &JsString::from(base_color),
                                if toggle { -30.0 } else { 30.0 },
                            )
                            .as_str(),
                        );
                    }
                    ctx.fill_rect(start_x, top_y, end_x - start_x, 15.0);
                } else {
                    let start_y = if toggle { top_y + 15.0 } else { top_y };
                    let end_y = if toggle { top_y } else { top_y + 15.0 };
                    ctx.move_to(start_x, start_y);
                    ctx.line_to(start_x, end_y);
                    ctx.line_to(end_x, end_y);
                }

                let adjusted_start_x = if start_x < graph_left {
                    graph_left
                } else {
                    start_x
                };
                if end_x - adjusted_start_x > 10.0 {
                    if str_is(&field.kind, "stripes") {
                        let shift = if field.color_map.is_some() {
                            if light { -130.0 } else { 130.0 }
                        } else if toggle {
                            130.0
                        } else {
                            -130.0
                        };
                        ctx.set_fill_style(
                            shift_color(&JsString::from(base_color), shift).as_str(),
                        );
                    } else {
                        ctx.set_fill_style(base_color);
                    }
                    let label = value.map_or_else(|| JsString::from("undefined"), JsString::from);
                    ctx.fill_text_max_width(
                        label,
                        adjusted_start_x + 5.0,
                        top_y + 15.0 / 2.0,
                        end_x - adjusted_start_x - 10.0,
                    );
                }
                i += 1;
            }
            if str_is(&field.kind, "graph") {
                ctx.set_stroke_style(field.color.as_str());
                ctx.stroke();
            }
        }

        for (row_index, alerts_row) in command.alerts.iter().enumerate() {
            let top_y = graph_top + graph_height
                - 20.0
                - (command.discrete_fields.len() + row_index) as f64 * 20.0;
            for alert in alerts_row {
                let start_x = scale_value(alert.range[0], time_range, x_range);
                let end_x = scale_value(alert.range[1], time_range, x_range);

                let (base_color, text_color) = match alert.kind.name() {
                    "error" => (ensure_theme_contrast("#ff0000", !light), "#ffeeee"),
                    "warning" => (ensure_theme_contrast("#ffaa00", !light), "#331100"),
                    _ => (ensure_theme_contrast("#0000ff", !light), "#eeeeff"),
                };
                ctx.set_fill_style(base_color.as_str());
                ctx.fill_rect(start_x, top_y, end_x - start_x, 15.0);

                let adjusted_start_x = if start_x < graph_left {
                    graph_left
                } else {
                    start_x
                };
                if end_x - adjusted_start_x > 10.0 {
                    ctx.set_fill_style(text_color);
                    ctx.fill_text_max_width(
                        alert.text.clone(),
                        adjusted_start_x + 5.0,
                        top_y + 15.0 / 2.0,
                        end_x - adjusted_start_x - 10.0,
                    );
                }
            }
        }

        let x_scaler = ValueScaler::new(time_range, x_range);
        let draw_numeric_fields = |ctx: &mut Canvas2d,
                                   fields: &[NumericField],
                                   y_range: [f64; 2]| {
            let y_scaler = ValueScaler::new(y_range, [graph_top + graph_height_open, graph_top]);
            for field in fields {
                ctx.set_stroke_style(field.color.as_str());
                ctx.set_fill_style(field.color.as_str());
                ctx.set_line_cap("round");
                if str_is(&field.size, "normal") {
                    ctx.set_line_width(1.0);
                } else if str_is(&field.size, "bold") {
                    ctx.set_line_width(3.0);
                } else if str_is(&field.size, "verybold") {
                    ctx.set_line_width(6.0);
                }

                let timestamps = &field.timestamps;
                let values = &field.values;
                if str_is(&field.kind, "stepped") {
                    ctx.begin_path();
                    let last = values.len() as isize - 1;
                    ctx.move_to(
                        graph_left + graph_width,
                        y_scaler.calculate(at(values, last)),
                    );
                    let mut i = last;
                    loop {
                        let x = x_scaler.calculate(at(timestamps, i));

                        let value = at(values, i);
                        ctx.line_to(x, y_scaler.calculate(value));

                        let current_x = math::floor(x * dpr);
                        let mut vert_range = [value, value];
                        loop {
                            i -= 1;
                            let value = at(values, i);
                            if value < vert_range[0] {
                                vert_range[0] = value;
                            }
                            if value > vert_range[1] {
                                vert_range[1] = value;
                            }
                            let new_x = math::floor(x_scaler.calculate(at(timestamps, i)) * dpr);
                            if !(i >= 0 && new_x >= current_x) {
                                break;
                            }
                        }
                        if i < 0 {
                            break;
                        }

                        ctx.move_to(x, y_scaler.calculate(vert_range[0]));
                        ctx.line_to(x, y_scaler.calculate(vert_range[1]));

                        ctx.move_to(x, y_scaler.calculate(at(values, i)));
                    }
                    ctx.stroke();
                } else if str_is(&field.kind, "smooth") {
                    ctx.begin_path();
                    for (i, &t) in timestamps.iter().enumerate() {
                        let x = x_scaler.calculate(t);
                        let y = y_scaler.calculate(at(values, i as isize));
                        if i == 0 {
                            ctx.move_to(x, y);
                        } else {
                            ctx.line_to(x, y);
                        }
                    }
                    ctx.stroke();
                } else if str_is(&field.kind, "points") {
                    let delta_x = graph_width / math::max2(1.0, timestamps.len() as f64 - 1.0);
                    let radius = clamp_value(math::log(delta_x * 3.0), 1.0, 5.0);
                    ctx.begin_path();
                    for (i, &t) in timestamps.iter().enumerate() {
                        let x = x_scaler.calculate(t);
                        let y = y_scaler.calculate(at(values, i as isize));
                        ctx.move_to(x + radius, y);
                        ctx.arc(x, y, radius, 0.0, math::PI * 2.0);
                    }
                    ctx.fill();
                }
            }
        };
        if command.priority_axis == Axis::Left {
            draw_numeric_fields(ctx, &command.right_fields, command.right_range);
            draw_numeric_fields(ctx, &command.left_fields, command.left_range);
        } else {
            draw_numeric_fields(ctx, &command.left_fields, command.left_range);
            draw_numeric_fields(ctx, &command.right_fields, command.right_range);
        }

        let mut hovered_time = command.hovered_time;
        match self.last_cursor_x {
            Some(cursor_x) if !(cursor_x < graph_left || cursor_x > graph_left + graph_width) => {
                let mut time = scale_value(cursor_x, x_range, command.time_range);
                let nearest = discrete_borders
                    .iter()
                    .fold(f64::INFINITY, |prev, &border| {
                        if math::abs(time - border) < math::abs(time - prev) {
                            border
                        } else {
                            prev
                        }
                    });
                if nearest.is_finite() {
                    let nearest_x = scale_value(nearest, command.time_range, x_range);
                    if math::abs(nearest_x - cursor_x) < 5.0 {
                        time = nearest;
                    }
                }
                selection.set_hovered_time(Some(time));
                self.did_clear_hovered_time = false;
                hovered_time = Some(time);
            }
            _ => {
                if !self.did_clear_hovered_time {
                    selection.set_hovered_time(None);
                    self.did_clear_hovered_time = true;
                }
            }
        }

        if let Some(range) = command.grab_zoom_range {
            let start_x = scale_value(range[0], command.time_range, x_range);
            let end_x = scale_value(range[1], command.time_range, x_range);
            ctx.set_global_alpha(0.25);
            ctx.set_fill_style("yellow");
            ctx.fill_rect(start_x, graph_top, end_x - start_x, graph_height);
            ctx.set_global_alpha(1.0);
        }

        let is_start_at_0 = frame.preferences.map(|p| p.timestamps) != Some(Timestamps::Original);
        let format_marked_timestamp_text = |time: f64, is_delta: bool| -> Result<String, JsError> {
            let display_time = if is_delta {
                time
            } else {
                time + display_offset
            };
            let fraction_digits = math::max2(0.0, -math::floor(math::log10(time_step_size / 10.0)));
            let fraction_digits = if fraction_digits.is_nan() {
                0.0
            } else {
                fraction_digits
            };
            let mut text =
                number::to_fixed(display_time, fraction_digits).map_err(JsError::range)?;
            text.push('s');
            if is_start_at_0 && !is_delta {
                text.insert(0, '+');
            }
            Ok(text)
        };

        let write_centered_time = |ctx: &mut Canvas2d,
                                   text: &mut dyn TextBackend,
                                   label: &str,
                                   x: f64,
                                   alpha: f64,
                                   draw_rect: bool| {
            ctx.set_global_alpha(alpha);
            ctx.set_stroke_style(ink(light));
            ctx.set_fill_style(ink(light));
            let size = ctx.measure_text(text, label);
            let left = x - size.actual_bounding_box_left - 5.0;
            let w = size.width + 10.0;
            let h = size.actual_bounding_box_descent + 10.0;
            ctx.clear_rect(left, graph_top, w, h);
            if draw_rect {
                ctx.stroke_rect(left, graph_top, w, h);
            }
            ctx.fill_text(label, x, graph_top + 5.0);
            ctx.set_global_alpha(1.0);
        };

        let mark_time = |ctx: &mut Canvas2d, time: f64, alpha: f64| {
            if time >= time_range[0] && time <= time_range[1] {
                ctx.set_global_alpha(alpha);
                ctx.set_line_width(1.0);
                ctx.set_line_dash(&[5.0, 5.0]);
                ctx.set_stroke_style(ink(light));
                ctx.set_fill_style(ink(light));

                let x = scale_value(time, time_range, x_range);
                ctx.begin_path();
                ctx.move_to(x, graph_top);
                ctx.line_to(x, graph_top + graph_height);
                ctx.stroke();
                ctx.set_line_dash(&[]);
                ctx.set_global_alpha(1.0);
            }
        };

        ctx.set_text_baseline("top");
        ctx.set_text_align("center");
        let selected_time = command.selected_time;
        let selected_x = selected_time.map(|t| scale_value(t, time_range, x_range));
        let hovered_x = hovered_time.map(|t| scale_value(t, time_range, x_range));
        let selected_text = selected_time
            .map(|t| format_marked_timestamp_text(t, false))
            .transpose()?;
        let hovered_text = hovered_time
            .map(|t| format_marked_timestamp_text(t, false))
            .transpose()?;
        if let Some(t) = hovered_time {
            mark_time(ctx, t, 0.35);
        }
        if matches!(
            command.selection_mode,
            SelectionMode::Static | SelectionMode::Playback
        ) {
            let selected_t = selected_time.unwrap_or(f64::NAN);
            let selected_label = selected_text.clone().unwrap_or_else(|| "null".to_owned());
            mark_time(ctx, selected_t, 1.0);
            match hovered_time {
                Some(hovered_t) if hovered_t != selected_t => {
                    let hovered_label = hovered_text.clone().unwrap_or_default();
                    let mut sx = selected_x.unwrap_or(f64::NAN);
                    let mut hx = hovered_x.unwrap_or(f64::NAN);

                    let mut delta_text = String::from("\u{394}");
                    delta_text
                        .push_str(&format_marked_timestamp_text(hovered_t - selected_t, true)?);
                    let x_space = clamp_value(sx, graph_left, graph_left + graph_width) - hx;
                    let text_half_widths = (ctx.measure_text(text, &selected_label).width + 10.0)
                        / 2.0
                        + (ctx.measure_text(text, &hovered_label).width + 10.0) / 2.0
                        + 4.0;
                    let delta_metrics = ctx.measure_text(text, &delta_text);
                    let delta_width = delta_metrics.width + 10.0 + 4.0;
                    let mut offset_amount = text_half_widths - math::abs(x_space);
                    let does_delta_fit = delta_width <= math::abs(x_space);
                    if does_delta_fit {
                        offset_amount = text_half_widths + delta_width - math::abs(x_space);

                        let center_y =
                            (delta_metrics.actual_bounding_box_descent + 10.0) / 2.0 + graph_top;
                        ctx.set_global_alpha(0.35);
                        ctx.set_line_width(1.0);
                        ctx.set_line_dash(&[]);
                        ctx.set_stroke_style(ink(light));
                        ctx.begin_path();
                        ctx.move_to(sx, center_y);
                        ctx.line_to(hx, center_y);
                        ctx.stroke();
                        ctx.set_global_alpha(1.0);

                        let mut delta_x = (sx + hx) / 2.0;
                        if selected_t < time_range[0] {
                            delta_x = math::max2(delta_x, graph_left + delta_width / 2.0 - 2.0);
                        } else if selected_t > time_range[1] {
                            delta_x = math::min2(
                                delta_x,
                                graph_left + graph_width - delta_width / 2.0 + 2.0,
                            );
                        }
                        write_centered_time(ctx, text, &delta_text, delta_x, 0.35, false);
                    }
                    if offset_amount > 0.0 {
                        sx += (offset_amount / 2.0) * if sx < hx { -1.0 } else { 1.0 };
                        hx -= (offset_amount / 2.0) * if sx < hx { -1.0 } else { 1.0 };
                    }
                    write_centered_time(ctx, text, &selected_label, sx, 1.0, true);
                    write_centered_time(ctx, text, &hovered_label, hx, 0.35, true);
                }
                _ => {
                    write_centered_time(
                        ctx,
                        text,
                        &selected_label,
                        selected_x.unwrap_or(f64::NAN),
                        1.0,
                        true,
                    );
                }
            }
        } else if let (Some(label), Some(x)) = (hovered_text.as_deref(), hovered_x) {
            write_centered_time(ctx, text, label, x, 0.35, true);
        }
        self.last_hovered_time = hovered_time;

        ctx.set_line_width(1.0);
        ctx.set_stroke_style(ink(light));
        ctx.clear_rect(0.0, 0.0, width, graph_top);
        ctx.clear_rect(
            0.0,
            graph_top + graph_height,
            width,
            height - graph_top - graph_height,
        );
        ctx.clear_rect(0.0, graph_top, graph_left, graph_height);
        ctx.clear_rect(
            graph_left + graph_width,
            graph_top,
            width - graph_left - graph_width,
            graph_height,
        );
        ctx.stroke_rect(graph_left, graph_top, graph_width, graph_height);

        ctx.set_line_width(1.0);
        ctx.set_stroke_style(ink(light));
        ctx.set_fill_style(ink(light));
        ctx.set_text_baseline("middle");

        let y_axis = |ctx: &mut Canvas2d,
                      axis: Axis,
                      range: [f64; 2],
                      step_size: f64,
                      units: AxisUnits,
                      filter: &JsValue|
         -> Result<(), JsError> {
            let (text_x, tick_x) = match axis {
                Axis::Left => (graph_left - 15.0, graph_left - 5.0),
                Axis::Right => (
                    graph_left + graph_width + 15.0,
                    graph_left + graph_width + 5.0,
                ),
            };
            let edge_x = match axis {
                Axis::Left => graph_left,
                Axis::Right => graph_left + graph_width,
            };
            let mut step_pos = math::floor(range[1] / step_size) * step_size;
            let mut iter_count = 0;
            while {
                iter_count += 1;
                iter_count <= 100
            } {
                let y = scale_value(step_pos, range, [graph_top + graph_height_open, graph_top]);
                if y > graph_top + graph_height {
                    break;
                }

                ctx.set_global_alpha(1.0);
                if math::abs(step_pos) < MAX_DECIMAL_VALUE || step_pos % 1.0 == 0.0 {
                    ctx.fill_text(axis_value_text(step_pos, units, filter)?, text_x, y);
                    ctx.begin_path();
                    ctx.move_to(edge_x, y);
                    ctx.line_to(tick_x, y);
                    ctx.stroke();
                }

                if command.priority_axis == axis {
                    ctx.set_global_alpha(0.1);
                    ctx.begin_path();
                    ctx.move_to(graph_left, y);
                    ctx.line_to(graph_left + graph_width, y);
                    ctx.stroke();
                }

                step_pos -= step_size;
            }
            Ok(())
        };
        if command.show_left_axis {
            ctx.set_text_align("right");
            y_axis(
                ctx,
                Axis::Left,
                command.left_range,
                left_step_size,
                command.left_units,
                &command.left_filter,
            )?;
        }
        if command.show_right_axis {
            ctx.set_text_align("left");
            y_axis(
                ctx,
                Axis::Right,
                command.right_range,
                right_step_size,
                command.right_units,
                &command.right_filter,
            )?;
        }

        ctx.set_text_align("center");
        let mut step_pos =
            math::ceil(clean_float(display_time_range[0] / time_step_size)) * time_step_size;
        let mut iter_count = 0;
        while {
            iter_count += 1;
            iter_count <= 100
        } {
            let mut x = scale_value(step_pos, display_time_range, x_range);
            if x - graph_left - graph_width > 1.0 {
                break;
            } else if x - graph_left - graph_width > 0.0 {
                x = graph_left + graph_width;
            }

            let mut label = number::to_string(clean_float(step_pos));
            label.push('s');
            if is_start_at_0 {
                label.insert(0, '+');
            }

            ctx.set_global_alpha(1.0);
            ctx.fill_text(label, x, graph_top + graph_height + 15.0);
            ctx.begin_path();
            ctx.move_to(x, graph_top + graph_height);
            ctx.line_to(x, graph_top + graph_height + 5.0);
            ctx.stroke();

            ctx.set_global_alpha(0.1);
            ctx.begin_path();
            ctx.move_to(x, graph_top);
            ctx.line_to(x, graph_top + graph_height);
            ctx.stroke();

            step_pos += time_step_size;
        }
        Ok(())
    }
}

impl<C: TabCx + ?Sized> TabRenderer<C> for LineGraphRenderer {
    fn save_state(&self, _cx: &C) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _cx: &mut C, _state: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &C) -> Option<f64> {
        None
    }

    fn render(
        &mut self,
        cx: &mut C,
        command: &dyn asrs_model::tab_controller::TabCommand,
    ) -> Result<(), JsError> {
        let Some(command) = command.downcast_ref::<LineGraphCommand>() else {
            return Err(JsError::new(
                "TypeError",
                "LineGraphRenderer.render was given another tab's command",
            ));
        };
        let dom = Rc::clone(&self.dom);
        let text = Rc::clone(&self.text);
        let (width, height, device_pixel_ratio, light) = {
            let d = dom.borrow();
            (d.width, d.height, d.device_pixel_ratio, d.light)
        };
        let preferences = cx.tab_preferences().cloned();
        let frame = LineGraphFrame {
            width,
            height,
            device_pixel_ratio,
            light,
            preferences: preferences.as_ref(),
        };
        let result =
            self.render_frame(&frame, command, cx.tab_selection(), &mut *text.borrow_mut());
        match result {
            Ok(Some(canvas)) => {
                dom.borrow_mut().painted = Some(canvas);
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(RenderError { canvas, error }) => {
                dom.borrow_mut().painted = Some(canvas);
                Err(error)
            }
        }
    }
}
