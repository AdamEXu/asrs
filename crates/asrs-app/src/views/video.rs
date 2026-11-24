//! The Video tab: its renderer element (`#renderer7` in `www/hub.html`, styled by
//! `www/renderers.css`), which paints the frame image `VideoRenderer` points its `<img>` at, and
//! the controller's controls (`#controller7`, `www/hub.css`): the source buttons, the lock
//! buttons, the timeline slider and the transport buttons.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use asrs_canvas::{Canvas2d, Pixmap, Raster};
use asrs_hub::hub::{HubModel, HubPage};
use asrs_js::math;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::selection_impl::KeyTarget;
use asrs_tabs::form::RangeInput;
use asrs_tabs::video::{Buttons, VideoController};
use asrs_tabs::video_renderer::{Shown, VideoImage, VideoRenderer};
use asrs_ui::button::IconButton;
use asrs_ui::canvas_view::FramePresenter;
use asrs_ui::gpui::{
    AnyElement, AppContext, Bounds, Context, Corners, Entity, IntoElement, KeyDownEvent,
    ParentElement, Pixels, Styled, Subscription, Task, Window, canvas, div, point, px, rgb, size,
};
use asrs_ui::icon::IconFit;
use asrs_ui::input::{Slider, SliderEvent, SliderStyle, paint_in_rounded};
use asrs_ui::parity::probed;
use asrs_ui::text::{self, TextStyle, WhiteSpace};
use asrs_ui::theme::{Appearance, FONT_FAMILY, FONT_SIZE, IconTint, chains};
use asrs_ui::tooltip::titled;

use super::frame::{FrameHost, HubFrame};
use super::large_table::symbol;
use super::range::FormRange;
use super::video_frame;

const BUTTON: f64 = 30.0;
const SLIDER_HEIGHT: f64 = 25.0;
const TIMELINE_RADIUS: f64 = 5.0;
const TIMELINE_LEFT: f64 = 40.0;
const TIMELINE_INSET: f64 = 5.0;
const MARKER_BACKGROUND: u32 = 0x888888;
const MARKER: u32 = 0x0000ff;

/// `VideoController.BUTTON_BORDER_RADIUS`.
const BUTTON_RADIUS: f64 = 6.0;
/// `stroke-width` of an animating source button's path.
const STROKE: f64 = 8.0;
/// `transition: stroke-width 0.3s`.
const TRANSITION_MS: f64 = 300.0;
const ANIMATION_MS: f64 = 1000.0;

