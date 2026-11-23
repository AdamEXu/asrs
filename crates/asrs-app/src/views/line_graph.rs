//! The line graph tab's renderer element (`#renderer1` in `www/hub.html`, styled by
//! `www/renderers.css`): the canvas `asrs_tabs::line_graph::LineGraphRenderer` paints, the
//! scroll overlay its pointer and wheel listeners hang on, and the help overlay. Its controls are
//! the frame's source lists.

use std::collections::HashMap;
use std::rc::Rc;

use asrs_hub::hub::HubModel;
use asrs_js::math;
use asrs_model::scroll_sensor::ScrollSensor;
use asrs_model::tab_type::TabType;
use asrs_tabs::cx::TabCx;
use asrs_tabs::line_graph::{LineGraphRenderer, OverlayStyle, Pointer};
use asrs_ui::canvas_view::geometry::{client_size, mouse_client};
use asrs_ui::canvas_view::{CanvasMetrics, CanvasState, canvas_view};
use asrs_ui::gpui::{
    AnyElement, App, AppContext, ClickEvent, Context, Entity, InteractiveElement, IntoElement,
    MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, ScrollEvent,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use asrs_ui::icon::{Icon, IconFit, IconSource};
use asrs_ui::parity::probed;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, Text, TextStyle, snap_remainder};
use asrs_ui::theme::{Appearance, FONT_FAMILY, chains};

use super::frame::{FrameHost, HubFrame, wheel_of};

const SCROLL_BOTTOM: f64 = 35.0;
const DEFAULT_SCROLL_TOP: f64 = 8.0;
const HELP_OPACITY: f64 = 0.45;
const HELP_FADE_MS: f64 = 400.0;
const HELP_ICON: f64 = 30.0;
const HELP_ROW_GAP: f64 = 16.0;
const HELP_ICON_GAP: f64 = 12.0;
const HELP_ROWS: [(&str, &str); 2] = [
    ("height.svg", "Scroll to zoom"),
    ("arrowrange.svg", "Drag to pan"),
];

fn help_style() -> TextStyle {
    let font = format!("500 16px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("letter-spacing", "0.3px"),
        ("white-space", "nowrap"),
    ])
    .expect("line graph help style")
}

