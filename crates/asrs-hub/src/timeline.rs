//! `src/hub/Timeline.ts`: the timeline canvas painter and its pointer logic, plus the
//! mouse-drag pan of the `ScrollSensor` it owns. Wheel input reaches [`Timeline::scroll`]
//! through U08's sensor replacement.

use std::rc::Rc;

use asrs_canvas::Canvas2d;
use asrs_canvas::text::TextBackend;
use asrs_js::math;
use asrs_js::time::Clock;
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::robot_state::{RobotMode, get_robot_state_ranges};
use asrs_log::log::{Log, TimestampDisplay};
use asrs_log::util::{calc_axis_step_size, clamp_value, clean_float, scale_value};
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::{Preferences, Timestamps};
use asrs_model::selection::Selection;

const STEP_TARGET_PX: f64 = 125.0;
const HELP_FONT: &str = "200 14px ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";
const LABEL_FONT: &str = "200 12px ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";
const HELP_TEXT: &str = "Scroll timeline to zoom";
const HELP_TIMER_MS: f64 = 1000.0;

/// What one `periodic` call reads from the DOM and `window`.
#[derive(Clone, Copy, Debug)]
pub struct TimelineFrame<'a> {
    /// `CONTAINER.clientWidth`.
    pub width: f64,
    /// `CONTAINER.clientHeight`; zero means hidden and nothing is painted.
    pub height: f64,
    pub device_pixel_ratio: f64,
    /// `!matchMedia("(prefers-color-scheme: dark)").matches`.
    pub light: bool,
    /// `window.preferences`, `None` before `set-preferences` arrives.
    pub preferences: Option<&'a Preferences>,
}

pub struct Timeline {
    clock: Rc<dyn Clock>,
    mouse_down_x: f64,
    grab_zoom_active: bool,
    grab_zoom_start_time: f64,
    last_cursor_x: Option<f64>,
    last_cursor_in_rect: bool,
    scroll_time: Option<f64>,
    zoom_in_accumulator: f64,
    pan_active: bool,
    pan_last_cursor_x: f64,
    help_timers: Vec<f64>,
}

fn timestamp_display(preferences: Option<&Preferences>) -> Option<TimestampDisplay> {
    preferences.map(|p| match p.timestamps {
        Timestamps::StartZero => TimestampDisplay::StartZero,
        Timestamps::Original => TimestampDisplay::Original,
    })
}

impl Timeline {
    pub fn new(clock: Rc<dyn Clock>) -> Self {
        Timeline {
            clock,
            mouse_down_x: 0.0,
            grab_zoom_active: false,
            grab_zoom_start_time: 0.0,
            last_cursor_x: None,
            last_cursor_in_rect: false,
            scroll_time: None,
            zoom_in_accumulator: 0.0,
            pan_active: false,
            pan_last_cursor_x: 0.0,
            help_timers: Vec::new(),
        }
    }