/// `div.video-source > button > img`.
const SOURCE_FIT: IconFit = IconFit::Centered {
    max: 0.5,
    scale: 1.2,
};
const SOURCE_TINT: IconTint = IconTint {
    normal: [chains::VIDEO_SOURCE; 2],
    blurred: [chains::VIDEO_SOURCE; 2],
    disabled: [chains::VIDEO_SOURCE; 2],
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Source {
    Local,
    YouTube,
    Tba,
}

impl Source {
    const ALL: [Source; 3] = [Source::Local, Source::YouTube, Source::Tba];

    fn id(self) -> &'static str {
        match self {
            Source::Local => "video-local",
            Source::YouTube => "video-youtube",
            Source::Tba => "video-tba",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Source::Local => "Open local video file",
            Source::YouTube => "Paste YouTube video URL",
            Source::Tba => "Download match video from The Blue Alliance",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Source::Local => "videofile.svg",
            Source::YouTube => "contentpaste.svg",
            Source::Tba => "tbalamp.svg",
        }
    }

    fn backgrounds(self) -> [u32; 3] {
        match self {
            Source::Local => [0x666666, 0x565656, 0x464646],
            Source::YouTube => [0xff0000, 0xe00000, 0xc00000],
            Source::Tba => [0x4556a5, 0x354695, 0x253685],
        }
    }

    fn stroke(self) -> &'static str {
        match self {
            Source::Local => "none",
            Source::YouTube => "#ffb8b8",
            Source::Tba => "#9aacff",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Control {
    SkipBack,
    FrameBack,
    Play,
    Pause,
    FrameForward,
    SkipForward,
    Lock,
    Unlock,
}

impl Control {
    const TRANSPORT: [Control; 6] = [
        Control::SkipBack,
        Control::FrameBack,
        Control::Play,
        Control::Pause,
        Control::FrameForward,
        Control::SkipForward,
    ];

    fn id(self) -> &'static str {
        match self {
            Control::SkipBack => "video-skip-back",
            Control::FrameBack => "video-frame-back",
            Control::Play => "video-play",
            Control::Pause => "video-pause",
            Control::FrameForward => "video-frame-forward",
            Control::SkipForward => "video-skip-forward",
            Control::Lock => "video-lock",
            Control::Unlock => "video-unlock",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Control::SkipBack => "Go back 5 seconds",
            Control::FrameBack => "Previous frame",
            Control::Play => "Play video",
            Control::Pause => "Pause video",
            Control::FrameForward => "Next frame",
            Control::SkipForward => "Go forward 5 seconds",
            Control::Lock => "Lock video timeline synchronization",
            Control::Unlock => "Unlock video timeline synchronization",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Control::SkipBack => "replay5.svg",
            Control::FrameBack => "skipprevious.svg",
            Control::Play => "play.svg",
            Control::Pause => "pause.svg",
            Control::FrameForward => "skipnext.svg",
            Control::SkipForward => "forward5.svg",
            Control::Lock => "lockopenright.svg",
            Control::Unlock => "lock.svg",
        }
    }

    fn hidden(self, buttons: Buttons) -> bool {
        match self {
            Control::Play => buttons.play_hidden,
            Control::Pause => buttons.pause_hidden,
            Control::Lock => buttons.lock_hidden,
            Control::Unlock => buttons.unlock_hidden,
            _ => false,
        }
    }

    fn disabled(self, buttons: Buttons) -> bool {
        match self {
            Control::Lock => buttons.lock_disabled,
            Control::Unlock => buttons.unlock_disabled,
            _ => buttons.controls_disabled,
        }
    }

    fn click(self, controller: &mut VideoController, page: &mut HubPage) {
        match self {
            Control::SkipBack => controller.skip_time(page, -5.0),
            Control::FrameBack => controller.change_frame(-1.0),
            Control::Play | Control::Pause => controller.toggle_play_pause(page),
            Control::FrameForward => controller.change_frame(1.0),
            Control::SkipForward => controller.skip_time(page, 5.0),
            Control::Lock | Control::Unlock => controller.toggle_lock(page),
        }
    }
}

/// A length in LayoutUnits, truncated as `LayoutUnit(float)` truncates.
fn lu(v: f64) -> f64 {
    math::trunc(v * 64.0) / 64.0
}

/// `percent` of `v` as `MinimumValueForLength` resolves it, in single precision.
fn percent(v: f64, percent: f64) -> f64 {
    lu(f64::from((v * percent / 100.0) as f32))
}

/// The device px of a laid out CSS length, which is a whole number of LayoutUnits (1/64 device px)
/// but for the division by the scale.
fn device(v: f64, scale: f64) -> f64 {
    math::floor(v * scale * 64.0 + 0.5) / 64.0
}

/// `v` on the device pixel grid, in CSS px.
fn snap(v: f64, scale: f64) -> f64 {
    math::floor(device(v, scale) + 0.5) / scale
}

/// The width of a 1 px border, in CSS px: Blink floors it to whole device px, at least one.
fn border(scale: f64) -> f64 {
    math::max2(math::floor(scale), 1.0) / scale
}

/// Where the controls sit in `div.controls-content`, in client coordinates.
#[derive(Clone, Debug, PartialEq)]
struct ControlsLayout {
    /// The border box, whose right edge is the `border-right`.
    source: [f64; 4],
    sources: [[f64; 4]; 3],
    lock: [f64; 4],
    timeline: [f64; 4],
    slider: [f64; 4],
    marker: [f64; 4],
    /// One per shown transport button.
    transport: Vec<[f64; 4]>,
}

impl ControlsLayout {
    /// `space` is the width of the white space between two inline buttons.
    fn new(controls: [f64; 4], transport: usize, space: f64, scale: f64) -> ControlsLayout {
        let [x, y, w, h] = controls.map(|v| device(v, scale));
        let space = device(space, scale);
        let [button, inset, timeline_left, slider_height] =
            [BUTTON, TIMELINE_INSET, TIMELINE_LEFT, SLIDER_HEIGHT]
                .map(|v| lu(f64::from((v * scale) as f32)));
        let css = |rect: [f64; 4]| rect.map(|v| v / scale);

        let source_width = percent(w, 30.0);
        let button_width = percent(source_width, 80.0 / 3.0);
        let button_top = y + percent(h, 15.0);
        let sources = std::array::from_fn(|i| {
            let i = i as f64;
            let left = percent(source_width, 5.0 * (i + 1.0) + 80.0 / 3.0 * i);
            css([x + left, button_top, button_width, percent(h, 70.0)])
        });
        let sync = x + percent(w, 30.0);
        let sync_width = percent(w, 70.0);
        let half = percent(h, 50.0);
        let middle = percent(half, 50.0) - button / 2.0;
        let timeline = [
            sync + timeline_left,
            y + inset,
            math::max2(sync_width - timeline_left - inset, 0.0),
            math::max2(half - 2.0 * inset, 0.0),
        ];
        let line = button * transport as f64 + space * (transport as f64 - 1.0);
        let first = sync + lu(math::max2(sync_width - line, 0.0) / 2.0);
        ControlsLayout {
            source: css([x, y, source_width + border(scale) * scale, h]),
            sources,
            lock: css([sync + inset, y + middle, button, button]),
            timeline: css(timeline),
            slider: css([timeline[0], timeline[1], timeline[2], slider_height]),
            marker: css([
                timeline[0],
                timeline[1] + slider_height,
                timeline[2],
                math::max2(timeline[3] - slider_height, 0.0),
            ]),
            transport: (0..transport)
                .map(|i| {
                    let left = first + (button + space) * i as f64;
                    css([left, y + half + middle, button, button])
                })
                .collect(),
        }
    }
}

/// The used width of `VIDEO_TIMELINE_PROGRESS` in a container `width` px wide. Without a
/// `width` the empty box shrinks to nothing.
fn progress_width(style: &JsString, width: f64, scale: f64) -> f64 {
    let style = style.to_string_lossy();
    let share = style.strip_suffix('%').and_then(|v| v.parse::<f64>().ok());
    share.map_or(0.0, |share| {
        math::max2(percent(device(width, scale), share), 0.0) / scale
    })
}

/// The stroke of a source button, as its transition and animation leave it.
#[derive(Clone, Copy, Debug, Default)]
struct Stroke {
    animating: bool,
    /// The transition's start on the frame clock, and the width it started from.
    changed: f64,
    from: f64,
    /// When the animation started: at the button's first layout.
    started: Option<f64>,
}

impl Stroke {
    fn width(&self, now: f64) -> f64 {
        let target = if self.animating { STROKE } else { 0.0 };
        let t = ((now - self.changed) / TRANSITION_MS).clamp(0.0, 1.0);
        self.from + (target - self.from) * ease(t)
    }

    fn set(&mut self, animating: bool, now: f64) {
        self.started.get_or_insert(now);
        if animating != self.animating {
            self.from = self.width(now);
            self.changed = now;
            self.animating = animating;
        }
    }

    fn moving(&self, now: f64) -> bool {
        self.animating || self.width(now) > 0.0
    }

    /// `strokeDashoffset`, animated in reverse from the path's length to 0, over that length.
    fn offset(&self, now: f64) -> f64 {
        let elapsed = now - self.started.unwrap_or(now);
        1.0 - (elapsed.max(0.0) % ANIMATION_MS) / ANIMATION_MS
    }
}

/// CSS `ease`, `cubic-bezier(0.25, 0.1, 0.25, 1)`.
fn ease(x: f64) -> f64 {
    if x <= 0.0 || x >= 1.0 {
        return x.clamp(0.0, 1.0);
    }
    let bezier = |t: f64, a: f64, b: f64| {
        let u = 1.0 - t;
        3.0 * u * u * t * a + 3.0 * u * t * t * b + t * t * t
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..40 {
        let t = (low + high) / 2.0;
        if bezier(t, 0.25, 0.25) < x {
            low = t;
        } else {
            high = t;
        }
    }
    bezier((low + high) / 2.0, 0.1, 1.0)
}

fn rounded_path(canvas: &mut Canvas2d, width: f64, height: f64) {
    let r = BUTTON_RADIUS;
    let quarter = std::f64::consts::PI / 2.0;
    canvas.begin_path();
    canvas.move_to(width - r, 0.0);
    canvas.arc(width - r, r, r, -quarter, 0.0);
    canvas.line_to(width, height - r);
    canvas.arc(width - r, height - r, r, 0.0, quarter);
    canvas.line_to(r, height);
    canvas.arc(r, height - r, r, quarter, 2.0 * quarter);
    canvas.line_to(0.0, r);
    canvas.arc(r, r, r, 2.0 * quarter, 3.0 * quarter);
    canvas.close_path();
}

/// A source button's path inside its `overflow: hidden` box, at `scale`.
fn stroke_image(
    [width, height]: [f64; 2],
    color: &str,
    stroke: f64,
    offset: f64,
    scale: f64,
) -> Option<Pixmap> {
    let device = |v: f64| math::round(v * scale).max(0.0) as u32;
    let mut canvas = Canvas2d::new(device(width), device(height));
    canvas.scale(scale, scale);
    rounded_path(&mut canvas, width, height);
    canvas.clip();
    let straight = 2.0 * (width + height) - 8.0 * BUTTON_RADIUS;
    let length = straight + 2.0 * std::f64::consts::PI * BUTTON_RADIUS;
    canvas.set_stroke_style(color);
    canvas.set_line_width(stroke);
    canvas.set_line_dash(&[length / 4.0]);
    canvas.set_line_dash_offset(offset * length);
    rounded_path(&mut canvas, width, height);
    canvas.stroke();
    let mut raster = Raster::new(canvas.width, canvas.height)?;
    raster.run(&canvas.calls);
    Some(raster.into_pixmap())
}

#[derive(Default)]
struct SourceStroke {
    stroke: Stroke,
    presenter: FramePresenter,
}

/// The decoded frame the tab paints.
struct Decoded {
    src: JsString,
    image: Arc<Pixmap>,
    /// The frame resampled for its size on screen when it was requested, until it is presented.
    fitted: Option<Fitted>,
}

struct Fitted {
    size: [i32; 2],
    bgra: Vec<u8>,
}

impl Fitted {
    fn new(image: &Pixmap, size: [i32; 2]) -> Option<Fitted> {
        let [width, height] = size.map(|v| u32::try_from(v).ok().filter(|v| *v > 0));
        let bgra = video_frame::resample(image, width?, height?)?;
        Some(Fitted { size, bgra })
    }
}

#[derive(Default)]
struct TabView {
    presenter: FramePresenter,
    decoded: Option<Decoded>,
    presented: Option<(JsString, [i32; 2])>,
    load: Option<Task<()>>,
    wanted: Option<JsString>,
    /// `div.renderer-content` and the window's scale when the tab was last displayed.
    place: Option<([f64; 4], f64)>,
    strokes: [SourceStroke; 2],
    slider: Option<SliderView>,
}

impl TabView {
    fn presenters(&mut self) -> impl Iterator<Item = &mut FramePresenter> {
        let strokes = self.strokes.iter_mut().map(|s| &mut s.presenter);
        std::iter::once(&mut self.presenter).chain(strokes)
    }
}

struct SliderView {
    slider: Entity<Slider<FormRange>>,
    _subscription: Subscription,
}

#[derive(Default)]
pub struct VideoViews {
    tabs: HashMap<usize, TabView>,
}

/// A tab by the address of its renderer, which stays put while the tab exists; a satellite's has
/// no controller.
fn renderer_key(renderer: &VideoRenderer) -> usize {
    std::ptr::from_ref(renderer) as usize
}

/// Every Video tab's index and key, and whether it is displayed.
fn video_tabs(hub: &HubModel) -> Vec<(usize, usize, bool)> {
    hub.tabs()
        .tabs()
        .enumerate()
        .filter_map(|(i, tab)| {
            let renderer = tab.renderer().downcast_ref::<VideoRenderer>()?;
            Some((i, renderer_key(renderer), !tab.content_hidden()))
        })
        .collect()
}

fn displayed(hub: &HubModel) -> Option<(usize, usize)> {
    video_tabs(hub)
        .into_iter()
        .find_map(|(index, key, shown)| shown.then_some((index, key)))
}

fn tab_index(hub: &HubModel, key: usize) -> Option<usize> {
    video_tabs(hub)
        .into_iter()
        .find_map(|(index, k, _)| (k == key).then_some(index))
}

fn with_controller<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut VideoController, &mut HubPage, &JsValue) -> R,
) -> Option<R> {
    let (tabs, page) = hub.tabs_mut();
    let tab = tabs.tab_mut(index)?;
    let uuid = tab.uuid.clone();
    let controller = tab.controller.downcast_mut::<VideoController>()?;
    Some(f(controller, page, &uuid))
}

