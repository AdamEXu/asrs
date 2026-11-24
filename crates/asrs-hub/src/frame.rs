//! The hub window's frame: `www/hub.html`'s shell elements laid out by `www/hub.css` from the
//! root element's custom properties and the `body` classes, plus the parts of `hub.ts` that
//! drive them directly (window mouse listeners in registration order, the body cursor, the
//! drag item, file drops and the FPS counter).
//!
//! Geometry follows Blink with zoom-for-DSF: lengths are computed in device pixels as `float`,
//! stored as `LayoutUnit` (1/64 device px, truncated), and read back by
//! `getBoundingClientRect` divided by the zoom.

use asrs_js::number;
use asrs_js::ops::object;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::selection_impl::ControlButton;
use asrs_model::source_list::{ItemDragPayload, ItemDragStart, ItemView, ListInput};

use crate::drag::{Drag, DragItem, DragPayload, DragUpdate};
use crate::hub::HubModel;
use crate::tabs::{TabDragData, TabDragStart, TabsError};

pub const TITLE_BAR_HEIGHT: f64 = 38.0;
const FPS_SAMPLE_LENGTH: usize = 10;

/// The `fancy-*` classes `updateFancyWindow` puts on `body`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BodyClasses {
    pub title_bar_mac: bool,
    pub title_bar_win: bool,
    pub title_bar_linux: bool,
    pub title_bar_lite: bool,
    pub side_bar_mac: bool,
    pub side_bar_mac_27: bool,
    pub side_bar_win: bool,
}

impl BodyClasses {
    pub fn from_names<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        let mut c = BodyClasses::default();
        for name in names {
            match name {
                "fancy-title-bar-mac" => c.title_bar_mac = true,
                "fancy-title-bar-win" => c.title_bar_win = true,
                "fancy-title-bar-linux" => c.title_bar_linux = true,
                "fancy-title-bar-lite" => c.title_bar_lite = true,
                "fancy-side-bar-mac" => c.side_bar_mac = true,
                "fancy-side-bar-mac-27" => c.side_bar_mac_27 = true,
                "fancy-side-bar-win" => c.side_bar_win = true,
                _ => {}
            }
        }
        c
    }

    fn mac_or_lite(&self) -> bool {
        self.title_bar_mac || self.title_bar_lite
    }

    fn title_bar_shown(&self) -> bool {
        self.title_bar_mac || self.title_bar_win || self.title_bar_lite
    }

    fn any_title_bar(&self) -> bool {
        self.title_bar_shown() || self.title_bar_linux
    }
}

/// Everything hub.css reads: the window, the body classes, the root element's custom
/// properties (`None` while unset, so the `:root` default applies) and the `hidden` attributes
/// upstream toggles.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameInputs {
    pub width: f64,
    pub height: f64,
    pub device_pixel_ratio: f64,
    pub classes: BodyClasses,
    pub side_bar_width: f64,
    pub show_side_bar: bool,
    pub tab_controls_height: Option<f64>,
    pub show_tab_controls: Option<bool>,
    pub show_timeline: Option<bool>,
    pub show_lock_buttons: Option<bool>,
    pub show_update_button: Option<bool>,
    pub show_feedback_button: Option<bool>,
    pub show_loading_glow: Option<bool>,
    pub loading_glow_progress: Option<JsString>,
    pub hidden: Hidden,
    /// The `.fps` text's size, `(0, 0)` while it is empty.
    pub fps_size: (f64, f64),
}

/// The `hidden` attributes of the frame's elements.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hidden {
    pub update: bool,
    pub feedback: bool,
    pub play: bool,
    pub pause: bool,
    pub lock: bool,
    pub unlock: bool,
    pub controls_content: bool,
    pub controls_handle: bool,
    pub timeline: bool,
    pub fps: bool,
}

impl Hidden {
    /// `www/hub.html` as written.
    pub const INITIAL: Hidden = Hidden {
        update: true,
        feedback: true,
        play: false,
        pause: true,
        lock: true,
        unlock: true,
        controls_content: false,
        controls_handle: false,
        timeline: false,
        fps: false,
    };
}

