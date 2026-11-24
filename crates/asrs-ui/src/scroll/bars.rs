//! Scrollbars as Chromium draws and works them: macOS's overlay ones, which show while a scroller
//! scrolls, its classic ones, which take space, and the Fluent ones of the other platforms.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::{
    App, Bounds, Corners, DispatchPhase, Global, Hitbox, HitboxBehavior, IntoElement, MouseButton,
    MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollEvent,
    Styled, Window, canvas, fill, point, px, rgba, size,
};

use super::keys::{LINE, page_step};
use crate::canvas_view::geometry::mouse_client;
use crate::input::skia;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Style {
    #[default]
    Overlay,
    Classic,
    Fluent,
}

impl Style {
    /// The platform's style, where only macOS has overlay scrollbars.
    pub fn of(classic: bool) -> Style {
        match (cfg!(target_os = "macos"), classic) {
            (true, false) => Style::Overlay,
            (true, true) => Style::Classic,
            (false, _) => Style::Fluent,
        }
    }

    /// What a bar takes from its scroller's content.
    pub fn reserved(self) -> f64 {
        match self {
            Style::Overlay => 0.0,
            Style::Classic | Style::Fluent => 15.0,
        }
    }

    fn thickness(self) -> f64 {
        match self {
            Style::Overlay => 16.0,
            Style::Classic | Style::Fluent => 15.0,
        }
    }

    /// The length of the button at each end.
    fn button(self) -> f64 {
        match self {
            Style::Fluent => 18.0,
            _ => 0.0,
        }
    }

    fn min_thumb(self) -> f64 {
        match self {
            Style::Overlay => 26.0,
            Style::Classic => 20.0,
            Style::Fluent => 17.0,
        }
    }