fn with_image<R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut VideoImage) -> R,
) -> Option<R> {
    let tab = hub.tabs_mut().0.tab_mut(index)?;
    let renderer = tab.renderer.downcast_mut::<VideoRenderer>()?;
    Some(f(&mut renderer.image().borrow_mut()))
}

fn device_bounds([x, y, w, h]: [i32; 4], scale: f32) -> Bounds<Pixels> {
    let to_px = |v: i32| px(v as f32 / scale);
    Bounds {
        origin: point(to_px(x), to_px(y)),
        size: size(to_px(w), to_px(h)),
    }
}

impl<H: FrameHost> HubFrame<H> {
    /// The hub window's `keydown` listeners of the selection and of the displayed Video tab.
    /// Returns whether `preventDefault` was called.
    pub(super) fn playback_key_down(
        &mut self,
        event: &KeyDownEvent,
        code: &str,
        target: KeyTarget,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let m = event.keystroke.modifiers;
        let command = if cfg!(target_os = "macos") {
            m.platform
        } else {
            m.control
        };
        let shown = displayed(self.host().hub(cx));
        let prevented = self.host_mut().input(cx, |hub| {
            let selection = hub.page_mut().selection_mut();
            let prevented = selection.key_down(code, m.platform, m.control, target);
            if let Some((index, _)) = shown
                && target == KeyTarget::Other
                && !command
            {
                with_controller(hub, index, |video, page, _| video.key_down(page, code));
            }
            prevented
        });
        cx.notify();
        prevented
    }