impl FrameInputs {
    /// The frame for `hub` in a `width` x `height` viewport. `fps` is the counter's state.
    pub fn from_hub(
        hub: &HubModel,
        width: f64,
        height: f64,
        device_pixel_ratio: f64,
        fps: &FpsMeter,
        fps_size: (f64, f64),
    ) -> Self {
        let controls = hub.tabs().controls();
        let buttons = hub.page().selection().controls();
        let show_timeline = hub.tabs().show_timeline();
        let loading = hub.loading();
        FrameInputs {
            width,
            height,
            device_pixel_ratio,
            classes: BodyClasses::from_names(hub.body_classes().iter().copied()),
            side_bar_width: hub.sidebar().applied_width(),
            show_side_bar: hub.sidebar().sidebar_shown(),
            tab_controls_height: controls.tab_controls_height,
            show_tab_controls: controls.show_tab_controls,
            show_timeline,
            show_lock_buttons: buttons.show_lock_buttons,
            show_update_button: hub.update_button(),
            show_feedback_button: hub.feedback_button(),
            show_loading_glow: loading.show,
            loading_glow_progress: loading.progress.clone(),
            hidden: Hidden {
                update: hub.update_button().is_none_or(|shown| !shown),
                feedback: hub.feedback_button().is_none_or(|shown| !shown),
                play: buttons.hidden(ControlButton::Play),
                pause: buttons.hidden(ControlButton::Pause),
                lock: buttons.hidden(ControlButton::Lock),
                unlock: buttons.hidden(ControlButton::Unlock),
                controls_content: controls.content_hidden,
                controls_handle: controls.handle_hidden,
                timeline: show_timeline == Some(false),
                fps: fps.hidden(),
            },
            fps_size,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FrameElement {
    TitleBar,
    TitleBarText,
    TitleBarMenu,
    Update,
    Feedback,
    Play,
    Pause,
    Lock,
    Unlock,
    Close,
    Popup,
    AddTab,
    SideBarBackground,
    SideBarShadow,
    ViewerBackground,
    SideBarHandle,
    LoadingGlow,
    SideBar,
    Viewer,
    TabBar,
    TabBarShadowLeft,
    TabBarShadowRight,
    TabBarScroll,
    RendererContent,
    ControlsContent,
    ControlsHandle,
    Timeline,
    Fps,
}

impl FrameElement {
    pub const ALL: [FrameElement; 28] = [
        FrameElement::TitleBar,
        FrameElement::TitleBarText,
        FrameElement::TitleBarMenu,
        FrameElement::Update,
        FrameElement::Feedback,
        FrameElement::Play,
        FrameElement::Pause,
        FrameElement::Lock,
        FrameElement::Unlock,
        FrameElement::Close,
        FrameElement::Popup,
        FrameElement::AddTab,
        FrameElement::SideBarBackground,
        FrameElement::SideBarShadow,
        FrameElement::ViewerBackground,
        FrameElement::SideBarHandle,
        FrameElement::LoadingGlow,
        FrameElement::SideBar,
        FrameElement::Viewer,
        FrameElement::TabBar,
        FrameElement::TabBarShadowLeft,
        FrameElement::TabBarShadowRight,
        FrameElement::TabBarScroll,
        FrameElement::RendererContent,
        FrameElement::ControlsContent,
        FrameElement::ControlsHandle,
        FrameElement::Timeline,
        FrameElement::Fps,
    ];

    pub fn selector(self) -> &'static str {
        match self {
            FrameElement::TitleBar => "div.title-bar",
            FrameElement::TitleBarText => "div.title-bar-text",
            FrameElement::TitleBarMenu => "div.title-bar-menu",
            FrameElement::Update => "button.update",
            FrameElement::Feedback => "button.feedback",
            FrameElement::Play => "button.play",
            FrameElement::Pause => "button.pause",
            FrameElement::Lock => "button.lock",
            FrameElement::Unlock => "button.unlock",
            FrameElement::Close => "button.close",
            FrameElement::Popup => "button.popup",
            FrameElement::AddTab => "button.add-tab",
            FrameElement::SideBarBackground => "div.side-bar-background",
            FrameElement::SideBarShadow => "div.side-bar-shadow",
            FrameElement::ViewerBackground => "div.viewer-background",
            FrameElement::SideBarHandle => "div.side-bar-handle",
            FrameElement::LoadingGlow => "div.loading-glow",
            FrameElement::SideBar => "div.side-bar",
            FrameElement::Viewer => "div.viewer",
            FrameElement::TabBar => "div.tab-bar",
            FrameElement::TabBarShadowLeft => "div.tab-bar-shadow-left",
            FrameElement::TabBarShadowRight => "div.tab-bar-shadow-right",
            FrameElement::TabBarScroll => "div.tab-bar-scroll",
            FrameElement::RendererContent => "div.renderer-content",
            FrameElement::ControlsContent => "div.controls-content",
            FrameElement::ControlsHandle => "div.controls-handle",
            FrameElement::Timeline => "div.timeline",
            FrameElement::Fps => "div.fps",
        }
    }

    /// The element's `hidden` attribute, which hub.css overrides for the title bar.
    pub fn hidden_attribute(self, hidden: &Hidden) -> bool {
        match self {
            FrameElement::TitleBar => true,
            FrameElement::Update => hidden.update,
            FrameElement::Feedback => hidden.feedback,
            FrameElement::Play => hidden.play,
            FrameElement::Pause => hidden.pause,
            FrameElement::Lock => hidden.lock,
            FrameElement::Unlock => hidden.unlock,
            FrameElement::ControlsContent => hidden.controls_content,
            FrameElement::ControlsHandle => hidden.controls_handle,
            FrameElement::Timeline => hidden.timeline,
            FrameElement::Fps => hidden.fps,
            _ => false,
        }
    }
}

/// The default `<body>` margin, where a box with `left: auto` starts.
const BODY_MARGIN: f64 = 8.0;
/// What `width: auto` gives the boxes sized by their text, with the default title and no log.
const AUTO_TITLE_WIDTH: f64 = 129.03125;
const AUTO_MENU_WIDTH: f64 = 203.234375;
const AUTO_SIDE_BAR_WIDTH: f64 = 243.84375;

/// A `getBoundingClientRect()` in CSS px: `[x, y, width, height]`.
pub type ClientRect = [f64; 4];

/// A box in `LayoutUnit`s (1/64 device px) before transforms.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Lu {
    x: i64,
    y: i64,
    w: i64,
    h: i64,
}

/// Blink's `LayoutUnit(float)`: truncation toward zero, saturating.
fn layout_unit(device_px: f32) -> i64 {
    i64::from((device_px * 64.0) as i32)
}

struct Metrics {
    zoom: f64,
}

impl Metrics {
    /// A fixed length in CSS px as Blink stores it: `float` device px.
    fn device(&self, css_px: f64) -> f32 {
        (css_px * self.zoom) as f32
    }