    /// What is left of a thumb's ends, and the least length drawn.
    fn thumb_inset(self) -> (f64, f64) {
        match self {
            Style::Overlay => (2.0, 18.0),
            Style::Classic => (3.0, 18.0),
            Style::Fluent => (0.0, 0.0),
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

/// Takes the system's style, and follows it.
pub fn use_system_style(cx: &mut App) {
    take_system_style(cx);
    crate::platform::observe_scrollbar_preferences(cx, take_system_style);
}

fn take_system_style(cx: &mut App) {
    let (classic, jump) = crate::platform::scrollbar_preferences();
    set_style(Style::of(classic), jump, cx);
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

type Requests = Rc<dyn Fn(Request, &mut Window, &mut App)>;

/// The delay and length of an overlay bar's fade, in ms.
const FADE_DELAY: f64 = 500.0;
const FADE: f64 = 250.0;
const EXPANSION: f64 = 250.0;
/// `kInitialAutoscrollTimerDelay`, and the steps a held press scrolls by a second.
const HELD_DELAY: f64 = 250.0;
const HELD_STEPS: f64 = 20.0;

/// One bar, in device px: along its axis, and from the box's far edge across it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bar {
    pub track: f64,
    /// Its buttons' length.
    pub button: f64,
    pub thumb: f64,
    /// The thumb's length, which is 0 where it has no room.
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
        let corner = if shown[1 - a] {
            style.thickness() * scale
        } else {
            0.0
        };
        let track = rect[a] - corner;
        if track <= 0.0 {
            return None;
        }
        let button = (style.button() * scale).min((track / 2.0).floor());
        let inner = track - 2.0 * button;
        let length = (inner * client / content[a])
            .round()
            .max(style.min_thumb() * scale);
        let length = match style {
            // The themes do not scale the room a thumb needs.
            Style::Overlay | Style::Classic if inner <= style.min_thumb() => 0.0,
            _ => length.min(inner),
        };
        let max = content[a] - client;
        let offset = (extent.offset[a] * scale).clamp(0.0, max);
        Some(Bar {
            track,
            button,
            thumb: button + (offset / max * (inner - length)).floor(),
            length,
            max,
        })
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Part {
    Back,
    Before,
    Thumb,
    After,
    Forward,
}

impl Bar {
    fn part(&self, at: f64) -> Part {
        match at {
            at if at < self.button => Part::Back,
            at if at >= self.track - self.button => Part::Forward,
            at if at < self.thumb => Part::Before,
            at if at < self.thumb + self.length => Part::Thumb,
            _ => Part::After,
        }
    }

    fn ratio(&self) -> f64 {
        let travel = self.track - 2.0 * self.button - self.length;
        if travel > 0.0 { self.max / travel } else { 0.0 }
    }

    /// The thumb's drawn start and length.
    pub fn drawn(&self, style: Style, scale: f64) -> [f64; 2] {
        let (inset, least) = style.thumb_inset();
        let length = self.length - 2.0 * inset * scale;
        let least = (least * scale).min(self.length);
        if length >= least {
            return [self.thumb + inset * scale, length];
        }
        let start = self.thumb + (self.length - least) / 2.0;
        [start.round(), least]
    }
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    axis: usize,
    pointer: f64,
    offset: f64,
    /// The largest offset as of the last move.
    max: f64,
}

#[derive(Clone, Copy, Debug)]
struct Held {
    axis: usize,
    part: Part,
    /// Along the bar, in device px, while the pointer is on the part.
    pointer: Option<f64>,
    /// What a press scrolls by, in CSS px.
    step: f64,
    /// When it goes on.
    due: f64,
    /// Where the scroll was when it last began to go on, and then.
    going: Option<(f64, f64)>,
    /// The last frame that went on.
    frame: Option<f64>,
    reached: bool,
}

impl Held {
    fn direction(&self) -> f64 {
        match self.part {
            Part::Back | Part::Before => -1.0,
            _ => 1.0,
        }
    }

    fn on_button(&self) -> bool {
        matches!(self.part, Part::Back | Part::Forward)
    }

    /// It waits while the pointer is elsewhere, and goes on as the pointer comes back.
    fn pointed(&mut self, bar: &Bar, at: Option<f64>, now: f64) {
        let on = at.filter(|at| bar.part(*at) == self.part);
        if on.is_none() {
            self.going = None;
        } else if self.pointer.is_none() {
            self.due = now;
        }
        self.pointer = on;
        self.reached = false;
    }
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
        if style != Style::Overlay || activity.hovered || self.held(axis) {
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

    /// [`Frame::over`] of each bar that nothing covers there.
    fn hovered(&self, position: [f64; 2], window: &Window) -> [Option<(f64, bool)>; 2] {
        [0, 1].map(|axis| {
            let hitbox = self.hitboxes[axis].as_ref();
            self.over(axis, position)
                .filter(|_| hitbox.is_some_and(|h| h.is_hovered(window)))
        })
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
            self.paint_bar(axis, &bar, state, dark, window, cx);
        }
        if self.style == Style::Fluent && self.bars.iter().all(Option::is_some) {
            let thickness = self.style.thickness() * self.scale;
            let end = self.box_size()[1];
            let bounds = self.place(1, [end - thickness, thickness], [0.0, thickness]);
            window.paint_quad(fill(bounds, rgba(FLUENT_TRACK)));
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

    fn paint_arrow(&self, axis: usize, bar: &Bar, end: usize, pressed: bool, window: &mut Window) {
        let scale = self.scale;
        let color = if pressed {
            FLUENT_PRESSED
        } else {
            FLUENT_THUMB
        };
        let side = ((if pressed { 7.0 } else { 9.0 }) * scale).ceil();
        let rows = (side / 2.0).ceil();
        // `NativeThemeFluent::GetArrowRect` without its icon font, and `PathForArrow` in it.
        let outward = scale.round() * [-1.0, 1.0][end];
        let square = ((bar.button - side) / 2.0).floor() + outward;
        let height = (side / 2.0).floor() + 1.0;
        let below = if axis == 1 { 1.0 } else { 0.0 };
        let base = square + side - (height / 2.0).floor() + below;
        let start = [base - rows, 2.0 * square + side - base][end];
        let top = start + [0.0, bar.track - bar.button][end];
        let thickness = self.style.thickness() * scale;
        for row in 0..rows as usize {
            let wide = if end == 0 {
                rows - 1.0 - row as f64
            } else {
                row as f64
            };
            let width = side - 2.0 * wide;
            let across = [((thickness - width) / 2.0).floor(), width];
            let bounds = self.place(axis, [top + row as f64, 1.0], across);
            window.paint_quad(fill(bounds, rgba(color)));
        }
    }

    fn paint_bar(
        &self,
        axis: usize,
        bar: &Bar,
        state: &State,
        dark: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let scale = self.scale;
        let active = state.axes[axis].over_thumb || state.held(axis);
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
            (Style::Fluent, _) => (FLUENT_TRACK, FLUENT_TRACK, FLUENT_TRACK, 1.0),
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
            Style::Fluent => 9.0,
            Style::Overlay => 7.0 + 4.0 * self.expansion[axis],
        };
        let width = (width * scale).round();
        let far = 3.0
            - if self.style == Style::Overlay {
                1.0
            } else {
                0.0
            };
        let color = match (self.style, dark, active) {
            (Style::Fluent, ..) if state.drag.is_some_and(|d| d.axis == axis) => FLUENT_PRESSED,
            (Style::Fluent, ..) => FLUENT_THUMB,
            (Style::Classic, _, false) => 0x0000003a,
            (_, false, _) | (Style::Classic, ..) => 0x00000080,
            (Style::Overlay, true, _) => 0xffffff80,
        };
        if self.style == Style::Fluent {
            for (end, button) in [(0, Part::Back), (1, Part::Forward)] {
                let pressed = state
                    .held
                    .is_some_and(|h| h.axis == axis && h.part == button);
                self.paint_arrow(axis, bar, end, pressed, window);
            }
        }
        if bar.length == 0.0 {
            return;
        }
        let [start, length] = bar.drawn(self.style, scale);
        let bounds = self.place(
            axis,
            [start, length],
            [thickness - far * scale - width, width],
        );
        let (w, h) = if axis == 1 {
            (width, length)
        } else {
            (length, width)
        };
        let radius = w.min(h) / 2.0;
        let color = faded(color, alpha);
        // Ganesh covers a corner by its distance, as GPUI's quads do.
        if alpha < 1.0 || self.style == Style::Fluent {
            let mut quad = fill(bounds, rgba(color));
            quad.corner_radii = Corners::all(px((radius / scale) as f32));
            window.paint_quad(quad);
            return;
        }
        let draw = || {
            let body = skia::rect(0.0, 0.0, w, h, radius, &skia::paint_attrs("fill", color));
            skia::svg(w as u32, h as u32, 1.0, &body)
        };
        skia::paint(
            ("scrollbar-thumb", w as u32, h as u32, color),
            bounds,
            draw,
            window,
            cx,
        );
    }
}

const FLUENT_TRACK: u32 = 0xfcfcfcff;
const FLUENT_THUMB: u32 = 0x8b8b8bff;
const FLUENT_PRESSED: u32 = 0x636363ff;
const CLASSIC_TRACK: u32 = 0xfafafaff;
const CLASSIC_INNER: u32 = 0xe8e8e8ff;
const CLASSIC_OUTER: u32 = 0xedededff;

/// The scrollbars of the box this fills, over what is in it. Place it last in the box.
pub fn scrollbars<F: Fn(Request, &mut Window, &mut App) + 'static>(
    bars: &Scrollbars,
    extent: Extent,
    request: F,
) -> impl IntoElement + use<F> {
    let request: Requests = Rc::new(request);
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
    if now < held.due || held.reached || held.frame == Some(now) {
        return None;
    }
    let pointer = held.pointer?;
    let axis = held.axis;
    held.frame = Some(now);
    let (from, began) = *held.going.get_or_insert((extent.offset[axis], now));
    let speed = held.step * HELD_STEPS / 1000.0;
    let max = bar.max / frame.scale;
    let to = (from + held.direction() * speed * (now - began)).clamp(0.0, max);
    held.reached =
        !held.on_button() && pointer >= bar.thumb - 1.0 && pointer < bar.thumb + bar.length + 1.0;
    state.held = Some(held);
    let mut offset = extent.offset;
    offset[axis] = to;
    Some(offset)
}

fn listen(
    frame: Rc<Frame>,
    state: Rc<RefCell<State>>,
    extent: Extent,
    request: Requests,
    window: &mut Window,
) {
    let client = |p: Point<Pixels>| [p.x, p.y].map(|v| mouse_client(f32::from(v)));
    let scrolls =
        move |bar: &Bar, axis: usize, scale: f64| (extent.content[axis] * scale - bar.max) / scale;
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
                if bar.length == 0.0 && frame.style == Style::Overlay {
                    return;
                }
                if thumb {
                    state.drag = Some(Drag {
                        axis,
                        pointer,
                        offset,
                        max: bar.max,
                    });
                } else if jump && bar.button == 0.0 {
                    let half = (bar.length / 2.0).floor();
                    let delta = (at - bar.thumb - half) * bar.ratio() / frame.scale;
                    to[axis] = (offset + delta).clamp(0.0, bar.max / frame.scale);
                    state.drag = Some(Drag {
                        axis,
                        pointer,
                        offset: to[axis],
                        max: bar.max,
                    });
                    drop(state);
                    request(Request::To(to), window, cx);
                } else {
                    let mut held = Held {
                        axis,
                        part: bar.part(at),
                        pointer: Some(at),
                        step: LINE,
                        due: state.now(cx) + HELD_DELAY,
                        going: None,
                        frame: None,
                        reached: false,
                    };
                    if !held.on_button() {
                        held.step = page_step(scrolls(&bar, axis, frame.scale), frame.scale);
                    }
                    state.held = Some(held);
                    drop(state);
                    let mut by = [0.0; 2];
                    by[axis] = held.direction() * held.step;
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
            if let Some(mut drag) = inner.drag {
                if event.pressed_button != Some(MouseButton::Left) {
                    inner.drag = None;
                } else if let Some(bar) = frame.bars[drag.axis] {
                    let pointer = client(event.position)[drag.axis];
                    if drag.max != bar.max {
                        drag.max = bar.max;
                        drag.pointer = pointer;
                        drag.offset = extent.offset[drag.axis];
                        inner.drag = Some(drag);
                        cx.stop_propagation();
                        return;
                    }
                    let moved = pointer - drag.pointer;
                    let mut to = extent.offset;
                    let max = bar.max / frame.scale;
                    to[drag.axis] = (drag.offset + moved * bar.ratio()).clamp(0.0, max);
                    drop(inner);
                    cx.stop_propagation();
                    request(Request::To(to), window, cx);
                    return;
                }
            }
            if let Some(mut held) = inner.held {
                inner.held = match (event.pressed_button, &frame.bars[held.axis]) {
                    (Some(MouseButton::Left), Some(bar)) => {
                        let over = frame.over(held.axis, client(event.position));
                        let now = inner.now(cx);
                        held.pointed(bar, over.map(|(at, _)| at), now);
                        Some(held)
                    }
                    _ => None,
                };
            }
            if event.pressed_button.is_some() {
                return;
            }
            let over = frame.hovered(client(event.position), window);
            let now = inner.now(cx);
            if inner.hover(Some(over), now) {
                window.refresh();
            }
        }
    });
    window.on_mouse_event({
        let (frame, state) = (frame.clone(), state.clone());
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
                let over = frame.hovered(client(event.position), window);
                state.hover(Some(over), now);
                window.refresh();
            }
        }
    });
    window.on_mouse_event({
        let (frame, state) = (frame.clone(), state.clone());
        move |event: &ScrollEvent, phase, _, _| {
            if phase == DispatchPhase::Capture && frame.bounds.contains(&event.position) {
                state.borrow_mut().drag = None;
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