    /// Starts and tracks the image loads of every Video tab.
    fn video_loads(&mut self, cx: &mut Context<'_, Self>) {
        let tabs = video_tabs(self.host().hub(cx));
        let live: HashSet<usize> = tabs.iter().map(|(_, key, _)| *key).collect();
        let mut dropped = Vec::new();
        self.video.tabs.retain(|key, tab| {
            let keep = live.contains(key);
            if !keep {
                dropped.extend(tab.presenters().map(std::mem::take));
            }
            keep
        });
        for mut presenter in dropped {
            presenter.release(cx);
        }
        for (index, key, _) in tabs {
            let (request, cleared) = self
                .host_mut()
                .input(cx, |hub| {
                    with_image(hub, index, |image| (image.begin_load(), image.cleared()))
                })
                .unwrap_or_default();
            let view = self.video.tabs.entry(key).or_default();
            if cleared {
                view.load = None;
                view.wanted = None;
                view.decoded = None;
            }
            if let Some(src) = request {
                view.wanted = Some(src);
                self.start_video_load(key, cx);
            }
        }
    }

    /// Reads and decodes the wanted frame, one at a time: while one decodes, only the latest
    /// request waits.
    fn start_video_load(&mut self, key: usize, cx: &mut Context<'_, Self>) {
        let Some(view) = self.video.tabs.get_mut(&key) else {
            return;
        };
        if view.load.is_some() {
            return;
        }
        let Some(src) = view.wanted.take() else {
            return;
        };
        let path = src.clone();
        let place = view.place;
        let decode = cx.background_spawn(async move {
            let image = Arc::new(video_frame::load(&path)?);
            let fitted = place.and_then(|(rect, scale)| {
                let natural = [image.width(), image.height()];
                let [_, _, width, height] = video_frame::contained(rect, natural, scale);
                Fitted::new(&image, [width, height])
            });
            Some(Decoded {
                src: path,
                image,
                fitted,
            })
        });
        view.load = Some(cx.spawn(async move |this, cx| {
            let decoded = decode.await;
            let _ = this.update(cx, |frame, cx| frame.video_loaded(key, src, decoded, cx));
        }));
    }