    fn px(&self, css_px: f64) -> i64 {
        layout_unit(self.device(css_px))
    }

    fn to_css(&self, lu: f32) -> f64 {
        f64::from(lu / 64.0 / self.zoom as f32)
    }
}

/// Edges of an absolutely positioned box against its containing block; `None` is `auto`.
struct Edges {
    left: Option<i64>,
    right: Option<i64>,
    width: Option<i64>,
    top: Option<i64>,
    bottom: Option<i64>,
    height: Option<i64>,
}

fn axis(start: Option<i64>, end: Option<i64>, size: Option<i64>, container: i64) -> (i64, i64) {
    match (start, end, size) {
        (Some(s), _, Some(z)) => (s, z),
        (None, Some(e), Some(z)) => (container - e - z, z),
        (Some(s), Some(e), None) => (s, (container - s - e).max(0)),
        (Some(s), None, None) => (s, 0),
        (None, Some(e), None) => (container - e, 0),
        (None, None, z) => (0, z.unwrap_or(0)),
    }
}

fn place(edges: Edges, container: Lu) -> Lu {
    let (x, w) = axis(edges.left, edges.right, edges.width, container.w);
    let (y, h) = axis(edges.top, edges.bottom, edges.height, container.h);
    Lu {
        x: container.x + x,
        y: container.y + y,
        w,
        h,
    }
}

/// The frame's boxes for one state of the page.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameLayout {
    rects: Vec<(FrameElement, Option<ClientRect>)>,
}

impl FrameLayout {
    /// `None` when the element is not rendered (`display: none` on it or an ancestor), where
    /// `getBoundingClientRect()` gives all zeros.
    pub fn rect(&self, element: FrameElement) -> Option<ClientRect> {
        self.rects
            .iter()
            .find(|(e, _)| *e == element)
            .and_then(|(_, r)| *r)
    }

    /// A satellite window's page, whose renderer element's children fill the viewport.
    pub fn satellite(width: f64, height: f64) -> FrameLayout {
        let viewport = Some([0.0, 0.0, width, height]);
        FrameLayout {
            rects: vec![
                (FrameElement::ViewerBackground, viewport),
                (FrameElement::Viewer, viewport),
                (FrameElement::RendererContent, viewport),
            ],
        }
    }

    /// `getBoundingClientRect()` as upstream reads it.
    pub fn client_rect(&self, element: FrameElement) -> ClientRect {
        self.rect(element).unwrap_or([0.0; 4])
    }