/// `img.line-graph-help-icon`: 30 px square with `object-fit: contain`, which for these square
/// symbols is the SVG drawn at its aspect ratio across the box's width.
fn help_icon(name: &str) -> Option<IconSource> {
    let svg = std::str::from_utf8(Symbol::named(name)?.svg()).ok()?;
    Some(IconSource::Svg(
        svg.replacen(r#" height="24px""#, "", 1)
            .replacen(r#" width="24px""#, "", 1)
            .into(),
    ))
}

/// `cubic-bezier(0, 0, 0.58, 1)` (`ease-out`) at `t`.
fn ease_out(t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let (x1, x2) = (0.0, 0.58);
    let (y1, y2) = (0.0, 1.0);
    let bezier = |a: f64, b: f64, s: f64| {
        3.0 * a * s * (1.0 - s).powi(2) + 3.0 * b * s * s * (1.0 - s) + s.powi(3)
    };
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..64 {
        let mid = (lo + hi) / 2.0;
        if bezier(x1, x2, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bezier(y1, y2, (lo + hi) / 2.0)
}

/// The help container's opacity: its `help-fade-in` animation from when it was last displayed,
/// then the `fading-out` class's transition to 0.
#[derive(Default)]
struct HelpFade {
    displayed_at: Option<f64>,
    fading_since: Option<f64>,
}

impl HelpFade {
    fn opacity(&mut self, displayed: bool, fading_out: bool, now: f64) -> (f64, bool) {
        if !displayed {
            *self = HelpFade::default();
            return (0.0, false);
        }
        let shown = *self.displayed_at.get_or_insert(now);
        if !fading_out {
            self.fading_since = None;
        } else if self.fading_since.is_none() {
            self.fading_since = Some(now);
        }
        let progress = |since: f64| math::min2(math::max2((now - since) / HELP_FADE_MS, 0.0), 1.0);
        let fade_in = progress(shown);
        if fade_in < 1.0 {
            return (HELP_OPACITY * ease_out(fade_in), true);
        }
        match self.fading_since {
            Some(since) => {
                let t = progress(since);
                (HELP_OPACITY * (1.0 - ease_out(t)), t < 1.0)
            }
            None => (HELP_OPACITY, false),
        }
    }
}

#[derive(Default)]
struct TabView {
    canvas: Option<Entity<CanvasState>>,
    sensor: ScrollSensor,
    help: HelpFade,
}

/// The displayed line graph's boxes in client coordinates: `ROOT` (whose x is the renderer
/// content's) and `SCROLL_OVERLAY`.
#[derive(Clone, Copy, Debug)]
struct Shown {
    key: usize,
    index: usize,
    root: [f64; 4],
    overlay: [f64; 4],
    device_pixel_ratio: f64,
}

impl Shown {
    fn overlay_client_size(&self) -> [f64; 2] {
        let dpr = self.device_pixel_ratio as f32;
        [
            f64::from(client_size(self.overlay[2] as f32, dpr)),
            f64::from(client_size(self.overlay[3] as f32, dpr)),
        ]
    }

    fn pointer(&self, client_x: f64, shift_key: bool) -> Pointer {
        Pointer {
            client_x,
            shift_key,
            root_x: self.root[0],
            overlay_x: self.overlay[0],
            overlay_client_width: self.overlay_client_size()[0],
            root_hidden: false,
        }
    }

    fn contains(&self, x: f64, y: f64) -> bool {
        let [ox, oy, w, h] = self.overlay;
        w > 0.0 && h > 0.0 && x >= ox && x < ox + w && y >= oy && y < oy + h
    }
}

#[derive(Default)]
pub struct LineGraphViews {
    tabs: HashMap<usize, TabView>,
    shown: Option<Shown>,
    hovered: bool,
    pointer: Option<[f64; 2]>,
    pressed: bool,
}

fn renderer_key(renderer: &LineGraphRenderer) -> usize {
    Rc::as_ptr(renderer.dom()) as *const () as usize
}

/// The displayed tab when it is a line graph, with its index.
fn displayed(hub: &HubModel) -> Option<(usize, &LineGraphRenderer)> {
    hub.tabs().tabs().enumerate().find_map(|(i, tab)| {
        if tab.content_hidden() || tab.tab_type() != Some(TabType::LineGraph) {
            return None;
        }
        tab.renderer()
            .downcast_ref::<LineGraphRenderer>()
            .map(|r| (i, r))
    })
}

fn with_renderer<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut LineGraphRenderer, &mut asrs_hub::hub::HubPage) -> R,
) -> Option<R> {
    let (tabs, page) = hub.tabs_mut();
    let tab = tabs.tab_mut(index)?;
    let renderer = tab.renderer.downcast_mut::<LineGraphRenderer>()?;
    Some(f(renderer, page))
}

/// `SCROLL_OVERLAY`'s box in `root`: the stylesheet's `left: 0; right: 0; top: 8px;
/// bottom: 35px` until `render` writes its inline `top`, `left` and `right`.
fn overlay_box(root: [f64; 4], style: &OverlayStyle) -> [f64; 4] {
    let left = style.scroll_left.unwrap_or(0.0);
    let right = style.scroll_right.unwrap_or(0.0);
    let top = style.scroll_top.unwrap_or(DEFAULT_SCROLL_TOP);
    [
        root[0] + left,
        root[1] + top,
        math::max2(root[2] - left - right, 0.0),
        math::max2(root[3] - top - SCROLL_BOTTOM, 0.0),
    ]
}

impl<H: FrameHost> HubFrame<H> {
    /// The displayed line graph's renderer element over `rect` (`div.renderer-content`), if the
    /// displayed tab is a line graph.
    pub(super) fn line_graph_element(
        &mut self,
        rect: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let hub = self.host().hub(cx);
        let live: Vec<usize> = hub
            .tabs()
            .tabs()
            .filter_map(|t| t.renderer().downcast_ref::<LineGraphRenderer>())
            .map(renderer_key)
            .collect();
        let shown = displayed(hub).map(|(index, renderer)| {
            let style = renderer.style().clone();
            (index, renderer_key(renderer), style)
        });
        let now = self.host().frame_time(cx);
        let views = self.line_graph_mut();
        views.tabs.retain(|key, _| live.contains(key));
        let Some((index, key, style)) = shown else {
            views.shown = None;
            views.hovered = false;
            return None;
        };
        let shown = Shown {
            key,
            index,
            root: rect,
            overlay: overlay_box(rect, &style),
            device_pixel_ratio: f64::from(scale),
        };
        if views.shown.is_some_and(|s| s.key != key) {
            views.hovered = false;
        }
        views.shown = Some(shown);
        // Chromium's fake mouse move after a layout change: the overlay moved out from under a
        // still pointer, which fires its `mouseleave` unless a button is held.
        let left_under_pointer = views.hovered
            && !views.pressed
            && views.pointer.is_some_and(|[x, y]| !shown.contains(x, y));
        if left_under_pointer {
            views.hovered = false;
        }
        let tab = views.tabs.entry(key).or_default();
        let canvas = tab
            .canvas
            .get_or_insert_with(|| cx.new(CanvasState::new))
            .clone();
        let help_displayed = style.help_visible != Some(false);
        let (opacity, animating) = tab.help.opacity(help_displayed, style.help_fading_out, now);
        if animating || tab.sensor.animating() {
            window.request_animation_frame();
        }

        if left_under_pointer {
            self.host_mut().input(cx, |hub| {
                with_renderer(hub, index, |renderer, page| {
                    renderer.mouse_leave(page.tab_selection())
                });
            });
        }
        let dark = appearance == Appearance::Dark;
        let entity = cx.entity().downgrade();
        let painter = move |metrics: &CanvasMetrics, cx: &mut App| {
            entity
                .update(cx, |frame, cx| {
                    frame.paint_line_graph(key, metrics, dark, cx)
                })
                .ok()
                .flatten()
        };
        let mut root = div().absolute().left_0().top_0().size_full().child(probed(
            "canvas.line-graph-canvas",
            div()
                .absolute()
                .left_0()
                .top_0()
                .size_full()
                .child(canvas_view(canvas, painter).size_full()),
        ));
        root = root.child(self.line_graph_overlay(&shown, cx));
        if help_displayed {
            root = root.child(help_overlay(rect, opacity, appearance, scale, cx));
        }
        Some(root.into_any_element())
    }

    fn line_graph_overlay(&self, shown: &Shown, cx: &mut Context<'_, Self>) -> AnyElement {
        let [x, y, w, h] = shown.overlay;
        probed(
            "div.line-graph-scroll",
            div()
                .id("line-graph-scroll")
                .absolute()
                .left(px((x - shown.root[0]) as f32))
                .top(px((y - shown.root[1]) as f32))
                .w(px(w as f32))
                .h(px(h as f32))
                .on_any_mouse_down(cx.listener(|frame, event: &MouseDownEvent, _, cx| {
                    frame.line_graph_mouse_down(event, cx)
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|frame, _: &MouseUpEvent, _, cx| {
                        frame.line_graph_overlay_mouse_up(cx)
                    }),
                )
                .on_mouse_up(
                    MouseButton::Right,
                    cx.listener(|frame, _: &MouseUpEvent, _, cx| {
                        frame.line_graph_overlay_mouse_up(cx)
                    }),
                )
                .on_click(
                    cx.listener(|frame, event: &ClickEvent, _, cx| {
                        frame.line_graph_click(event, cx)
                    }),
                )
                .on_scroll(cx.listener(|frame, event: &ScrollEvent, _, cx| {
                    frame.line_graph_wheel(event, cx)
                })),
        )
        .into_any_element()
    }

    /// The displayed renderer's frame before its `render`: the canvas box upstream reads, then
    /// the overlay's `scroll` event and `ScrollSensor.periodic()`. Returns the frame `render`
    /// painted since the last call.
    fn paint_line_graph(
        &mut self,
        key: usize,
        metrics: &CanvasMetrics,
        dark: bool,
        cx: &mut Context<'_, Self>,
    ) -> Option<asrs_canvas::Canvas2d> {
        let shown = self.line_graph().shown.filter(|s| s.key == key)?;
        let now = self.host().frame_time(cx);
        let client = shown.overlay_client_size();
        let tab = self.line_graph_mut().tabs.get_mut(&key)?;
        tab.sensor.set_layout(client, shown.device_pixel_ratio);
        tab.sensor.animate(now);
        let scroll = tab
            .sensor
            .scroll_step(now, client[0] != 0.0 || client[1] != 0.0);
        tab.sensor.periodic(now);
        let pointer = shown.pointer(0.0, false);
        self.host_mut().input(cx, |hub| {
            with_renderer(hub, shown.index, |renderer, page| {
                {
                    let mut dom = renderer.dom().borrow_mut();
                    dom.width = f64::from(metrics.client_width);
                    dom.height = f64::from(metrics.client_height);
                    dom.device_pixel_ratio = metrics.device_pixel_ratio;
                    dom.light = !dark;
                }
                if let Some(d) = scroll {
                    let preferences = page.preferences().cloned();
                    renderer.scroll(
                        d.dx,
                        d.dy,
                        false,
                        pointer,
                        preferences.as_ref(),
                        page.tab_selection(),
                    );
                }
                renderer.dom().borrow_mut().painted.take()
            })
            .flatten()
        })
    }

    fn line_graph(&self) -> &LineGraphViews {
        &self.line_graph
    }

    fn line_graph_mut(&mut self) -> &mut LineGraphViews {
        &mut self.line_graph
    }

    /// Sends the `update-preferences` the help overlay's timers queued.
    pub(super) fn line_graph_timers(&mut self, cx: &mut App) {
        let Some(shown) = self.line_graph().shown else {
            return;
        };
        let messages = self.host_mut().input(cx, |hub| {
            let (tabs, _) = hub.tabs_mut();
            tabs.tab_mut(shown.index)
                .and_then(|t| t.renderer.downcast_mut::<LineGraphRenderer>())
                .map(|r| r.run_timers())
                .unwrap_or_default()
        });
        for message in messages {
            self.host_mut().send_main_message(message, cx);
        }
    }

    /// The renderer's window `mousemove` listeners, after the overlay's own when the pointer is
    /// over it, and the overlay's `mouseleave` when it left.
    pub(super) fn line_graph_mouse_move(
        &mut self,
        x: f64,
        y: f64,
        shift: bool,
        pressed: bool,
        cx: &mut Context<'_, Self>,
    ) {
        let views = self.line_graph_mut();
        views.pointer = Some([x, y]);
        views.pressed = pressed;
        let Some(shown) = views.shown else {
            return;
        };
        let inside = shown.contains(x, y);
        let was_inside = std::mem::replace(&mut self.line_graph_mut().hovered, inside);
        let pointer = shown.pointer(x, shift);
        self.host_mut().input(cx, |hub| {
            with_renderer(hub, shown.index, |renderer, page| {
                let preferences = page.preferences().cloned();
                if was_inside && !inside {
                    renderer.mouse_leave(page.tab_selection());
                }
                if inside {
                    renderer.overlay_mouse_move(
                        pointer,
                        preferences.as_ref(),
                        page.tab_selection(),
                    );
                }
                renderer.window_mouse_move(pointer, page.tab_selection());
            });
        });
        cx.notify();
    }

    /// The renderer's window `mouseup` listener.
    pub(super) fn line_graph_mouse_up(&mut self, x: f64, shift: bool, cx: &mut Context<'_, Self>) {
        self.line_graph_mut().pressed = false;
        let Some(shown) = self.line_graph().shown else {
            return;
        };
        let pointer = shown.pointer(x, shift);
        self.host_mut().input(cx, |hub| {
            with_renderer(hub, shown.index, |renderer, page| {
                renderer.window_mouse_up(pointer, page.tab_selection())
            });
        });
        cx.notify();
    }

    fn line_graph_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.line_graph().shown else {
            return;
        };
        let x = mouse_client(f32::from(event.position.x));
        let pointer = shown.pointer(x, event.modifiers.shift);
        let right = event.button == MouseButton::Right;
        self.host_mut().input(cx, |hub| {
            with_renderer(hub, shown.index, |renderer, page| {
                renderer.mouse_down(pointer);
                if right {
                    renderer.context_menu(page.tab_selection());
                }
            });
        });
        cx.notify();
    }

    fn line_graph_overlay_mouse_up(&mut self, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.line_graph().shown else {
            return;
        };
        self.host_mut().input(cx, |hub| {
            with_renderer(hub, shown.index, |renderer, _| renderer.overlay_mouse_up());
        });
    }

    fn line_graph_click(&mut self, event: &ClickEvent, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.line_graph().shown else {
            return;
        };
        let position = event.position();
        let x = mouse_client(f32::from(position.x));
        let pointer = shown.pointer(x, event.modifiers().shift);
        self.host_mut().input(cx, |hub| {
            with_renderer(hub, shown.index, |renderer, page| {
                renderer.click(pointer, page.tab_selection())
            });
        });
        cx.notify();
    }

    fn line_graph_wheel(&mut self, event: &ScrollEvent, cx: &mut Context<'_, Self>) {
        let Some(shown) = self.line_graph().shown else {
            return;
        };
        let now = self.host().frame_time(cx);
        let client = shown.overlay_client_size();
        if let Some(tab) = self.line_graph_mut().tabs.get_mut(&shown.key) {
            tab.sensor.set_layout(client, shown.device_pixel_ratio);
            tab.sensor.wheel(wheel_of(event), now);
        }
        cx.notify();
    }
}

/// `div.line-graph-help` over `root`: a column of the two rows, centred in the box between
/// `top: 8px` and `bottom: 35px`, its container at `opacity`.
fn help_overlay(
    root: [f64; 4],
    opacity: f64,
    appearance: Appearance,
    scale: f32,
    cx: &mut App,
) -> AnyElement {
    let dark = appearance == Appearance::Dark;
    let style = help_style();
    let lu = |v: f64| math::floor(v * 64.0) / 64.0;
    let rows: Vec<(&str, &str, f64, f64)> = HELP_ROWS
        .iter()
        .map(|&(icon, label)| {
            let (w, h) = text::measure(cx, &style, label, scale);
            (icon, label, w, h)
        })
        .collect();
    let container_w = rows
        .iter()
        .map(|r| HELP_ICON + HELP_ICON_GAP + r.2)
        .fold(0.0, math::max2);
    let container_h = HELP_ICON * rows.len() as f64 + HELP_ROW_GAP * (rows.len() - 1) as f64;
    let box_w = root[2];
    let box_h = math::max2(root[3] - DEFAULT_SCROLL_TOP - SCROLL_BOTTOM, 0.0);
    let left = lu((box_w - container_w) / 2.0);
    let top = lu((box_h - container_h) / 2.0);
    let color = if dark { 0xffffff } else { 0x000000 };
    let chain = if dark { chains::INVERT_ONE } else { "" };
    let mut container = div()
        .absolute()
        .left(px(left as f32))
        .top(px(top as f32))
        .w(px(container_w as f32))
        .h(px(container_h as f32))
        .opacity(opacity as f32);
    for (i, (icon, label, w, h)) in rows.into_iter().enumerate() {
        let row_top = i as f64 * (HELP_ICON + HELP_ROW_GAP);
        if let Some(source) = help_icon(icon) {
            container = container.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(row_top as f32))
                    .w(px(HELP_ICON as f32))
                    .h(px(HELP_ICON as f32))
                    .child(Icon::new(source, chain).fit(IconFit::Natural)),
            );
        }
        let text_left = HELP_ICON + HELP_ICON_GAP;
        let text_top = row_top + lu((HELP_ICON - h) / 2.0);
        let abs_x = (root[0] + left + text_left) as f32;
        let abs_y = (root[1] + DEFAULT_SCROLL_TOP + top + text_top) as f32;
        container = container.child(
            div()
                .absolute()
                .left(px(text_left as f32))
                .top(px(text_top as f32))
                .w(px(w as f32))
                .h(px(h as f32))
                .child(
                    Text::new(label, style)
                        .color(color)
                        .subpixel(snap_remainder(abs_x, scale), snap_remainder(abs_y, scale)),
                ),
        );
    }
    probed(
        "div.line-graph-help",
        div()
            .absolute()
            .left_0()
            .top(px(DEFAULT_SCROLL_TOP as f32))
            .w_full()
            .h(px(box_h as f32))
            .child(container),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_matches_its_control_points() {
        assert_eq!((ease_out(0.0), ease_out(1.0)), (0.0, 1.0));
        assert!((0.68..0.69).contains(&ease_out(0.5)));
    }

    #[test]
    fn help_fades_in_then_out() {
        let mut fade = HelpFade::default();
        assert_eq!(fade.opacity(true, false, 0.0), (0.0, true));
        assert_eq!(fade.opacity(true, false, 400.0), (HELP_OPACITY, false));
        let (mid, animating) = fade.opacity(true, true, 500.0);
        assert_eq!((mid, animating), (HELP_OPACITY, true));
        assert_eq!(fade.opacity(true, true, 900.0), (0.0, false));
        assert_eq!(fade.opacity(false, false, 1000.0), (0.0, false));
        assert_eq!(fade.opacity(true, false, 1100.0), (0.0, true));
    }
}