    fn video_loaded(
        &mut self,
        key: usize,
        src: JsString,
        decoded: Option<Decoded>,
        cx: &mut Context<'_, Self>,
    ) {
        let natural = decoded
            .as_ref()
            .map(|d| [f64::from(d.image.width()), f64::from(d.image.height())]);
        let index = tab_index(self.host().hub(cx), key);
        if let Some(index) = index {
            self.host_mut().input(cx, |hub| {
                with_image(hub, index, |image| image.loaded(&src, natural))
            });
        }
        if let Some(view) = self.video.tabs.get_mut(&key) {
            view.load = None;
            if decoded.is_some() {
                view.decoded = decoded;
            }
        }
        self.start_video_load(key, cx);
        cx.notify();
    }

    /// The displayed video's renderer element over `rect` (`div.renderer-content`).
    pub(super) fn video_element(
        &mut self,
        rect: [f64; 4],
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        if let Some((_, key)) = displayed(self.host().hub(cx)) {
            self.video.tabs.entry(key).or_default().place = Some((rect, f64::from(scale)));
        }
        self.video_loads(cx);
        let hub = self.host().hub(cx);
        let (index, key) = displayed(hub)?;
        let renderer = hub.tabs().tab(index)?.renderer();
        let renderer = renderer.downcast_ref::<VideoRenderer>()?;
        let (hidden, shown) = {
            let image = renderer.image().borrow();
            (image.hidden(), image.shown().cloned())
        };
        let view = self.video.tabs.get_mut(&key)?;
        let mut placed = None;
        if let (Some(Shown::Image(_)), Some(decoded)) = (&shown, &mut view.decoded) {
            let natural = [decoded.image.width(), decoded.image.height()];
            let dest = video_frame::contained(rect, natural, f64::from(scale));
            let target = [dest[2], dest[3]];
            let presented = (decoded.src.clone(), target);
            if view.presented.as_ref() != Some(&presented) {
                let fitted = decoded.fitted.take().filter(|f| f.size == target);
                let fitted = fitted.or_else(|| Fitted::new(&decoded.image, target));
                if let Some(Fitted { bgra, .. }) = fitted {
                    let [width, height] = target.map(|v| v as u32);
                    view.presenter.present_bgra(width, height, bgra, window, cx);
                    view.presented = Some(presented);
                }
            }
            placed = view.presenter.image().cloned().map(|image| (image, dest));
        } else if view.presented.take().is_some() {
            view.presenter.clear(window, cx);
        }
        let picture = placed.map(|(image, dest)| {
            canvas(
                |_, _, _| {},
                move |_, _, window, _| {
                    let bounds = device_bounds(dest, window.scale_factor());
                    let _ = window.paint_image(
                        bounds,
                        bounds,
                        Corners::default(),
                        image.clone(),
                        0,
                        false,
                    );
                },
            )
            .absolute()
            .size_full()
        });
        let image = (!hidden).then(|| {
            probed(
                "video-image",
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .size_full()
                    .children(picture),
            )
        });
        Some(
            probed(
                "div.video-container",
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .size_full()
                    .overflow_hidden()
                    .children(image),
            )
            .into_any_element(),
        )
    }