    pub fn compute(inputs: &FrameInputs) -> FrameLayout {
        let m = Metrics {
            zoom: inputs.device_pixel_ratio,
        };
        let c = inputs.classes;
        let flag = |v: Option<bool>| if v.unwrap_or(false) { 1.0 } else { 0.0 };
        // `NaNpx` is no length, so every declaration that reads the variable falls back to its
        // initial value, `auto`.
        let sw = Some(inputs.side_bar_width).filter(|w| !w.is_nan());
        let from_sw = |f: &dyn Fn(f64) -> f64| sw.map(f);
        let show_side_bar = if inputs.show_side_bar { 1.0 } else { 0.0 };
        let controls_height = inputs.tab_controls_height.unwrap_or(200.0);
        let show_controls = if inputs.show_tab_controls.unwrap_or(true) {
            1.0
        } else {
            0.0
        };
        let show_timeline = if inputs.show_timeline.unwrap_or(true) {
            1.0
        } else {
            0.0
        };
        let lock = flag(inputs.show_lock_buttons);
        let update = flag(inputs.show_update_button);
        let feedback = flag(inputs.show_feedback_button);
        let glow = flag(inputs.show_loading_glow);
        let viewport = Lu {
            x: 0,
            y: 0,
            w: m.px(inputs.width),
            h: m.px(inputs.height),
        };
        let mut rects = Vec::with_capacity(FrameElement::ALL.len());
        let mut push = |element: FrameElement, lu: Option<Lu>, shift: (f32, f32)| {
            let rect = lu.map(|b| {
                [
                    m.to_css(b.x as f32 + shift.0),
                    m.to_css(b.y as f32 + shift.1),
                    m.to_css(b.w as f32),
                    m.to_css(b.h as f32),
                ]
            });
            rects.push((element, rect));
        };
        let abs = |left: Option<f64>,
                   right: Option<f64>,
                   width: Option<f64>,
                   top: Option<f64>,
                   bottom: Option<f64>,
                   height: Option<f64>,
                   container: Lu| {
            place(
                Edges {
                    left: left.map(|v| m.px(v)),
                    right: right.map(|v| m.px(v)),
                    width: width.map(|v| m.px(v)),
                    top: top.map(|v| m.px(v)),
                    bottom: bottom.map(|v| m.px(v)),
                    height: height.map(|v| m.px(v)),
                },
                container,
            )
        };

        let title_bar = c.title_bar_shown().then(|| {
            abs(
                Some(0.0),
                Some(0.0),
                None,
                Some(0.0),
                None,
                Some(TITLE_BAR_HEIGHT),
                viewport,
            )
        });
        push(FrameElement::TitleBar, title_bar, (0.0, 0.0));
        let buttons_right = 154.0 + lock * 28.0 + update * 38.0 + feedback * 38.0;
        let title_text = title_bar.map(|bar| {
            let (left, right) = if c.title_bar_win {
                (20.0, 140.0)
            } else if c.title_bar_lite {
                (20.0, buttons_right - 33.0)
            } else {
                (20.0 + (1.0 - show_side_bar) * 60.0, buttons_right)
            };
            let width = sw.is_none().then_some(AUTO_TITLE_WIDTH);
            abs(
                from_sw(&|sw| sw + left),
                Some(right),
                width,
                Some(0.0),
                Some(0.0),
                None,
                bar,
            )
        });
        push(FrameElement::TitleBarText, title_text, (0.0, 0.0));

        let menu_shown = c.title_bar_win || c.title_bar_linux || c.title_bar_lite;
        let menu = menu_shown.then(|| {
            let max_width = if c.title_bar_lite { 180.0 } else { 250.0 };
            let width = sw.map_or(AUTO_MENU_WIDTH, |sw| sw - 20.0);
            let width = m.px(width).min(m.px(max_width)).max(m.px(4.0) * 2);
            let b = place(
                Edges {
                    left: Some(m.px(sw.map_or(BODY_MARGIN, |sw| sw * 0.5))),
                    right: None,
                    width: Some(width),
                    top: Some(m.px(19.0)),
                    bottom: None,
                    height: Some(m.px(36.0)),
                },
                viewport,
            );
            (b, (-(b.w as f32) / 2.0, -(b.h as f32) / 2.0))
        });
        push(
            FrameElement::TitleBarMenu,
            menu.map(|(b, _)| b),
            menu.map_or((0.0, 0.0), |(_, s)| s),
        );

        let lite_shift = if c.title_bar_lite { 33.0 } else { 0.0 };
        let button_top = if c.title_bar_win {
            49.0
        } else if c.mac_or_lite() {
            8.0
        } else {
            11.0
        };
        let tab_control = |hidden: bool, right: f64| {
            (!hidden).then(|| {
                abs(
                    None,
                    Some(right),
                    Some(28.0),
                    Some(button_top),
                    None,
                    Some(28.0),
                    viewport,
                )
            })
        };
        let h = &inputs.hidden;
        push(
            FrameElement::Update,
            tab_control(h.update, 154.0 + lock * 28.0 + feedback * 38.0 - lite_shift),
            (0.0, 0.0),
        );
        push(
            FrameElement::Feedback,
            tab_control(h.feedback, 154.0 + lock * 28.0 - lite_shift),
            (0.0, 0.0),
        );
        let play_right = 116.0 + lock * 28.0 - lite_shift;
        push(
            FrameElement::Play,
            tab_control(h.play, play_right),
            (0.0, 0.0),
        );
        push(
            FrameElement::Pause,
            tab_control(h.pause, play_right),
            (0.0, 0.0),
        );
        push(
            FrameElement::Lock,
            tab_control(h.lock, 116.0 - lite_shift),
            (0.0, 0.0),
        );
        push(
            FrameElement::Unlock,
            tab_control(h.unlock, 116.0 - lite_shift),
            (0.0, 0.0),
        );
        push(
            FrameElement::Close,
            tab_control(false, 78.0 - lite_shift),
            (0.0, 0.0),
        );
        push(
            FrameElement::Popup,
            tab_control(c.title_bar_lite, 45.0),
            (0.0, 0.0),
        );
        push(FrameElement::AddTab, tab_control(false, 12.0), (0.0, 0.0));

        let side_bar_background = (!c.side_bar_mac).then(|| {
            abs(
                Some(0.0),
                None,
                sw,
                Some(0.0),
                Some(0.0),
                None,
                viewport,
            )
        });
        push(
            FrameElement::SideBarBackground,
            side_bar_background,
            (0.0, 0.0),
        );
        let shadow = c.any_title_bar().then(|| {
            abs(
                Some(0.0),
                None,
                sw,
                Some(0.0),
                None,
                Some(TITLE_BAR_HEIGHT),
                viewport,
            )
        });
        push(FrameElement::SideBarShadow, shadow, (0.0, 0.0));
        push(
            FrameElement::ViewerBackground,
            Some(abs(
                sw,
                Some(0.0),
                None,
                Some(0.0),
                Some(0.0),
                None,
                viewport,
            )),
            (0.0, 0.0),
        );
        push(
            FrameElement::SideBarHandle,
            Some(abs(
                from_sw(&|sw| sw - 5.0).or(Some(BODY_MARGIN)),
                None,
                Some(20.0 - show_side_bar * 10.0),
                Some(0.0),
                Some(0.0),
                None,
                viewport,
            )),
            (0.0, 0.0),
        );

        let progress = inputs
            .loading_glow_progress
            .as_ref()
            .map_or(1.0, |p| number::to_number(p.units()));
        // Blink's calc simplification distributes the product over the sum, so this evaluates
        // as `p * 100% - p * sw`: a float pixels and percent pair, not `(W - sw) * p`.
        let glow_width = if let Some(sw) = sw.filter(|_| progress.is_finite()) {
            let pixels = (-(sw * progress) * m.zoom) as f32;
            let percent = (progress * 100.0) as f32;
            let device = pixels + percent / 100.0 * (viewport.w as f32 / 64.0);
            Some(layout_unit(device).max(0))
        } else {
            None
        };
        let glow_box = place(
            Edges {
                left: Some(m.px(sw.unwrap_or(BODY_MARGIN))),
                right: None,
                width: glow_width,
                top: Some(0),
                bottom: None,
                height: Some(m.px(4.0)),
            },
            viewport,
        );
        let glow_shift = m.device(-4.0) + m.device(4.0) * glow as f32;
        push(
            FrameElement::LoadingGlow,
            Some(glow_box),
            (0.0, glow_shift * 64.0),
        );

        let side_bar_top = if c.any_title_bar() {
            TITLE_BAR_HEIGHT
        } else {
            0.0
        };
        push(
            FrameElement::SideBar,
            Some(abs(
                Some(0.0),
                None,
                sw.or(Some(AUTO_SIDE_BAR_WIDTH)),
                Some(side_bar_top),
                Some(0.0),
                None,
                viewport,
            )),
            (0.0, 0.0),
        );
        let viewer = abs(
            sw,
            Some(0.0),
            None,
            Some(viewer_top(c)),
            Some(0.0),
            None,
            viewport,
        );
        push(FrameElement::Viewer, Some(viewer), (0.0, 0.0));

        let bar_right = if c.mac_or_lite() { 10.0 } else { buttons_right };
        let bar = abs(
            Some(10.0),
            Some(bar_right),
            None,
            Some(0.0),
            None,
            Some(50.0),
            viewer,
        );
        push(FrameElement::TabBar, Some(bar), (0.0, 0.0));
        push(
            FrameElement::TabBarShadowLeft,
            Some(abs(
                Some(10.0),
                None,
                Some(60.0),
                Some(0.0),
                None,
                Some(50.0),
                viewer,
            )),
            (0.0, 0.0),
        );
        push(
            FrameElement::TabBarShadowRight,
            Some(abs(
                None,
                Some(bar_right),
                Some(60.0),
                Some(0.0),
                None,
                Some(50.0),
                viewer,
            )),
            (0.0, 0.0),
        );
        push(FrameElement::TabBarScroll, Some(bar), (0.0, 0.0));
        push(
            FrameElement::RendererContent,
            Some(abs(
                Some(0.0),
                Some(0.0),
                None,
                Some(50.0 + show_timeline * 42.0),
                Some(controls_height),
                None,
                viewer,
            )),
            (0.0, 0.0),
        );
        push(
            FrameElement::ControlsContent,
            (!h.controls_content).then(|| {
                abs(
                    Some(0.0),
                    Some(0.0),
                    None,
                    None,
                    Some(0.0),
                    Some(controls_height),
                    viewer,
                )
            }),
            (0.0, 0.0),
        );
        push(
            FrameElement::ControlsHandle,
            (!h.controls_handle).then(|| {
                abs(
                    Some(0.0),
                    Some(0.0),
                    None,
                    None,
                    Some(controls_height - 5.0),
                    Some(20.0 - show_controls * 10.0),
                    viewer,
                )
            }),
            (0.0, 0.0),
        );
        push(
            FrameElement::Timeline,
            (!h.timeline).then(|| {
                abs(
                    Some(10.0),
                    Some(10.0),
                    None,
                    Some(50.0),
                    None,
                    Some(32.0 * show_timeline),
                    viewer,
                )
            }),
            (0.0, 0.0),
        );
        push(
            FrameElement::Fps,
            (!h.fps).then(|| {
                abs(
                    None,
                    Some(10.0),
                    Some(inputs.fps_size.0),
                    None,
                    Some(7.0),
                    Some(inputs.fps_size.1),
                    viewport,
                )
            }),
            (0.0, 0.0),
        );
        FrameLayout { rects }
    }
}

