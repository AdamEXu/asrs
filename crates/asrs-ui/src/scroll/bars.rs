//! Scrollbars as Chromium's macOS theme draws and works them: overlay ones, which show while a
//! scroller scrolls, and classic ones, which take space.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::{
    App, Bounds, Corners, DispatchPhase, Global, Hitbox, HitboxBehavior, IntoElement, MouseButton,
    MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, Styled, Window,
    canvas, fill, point, px, rgba, size,
};

use super::keys::page_step;
use crate::canvas_view::geometry::mouse_client;
use crate::input::skia;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    #[default]
    Overlay,
    Classic,
}

impl Style {
    /// What a bar takes from its scroller's content.
    pub fn reserved(self) -> f64 {
        match self {
            Style::Overlay => 0.0,
            Style::Classic => 15.0,
        }
    }

    fn thickness(self) -> f64 {
        match self {
            Style::Overlay => 16.0,
            Style::Classic => 15.0,
        }
    }

    fn min_thumb(self) -> f64 {
        match self {
            Style::Overlay => 26.0,
            Style::Classic => 20.0,
        }
    }

    /// What is left of a thumb's ends, and the least length drawn.
    fn thumb_inset(self) -> (f64, f64) {
        match self {
            Style::Overlay => (2.0, 18.0),
            Style::Classic => (3.0, 18.0),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Settings {
    style: Style,
    jump: bool,
}

impl Global for Settings {}

/// How the app's scrollbars are: overlays, unless [`set_style`] says otherwise.
pub fn style(cx: &App) -> Style {
    cx.try_global::<Settings>()
        .copied()
        .unwrap_or_default()
        .style
}

/// Sets the style, and whether a press on a track goes there rather than a page on.
pub fn set_style(style: Style, jump: bool, cx: &mut App) {
    cx.set_global(Settings { style, jump });
    cx.refresh_windows();
}

/// A scroller's content in CSS px. Its box is the element's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Extent {
    pub content: [f64; 2],
    pub offset: [f64; 2],
    /// Which axes have a bar when they overflow.
    pub axes: [bool; 2],
    /// Whether its own background is dark (`RecalculateOverlayScrollbarColorScheme`).
    pub dark: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Request {
    /// A press on a bar.
    Press,
    To([f64; 2]),
    /// With a smooth scroll.
    By([f64; 2]),
}

/// The delay and length of an overlay bar's fade, in ms.
const FADE_DELAY: f64 = 500.0;
const FADE: f64 = 250.0;
const EXPANSION: f64 = 250.0;
/// `kInitialAutoscrollTimerDelay`, and the pages a held press scrolls by a second.
const HELD_DELAY: f64 = 250.0;
const HELD_PAGES: f64 = 20.0;

/// One bar, in device px: along its axis, and from the box's far edge across it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub track: f64,
    pub thumb: f64,
    pub length: f64,
    pub max: f64,
}

/// The bars of a box of `rect` device px, where they show.
pub fn geometry(style: Style, extent: &Extent, rect: [f64; 2], scale: f64) -> [Option<Bar>; 2] {
    let content = extent.content.map(|v| (v * scale).round());
    let reserved = style.reserved() * scale;
    let mut shown = [false; 2];
    for _ in 0..2 {
        shown = [0, 1].map(|a| {
            let other = if shown[1 - a] { reserved } else { 0.0 };
            extent.axes[a] && content[a] > rect[a] - other
        });
    }
    [0, 1].map(|a| {
        if !shown[a] {
            return None;
        }
        let other = if shown[1 - a] { reserved } else { 0.0 };
        let client = rect[a] - other;
        let corner = if shown[1 - a] { style.thickness() * scale } else { 0.0 };
        let track = rect[a] - corner;
        let length = (track * client / content[a])
            .round()
            .max(style.min_thumb() * scale);
        let max = content[a] - client;
        if length > track {
            return None;
        }
        let offset = (extent.offset[a] * scale).clamp(0.0, max);
        Some(Bar {
            track,
            thumb: (offset / max * (track - length)).floor(),
            length,
            max,
        })
    })
}

impl Bar {
    fn ratio(&self) -> f64 {
        if self.track > self.length {
            self.max / (self.track - self.length)
        } else {
            0.0
        }
    }

    /// The thumb's drawn start and length.
    pub fn drawn(&self, style: Style, scale: f64) -> [f64; 2] {
        let (inset, least) = style.thumb_inset();
        let length = self.length - 2.0 * inset * scale;
        let least = least * scale;
        if length >= least {
            return [self.thumb + inset * scale, length];
        }
        let start = self.thumb + (self.length - least) / 2.0;
        [start.floor(), least]
    }
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    axis: usize,
    pointer: f64,
    offset: f64,
}

#[derive(Clone, Copy, Debug)]
struct Held {
    axis: usize,
    /// Along the bar, in device px.
    pointer: f64,
    direction: f64,
    since: f64,
    /// Where the scroll was when it began to go on, and then.
    going: Option<(f64, f64)>,
    reached: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct Activity {
    /// When the axis last scrolled or the pointer left its bar.
    at: Option<f64>,
    hovered: bool,
    over_thumb: bool,
    expanded: Option<f64>,
}

#[derive(Debug, Default)]
struct State {
    origin: Option<Instant>,
    offset: Option<[f64; 2]>,
    axes: [Activity; 2],
    drag: Option<Drag>,
    held: Option<Held>,
    /// Where the bars that show are.
    shown: [Option<Bounds<Pixels>>; 2],
    laid_out: bool,
}

/// A scroller's scrollbars.
#[derive(Clone, Debug, Default)]
pub struct Scrollbars(Rc<RefCell<State>>);

impl Scrollbars {
    /// Whether a bar shows there.
    pub fn contains(&self, position: Point<Pixels>) -> bool {
        let state = self.0.borrow();
        state.shown.iter().flatten().any(|b| b.contains(&position))
    }

    /// Whether they were laid out since the last call.
    pub fn take_laid_out(&self) -> bool {
        std::mem::take(&mut self.0.borrow_mut().laid_out)
    }
}

fn ease(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - 2.0 * (1.0 - t) * (1.0 - t)
    }
}

impl State {
    fn now(&mut self, cx: &App) -> f64 {
        let now = cx.background_executor().now();
        let origin = *self.origin.get_or_insert(now);
        now.saturating_duration_since(origin).as_secs_f64() * 1000.0
    }

    fn held(&self, axis: usize) -> bool {
        self.drag.is_some_and(|d| d.axis == axis) || self.held.is_some_and(|h| h.axis == axis)
    }

    fn alpha(&self, style: Style, axis: usize, now: f64) -> f64 {
        let activity = self.axes[axis];
        if style == Style::Classic || activity.hovered || self.held(axis) {
            return 1.0;
        }
        match activity.at {
            Some(at) => 1.0 - ease((now - at - FADE_DELAY) / FADE),
            None => 0.0,
        }
    }

    fn expansion(&self, axis: usize, now: f64) -> f64 {
        self.axes[axis]
            .expanded
            .map_or(0.0, |since| ease((now - since) / EXPANSION))
    }

    /// Notices what scrolled since the last frame.
    fn sync(&mut self, offset: [f64; 2], now: f64) {
        if let Some(before) = self.offset.replace(offset) {
            for axis in 0..2 {
                if before[axis] != offset[axis] {
                    self.axes[axis].at = Some(now);
                }
            }
        }
        for axis in 0..2 {
            if self.alpha(Style::Overlay, axis, now) <= 0.0 {
                self.axes[axis].expanded = None;
            }
        }
    }

    fn hover(&mut self, at: Option<[Option<(f64, bool)>; 2]>, now: f64) -> bool {
        let mut changed = false;
        for axis in 0..2 {
            let over = at.and_then(|at| at[axis]);
            let activity = &mut self.axes[axis];
            let (hovered, over_thumb) = (over.is_some(), over.is_some_and(|(_, thumb)| thumb));
            changed |= activity.hovered != hovered || activity.over_thumb != over_thumb;
            if activity.hovered && !hovered {
                activity.at = Some(now);
            }
            if hovered && activity.expanded.is_none() {
                activity.expanded = Some(now);
            }
            activity.hovered = hovered;
            activity.over_thumb = over_thumb;
        }
        changed
    }
}

struct Frame {
    style: Style,
    scale: f64,
    bounds: Bounds<Pixels>,
    bars: [Option<Bar>; 2],
    alpha: [f64; 2],
    expansion: [f64; 2],
    hitboxes: [Option<Hitbox>; 2],
}

impl Frame {
    /// A pointer's position along `axis`' bar in device px and whether the thumb is there.
    fn over(&self, axis: usize, position: [f64; 2]) -> Option<(f64, bool)> {
        let bar = self.bars[axis]?;
        if self.alpha[axis] <= 0.0 {
            return None;
        }
        let origin = [self.bounds.origin.x, self.bounds.origin.y].map(|v| f64::from(f32::from(v)));
        let extent = self.box_size();
        let at = [0, 1].map(|a| ((position[a] - origin[a]) * self.scale).floor());
        let across = extent[1 - axis] - self.style.thickness() * self.scale;
        let inside = at[1 - axis] >= across
            && at[1 - axis] < extent[1 - axis]
            && at[axis] >= 0.0
            && at[axis] < bar.track;
        let thumb = at[axis] >= bar.thumb && at[axis] < bar.thumb + bar.length;
        inside.then_some((at[axis], thumb))
    }

    fn box_size(&self) -> [f64; 2] {
        [self.bounds.size.width, self.bounds.size.height]
            .map(|v| (f64::from(f32::from(v)) * self.scale).round())
    }

    /// A box along and across `axis`, in device px from the bar's edges.
    fn place(&self, axis: usize, along: [f64; 2], across: [f64; 2]) -> Bounds<Pixels> {
        let start = self.box_size()[1 - axis] - self.style.thickness() * self.scale + across[0];
        let css = |v: f64| px((v / self.scale) as f32);
        let (at, extent) = if axis == 1 {
            ([start, along[0]], [across[1], along[1]])
        } else {
            ([along[0], start], [along[1], across[1]])
        };
        Bounds {
            origin: self.bounds.origin + point(css(at[0]), css(at[1])),
            size: size(css(extent[0]), css(extent[1])),
        }
    }

    fn paint(&self, state: &State, dark: bool, window: &mut Window, cx: &mut App) {
        for axis in 0..2 {
            let Some(bar) = self.bars[axis] else {
                continue;
            };
            self.paint_bar(axis, &bar, state.axes[axis].over_thumb || state.held(axis), dark, window, cx);
        }
        if self.style == Style::Classic && self.bars.iter().all(Option::is_some) {
            let [w, h] = self.box_size();
            let thickness = self.style.thickness() * self.scale;
            let line = self.scale.floor().max(1.0);
            let css = |v: f64| px((v / self.scale) as f32);
            let mut quad = |x: f64, y: f64, qw: f64, qh: f64, color: u32| {
                let bounds = Bounds {
                    origin: self.bounds.origin + point(css(x), css(y)),
                    size: size(css(qw), css(qh)),
                };
                window.paint_quad(fill(bounds, rgba(color)));
            };
            let (x, y) = (w - thickness, h - thickness);
            quad(x, y, thickness, thickness, CLASSIC_TRACK);
            quad(x, y, line, line, CLASSIC_INNER);
            quad(w - line, y, line, thickness, CLASSIC_OUTER);
            quad(x, h - line, thickness, line, CLASSIC_OUTER);
        }
    }

    fn paint_bar(
        &self,
        axis: usize,
        bar: &Bar,
        active: bool,
        dark: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let scale = self.scale;
        let alpha = self.alpha[axis];
        if alpha <= 0.0 {
            return;
        }
        let thickness = self.style.thickness() * scale;
        let line = scale.floor().max(1.0);
        let faded = |color: u32, by: f64| {
            let a = f64::from(color & 0xff) * by;
            color & 0xffff_ff00 | a.round() as u32
        };
        let (track, inner, outer, track_alpha) = match (self.style, dark) {
            (Style::Classic, _) => (CLASSIC_TRACK, CLASSIC_INNER, CLASSIC_OUTER, 1.0),
            (Style::Overlay, false) => (0xf8f8f8c6, 0xe0e0e0ff, 0xe8e8e8c6, self.expansion[axis]),
            (Style::Overlay, true) => (0xd8d8d826, 0xe3e3e333, 0xd8d8d828, self.expansion[axis]),
        };
        if track_alpha > 0.0 {
            let by = track_alpha * alpha;
            for (across, color) in [
                ([0.0, thickness], track),
                ([0.0, line], inner),
                ([thickness - line, line], outer),
            ] {
                let bounds = self.place(axis, [0.0, bar.track], across);
                window.paint_quad(fill(bounds, rgba(faded(color, by))));
            }
        }
        let width = match self.style {
            Style::Classic => 8.0,
            Style::Overlay => 7.0 + 4.0 * self.expansion[axis],
        };
        let width = (width * scale).round();
        let far = 3.0 - if self.style == Style::Overlay { 1.0 } else { 0.0 };
        let color = match (self.style, dark, active) {
            (Style::Classic, _, false) => 0x0000003a,
            (_, false, _) | (Style::Classic, ..) => 0x00000080,
            (Style::Overlay, true, _) => 0xffffff80,
        };
        let [start, length] = bar.drawn(self.style, scale);
        let bounds = self.place(axis, [start, length], [thickness - far * scale - width, width]);
        let (w, h) = if axis == 1 { (width, length) } else { (length, width) };
        let radius = w.min(h) / 2.0;
        let color = faded(color, alpha);
        if alpha < 1.0 {
            let mut quad = fill(bounds, rgba(color));
            quad.corner_radii = Corners::all(px((radius / scale) as f32));
            window.paint_quad(quad);
            return;
        }
        let draw = || {
            let body = skia::rect(0.0, 0.0, w, h, radius, &skia::paint_attrs("fill", color));
            skia::svg(w as u32, h as u32, 1.0, &body)
        };
        skia::paint(("scrollbar-thumb", w as u32, h as u32, color), bounds, draw, window, cx);
    }
}

const CLASSIC_TRACK: u32 = 0xfafafaff;
const CLASSIC_INNER: u32 = 0xe8e8e8ff;
const CLASSIC_OUTER: u32 = 0xedededff;

/// The scrollbars of the box this fills, over what is in it. Place it last in the box.
pub fn scrollbars<F: Fn(Request, &mut Window, &mut App) + 'static>(
    bars: &Scrollbars,
    extent: Extent,
    request: F,
) -> impl IntoElement + use<F> {
    let request: Rc<dyn Fn(Request, &mut Window, &mut App)> = Rc::new(request);
    let (laid_out, painted) = (bars.0.clone(), bars.0.clone());
    let advance = request.clone();
    canvas(
        move |bounds, window, cx| {
            let settings = cx.try_global::<Settings>().copied().unwrap_or_default();
            let scale = f64::from(window.scale_factor());
            let mut frame = Frame {
                style: settings.style,
                scale,
                bounds,
                bars: [None; 2],
                alpha: [0.0; 2],
                expansion: [0.0; 2],
                hitboxes: [const { None }; 2],
            };
            frame.bars = geometry(settings.style, &extent, frame.box_size(), scale);
            let mut state = laid_out.borrow_mut();
            let now = state.now(cx);
            state.sync(extent.offset, now);
            frame.alpha = [0, 1].map(|a| state.alpha(settings.style, a, now));
            frame.expansion = [0, 1].map(|a| state.expansion(a, now));
            state.laid_out = true;
            state.shown = [None; 2];
            for axis in 0..2 {
                if let Some(bar) = frame.bars[axis].filter(|_| frame.alpha[axis] > 0.0) {
                    let across = [0.0, settings.style.thickness() * scale];
                    let bounds = frame.place(axis, [0.0, bar.track], across);
                    state.shown[axis] = Some(bounds);
                    let behavior = HitboxBehavior::BlockMouseExceptScroll;
                    frame.hitboxes[axis] = Some(window.insert_hitbox(bounds, behavior));
                }
            }
            if let Some(to) = held_step(&mut state, &frame, &extent, now) {
                window.defer(cx, move |window, cx| advance(Request::To(to), window, cx));
            }
            let moving = state.held.is_some()
                || (0..2).any(|a| {
                    let idle = !state.axes[a].hovered && !state.held(a);
                    frame.bars[a].is_some()
                        && settings.style == Style::Overlay
                        && frame.alpha[a] > 0.0
                        && (idle || frame.expansion[a] < 1.0)
                });
            if moving {
                window.request_animation_frame();
            }
            frame
        },
        move |_, frame, window, cx| {
            frame.paint(&painted.borrow(), extent.dark, window, cx);
            listen(Rc::new(frame), painted, extent, request, window);
        },
    )
    .absolute()
    .inset_0()
}

/// A frame of a press that is held on a track: where it scrolls to.
fn held_step(state: &mut State, frame: &Frame, extent: &Extent, now: f64) -> Option<[f64; 2]> {
    let mut held = state.held?;
    let Some(bar) = frame.bars[held.axis] else {
        state.held = None;
        return None;
    };
    if now - held.since < HELD_DELAY || held.reached {
        return None;
    }
    let axis = held.axis;
    let client = (extent.content[axis] * frame.scale - bar.max) / frame.scale;
    let (from, began) = *held.going.get_or_insert((extent.offset[axis], now));
    let speed = page_step(client, frame.scale) * HELD_PAGES / 1000.0;
    let max = bar.max / frame.scale;
    let to = (from + held.direction * speed * (now - began)).clamp(0.0, max);
    held.reached = held.pointer >= bar.thumb - 1.0 && held.pointer < bar.thumb + bar.length + 1.0;
    state.held = Some(held);
    let mut offset = extent.offset;
    offset[axis] = to;
    Some(offset)
}

fn listen(
    frame: Rc<Frame>,
    state: Rc<RefCell<State>>,
    extent: Extent,
    request: Rc<dyn Fn(Request, &mut Window, &mut App)>,
    window: &mut Window,
) {
    let client = |p: Point<Pixels>| [p.x, p.y].map(|v| mouse_client(f32::from(v)));
    let scrolls = move |bar: &Bar, axis: usize, scale: f64| {
        (extent.content[axis] * scale - bar.max) / scale
    };
    window.on_mouse_event({
        let (frame, state, request) = (frame.clone(), state.clone(), request.clone());
        move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                return;
            }
            for axis in 0..2 {
                let hovered = frame.hitboxes[axis]
                    .as_ref()
                    .is_some_and(|hitbox| hitbox.is_hovered(window));
                let over = frame.over(axis, client(event.position));
                let (Some((at, thumb)), Some(bar), true) = (over, frame.bars[axis], hovered) else {
                    continue;
                };
                cx.stop_propagation();
                window.prevent_default();
                request(Request::Press, window, cx);
                let jump = cx.try_global::<Settings>().is_some_and(|s| s.jump);
                let jump = jump != event.modifiers.alt;
                let offset = extent.offset[axis];
                let mut to = extent.offset;
                let mut state = state.borrow_mut();
                let pointer = client(event.position)[axis];
                if thumb {
                    state.drag = Some(Drag { axis, pointer, offset });
                } else if jump {
                    let half = (bar.length / 2.0).floor();
                    let delta = (at - bar.thumb - half) * bar.ratio() / frame.scale;
                    to[axis] = (offset + delta).clamp(0.0, bar.max / frame.scale);
                    state.drag = Some(Drag { axis, pointer, offset: to[axis] });
                    drop(state);
                    request(Request::To(to), window, cx);
                } else {
                    let direction = if at < bar.thumb { -1.0 } else { 1.0 };
                    let since = state.now(cx);
                    state.held = Some(Held {
                        axis,
                        pointer: at,
                        direction,
                        since,
                        going: None,
                        reached: false,
                    });
                    drop(state);
                    let mut by = [0.0; 2];
                    by[axis] = direction * page_step(scrolls(&bar, axis, frame.scale), frame.scale);
                    request(Request::By(by), window, cx);
                }
                window.refresh();
            }
        }
    });
    window.on_mouse_event({
        let (frame, state) = (frame.clone(), state.clone());
        move |event: &MouseMoveEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture {
                return;
            }
            let mut inner = state.borrow_mut();
            if let Some(drag) = inner.drag {
                if event.pressed_button != Some(MouseButton::Left) {
                    inner.drag = None;
                } else if let Some(bar) = frame.bars[drag.axis] {
                    let moved = client(event.position)[drag.axis] - drag.pointer;
                    let mut to = extent.offset;
                    let max = bar.max / frame.scale;
                    to[drag.axis] = (drag.offset + moved * bar.ratio()).clamp(0.0, max);
                    drop(inner);
                    cx.stop_propagation();
                    request(Request::To(to), window, cx);
                    return;
                }
            }
            if inner.held.is_some() && event.pressed_button != Some(MouseButton::Left) {
                inner.held = None;
            }
            let over = [0, 1].map(|axis| {
                let hovered = frame.hitboxes[axis]
                    .as_ref()
                    .is_some_and(|hitbox| hitbox.is_hovered(window));
                frame
                    .over(axis, client(event.position))
                    .filter(|_| hovered && event.pressed_button.is_none())
            });
            let now = inner.now(cx);
            if inner.hover(Some(over), now) {
                window.refresh();
            }
        }
    });
    window.on_mouse_event({
        let state = state.clone();
        move |event: &MouseUpEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture || event.button != MouseButton::Left {
                return;
            }
            let mut state = state.borrow_mut();
            let axis = state.drag.take().map(|d| d.axis);
            let axis = axis.or(state.held.take().map(|h| h.axis));
            if let Some(axis) = axis {
                let now = state.now(cx);
                state.axes[axis].at = Some(now);
                window.refresh();
            }
        }
    });
    window.on_mouse_event(move |_: &MouseExitEvent, phase, window, cx| {
        if phase == DispatchPhase::Capture {
            let mut state = state.borrow_mut();
            let now = state.now(cx);
            if state.hover(None, now) {
                window.refresh();
            }
        }
    });
}