    fn video_click(
        &mut self,
        key: usize,
        f: impl FnOnce(&mut VideoController, &mut HubPage, &JsValue),
        cx: &mut Context<'_, Self>,
    ) {
        let index = tab_index(self.host().hub(cx), key);
        if let Some(index) = index {
            self.host_mut()
                .input(cx, |hub| with_controller(hub, index, f));
        }
        cx.notify();
    }

    fn video_slider(
        &mut self,
        key: usize,
        model: RangeInput,
        disabled: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<Entity<Slider<FormRange>>> {
        let view = self.video.tabs.get_mut(&key)?;
        if view.slider.is_none() {
            let model = FormRange(model.clone());
            let slider = cx.new(|cx| Slider::new(model, SliderStyle::VIDEO_TIMELINE, cx));
            let subscription = cx.subscribe(&slider, move |frame, slider, event, cx| match event {
                SliderEvent::Input => {
                    let value = slider.read(cx).model().0.value().clone();
                    frame.video_click(
                        key,
                        |video, page, _| {
                            video.set_slider(value);
                            video.slider_input(page);
                        },
                        cx,
                    );
                }
                SliderEvent::Change => {}
            });
            view.slider = Some(SliderView {
                slider,
                _subscription: subscription,
            });
        }
        let slider = view.slider.as_ref()?.slider.clone();
        slider.update(cx, |slider, cx| {
            let current = &slider.model().0;
            if current.value() != model.value() || current.max() != model.max() {
                *slider.model_mut(cx) = FormRange(model);
            }
            slider.set_disabled(disabled, window, cx);
        });
        Some(slider)
    }

    /// `#controller7` in `controls` (`div.controls-content`).
    pub(super) fn video_controls(
        &mut self,
        controls: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let hub = self.host().hub(cx);
        let (index, key) = displayed(hub)?;
        let blurred = !hub.is_focused();
        let controller = hub.tabs().tab(index)?.controller();
        let controller = controller.downcast_ref::<VideoController>()?;
        let buttons = controller.buttons();
        let model = controller.slider().clone();
        let progress = controller.progress_width();
        let animating = [controller.youtube_animating(), controller.tba_animating()];
        let now = self.host().frame_time(cx);
        self.video.tabs.entry(key).or_default();
        let slider = self.video_slider(key, model, buttons.controls_disabled, window, cx)?;

        let mut space = TextStyle::from_css([(
            "font",
            format!("{FONT_SIZE}px {}", FONT_FAMILY.join(", ")).as_str(),
        )])
        .expect("body font");
        space.white_space = WhiteSpace::Pre;
        let space = text::measure(cx, &space, " ", scale).0;
        let transport: Vec<Control> = Control::TRANSPORT
            .into_iter()
            .filter(|c| !c.hidden(buttons))
            .collect();
        let layout = ControlsLayout::new(controls, transport.len(), space, f64::from(scale));

        let snap = |v: f64| snap(v, f64::from(scale));
        let at = move |[x, y, w, h]: [f64; 4]| {
            let (left, top) = (snap(x), snap(y));
            div()
                .absolute()
                .left(px((left - controls[0]) as f32))
                .top(px((top - controls[1]) as f32))
                .w(px((snap(x + w) - left) as f32))
                .h(px((snap(y + h) - top) as f32))
        };
        let border_color = appearance.pick(0xdddddd, 0x000000);
        let border = border(f64::from(scale));
        let [sx, sy, sw, sh] = layout.source;
        let mut root = div()
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .child(probed("div.video-source", at(layout.source)))
            .child(at([sx + sw - border, sy, border, sh]).bg(rgb(border_color)));

        for (i, source) in Source::ALL.into_iter().enumerate() {
            let rect = layout.sources[i];
            let overlay = match source {
                Source::Local => None,
                Source::YouTube | Source::Tba => {
                    self.source_stroke(key, i - 1, animating[i - 1], rect, scale, now, window, cx)
                }
            };
            let mut button = IconButton::new(source.id(), symbol(source.icon()))
                .title(source.title())
                .size(
                    (snap(rect[0] + rect[2]) - snap(rect[0])) as f32,
                    (snap(rect[1] + rect[3]) - snap(rect[1])) as f32,
                )
                .box_size(rect[2], rect[3])
                .subpixel(rect[0] - snap(rect[0]), rect[1] - snap(rect[1]))
                .tint(SOURCE_TINT)
                .fit(SOURCE_FIT)
                .backgrounds(source.backgrounds())
                .blurred(blurred)
                .on_click(cx.listener(move |frame, _, _, cx| {
                    let corner = [rect[0] + rect[2], rect[1]];
                    frame.video_click(
                        key,
                        |video, page, uuid| match source {
                            Source::Local => video.select_local(page, uuid),
                            Source::YouTube => video.select_youtube(page, uuid),
                            Source::Tba => video.select_tba(page, uuid, corner),
                        },
                        cx,
                    );
                }));
            if let Some(overlay) = overlay {
                button = button.overlay(overlay);
            }
            root = root.child(probed("div.video-source > button", at(rect).child(button)));
        }

        let lock = if buttons.lock_hidden {
            Control::Unlock
        } else {
            Control::Lock
        };
        let shown = transport
            .iter()
            .zip(&layout.transport)
            .map(|(control, rect)| (*control, *rect, "div.video-timeline-controls > button"))
            .chain([(lock, layout.lock, "div.video-timeline-section > button")]);
        for (control, rect, id) in shown {
            let button = IconButton::new(control.id(), symbol(control.icon()))
                .title(control.title())
                .size(BUTTON as f32, BUTTON as f32)
                .subpixel(rect[0] - snap(rect[0]), rect[1] - snap(rect[1]))
                .translate(0.0, -BUTTON / 2.0)
                .disabled(control.disabled(buttons))
                .blurred(blurred)
                .on_click(cx.listener(move |frame, _, _, cx| {
                    frame.video_click(key, |video, page, _| control.click(video, page), cx);
                }));
            root = root.child(probed(id, at(rect).child(button)));
        }

        slider.update(cx, |slider, _| {
            slider.set_clip(TIMELINE_RADIUS as f32, layout.timeline[3] as f32);
        });
        let [mx, my, mw, mh] = layout.marker;
        let filled = [mx, my, progress_width(&progress, mw, f64::from(scale)), mh];
        let clip = layout.timeline;
        let marker = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                for (part, color) in [(layout.marker, MARKER_BACKGROUND), (filled, MARKER)] {
                    paint_in_clip(part, clip, color, window);
                }
            },
        )
        .absolute()
        .size_full();
        root = root
            .child(probed("div.video-timeline-container", at(layout.timeline)))
            .child(probed(
                "input.video-timeline-slider",
                titled("Video timeline", at(layout.slider).child(slider)),
            ))
            .child(marker)
            .child(probed(
                "div.video-timeline-marker-container > div",
                at(filled),
            ));
        Some(root.into_any_element())
    }

    /// The path over source button `i`, while it is stroked at all.
    #[allow(clippy::too_many_arguments)]
    fn source_stroke(
        &mut self,
        key: usize,
        i: usize,
        animating: bool,
        rect: [f64; 4],
        scale: f32,
        now: f64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let view = self.video.tabs.get_mut(&key)?;
        let source = &mut view.strokes[i];
        source.stroke.set(animating, now);
        if !source.stroke.moving(now) {
            source.presenter.clear(window, cx);
            return None;
        }
        window.request_animation_frame();
        let color = Source::ALL[i + 1].stroke();
        let pixmap = stroke_image(
            [rect[2], rect[3]],
            color,
            source.stroke.width(now),
            source.stroke.offset(now),
            f64::from(scale),
        )?;
        source.presenter.present(pixmap, window, cx);
        let image = source.presenter.image()?.clone();
        Some(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let _ = window.paint_image(
                        bounds,
                        bounds,
                        Corners::default(),
                        image.clone(),
                        0,
                        false,
                    );
                },
            )
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .into_any_element(),
        )
    }
}