fn viewer_top(c: BodyClasses) -> f64 {
    if c.title_bar_shown() {
        TITLE_BAR_HEIGHT
    } else {
        0.0
    }
}

/// `RENDERER_CONTENT.getBoundingClientRect().top` while `--show-timeline` is `show_timeline`.
pub fn renderer_top(classes: BodyClasses, show_timeline: bool) -> f64 {
    viewer_top(classes) + 50.0 + if show_timeline { 42.0 } else { 0.0 }
}

/// `document.body.style.cursor`, which the resize handles and drags set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BodyCursor {
    #[default]
    Unset,
    ColResize,
    RowResize,
    Grabbing,
    Initial,
}

impl BodyCursor {
    pub fn css(self) -> &'static str {
        match self {
            BodyCursor::Unset => "",
            BodyCursor::ColResize => "col-resize",
            BodyCursor::RowResize => "row-resize",
            BodyCursor::Grabbing => "grabbing",
            BodyCursor::Initial => "initial",
        }
    }
}

/// `hub.ts`'s FPS measurement, run once per animation frame.
#[derive(Clone, Debug, Default)]
pub struct FpsMeter {
    frame_times: Vec<f64>,
    hidden: bool,
    text: Option<String>,
}

impl FpsMeter {
    /// One `periodic` frame at `performance.now()` = `now`.
    pub fn frame(&mut self, now: f64, show: bool) {
        self.frame_times.push(now);
        while self.frame_times.len() > FPS_SAMPLE_LENGTH {
            self.frame_times.remove(0);
        }
        let mut fps = 0.0;
        if self.frame_times.len() > 1 {
            let first = self.frame_times[0];
            let last = self.frame_times[self.frame_times.len() - 1];
            let avg_frame_time = (last - first) / (self.frame_times.len() - 1) as f64;
            fps = 1000.0 / avg_frame_time;
        }
        self.hidden = !show;
        if show {
            let fixed = number::to_fixed(fps, 1.0).unwrap_or_default();
            self.text = Some(format!("FPS: {fixed}"));
        }
    }

    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// `innerText`, once a shown frame has set it.
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
}