    /// A `mousemove` at `x` (client x minus the overlay's left edge). `on_overlay` is whether
    /// the event targets the scroll overlay, whose listeners run before the window's.
    /// `container_hidden` is the container's `hidden` attribute; `overlay` is what
    /// [`Timeline::scroll`] needs when a drag pans.
    pub fn mouse_move(
        &mut self,
        x: f64,
        on_overlay: bool,
        container_hidden: bool,
        overlay: ScrollTarget,
        selection: &mut dyn Selection,
    ) {
        if on_overlay {
            self.last_cursor_x = Some(x);
            self.last_cursor_in_rect = true;
            if self.pan_active {
                let dx = self.pan_last_cursor_x - x;
                self.scroll(dx, 0.0, overlay, selection);
                self.pan_last_cursor_x = x;
            }
        }
        if !container_hidden && self.grab_zoom_active {
            self.last_cursor_x = Some(x);
        }
        if container_hidden {
            return;
        }
        let hovered_time = selection.get_hovered_time();
        if let (true, Some(hovered_time)) = (self.grab_zoom_active, hovered_time) {
            selection.set_grab_zoom_range(Some([self.grab_zoom_start_time, hovered_time]));
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
    pub fn mouse_down(&mut self, x: f64, shift_key: bool, selection: &mut dyn Selection) {
        self.mouse_down_x = x;
        let hovered_time = selection.get_hovered_time();
        if let (true, Some(hovered_time)) = (shift_key, hovered_time) {
            self.grab_zoom_active = true;
            self.grab_zoom_start_time = hovered_time;
        }
        if !shift_key {
            self.pan_active = true;
            self.pan_last_cursor_x = x;
        }
    }

    /// A `mouseup` anywhere; `on_overlay` when it targets the overlay.
    pub fn mouse_up(
        &mut self,
        on_overlay: bool,
        container_hidden: bool,
        selection: &mut dyn Selection,
    ) {
        if on_overlay {
            self.pan_active = false;
        }
        if container_hidden {
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
    pub fn click(&mut self, x: f64, selection: &mut dyn Selection) {
        if math::abs(x - self.mouse_down_x) <= 5.0 {
            let hovered_time = selection.get_hovered_time();
            if let Some(time) = hovered_time.filter(|t| *t != 0.0 && !t.is_nan()) {
                selection.set_selected_time(time);
            }
        }
    }

    /// `contextmenu` on the overlay.
    pub fn context_menu(&mut self, selection: &mut dyn Selection) {
        selection.go_idle();
    }

    /// The scroll sensor's callback: wheel deltas from U08's sensor, or a drag pan.
    pub fn scroll(
        &mut self,
        dx: f64,
        dy: f64,
        target: ScrollTarget,
        selection: &mut dyn Selection,
    ) {
        if target.client_height == 0.0 {
            return;
        }
        selection.apply_timeline_scroll(dx, dy, target.client_width);

        if target.preferences_loaded
            && !target.has_scrolled_timeline
            && self.scroll_time.is_none()
            && dy < 0.0
        {
            self.zoom_in_accumulator += dy;
            if self.zoom_in_accumulator <= -200.0 {
                let now = self.clock.now();
                self.scroll_time = Some(now);
                self.help_timers.push(now + HELP_TIMER_MS);
            }
        }
    }

    /// The earliest pending help-overlay timer, so the event loop can wake for it.
    pub fn next_timer(&self) -> Option<f64> {
        self.help_timers.first().copied()
    }

    /// Runs the help-overlay timers due at or before `now` and returns the
    /// `update-preferences` messages they send.
    pub fn run_timers(&mut self, now: f64) -> Vec<NamedMessage<HubToMain>> {
        let mut out = Vec::new();
        while self.help_timers.first().is_some_and(|due| *due <= now) {
            self.help_timers.remove(0);
            let mut data = JsObject::new();
            data.insert("hasScrolledTimeline", JsValue::Bool(true));
            out.push(NamedMessage::new(
                HubToMain::UpdatePreferences,
                JsValue::Object(data),
            ));
        }
        out
    }

    /// `periodic()`: updates the hovered time from the cursor and paints one frame. `None`
    /// when the container is hidden and upstream returns before touching the canvas.
    pub fn periodic(
        &mut self,
        frame: &TimelineFrame<'_>,
        selection: &mut dyn Selection,
        log: &Log,
        text: &mut dyn TextBackend,
    ) -> Option<Canvas2d> {
        if frame.height == 0.0 {
            return None;
        }
        let dpr = frame.device_pixel_ratio;
        let width = frame.width;
        let height = frame.height;
        let light = frame.light;
        let time_range = selection.get_timeline_range();
        let mut ctx = Canvas2d::new(math::to_uint32(width * dpr), math::to_uint32(height * dpr));
        ctx.scale(dpr, dpr);
        ctx.clear_rect(0.0, 0.0, width, height);

        let display_offset = log.get_timestamp_display_offset(timestamp_display(frame.preferences));
        let display_time_range = [
            time_range[0] + display_offset,
            time_range[1] + display_offset,
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
        let step_size = calc_axis_step_size(
            display_time_range,
            width,
            STEP_TARGET_PX + extra_digits * 12.0,
        );

        ctx.set_line_width(1.0);
        let mut range_borders: Vec<f64> = Vec::new();
        let includes = |borders: &[f64], v: f64| {
            borders
                .iter()
                .any(|b| *b == v || (b.is_nan() && v.is_nan()))
        };
        for range in get_robot_state_ranges(log) {
            let color = match range.mode {
                RobotMode::Disabled => continue,
                RobotMode::Teleop => light_dark(light, "#5a83ff", "#3957dd"),
                RobotMode::Auto => light_dark(light, "#efab28", "#c38615"),
                RobotMode::Utility => light_dark(light, "#888888", "#777777"),
            };
            let start_time = range.start;
            let end_time = range.end.unwrap_or(time_range[1]);
            ctx.set_fill_style(color);
            let start_x = clamp_value(
                scale_value(start_time, time_range, [0.0, width]),
                0.0,
                width,
            );
            let end_x = clamp_value(scale_value(end_time, time_range, [0.0, width]), 0.0, width);
            ctx.fill_rect(start_x, 0.0, end_x - start_x, height);

            if !includes(&range_borders, range.start) {
                range_borders.push(range.start);
            }
            if let Some(end) = range.end
                && !includes(&range_borders, end)
            {
                range_borders.push(end);
            }
        }

        if let Some(grab_zoom_range) = selection.get_grab_zoom_range() {
            let start_x = scale_value(grab_zoom_range[0], time_range, [0.0, width]);
            let end_x = scale_value(grab_zoom_range[1], time_range, [0.0, width]);
            ctx.set_global_alpha(0.6);
            ctx.set_fill_style("yellow");
            ctx.fill_rect(start_x, 0.0, end_x - start_x, height);
            ctx.set_global_alpha(1.0);
        }

        if let Some(cursor_x) = self.last_cursor_x
            && cursor_x > 0.0
            && cursor_x < width
        {
            let cursor_time = scale_value(cursor_x, [0.0, width], time_range);
            let nearest_range_border = range_borders.iter().fold(f64::INFINITY, |prev, &border| {
                if math::abs(cursor_time - border) < math::abs(cursor_time - prev) {
                    border
                } else {
                    prev
                }
            });
            let nearest_range_border_x =
                scale_value(nearest_range_border, time_range, [0.0, width]);
            selection.set_hovered_time(Some(
                if math::abs(cursor_x - nearest_range_border_x) < 5.0 {
                    nearest_range_border
                } else {
                    cursor_time
                },
            ));
        }

        let mut marked_xs: Vec<f64> = Vec::new();
        let mut mark_time = |ctx: &mut Canvas2d, time: f64, alpha: f64| {
            if time >= time_range[0] && time <= time_range[1] {
                ctx.set_global_alpha(alpha);
                ctx.set_line_width(1.0);
                ctx.set_stroke_style(light_dark(light, "#222", "#eee"));

                let x = scale_value(time, time_range, [0.0, width]);
                if x > 1.0 && x < width - 1.0 {
                    let triangle_side_length = 6.0;
                    let triangle_height = 0.5 * math::sqrt(3.0) * triangle_side_length;

                    marked_xs.push(x);
                    ctx.begin_path();
                    ctx.move_to(x, triangle_height);
                    let mut i = x - triangle_side_length / 2.0;
                    while i <= x + triangle_side_length / 2.0 {
                        ctx.line_to(i, -1.0);
                        ctx.move_to(x, triangle_height);
                        i += 1.0;
                    }
                    ctx.line_to(x, height - triangle_height);
                    let mut i = x - triangle_side_length / 2.0;
                    while i <= x + triangle_side_length / 2.0 {
                        ctx.line_to(i, height + 1.0);
                        ctx.move_to(x, height - triangle_height);
                        i += 1.0;
                    }
                    ctx.stroke();
                }
                ctx.set_global_alpha(1.0);
            }
        };

        let hovered_time = selection.get_hovered_time();
        let selected_time = selection.get_selected_time();
        if let Some(t) = hovered_time {
            mark_time(&mut ctx, t, 0.5);
        }
        if let Some(t) = selected_time {
            mark_time(&mut ctx, t, 1.0);
        }

        ctx.set_font(HELP_FONT);
        let help_text_width = ctx.measure_text(text, HELP_TEXT).width;

        let has_scrolled_timeline = frame.preferences.is_some_and(|p| p.has_scrolled_timeline);
        if has_scrolled_timeline {
            self.scroll_time = None;
            self.zoom_in_accumulator = 0.0;
        }

        let help_text_opacity = if has_scrolled_timeline {
            0.0
        } else if let Some(scroll_time) = self.scroll_time {
            let elapsed = self.clock.now() - scroll_time;
            if elapsed < 600.0 {
                0.5
            } else if elapsed < 1000.0 {
                0.5 * (1.0 - (elapsed - 600.0) / 400.0)
            } else {
                0.0
            }
        } else {
            0.5
        };

        let ink = light_dark(light, "#222", "#eee");
        ctx.set_line_width(0.5);
        ctx.set_stroke_style(ink);
        ctx.set_fill_style(ink);
        ctx.set_text_align("center");
        ctx.set_text_baseline("middle");
        let is_start_at_0 = !frame
            .preferences
            .is_some_and(|p| p.timestamps == Timestamps::Original);
        let mut step_pos = math::ceil(clean_float(display_time_range[0] / step_size)) * step_size;
        let mut iter_count = 0;
        while {
            iter_count += 1;
            iter_count <= 100
        } {
            let x = scale_value(step_pos, display_time_range, [0.0, width]);
            if x > width + 1.0 {
                break;
            }

            let mut label = asrs_js::number::to_string(clean_float(step_pos));
            label.push('s');
            if is_start_at_0 {
                label.insert(0, '+');
            }
            ctx.set_font(LABEL_FONT);
            let text_width = ctx.measure_text(text, &label).width;
            let text_x = clamp_value(x, text_width / 2.0 + 3.0, width - text_width / 2.0 - 3.0);
            let text_x_range = [text_x - text_width / 2.0, text_x + text_width / 2.0];

            let mark_distance = marked_xs.iter().fold(f64::INFINITY, |min, &x| {
                let mut dist = 0.0;
                if x < text_x_range[0] {
                    dist = text_x_range[0] - x;
                }
                if x > text_x_range[1] {
                    dist = x - text_x_range[1];
                }
                if dist < min { dist } else { min }
            });

            let help_left = width / 2.0 - help_text_width / 2.0;
            let help_right = width / 2.0 + help_text_width / 2.0;
            let help_distance = if help_right < text_x_range[0] {
                text_x_range[0] - help_right
            } else if help_left > text_x_range[1] {
                help_left - text_x_range[1]
            } else {
                0.0
            };

            let adjusted_help_distance = if help_text_opacity == 0.0 {
                f64::INFINITY
            } else {
                help_distance / (help_text_opacity / 0.5)
            };

            let min_distance = math::min2(mark_distance, adjusted_help_distance);
            ctx.set_global_alpha(clamp_value(
                scale_value(min_distance, [0.0, 20.0], [0.2, 0.5]),
                0.0,
                1.0,
            ));
            ctx.fill_text(label.as_str(), text_x, height / 2.0);

            ctx.begin_path();
            ctx.move_to(x, 0.0);
            ctx.line_to(x, 8.0);
            ctx.move_to(x, height - 8.0);
            ctx.line_to(x, height);
            ctx.stroke();

            step_pos += step_size;
        }

        if help_text_opacity > 0.0 {
            ctx.set_font(HELP_FONT);
            ctx.set_fill_style(ink);
            ctx.set_text_align("center");
            ctx.set_text_baseline("middle");
            ctx.set_global_alpha(help_text_opacity);
            ctx.fill_text(HELP_TEXT, width / 2.0, height / 2.0);
        }
        ctx.set_global_alpha(1.0);
        Some(ctx)
    }
}

/// What the scroll callback reads besides its deltas: the overlay's client size and
/// `window.preferences`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollTarget {
    /// `SCROLL_OVERLAY.clientWidth`.
    pub client_width: f64,
    /// `CONTAINER.clientHeight`; zero means hidden.
    pub client_height: f64,
    pub preferences_loaded: bool,
    pub has_scrolled_timeline: bool,
}

impl ScrollTarget {
    pub fn new(client_width: f64, client_height: f64, preferences: Option<&Preferences>) -> Self {
        ScrollTarget {
            client_width,
            client_height,
            preferences_loaded: preferences.is_some(),
            has_scrolled_timeline: preferences.is_some_and(|p| p.has_scrolled_timeline),
        }
    }
}

fn light_dark(light: bool, l: &'static str, d: &'static str) -> &'static str {
    if light { l } else { d }
}