/// Fills `part` (client px) as `div.video-timeline-container`'s rounded overflow clip leaves it.
fn paint_in_clip(part: [f64; 4], clip: [f64; 4], color: u32, window: &mut Window) {
    let scale = f64::from(window.scale_factor());
    let snap = |v: f64| px(snap(v, scale) as f32);
    let bounds = |[x, y, w, h]: [f64; 4]| Bounds {
        origin: point(snap(x), snap(y)),
        size: size(snap(x + w) - snap(x), snap(y + h) - snap(y)),
    };
    paint_in_rounded(
        bounds(part),
        bounds(clip),
        TIMELINE_RADIUS as f32,
        color,
        window,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blink's boxes in the `video` scenario at 1100x650.
    #[test]
    fn controls_sit_where_blink_lays_them_out() {
        let layout = ControlsLayout::new([300.0, 565.0, 800.0, 85.0], 5, 4.1875, 1.0);
        assert_eq!(layout.source, [300.0, 565.0, 241.0, 85.0]);
        assert_eq!(
            layout.sources,
            [312.0, 388.0, 464.0].map(|x| [x, 577.75, 64.0, 59.5])
        );
        assert_eq!(layout.lock, [545.0, 571.25, 30.0, 30.0]);
        assert_eq!(layout.timeline, [580.0, 570.0, 515.0, 32.5]);
        assert_eq!(layout.slider, [580.0, 570.0, 515.0, 25.0]);
        assert_eq!(layout.marker, [580.0, 595.0, 515.0, 7.5]);
        assert_eq!(
            layout.transport,
            [736.625, 770.8125, 805.0, 839.1875, 873.375].map(|x| [x, 613.75, 30.0, 30.0])
        );
    }

    /// Blink's boxes in the `video` scenario on Windows, where a space is 4.3828125 px wide before
    /// it is rounded up to a LayoutUnit.
    #[test]
    fn controls_sit_on_the_device_grid() {
        let transport = |space, scale| {
            ControlsLayout::new([300.0, 565.0, 800.0, 85.0], 5, space, scale).transport
        };
        let boxes = |left: [f64; 5]| left.map(|x| [x, 613.75, 30.0, 30.0]);
        assert_eq!(
            transport(4.390625, 1.0),
            boxes([736.21875, 770.609375, 805.0, 839.390625, 873.78125])
        );
        assert_eq!(
            transport(4.3828125, 2.0),
            boxes([736.234375, 770.6171875, 805.0, 839.3828125, 873.765625])
        );
    }

    /// Blink's boxes of `#controller7` in a 85 px high `div.controls-content` at fractional scales,
    /// in device px: the space, the source box's width, the source buttons' lefts and width, the
    /// timeline's left and width, the transport buttons' lefts.
    #[test]
    fn controls_sit_on_the_grid_of_fractional_scales() {
        let recorded = [
            (
                1.25,
                508.0,
                6.359375,
                191.5,
                [384.515625, 444.84375, 505.171875, 50.796875],
                [615.5, 388.25],
                [681.28125, 725.140625, 769.0, 812.859375, 856.71875],
            ),
            (
                1.25,
                612.0,
                6.359375,
                230.5,
                [386.46875, 459.140625, 531.8125, 61.1875],
                [654.5, 479.25],
                [765.78125, 809.640625, 853.5, 897.359375, 941.21875],
            ),
            (
                1.5,
                868.0,
                7.640625,
                391.59375,
                [469.515625, 593.203125, 716.890625, 104.15625],
                [900.59375, 843.890625],
                [1168.5, 1221.140625, 1273.78125, 1326.421875, 1379.0625],
            ),
            (
                3.0,
                824.0,
                15.265625,
                744.59375,
                [937.078125, 1171.90625, 1406.75, 197.75],
                [1761.59375, 1595.390625],
                [2251.25, 2356.515625, 2461.78125, 2567.046875, 2672.3125],
            ),
            (
                3.0,
                1024.0,
                15.265625,
                924.59375,
                [946.078125, 1237.90625, 1529.75, 245.75],
                [1941.59375, 2015.390625],
                [2641.25, 2746.515625, 2851.78125, 2957.046875, 3062.3125],
            ),
            (
                1.25,
                577.0,
                6.359375,
                217.375,
                [385.8125, 454.328125, 522.84375, 57.6875],
                [641.375, 448.625],
                [737.34375, 781.203125, 825.0625, 868.921875, 912.78125],
            ),
            (
                3.0,
                1136.0,
                15.265625,
                1025.390625,
                [951.109375, 1274.875, 1598.625, 272.625],
                [2042.390625, 2250.59375],
                [2859.65625, 2964.921875, 3070.1875, 3175.453125, 3280.71875],
            ),
        ];
        for (scale, width, space, source, sources, timeline, transport) in recorded {
            let layout = ControlsLayout::new([300.0, 565.0, width, 85.0], 5, space / scale, scale);
            let px = |v: f64| device(v, scale);
            let pixels = |v: f64| math::floor(v + 0.5);
            assert_eq!(px(layout.source[2]), source);
            assert_eq!(
                layout.sources.map(|b| px(snap(b[0] + b[2], scale))),
                [0, 1, 2].map(|i| pixels(sources[i] + sources[3]))
            );
            assert_eq!(layout.sources.map(|b| px(b[0])), sources[..3]);
            assert_eq!(px(layout.sources[0][2]), sources[3]);
            assert_eq!([0, 2].map(|i| px(layout.timeline[i])), timeline);
            let lefts = layout.transport.iter().map(|b| b[0]);
            assert_eq!(lefts.clone().map(px).collect::<Vec<_>>(), transport);
            assert_eq!(
                lefts.map(|v| px(snap(v, scale))).collect::<Vec<_>>(),
                transport.map(pixels)
            );
        }
    }

    #[test]
    fn progress_is_a_share_of_the_track() {
        let width = |s: &str| progress_width(&JsString::from(s), 515.0, 1.0);
        assert_eq!(width("50%"), 257.5);
        assert_eq!(width(""), 0.0);
        assert_eq!(width("120%"), 618.0);
    }

    #[test]
    fn strokes_grow_and_shrink_with_css_ease() {
        let mut stroke = Stroke::default();
        stroke.set(false, 0.0);
        assert!(!stroke.moving(0.0));
        stroke.set(true, 1000.0);
        assert_eq!(stroke.width(1000.0), 0.0);
        assert!((stroke.width(1150.0) / STROKE - 0.8024).abs() < 1e-3);
        assert_eq!(stroke.width(1300.0), STROKE);
        assert_eq!(stroke.offset(1250.0), 0.75);
        stroke.set(false, 2000.0);
        assert!(stroke.moving(2100.0) && !stroke.moving(2300.0));
    }
}