/// A drag in progress: hub.ts's `startDrag` data and the `#dragItem` contents.
#[derive(Clone, Debug, PartialEq)]
pub enum DragData {
    Fields(DragPayload),
    Tab(TabDragData),
    ListItem(ItemDragPayload),
    /// A Table tab's column, as `tableIndex`.
    Column(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub enum DragContent {
    Field(DragItem),
    Tab(TabDragStart),
    /// A copy of the dragged source list item, `width` wide (its `clientWidth`).
    ListItem {
        item: ItemView,
        width: f64,
    },
    /// A copy of a Table column's key container in a box `width` wide.
    Column {
        text: JsString,
        available: bool,
        width: f64,
    },
}

/// A `drag-update` event's `detail`, as the controllers that take field and column drags read it.
fn drag_detail(update: &DragUpdate<DragData>) -> JsValue {
    let strings =
        |keys: &[JsString]| JsValue::Array(keys.iter().cloned().map(JsValue::String).collect());
    let data = match &update.data {
        DragData::Fields(fields) => object([
            ("fields", strings(&fields.fields)),
            ("children", strings(&fields.children)),
        ]),
        DragData::Column(index) => object([("tableIndex", JsValue::Number(*index as f64))]),
        DragData::Tab(_) | DragData::ListItem(_) => object([]),
    };
    object([
        ("end", JsValue::Bool(update.end)),
        ("x", JsValue::Number(update.x)),
        ("y", JsValue::Number(update.y)),
        ("data", data),
    ])
}

impl From<DragPayload> for DragData {
    fn from(payload: DragPayload) -> Self {
        DragData::Fields(payload)
    }
}

impl From<DragItem> for DragContent {
    fn from(item: DragItem) -> Self {
        DragContent::Field(item)
    }
}

/// Which resize handle a pointer event targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    SideBar,
    Controls,
}

/// The frame state `hub.ts`, `Sidebar` and `Tabs` keep in the DOM rather than in their models.
#[derive(Debug, Default)]
pub struct HubShell {
    cursor: BodyCursor,
    drag: Drag<DragData, DragContent>,
    fps: FpsMeter,
}

/// Blink's `MouseEvent` client coordinates are whole CSS pixels.
pub fn client_coordinate(css_px: f64) -> f64 {
    asrs_js::math::trunc(css_px)
}

impl HubShell {
    pub fn cursor(&self) -> BodyCursor {
        self.cursor
    }

    pub fn drag(&self) -> &Drag<DragData, DragContent> {
        &self.drag
    }

    pub fn drag_mut(&mut self) -> &mut Drag<DragData, DragContent> {
        &mut self.drag
    }

    pub fn fps(&self) -> &FpsMeter {
        &self.fps
    }

    pub fn fps_mut(&mut self) -> &mut FpsMeter {
        &mut self.fps
    }

    /// `window.startDrag`.
    pub fn start_drag(
        &mut self,
        x: f64,
        y: f64,
        offset_x: f64,
        offset_y: f64,
        content: DragContent,
        data: DragData,
    ) {
        self.drag.start(x, y, offset_x, offset_y, content, data);
        self.cursor = BodyCursor::Grabbing;
    }

    /// `mousedown` on a resize handle.
    pub fn handle_mouse_down(&mut self, hub: &mut HubModel, handle: Handle) {
        match handle {
            Handle::SideBar => {
                hub.with_sidebar(|sidebar, _| sidebar.handle_mouse_down());
                self.cursor = BodyCursor::ColResize;
            }
            Handle::Controls => {
                hub.tabs_mut().0.controls_handle_mouse_down();
                self.cursor = BodyCursor::RowResize;
            }
        }
    }

    /// `click` on a resize handle at `new Date().getTime()` = `now`.
    pub fn handle_click(
        &mut self,
        hub: &mut HubModel,
        handle: Handle,
        now: f64,
    ) -> Result<(), TabsError> {
        match handle {
            Handle::SideBar => {
                hub.with_sidebar(|sidebar, _| sidebar.handle_click(now));
                Ok(())
            }
            Handle::Controls => {
                let (tabs, page) = hub.tabs_mut();
                tabs.controls_handle_click(page, now)
            }
        }
    }

    /// The window's `mousemove` listeners in registration order: the sidebar handle's, the
    /// controls handle's, hub.ts's `dragMove`, then the field labels'. `sync` runs between them so
    /// the page's geometry reads see what the previous listener changed.
    pub fn window_mouse_move(
        &mut self,
        hub: &mut HubModel,
        x: f64,
        y: f64,
        sync: &mut dyn FnMut(&HubModel),
    ) -> Result<(), TabsError> {
        hub.with_sidebar(|sidebar, _| sidebar.handle_mouse_move(x));
        sync(hub);
        let (tabs, page) = hub.tabs_mut();
        let result = tabs.window_mouse_move(page, y);
        sync(hub);
        self.drag.move_to(x, y);
        let result = result.and(self.dispatch_tab_drag(hub));
        let dispatched = self.dispatch_drag(hub);
        let was_active = self.drag.active();
        let drag = &mut self.drag;
        hub.with_sidebar(|sidebar, _| sidebar.fields_mouse_move(drag, x, y));
        if self.drag.active() && !was_active {
            self.cursor = BodyCursor::Grabbing;
        }
        result.and(dispatched)
    }

    /// The `drag-update` events `dragMove` and `dragEnd` dispatched, for the source lists.
    fn dispatch_drag(&mut self, hub: &mut HubModel) -> Result<(), TabsError> {
        let mut result = Ok(());
        for update in self.drag.take_updates() {
            let (x, y, end) = (update.x, update.y, update.end);
            let thrown = hub.tab_drag_update(&drag_detail(&update));
            if let (Ok(()), Err(e)) = (&result, thrown) {
                result = Err(TabsError::Thrown(e));
            }
            let input = match &update.data {
                DragData::Fields(fields) => ListInput::FieldDrag {
                    fields: &fields.fields,
                    children: &fields.children,
                    x,
                    y,
                    end,
                },
                DragData::ListItem(payload) => ListInput::ItemDrag { payload, x, y, end },
                DragData::Tab(_) | DragData::Column(_) => continue,
            };
            let (_, errors) = hub.source_list_input(input);
            if let (Ok(()), Some(e)) = (&result, errors.into_iter().next()) {
                result = Err(TabsError::Thrown(e));
            }
        }
        result
    }

    /// `touchstart` on the label of the sidebar's field `flat_index`.
    pub fn field_touch_start(&mut self, hub: &mut HubModel, flat_index: usize, x: f64, y: f64) {
        let drag = &mut self.drag;
        hub.with_sidebar(|sidebar, env| {
            sidebar.label_touch_start(env.geometry, drag, flat_index, x, y);
        });
        if self.drag.active() {
            self.cursor = BodyCursor::Grabbing;
        }
    }

    /// The window's `touchmove` listener, hub.ts's `dragMove`.
    pub fn window_touch_move(
        &mut self,
        hub: &mut HubModel,
        x: f64,
        y: f64,
    ) -> Result<(), TabsError> {
        self.drag.move_to(x, y);
        let result = self.dispatch_tab_drag(hub);
        result.and(self.dispatch_drag(hub))
    }

    /// The window's `touchend` listener, hub.ts's `dragEnd`.
    pub fn window_touch_end(&mut self, hub: &mut HubModel) -> Result<(), TabsError> {
        if self.drag.active() {
            self.drag.end();
            self.cursor = BodyCursor::Unset;
        }
        let result = self.dispatch_tab_drag(hub);
        result.and(self.dispatch_drag(hub))
    }

    /// A list item drag the source list started: `window.startDrag` with a copy of the item.
    pub fn start_list_drag(&mut self, start: ItemDragStart, item: ItemView) {
        self.start_drag(
            start.x,
            start.y,
            start.offset_x,
            start.offset_y,
            DragContent::ListItem {
                item,
                width: start.client_width,
            },
            DragData::ListItem(start.payload),
        );
    }

    /// The `drag-update` events of a tab drag, which `Tabs` handles.
    fn dispatch_tab_drag(&mut self, hub: &mut HubModel) -> Result<(), TabsError> {
        let updates = self.drag.take_updates_with(|data| match data {
            DragData::Tab(tab) => Some(*tab),
            DragData::Fields(_) | DragData::ListItem(_) | DragData::Column(_) => None,
        });
        let mut result = Ok(());
        for update in updates {
            let (tabs, page) = hub.tabs_mut();
            result = result.and(tabs.drag_update(page, &update));
        }
        result
    }

    /// The window's `mouseup` listeners: the sidebar handle's, the controls handle's, then
    /// hub.ts's `dragEnd`.
    pub fn window_mouse_up(&mut self, hub: &mut HubModel) -> Result<(), TabsError> {
        hub.with_sidebar(|sidebar, _| sidebar.handle_mouse_up());
        self.cursor = BodyCursor::Initial;
        hub.tabs_mut().0.window_mouse_up();
        self.cursor = BodyCursor::Initial;
        if self.drag.active() {
            self.drag.end();
            self.cursor = BodyCursor::Unset;
        }
        let result = self.dispatch_tab_drag(hub);
        result.and(self.dispatch_drag(hub))
    }

    /// The document's `drop` listener: the dropped files' paths as `open-files-request`, or
    /// nothing when no file was dropped.
    pub fn drop_files(paths: Vec<JsString>) -> Option<NamedMessage<HubToMain>> {
        if paths.is_empty() {
            return None;
        }
        let mut data = asrs_js::value::JsObject::new();
        data.insert(
            "files",
            JsValue::Array(paths.into_iter().map(JsValue::String).collect()),
        );
        Some(NamedMessage::new(
            HubToMain::OpenFilesRequest,
            JsValue::Object(data),
        ))
    }
}
