//! The hub window's frame (`www/hub.html` and the shell rules of `www/hub.css`): backgrounds,
//! title bar, tab control buttons, loading glow, resize handles, FPS counter, drag item and
//! file drop. Every box comes from `asrs_hub::frame::FrameLayout`; the views that fill the
//! sidebar, tab bar, renderer, controls and timeline plug into [`FrameSlot`]s.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use asrs_canvas::Canvas2d;
use asrs_hub::frame::{
    BodyClasses, BodyCursor, DragContent, DragData, FrameElement, FrameInputs, FrameLayout, Handle,
    HubShell, renderer_top,
};
use asrs_hub::hub::HubModel;
use asrs_hub::tabs::{TabDragData, TitleElement};
use asrs_hub::timeline::{ScrollTarget, Timeline, TimelineFrame};
use asrs_js::string::JsString;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::scroll_sensor::{SIZE_PX, ScrollSensor, Wheel};
use asrs_model::selection_impl::{ControlButton, KeyTarget};
use asrs_model::source_list::Rect;
use asrs_ui::button::IconButton;
use asrs_ui::canvas_view::geometry::mouse_client;
use asrs_ui::canvas_view::{
    CanvasBorder, CanvasMetrics, CanvasState, canvas_view, with_text_backend,
};
use asrs_ui::caption::caption_buttons;
use asrs_ui::focus::{self, FocusRing, ScrollAxis, Traversal};
use asrs_ui::gpui::{
    AnyElement, App, AppContext, BoxShadow, ClickEvent, Context, CursorStyle, DispatchPhase,
    Entity, ExternalPaths, FocusHandle, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent,
    MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    Render, ScrollDelta, ScrollEvent, ScrollHandle, SharedString, StatefulInteractiveElement,
    Styled, Subscription, TouchDragEvent, TouchPhase, Window, WindowControlArea, canvas, div,
    point, px, rgb, rgba,
};
use asrs_ui::icon::IconFit;
use asrs_ui::input;
use asrs_ui::menu;
use asrs_ui::parity::{Probed, probed};
use asrs_ui::platform;
use asrs_ui::selection;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, Text, TextStyle};
use asrs_ui::theme::{Appearance, FONT_FAMILY, IconTint, MONOSPACE_FAMILY};

use super::canvas_tabs::CanvasTabViews;
use super::console::ConsoleViews;
use super::documentation::DocumentationViews;
use super::field_2d::Field2dViews;
use super::field3d::{Field3dViews, key_code};
use super::line_graph::LineGraphViews;
use super::metadata::MetadataView;
use super::page_text::PageSelection;
use super::scrollers::{Area, Scrollers};
use super::source_list::{self, ListsGeometry};
use super::statistics::StatisticsViews;
use super::tab_bar::{self, TabBarGeometry};
use super::table::{self, TableViews};
use super::title_bar_menu::{self, MenuLayout};
use super::video::VideoViews;

/// `element` under its parity id, which is its selector in `www/hub.html` (`div.title-bar`).
fn probe(element: FrameElement, child: impl IntoElement) -> Probed {
    probed(element.selector(), child)
}

/// The regions other views draw into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameSlot {
    /// `div.side-bar`'s contents.
    SideBar,
    /// `div.tab-bar` and `div.tab-bar-scroll`.
    TabBar,
    RendererContent,
    ControlsContent,
    Timeline,
    /// `#dragItem` while a tab is dragged.
    DragTab,
}

/// What the frame needs from whoever owns the hub's model.
pub trait FrameHost: 'static {
    /// The geometry the hub model's `HubView` answers from.
    fn geometry(&self) -> FrameGeometry;
    fn hub<'a>(&'a self, cx: &'a App) -> &'a HubModel;
    /// The window's `resize` event, once the page has been laid out at its new size.
    fn resized(&mut self, _cx: &mut App) {}
    /// An animation frame of a page without a frame loop of its own.
    fn animation_frame(&mut self, _cx: &mut App) {}
    /// `satellite.ts`'s `keydown` listener, outside editable elements. Returns whether
    /// `preventDefault` was called.
    fn satellite_key_down(&mut self, _code: &str, _cx: &mut App) -> bool {
        false
    }
    /// The hub's frame clock (ms), which its scroll sensors animate on.
    fn frame_time(&self, cx: &App) -> f64;
    /// Runs `f`, the frame's handler for an input event, as one page task.
    fn input<R>(&mut self, cx: &mut App, f: impl FnOnce(&mut HubModel) -> R) -> R;
    fn send_main_message(&mut self, message: NamedMessage<HubToMain>, cx: &mut App);
    /// The layout the hub model's `HubView` answers source list reads from.
    fn source_list_geometry(&self) -> ListsGeometry;
    /// A key's smooth scroll of `div.side-bar`.
    fn side_bar_smooth(&mut self, _by: f64, _cx: &mut App) {}
    /// `div.side-bar`'s client size and largest scroll offsets.
    fn side_bar_range(&self, _cx: &App) -> Option<[[f64; 2]; 2]> {
        None
    }
    /// Scrolls `div.side-bar` to what has the focus in it, at this client y and height.
    fn scroll_side_bar(&mut self, _focused: [f64; 2]) {}
    /// The sidebar field whose label a touch just started on, by flat index.
    fn take_touched_field(&mut self) -> Option<usize> {
        None
    }
    /// The boxes of `div.title-bar-menu`'s buttons, as last laid out.
    fn set_menu_buttons(&mut self, _rects: Vec<[f64; 4]>) {}
    /// The contents of `slot`, laid out in a box of the slot's size.
    fn slot(
        &mut self,
        _slot: FrameSlot,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Option<AnyElement> {
        None
    }
}

/// What the page reads from its layout while the model runs: the frame keeps it current
/// before every input it dispatches and every render.
#[derive(Clone, Debug, Default)]
pub struct FrameGeometry(Rc<RefCell<GeometryState>>, Rc<RefCell<TabBarGeometry>>);

#[derive(Debug, Default)]
struct GeometryState {
    classes: BodyClasses,
    inner_height: f64,
    side_bar: [f64; 4],
    large_table: Option<(Rect, f64)>,
    dark: bool,
}

impl FrameGeometry {
    pub fn tab_bar(&self) -> &RefCell<TabBarGeometry> {
        &self.1
    }

    pub fn sync(&self, hub: &HubModel) {
        self.0.borrow_mut().classes = BodyClasses::from_names(hub.body_classes().iter().copied());
    }

    pub fn set_dark(&self, dark: bool) {
        self.0.borrow_mut().dark = dark;
    }

    /// `prefers-color-scheme: dark`.
    pub fn dark(&self) -> bool {
        self.0.borrow().dark
    }

    pub fn set_inner_height(&self, height: f64) {
        self.0.borrow_mut().inner_height = height;
    }

    /// `window.innerHeight`.
    pub fn inner_height(&self) -> f64 {
        self.0.borrow().inner_height
    }

    /// `RENDERER_CONTENT.getBoundingClientRect().top`.
    pub fn renderer_top(&self, show_timeline: bool) -> f64 {
        renderer_top(self.0.borrow().classes, show_timeline)
    }

    /// `div.renderer-content`'s box while a Table or Console tab is displayed, and the window's
    /// scale.
    pub fn large_table_box(&self) -> Option<(Rect, f64)> {
        self.0.borrow().large_table
    }

    /// `div.side-bar`'s box as last laid out.
    pub fn side_bar(&self) -> [f64; 4] {
        self.0.borrow().side_bar
    }
}

/// What a key event the page gets is dispatched to; `None` while an open menu takes the keys.
pub(super) fn key_target(window: &Window) -> Option<KeyTarget> {
    if menu::focused(window) {
        None
    } else if input::focused(window) {
        Some(KeyTarget::Input)
    } else {
        Some(KeyTarget::Other)
    }
}

/// The settings beside the source lists in `div.controls-content`, which come before them.
pub(super) const SETTINGS_SECTION: isize = 100;

/// The parts of the page in document order, as [`focus::section`] has them.
#[derive(Clone, Copy)]
pub(super) enum Section {
    TitleBar = 1,
    TitleBarMenu,
    TabControls,
    SideBarHandle,
    SearchResults,
    SideBar,
    Viewer,
}

/// The same of `div.viewer`.
#[derive(Clone, Copy)]
enum ViewerSection {
    TabBar = 1,
    TabBarScroll,
    RendererContent,
    ControlsContent,
    ControlsHandle,
    Timeline,
}

/// The elements of the frame that Tab goes on from after a press on them.
pub(super) struct Places {
    title_bar: FocusHandle,
    title_bar_menu: FocusHandle,
    side_bar_handle: FocusHandle,
    pub(super) side_bar: FocusHandle,
    viewer: FocusHandle,
    tab_bar: FocusHandle,
    tab_bar_scroll: FocusHandle,
    renderer_content: FocusHandle,
    controls_content: FocusHandle,
    controls_handle: FocusHandle,
    timeline: FocusHandle,
    /// The tab controls' own handles, which keep their place while they are hidden.
    tab_controls: Vec<(FrameElement, FocusHandle)>,
}

const TAB_CONTROLS: [FrameElement; 9] = [
    FrameElement::Update,
    FrameElement::Feedback,
    FrameElement::Play,
    FrameElement::Pause,
    FrameElement::Lock,
    FrameElement::Unlock,
    FrameElement::Close,
    FrameElement::Popup,
    FrameElement::AddTab,
];

impl Places {
    fn new(cx: &App) -> Places {
        Places {
            title_bar: cx.focus_handle(),
            title_bar_menu: cx.focus_handle(),
            side_bar_handle: cx.focus_handle(),
            side_bar: cx.focus_handle(),
            viewer: cx.focus_handle(),
            tab_bar: cx.focus_handle(),
            tab_bar_scroll: cx.focus_handle(),
            renderer_content: cx.focus_handle(),
            controls_content: cx.focus_handle(),
            controls_handle: cx.focus_handle(),
            timeline: cx.focus_handle(),
            tab_controls: TAB_CONTROLS
                .map(|element| (element, cx.focus_handle()))
                .into(),
        }
    }
}

pub struct HubFrame<H: FrameHost> {
    host: H,
    shell: HubShell,
    started: Instant,
    /// `Date.now()` then.
    opened: f64,
    viewport: Option<(f64, f64)>,
    /// The focus while no element has it, which is `body`'s in a page.
    pub(super) body: FocusHandle,
    pub(super) places: Places,
    traversal: Traversal,
    /// What [`Tabs::bar_builds`](asrs_hub::tabs::Tabs::bar_builds) was when the frame drew.
    bar_builds: u64,
    /// Each title's focus as last drawn.
    titles: Vec<FocusHandle>,
    tab_key: Option<Subscription>,
    drag_region: Option<[f64; 4]>,
    no_drag_regions: Vec<[f64; 4]>,
    geometry: FrameGeometry,
    timeline: Timeline,
    timeline_sensor: ScrollSensor,
    timeline_canvas: Option<Entity<CanvasState>>,
    timeline_rect: Option<[f64; 4]>,
    timeline_hovered: bool,
    menu_scroll: ScrollHandle,
    pub(super) line_graph: LineGraphViews,
    pub(super) field_2d: Field2dViews,
    pub(super) field3d: Field3dViews,
    pub(super) canvas_tabs: CanvasTabViews,
    pub(super) table: TableViews,
    pub(super) console: ConsoleViews,
    pub(super) documentation: DocumentationViews,
    pub(super) statistics: StatisticsViews,
    pub(super) metadata: MetadataView,
    pub(super) video: VideoViews,
    pub(super) text: PageSelection,
    pub(super) scrollers: Scrollers,
}

/// The timeline overlay's view of one pointer position: the client x relative to the overlay
/// and whether the pointer is over it.
struct TimelinePointer {
    x: f64,
    on_overlay: bool,
    hidden: bool,
    target: ScrollTarget,
}

pub(crate) fn wheel_of(event: &ScrollEvent) -> Wheel {
    let delta = platform::wheel_delta(event);
    Wheel {
        delta: [f64::from(delta.x), f64::from(delta.y)],
        precise: matches!(event.delta, ScrollDelta::Pixels(_)),
    }
}

const GLOW: u32 = 0x303dc9;
const FPS_COLOR: u32 = 0xff5900;

fn fps_style() -> TextStyle {
    let font = format!("bold 16px {}", MONOSPACE_FAMILY.join(", "));
    TextStyle::from_css([("font", font.as_str()), ("white-space", "nowrap")]).expect("fps style")
}

fn title_style() -> TextStyle {
    let font = format!("600 16px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "38px"),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ])
    .expect("title style")
}

fn drag_text_style(bold: bool) -> TextStyle {
    let weight = if bold { "bold " } else { "" };
    let font = format!("italic {weight}15px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([("font", font.as_str()), ("white-space", "nowrap")])
        .expect("drag item style")
}

/// A CSS `box-shadow`; GPUI's blur radius is the Gaussian's sigma, half the CSS blur.
fn box_shadow(color: u32, blur: f32, spread: f32) -> BoxShadow {
    let [r, g, b, a] = color.to_be_bytes();
    let c: asrs_ui::gpui::Hsla = rgba(u32::from_be_bytes([r, g, b, a])).into();
    BoxShadow {
        color: c,
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(blur / 2.0),
        spread_radius: px(spread),
        inset: false,
    }
}

/// `canvas.timeline-canvas`'s `border` and `border-radius`.
const TIMELINE_BORDER: CanvasBorder = CanvasBorder {
    width: 1.0,
    radius: 10.0,
};

/// `div.controls-content`'s dark `box-shadow: 0px 0px 0px 0.7px black`.
const CONTROLS_SPREAD: f32 = 0.7;

/// A black `spread` px wide unblurred shadow above a box, whose last device row is only partly
/// covered; GPUI would snap it to whole pixels.
fn spread_rows(spread: f32, scale: f32) -> Vec<asrs_ui::gpui::Div> {
    let device = spread * scale;
    let whole = device.floor();
    let row = |top: f32, rows: f32, alpha: f32| {
        div()
            .absolute()
            .left_0()
            .w_full()
            .top(px(-top / scale))
            .h(px(rows / scale))
            .bg(rgba((alpha * 255.0 + 0.5) as u32))
    };
    let mut out = Vec::new();
    if whole > 0.0 {
        out.push(row(whole, whole, 1.0));
    }
    if device > whole {
        out.push(row(whole + 1.0, 1.0, device - whole));
    }
    out
}

fn place(rect: [f64; 4], origin: (f64, f64)) -> asrs_ui::gpui::Div {
    div()
        .absolute()
        .left(px((rect[0] - origin.0) as f32))
        .top(px((rect[1] - origin.1) as f32))
        .w(px(rect[2] as f32))
        .h(px(rect[3] as f32))
}

/// The size and the largest offsets of a `ScrollSensor`'s overlay.
pub(super) fn overlay_range(client: [f64; 2]) -> [[f64; 2]; 2] {
    [client, client.map(|size| (SIZE_PX - size).max(0.0))]
}

impl<H: FrameHost> HubFrame<H> {
    pub fn new(host: H, cx: &App) -> Self {
        let geometry = host.geometry();
        geometry.sync(host.hub(cx));
        HubFrame {
            host,
            shell: HubShell::default(),
            started: Instant::now(),
            opened: asrs_js::time::Clock::now(&asrs_js::time::SystemClock),
            viewport: None,
            body: cx.focus_handle(),
            places: Places::new(cx),
            traversal: Traversal::default(),
            bar_builds: 0,
            titles: Vec::new(),
            tab_key: None,
            drag_region: None,
            no_drag_regions: Vec::new(),
            geometry,
            timeline: Timeline::new(Rc::new(asrs_js::time::SystemClock)),
            timeline_sensor: ScrollSensor::default(),
            timeline_canvas: None,
            timeline_rect: None,
            timeline_hovered: false,
            menu_scroll: ScrollHandle::new(),
            line_graph: LineGraphViews::default(),
            field_2d: Field2dViews::default(),
            field3d: Field3dViews::default(),
            canvas_tabs: CanvasTabViews::default(),
            table: TableViews::default(),
            console: ConsoleViews::default(),
            documentation: DocumentationViews::default(),
            statistics: StatisticsViews::default(),
            metadata: MetadataView::default(),
            video: VideoViews::default(),
            text: PageSelection::default(),
            scrollers: Scrollers::default(),
        }
    }

    pub fn host(&self) -> &H {
        &self.host
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.host
    }

    pub fn shell_mut(&mut self) -> &mut HubShell {
        &mut self.shell
    }

    /// The frame's layout for a `width` x `height` viewport at `scale`.
    pub fn layout(&self, cx: &App, width: f64, height: f64, scale: f64) -> FrameLayout {
        FrameLayout::compute(&self.inputs(cx, width, height, scale, (0.0, 0.0)))
    }

    fn inputs(
        &self,
        cx: &App,
        width: f64,
        height: f64,
        scale: f64,
        fps_size: (f64, f64),
    ) -> FrameInputs {
        FrameInputs::from_hub(
            self.host.hub(cx),
            width,
            height,
            scale,
            self.shell.fps(),
            fps_size,
        )
    }

    /// The window's `keydown` listeners, before the focused element's own key handling, which
    /// `preventDefault` keeps from running.
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(target) = key_target(window) else {
            return;
        };
        let editable = target == KeyTarget::Input;
        let code = key_code(&event.keystroke.key);
        let prevented = if !self.host.hub(cx).page().is_satellite() {
            self.playback_key_down(event, &code, target, cx)
        } else {
            !editable && self.host.satellite_key_down(&code, cx)
        };
        if prevented {
            window.prevent_default();
        }
        self.field3d_key_down(event, editable, cx);
        self.console_key_down(event, editable, window, cx);
        if !editable {
            self.text_key_down(event, cx);
        }
    }

    /// Tab moves the focus from anywhere in the page, before the bindings of what has it.
    fn intercept_tab(window: &Window, cx: &mut Context<'_, Self>) -> Subscription {
        let handle = window.window_handle();
        let this = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            let Some(back) = focus::tab_key(&event.keystroke) else {
                return;
            };
            let menu = menu::focused(window) || platform::menu_takes_keys(window);
            if window.window_handle() != handle || menu {
                return;
            }
            let _ = this.update(cx, |frame, cx| frame.tab(back, window, cx));
            // After the actions that moving the focus dispatches, which begin propagating again.
            cx.stop_propagation();
        })
    }

    fn tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<'_, Self>) {
        let body = self.body.clone();
        if let Some(focused) = self.traversal.advance(back, &body, window, cx) {
            self.scroll_to_focused(&focused, window, cx);
        }
        cx.notify();
    }

    /// `gone` were removed from before the rest of what `place` holds.
    pub(super) fn focus_removed(&mut self, gone: &[FocusHandle], place: &FocusHandle) {
        self.traversal.removed(gone, place);
    }

    /// Scrolls what holds the focused element to it.
    fn scroll_to_focused(
        &mut self,
        focused: &FocusHandle,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(index) = self.titles.iter().position(|title| title == focused) {
            let mut bar = self.geometry.tab_bar().borrow_mut();
            let title = bar.title_rect(index);
            let start = title.x - bar.bar[0];
            let visible = bar.client_width();
            let by = focus::reveal(ScrollAxis::Horizontal, visible, [start, title.width]);
            if by != 0.0 {
                let left = bar.scroll_left + by;
                bar.assign_scroll_left(left);
            }
            return;
        }
        let Some(bounds) = window.focus_bounds(focused) else {
            return;
        };
        if self.places.side_bar.contains(focused, window) {
            let top = f64::from(f32::from(bounds.origin.y));
            let height = f64::from(f32::from(bounds.size.height));
            self.host.scroll_side_bar([top, height]);
        }
        self.scroll_source_list(focused, bounds, window);
        self.canvas_tabs.scroll_to(focused, bounds, window);
        self.scroll_statistics_settings(focused, bounds, window, cx);
        self.scroll_table(focused, cx);
    }

    /// The `keydown` listener of `titleElement`.
    fn title_key_down(&mut self, index: usize, event: &KeyDownEvent, cx: &mut Context<'_, Self>) {
        let key = match event.keystroke.key.as_str() {
            "enter" => "Enter",
            "space" => " ",
            _ => return,
        };
        let geometry = &self.geometry;
        let result = self.host.input(cx, |hub| {
            geometry.sync(hub);
            let (tabs, page) = hub.tabs_mut();
            tabs.title_key_down(page, index, key)
        });
        if let Err(e) = result {
            eprintln!("asrs: {e}");
        }
        cx.notify();
    }

    fn key_up(&mut self, event: &KeyUpEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        if let Some(target) = key_target(window) {
            self.field3d_key_up(event, target == KeyTarget::Input, cx);
        }
    }

    fn handle_down(&mut self, handle: Handle, cx: &mut Context<'_, Self>) {
        let (shell, geometry) = (&mut self.shell, &self.geometry);
        self.host.input(cx, |hub| {
            geometry.sync(hub);
            shell.handle_mouse_down(hub, handle);
        });
        cx.notify();
    }

    fn handle_click(&mut self, handle: Handle, cx: &mut Context<'_, Self>) {
        let now = self.opened + self.host.frame_time(cx);
        let (shell, geometry) = (&mut self.shell, &self.geometry);
        let result = self.host.input(cx, |hub| {
            geometry.sync(hub);
            shell.handle_click(hub, handle, now)
        });
        if let Err(e) = result {
            eprintln!("asrs: {e}");
        }
        cx.notify();
    }

    fn timeline_pointer(&self, x: f64, y: f64, cx: &App) -> Option<TimelinePointer> {
        let rect = self.timeline_rect?;
        let hub = self.host.hub(cx);
        let hidden = rect[3] == 0.0;
        Some(TimelinePointer {
            x: x - rect[0],
            on_overlay: !hidden
                && x >= rect[0]
                && x < rect[0] + rect[2]
                && y >= rect[1]
                && y < rect[1] + rect[3],
            hidden,
            target: ScrollTarget::new(rect[2], rect[3], hub.page().preferences()),
        })
    }

    /// The timeline's client box while it shows.
    pub(super) fn timeline_rect(&self) -> Option<[f64; 4]> {
        self.timeline_rect
    }

    fn mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        self.text_mouse_move([x, y], event.pressed_button.is_some(), window, cx);
        self.source_list_mouse_move(x, y, cx);
        self.table_mouse_move(x, y, event.pressed_button.is_some(), cx);
        self.console_mouse_move(x, y, cx);
        self.statistics_mouse_move(x, y, event.pressed_button.is_some(), cx);
        let (shell, geometry) = (&mut self.shell, &self.geometry);
        let result = self.host.input(cx, |hub| {
            geometry.sync(hub);
            shell.window_mouse_move(hub, x, y, &mut |hub| geometry.sync(hub))
        });
        if let Err(e) = result {
            eprintln!("asrs: {e}");
        }
        self.field3d_window_move(event, cx);
        self.line_graph_mouse_move(
            x,
            y,
            event.modifiers.shift,
            event.pressed_button.is_some(),
            cx,
        );
        self.field_2d_mouse_move(
            x,
            y,
            event.modifiers.shift,
            event.pressed_button.is_some(),
            cx,
        );
        if let Some(p) = self.timeline_pointer(x, y, cx) {
            let (timeline, was_hovered) = (&mut self.timeline, self.timeline_hovered);
            self.host.input(cx, |hub| {
                let selection = hub.page_mut().selection_mut();
                if was_hovered && !p.on_overlay {
                    timeline.mouse_leave(selection);
                }
                timeline.mouse_move(p.x, p.on_overlay, p.hidden, p.target, selection);
            });
            self.timeline_hovered = p.on_overlay;
            cx.notify();
        }
        if !matches!(self.shell.cursor(), BodyCursor::Unset | BodyCursor::Initial) {
            cx.notify();
        }
    }

    /// Whether a press is on `div.title-bar`, whose `-webkit-app-region` is `drag`, and not on what
    /// is over it.
    fn drags_window(&self, event: &MouseDownEvent) -> bool {
        let at = [event.position.x, event.position.y].map(|v| f64::from(f32::from(v)));
        let holds = |r: &[f64; 4]| (0..2).all(|i| at[i] >= r[i] && at[i] < r[i] + r[i + 2]);
        self.drag_region.as_ref().is_some_and(holds) && !self.no_drag_regions.iter().any(holds)
    }

    /// The pointer left the window.
    fn timeline_mouse_exit(&mut self, cx: &mut Context<'_, Self>) {
        if std::mem::take(&mut self.timeline_hovered) {
            let timeline = &mut self.timeline;
            self.host.input(cx, |hub| {
                timeline.mouse_leave(hub.page_mut().selection_mut());
            });
            cx.notify();
        }
    }

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<'_, Self>) {
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        self.text_mouse_up(event.button, [x, y], cx);
        self.table_mouse_up(event, x, y, cx);
        self.console_mouse_up(event, x, y, cx);
        let shell = &mut self.shell;
        if let Err(e) = self.host.input(cx, |hub| shell.window_mouse_up(hub)) {
            eprintln!("asrs: {e}");
        }
        self.line_graph_mouse_up(x, event.modifiers.shift, cx);
        self.field3d_window_up(event, cx);
        if let Some(p) = self.timeline_pointer(x, y, cx) {
            let timeline = &mut self.timeline;
            self.host.input(cx, |hub| {
                timeline.mouse_up(p.on_overlay, p.hidden, hub.page_mut().selection_mut())
            });
        }
        cx.notify();
    }

    fn touch_drag(&mut self, event: &TouchDragEvent, cx: &mut Context<'_, Self>) {
        let x = f64::from(f32::from(event.position.x));
        let y = f64::from(f32::from(event.position.y));
        let touched = self.host.take_touched_field();
        let (shell, geometry) = (&mut self.shell, &self.geometry);
        let result = self.host.input(cx, |hub| {
            geometry.sync(hub);
            match event.phase {
                TouchPhase::Started => {
                    if let Some(field) = touched {
                        shell.field_touch_start(hub, field, x, y);
                    }
                    Ok(())
                }
                TouchPhase::Moved => shell.window_touch_move(hub, x, y),
                TouchPhase::Ended => shell.window_touch_end(hub),
                TouchPhase::Cancelled => Ok(()),
            }
        });
        if let Err(e) = result {
            eprintln!("asrs: {e}");
        }
        cx.notify();
    }

    fn tab_bar_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<'_, Self>) {
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        let geometry = &self.geometry;
        self.host.input(cx, |hub| {
            geometry.sync(hub);
            let (tabs, page) = hub.tabs_mut();
            tabs.overlay_mouse_down(x, y);
            if event.button == MouseButton::Right {
                tabs.overlay_context_menu(page, x, y);
            }
        });
        cx.notify();
    }

    fn tab_bar_mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<'_, Self>) {
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        let result = self.host.input(cx, |hub| {
            let (tabs, page) = hub.tabs_mut();
            tabs.overlay_mouse_up(page, x, y)
        });
        if let Err(e) = result {
            eprintln!("asrs: {e}");
        }
        cx.notify();
    }

    fn tab_bar_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        let start = self.host.input(cx, |hub| {
            let (tabs, page) = hub.tabs_mut();
            tabs.overlay_mouse_move(page, x, y)
        });
        if let Some(start) = start {
            let data = DragData::Tab(TabDragData {
                tab_index: start.tab_index as f64,
            });
            let (sx, sy, ox, oy) = (start.x, start.y, start.offset_x, start.offset_y);
            self.shell
                .start_drag(sx, sy, ox, oy, DragContent::Tab(start), data);
        }
        cx.notify();
    }

    fn tab_bar_hover(&mut self, hovered: bool, cx: &mut Context<'_, Self>) {
        if !hovered {
            self.host
                .input(cx, |hub| hub.tabs_mut().0.overlay_mouse_out());
            cx.notify();
        }
    }

    fn tab_bar_wheel(
        &mut self,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let wheel = wheel_of(event);
        let bar = self.geometry.tab_bar().borrow().bar;
        let scale = f64::from(window.scale_factor());
        let now = self.host.frame_time(cx);
        self.host.input(cx, |hub| {
            let tabs = hub.tabs_mut().0;
            tabs.tab_bar_sensor_mut()
                .set_layout([bar[2], bar[3]], scale);
            tabs.tab_bar_wheel(wheel, now);
        });
        cx.notify();
    }

    pub(super) fn tab_bar_smooth(&mut self, by: [f64; 2], cx: &mut Context<'_, Self>) {
        let now = self.host.frame_time(cx);
        self.host.input(cx, |hub| {
            hub.tabs_mut().0.tab_bar_sensor_mut().scroll_by(by, now);
        });
    }

    pub(super) fn tab_bar_range(&self) -> Option<[[f64; 2]; 2]> {
        let bar = self.geometry.tab_bar().borrow().bar;
        Some(overlay_range([bar[2], bar[3]]))
    }

    pub(super) fn timeline_smooth(&mut self, by: [f64; 2], cx: &mut Context<'_, Self>) {
        let now = self.host.frame_time(cx);
        self.timeline_sensor.scroll_by(by, now);
    }

    pub(super) fn timeline_range(&self) -> Option<[[f64; 2]; 2]> {
        self.timeline_rect.map(|[_, _, w, h]| overlay_range([w, h]))
    }

    fn timeline_mouse_down(&mut self, event: &MouseDownEvent, cx: &mut Context<'_, Self>) {
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        let Some(p) = self.timeline_pointer(x, y, cx) else {
            return;
        };
        let timeline = &mut self.timeline;
        self.host.input(cx, |hub| {
            let selection = hub.page_mut().selection_mut();
            timeline.mouse_down(p.x, event.modifiers.shift, selection);
            if event.button == MouseButton::Right {
                timeline.context_menu(selection);
            }
        });
        cx.notify();
    }

    fn timeline_click(&mut self, event: &ClickEvent, cx: &mut Context<'_, Self>) {
        let position = event.position();
        let x = mouse_client(f32::from(position.x));
        let y = mouse_client(f32::from(position.y));
        let Some(p) = self.timeline_pointer(x, y, cx) else {
            return;
        };
        let timeline = &mut self.timeline;
        self.host.input(cx, |hub| {
            timeline.click(p.x, hub.page_mut().selection_mut())
        });
        cx.notify();
    }

    fn timeline_wheel(
        &mut self,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(rect) = self.timeline_rect else {
            return;
        };
        self.timeline_sensor
            .set_layout([rect[2], rect[3]], f64::from(window.scale_factor()));
        let now = self.host.frame_time(cx);
        self.timeline_sensor.wheel(wheel_of(event), now);
        cx.notify();
    }

    /// One `Timeline.periodic()`, after the frame's scroll step for the timeline's overlay. It is
    /// part of `Tabs`' frame loop and ends with it; its scroll listener does not.
    fn paint_timeline(
        &mut self,
        metrics: &CanvasMetrics,
        dark: bool,
        cx: &mut App,
    ) -> Option<Canvas2d> {
        let now = self.host.frame_time(cx);
        let width = f64::from(metrics.client_width);
        let height = f64::from(metrics.client_height);
        let (timeline, sensor) = (&mut self.timeline, &mut self.timeline_sensor);
        sensor.animate(now);
        let scroll = sensor.scroll_step(now, width != 0.0 || height != 0.0);
        let alive = self.host.hub(cx).tabs().frame_loop_alive();
        if alive && height != 0.0 {
            sensor.periodic(now);
        }
        let host = &mut self.host;
        let canvas = with_text_backend(cx, |text, cx| {
            host.input(cx, |hub| {
                let preferences = hub.page().preferences().cloned();
                let target = ScrollTarget::new(width, height, preferences.as_ref());
                let log = hub.page().shared_log().clone();
                let selection = hub.page_mut().selection_mut();
                if let Some(d) = scroll {
                    timeline.scroll(d.dx, d.dy, target, selection);
                }
                let frame = TimelineFrame {
                    width,
                    height,
                    device_pixel_ratio: metrics.device_pixel_ratio,
                    light: !dark,
                    preferences: preferences.as_ref(),
                };
                let log = log.borrow();
                let text = text.filter(|_| alive);
                text.and_then(|text| timeline.periodic(&frame, selection, &log, text))
            })
        });
        for message in self.timeline.run_timers() {
            self.host.send_main_message(message, cx);
        }
        canvas
    }

    fn drop_files(&mut self, paths: &ExternalPaths, cx: &mut App) {
        let files = paths
            .paths()
            .iter()
            .map(|p| JsString::from(p.to_string_lossy().as_ref()))
            .collect();
        if let Some(message) = HubShell::drop_files(files) {
            self.host.send_main_message(message, cx);
        }
    }

    fn tab_control(
        &mut self,
        layout: &FrameLayout,
        element: FrameElement,
        blurred: bool,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let handle = self.places.tab_controls.iter().find(|(e, _)| *e == element);
        let handle = handle.map(|(_, handle)| handle.clone())?;
        let Some(rect) = layout.rect(element) else {
            let hidden = focus::hidden(&handle);
            let section = Section::TabControls as isize;
            return Some(hidden.tab_group().tab_index(section).into_any_element());
        };
        let (symbol, tint, fit) = match element {
            FrameElement::Update => ("update.svg", IconTint::UPDATE, IconFit::BUTTON),
            FrameElement::Feedback => (
                "feedback.svg",
                IconTint::FEEDBACK,
                IconFit::Centered {
                    max: 1.0,
                    scale: 0.9,
                },
            ),
            FrameElement::Play => ("play.svg", IconTint::BUTTON, IconFit::BUTTON),
            FrameElement::Pause => ("pause.svg", IconTint::BUTTON, IconFit::BUTTON),
            FrameElement::Lock => ("fastforwardcircle.svg", IconTint::BUTTON, IconFit::BUTTON),
            FrameElement::Unlock => ("fastforwardcircle.svg", IconTint::UNLOCK, IconFit::BUTTON),
            FrameElement::Close => ("dangerous.svg", IconTint::BUTTON, IconFit::BUTTON),
            FrameElement::Popup => ("shadowadd.svg", IconTint::BUTTON, IconFit::BUTTON),
            FrameElement::AddTab => ("add.svg", IconTint::BUTTON, IconFit::BUTTON),
            _ => return None,
        };
        let title = match element {
            FrameElement::Update => "Update available",
            FrameElement::Feedback => "Share feedback",
            FrameElement::Play => "Play",
            FrameElement::Pause => "Pause",
            FrameElement::Lock => "Lock playback to real-time",
            FrameElement::Unlock => "Unlock playback",
            FrameElement::Close => "Close tab",
            FrameElement::Popup => "Pop out tab",
            _ => "Add tab",
        };
        let symbol = Symbol::named(symbol)?;
        let entity = cx.entity().downgrade();
        let button = IconButton::new(element.selector(), symbol)
            .tint(tint)
            .fit(fit)
            .title(title)
            .blurred(blurred)
            .track_focus(&handle)
            .on_click(move |_, _, cx| {
                let _ = entity.update(cx, |frame, cx| {
                    frame.tab_control_click(element, cx);
                    cx.notify();
                });
            });
        // `-webkit-app-region: no-drag`, over the title bar.
        let place = place(rect, (0.0, 0.0))
            .tab_group()
            .tab_index(Section::TabControls as isize)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(button);
        Some(probe(element, place).into_any_element())
    }

    fn tab_control_click(&mut self, element: FrameElement, cx: &mut App) {
        let button = match element {
            FrameElement::Play => Some(ControlButton::Play),
            FrameElement::Pause => Some(ControlButton::Pause),
            FrameElement::Lock => Some(ControlButton::Lock),
            FrameElement::Unlock => Some(ControlButton::Unlock),
            _ => None,
        };
        let undefined = asrs_js::value::JsValue::Undefined;
        let result = match (element, button) {
            (_, Some(button)) => {
                self.host
                    .input(cx, |hub| hub.page_mut().selection_mut().click(button));
                Ok(())
            }
            (FrameElement::Close, _) => self.host.input(cx, |hub| {
                let (tabs, page) = hub.tabs_mut();
                tabs.close_button(page)
            }),
            (FrameElement::Popup, _) => self.host.input(cx, |hub| hub.tabs_mut().0.popup_button()),
            (FrameElement::AddTab, _) => {
                self.host.input(cx, |hub| {
                    let (tabs, page) = hub.tabs_mut();
                    tabs.add_button(page);
                });
                Ok(())
            }
            (FrameElement::Update, _) => {
                self.host
                    .send_main_message(NamedMessage::new(HubToMain::PromptUpdate, undefined), cx);
                Ok(())
            }
            (FrameElement::Feedback, _) => {
                self.host
                    .send_main_message(NamedMessage::new(HubToMain::OpenFeedback, undefined), cx);
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            eprintln!("asrs: {e}");
        }
    }
}

impl<H: FrameHost> HubFrame<H> {
    fn title_bar_menu(
        &mut self,
        rect: [f64; 4],
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let layout = MenuLayout::compute(rect[2], window.scale_factor(), cx);
        self.host
            .set_menu_buttons(layout.client_rects(rect, &self.menu_scroll));
        let clicked = cx.entity().downgrade();
        let left = clicked.clone();
        title_bar_menu::title_bar_menu(
            &layout,
            rect,
            &self.menu_scroll,
            move |index, _, cx| {
                let _ = clicked.update(cx, |frame, cx| {
                    frame.host.input(cx, |hub| {
                        hub.with_sidebar(|sidebar, env| {
                            sidebar.menu_button_click(env.geometry, index)
                        })
                    })
                });
            },
            move |index, cx| {
                let _ = left.update(cx, |frame, cx| {
                    frame.host.input(cx, |hub| {
                        hub.with_sidebar(|sidebar, _| sidebar.menu_button_mouse_leave(index))
                    })
                });
            },
            window,
            cx,
        )
        .into_any_element()
    }

    fn timeline_element(&mut self, dark: bool, cx: &mut Context<'_, Self>) -> AnyElement {
        let state = self
            .timeline_canvas
            .get_or_insert_with(|| cx.new(|cx| CanvasState::bordered(TIMELINE_BORDER, cx)))
            .clone();
        let (background, border) = if dark {
            (0x444444, 0xeeeeee)
        } else {
            (0xeeeeee, 0x222222)
        };
        let entity = cx.entity().downgrade();
        let painter = move |metrics: &CanvasMetrics, cx: &mut App| {
            entity
                .update(cx, |frame, cx| frame.paint_timeline(metrics, dark, cx))
                .ok()
                .flatten()
        };
        div()
            .id("timeline-scroll")
            .capture_any_mouse_down(cx.listener(|frame, _, _, _| frame.press_area(&Area::Timeline)))
            .size_full()
            .child(
                div()
                    .absolute()
                    .size_full()
                    .rounded(px(TIMELINE_BORDER.radius))
                    .border(px(TIMELINE_BORDER.width))
                    .border_color(rgb(border))
                    .bg(rgb(background)),
            )
            .child(canvas_view(state, painter).size_full())
            .on_any_mouse_down(cx.listener(|frame, event: &MouseDownEvent, _, cx| {
                frame.timeline_mouse_down(event, cx)
            }))
            .on_click(
                cx.listener(|frame, event: &ClickEvent, _, cx| frame.timeline_click(event, cx)),
            )
            .on_scroll(cx.listener(|frame, event: &ScrollEvent, window, cx| {
                frame.timeline_wheel(event, window, cx)
            }))
            .into_any_element()
    }
}

impl<H: FrameHost> Render for HubFrame<H> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        if self.tab_key.is_none() {
            self.tab_key = Some(Self::intercept_tab(window, cx));
        }
        let appearance = Appearance::of(window, cx);
        let theme = appearance.theme();
        let dark = appearance == Appearance::Dark;
        self.geometry.set_dark(dark);
        let size = window.viewport_size();
        let (width, height) = (
            f64::from(f32::from(size.width)),
            f64::from(f32::from(size.height)),
        );
        let scale = window.scale_factor();

        let hub = self.host.hub(cx);
        let show_fps = hub.show_fps();
        let shadow_shown = hub.sidebar().shadow_shown();
        let title =
            SharedString::from(crate::IDENTITY.window_title(&hub.title().to_string_lossy()));
        let blurred = !hub.is_focused();
        let now = self.started.elapsed().as_secs_f64() * 1000.0;
        self.shell.fps_mut().frame(now, show_fps);
        self.line_graph_timers(cx);
        self.field3d_frame(window, cx);
        self.text_frame(window, cx);
        self.scrollers_frame();
        if show_fps {
            window.request_animation_frame();
        }
        let fps_text = self
            .shell
            .fps()
            .text()
            .map(SharedString::from)
            .filter(|_| !self.shell.fps().hidden());
        let fps_size = fps_text
            .as_ref()
            .map_or((0.0, 0.0), |t| text::measure(cx, &fps_style(), t, scale));
        let inputs = self.inputs(cx, width, height, f64::from(scale), fps_size);
        let satellite = self.host.hub(cx).page().is_satellite();
        let compute = |inputs: &FrameInputs| {
            if satellite {
                FrameLayout::satellite(width, height)
            } else {
                FrameLayout::compute(inputs)
            }
        };
        let layout = compute(&inputs);
        self.drag_region = layout.rect(FrameElement::TitleBar);
        self.no_drag_regions = TAB_CONTROLS
            .into_iter()
            .chain([FrameElement::TitleBarMenu])
            .filter_map(|element| layout.rect(element))
            .collect();
        if self.viewport.replace((width, height)) != Some((width, height)) {
            let resized = cx.entity().downgrade();
            window.on_next_frame(move |_, cx| {
                let _ = resized.update(cx, |frame, cx| frame.host.resized(cx));
            });
        }
        self.geometry.set_inner_height(height);
        self.geometry.0.borrow_mut().large_table = compute(&FrameInputs {
            show_timeline: Some(false),
            show_tab_controls: Some(false),
            tab_controls_height: Some(0.0),
            ..inputs.clone()
        })
        .rect(FrameElement::RendererContent)
        .map(|[x, y, width, height]| {
            let rect = Rect {
                x,
                y,
                width,
                height,
            };
            (rect, f64::from(scale))
        });
        self.geometry.sync(self.host.hub(cx));
        let titles: Vec<TitleElement> = self.host.hub(cx).tabs().bar().map(|e| e.clone()).collect();
        if let Some(bar) = layout.rect(FrameElement::TabBar) {
            let texts: Vec<String> = titles
                .iter()
                .map(|t| t.inner_text().to_string_lossy())
                .collect();
            let mut geometry = self.geometry.tab_bar().borrow_mut();
            geometry.layout(cx, bar, &texts, scale);
            if let Some(add) = layout.rect(FrameElement::AddTab) {
                geometry.add_button = add;
            }
        }
        self.timeline_rect = layout
            .rect(FrameElement::Timeline)
            .filter(|_| self.host.hub(cx).tabs().show_timeline() != Some(false));
        if self.timeline_sensor.animating() || self.host.hub(cx).tabs().tab_bar_sensor().animating()
        {
            window.request_animation_frame();
        }
        let classes = inputs.classes;

        let entity = cx.entity().downgrade();
        // Window listeners, not hover-gated element ones: a press that leaves the window keeps
        // delivering moves and its release, as Chromium's mouse capture does upstream.
        let drawn = entity.clone();
        window.defer(cx, move |window, cx| {
            let _ = drawn.update(cx, |frame, cx| {
                let body = frame.body.clone();
                let builds = frame.host.hub(cx).tabs().bar_builds();
                if std::mem::replace(&mut frame.bar_builds, builds) != builds
                    && frame.titles.iter().any(|title| title.is_focused(window))
                {
                    frame.places.tab_bar.focus(window, cx);
                }
                frame.traversal.drawn(&body, window, cx);
            });
        });
        let window_listeners = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let frame = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = frame.update(cx, |frame, cx| frame.mouse_move(event, window, cx));
                    }
                });
                let frame = entity.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = frame.update(cx, |frame, cx| frame.mouse_up(event, cx));
                    }
                });
                let frame = entity.clone();
                window.on_mouse_event(move |event: &TouchDragEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = frame.update(cx, |frame, cx| frame.touch_drag(event, cx));
                    }
                });
                let frame = entity.clone();
                window.on_mouse_event(move |event: &MouseExitEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble && event.pressed_button.is_none() {
                        let _ = frame.update(cx, |frame, cx| {
                            frame.table_mouse_exit(cx);
                            frame.console_mouse_exit(cx);
                            frame.statistics_mouse_exit(cx);
                            frame.timeline_mouse_exit(cx);
                            frame.line_graph_mouse_exit(cx);
                            frame.field_2d_mouse_exit(cx);
                        });
                    }
                });
                let frame = entity.clone();
                // Before the press focuses what it is on.
                window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = frame.update(cx, |frame, cx| {
                            if frame.drags_window(event) {
                                // A page gets no events over its draggable regions.
                                window.prevent_default();
                                return;
                            }
                            frame.traversal.pressed();
                            frame.press_outside();
                            if !frame.on_scrollbar(event.position) {
                                frame.text_mouse_down(event, window, cx);
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let mut root = div()
            .id("hub-frame")
            .track_focus(&self.body)
            .capture_key_down(cx.listener(Self::key_down))
            .capture_key_up(cx.listener(Self::key_up))
            .on_key_down(cx.listener(Self::scroll_key))
            .relative()
            .size_full()
            .on_action(cx.listener(|frame, _: &selection::Copy, _, cx| frame.copy_text(cx)))
            .on_action(
                cx.listener(|frame, _: &selection::SelectAll, _, cx| frame.select_all_text(cx)),
            )
            .child(window_listeners);
        if !satellite {
            root = root.on_drop(
                cx.listener(|frame, paths: &ExternalPaths, _, cx| frame.drop_files(paths, cx)),
            );
        }
        root = match self.shell.cursor() {
            BodyCursor::ColResize => root.cursor(CursorStyle::ResizeColumn),
            BodyCursor::RowResize => root.cursor(CursorStyle::ResizeRow),
            BodyCursor::Grabbing => root.cursor(CursorStyle::ClosedHand),
            BodyCursor::Unset | BodyCursor::Initial if self.text.text_cursor() => {
                root.cursor(CursorStyle::IBeam)
            }
            BodyCursor::Unset | BodyCursor::Initial => root,
        };

        if let Some(rect) = layout.rect(FrameElement::SideBarBackground) {
            let mut bg = place(rect, (0.0, 0.0)).bg(rgb(if dark { 0x292929 } else { 0xe9e9e9 }));
            if classes.side_bar_win {
                bg = bg.opacity(0.5);
            }
            root = root.child(probe(FrameElement::SideBarBackground, bg));
        }
        if let Some(rect) = layout.rect(FrameElement::SideBarShadow)
            && shadow_shown
        {
            let shadow = if dark {
                box_shadow(0x000000ff, 0.0, 0.7)
            } else {
                box_shadow(0x777777ff, 3.0, 0.0)
            };
            root = root.child(probe(
                FrameElement::SideBarShadow,
                place(rect, (0.0, 0.0)).shadow(vec![shadow]),
            ));
        }
        if let Some(rect) = layout.rect(FrameElement::ViewerBackground) {
            let mut bg = place(rect, (0.0, 0.0)).bg(rgb(theme.background));
            if !classes.side_bar_mac_27 {
                bg = bg.shadow(vec![if dark {
                    box_shadow(0x000000ff, 0.0, 1.0)
                } else {
                    box_shadow(0x00000022, 3.0, 0.0)
                }]);
            }
            root = root.child(probe(FrameElement::ViewerBackground, bg));
        }
        if let Some(rect) = layout.rect(FrameElement::SideBar) {
            self.geometry.0.borrow_mut().side_bar = rect;
            let side_bar = place(rect, (0.0, 0.0))
                .capture_any_mouse_down(
                    cx.listener(|frame, _, _, _| frame.press_area(&Area::SideBar)),
                )
                .overflow_hidden();
            let mut side_bar =
                focus::section(side_bar, Section::SideBar as isize, &self.places.side_bar);
            if let Some(content) = self.host.slot(FrameSlot::SideBar, window, cx) {
                side_bar = side_bar.child(content);
            }
            root = root.child(probe(FrameElement::SideBar, side_bar));
        }
        if let Some(viewer_rect) = layout.rect(FrameElement::Viewer) {
            let origin = (viewer_rect[0], viewer_rect[1]);
            let viewer = place(viewer_rect, (0.0, 0.0)).overflow_hidden();
            let mut viewer = focus::section(viewer, Section::Viewer as isize, &self.places.viewer);
            if let Some(rect) = layout.rect(FrameElement::TabBar) {
                let content = match self.host.slot(FrameSlot::TabBar, window, cx) {
                    Some(content) => content,
                    None => {
                        let focus: Vec<FocusRing> = (0..titles.len())
                            .map(|i| tab_bar::title_focus(i, window, cx))
                            .collect();
                        self.titles = focus.iter().map(|f| f.handle.clone()).collect();
                        let entity = cx.entity().downgrade();
                        let on_key =
                            move |i, event: &KeyDownEvent, _: &mut Window, cx: &mut App| {
                                let _ = entity
                                    .update(cx, |frame, cx| frame.title_key_down(i, event, cx));
                            };
                        let geometry = self.geometry.tab_bar().borrow();
                        tab_bar::titles(&geometry, &titles, &focus, on_key, appearance)
                    }
                };
                let bar = focus::section(
                    place(rect, origin),
                    ViewerSection::TabBar as isize,
                    &self.places.tab_bar,
                );
                viewer = viewer.child(probe(FrameElement::TabBar, bar.child(content)));
            }
            let shadows = self.host.hub(cx).tabs().shadows();
            for (element, left, opacity) in [
                (FrameElement::TabBarShadowLeft, true, shadows[0]),
                (FrameElement::TabBarShadowRight, false, shadows[1]),
            ] {
                if let Some(rect) = layout.rect(element) {
                    viewer = viewer.child(tab_bar::shadow(rect, origin, left, opacity, dark));
                }
            }
            let highlight = self.host.hub(cx).tabs().drag_highlight();
            if let (false, Some(left)) = (highlight.hidden, highlight.left) {
                viewer = viewer.child(tab_bar::drag_highlight(left, origin, origin));
            }
            if let Some(rect) = layout.rect(FrameElement::TabBarScroll) {
                let overlay = focus::section(
                    place(rect, origin),
                    ViewerSection::TabBarScroll as isize,
                    &self.places.tab_bar_scroll,
                );
                viewer = viewer.child(
                    overlay
                        .id("tab-bar-scroll")
                        .capture_any_mouse_down(
                            cx.listener(|frame, _, _, _| frame.press_area(&Area::TabBar)),
                        )
                        .on_any_mouse_down(cx.listener(|frame, event: &MouseDownEvent, _, cx| {
                            frame.tab_bar_mouse_down(event, cx)
                        }))
                        .capture_any_mouse_up(cx.listener(|frame, event: &MouseUpEvent, _, cx| {
                            frame.tab_bar_mouse_up(event, cx)
                        }))
                        .on_mouse_move(cx.listener(|frame, event: &MouseMoveEvent, _, cx| {
                            frame.tab_bar_mouse_move(event, cx)
                        }))
                        .on_hover(cx.listener(|frame, hovered: &bool, _, cx| {
                            frame.tab_bar_hover(*hovered, cx)
                        }))
                        .on_scroll(cx.listener(|frame, event: &ScrollEvent, window, cx| {
                            frame.tab_bar_wheel(event, window, cx)
                        })),
                );
            }
            if let Some(rect) = layout.rect(FrameElement::RendererContent) {
                let mut renderer = focus::section(
                    place(rect, origin),
                    ViewerSection::RendererContent as isize,
                    &self.places.renderer_content,
                );
                if let Some(content) = self.host.slot(FrameSlot::RendererContent, window, cx) {
                    renderer = renderer.child(content);
                }
                if let Some(graph) = self.line_graph_element(rect, appearance, scale, window, cx) {
                    renderer = renderer.child(graph);
                }
                let covered = self.timeline_rect;
                if let Some(field) = self.field_2d_element(rect, covered, scale, window, cx) {
                    renderer = renderer.child(field);
                }
                if let Some(field) = self.field3d_element(rect, appearance, scale, window, cx) {
                    renderer = renderer.child(field);
                }
                if let Some(table) = self.table_element(rect, appearance, scale, window, cx) {
                    renderer = renderer.child(table);
                }
                if let Some(console) = self.console_element(rect, appearance, scale, window, cx) {
                    renderer = renderer.child(console);
                }
                if let Some(page) = self.documentation_element(rect, appearance, scale, window, cx)
                {
                    renderer = renderer.child(page);
                }
                if let Some(statistics) =
                    self.statistics_element(rect, covered, appearance, scale, window, cx)
                {
                    renderer = renderer.child(statistics);
                }
                if let Some(metadata) = self.metadata_element(rect, appearance, scale, window, cx) {
                    renderer = renderer.child(metadata);
                }
                if let Some(tab) = self.canvas_tab_element(rect, appearance, scale, window, cx) {
                    renderer = renderer.child(tab);
                }
                if let Some(video) = self.video_element(rect, scale, window, cx) {
                    renderer = renderer.child(video);
                }
                viewer = viewer.child(probe(FrameElement::RendererContent, renderer));
            }
            if let Some(rect) = layout.rect(FrameElement::ControlsContent) {
                let mut controls = focus::section(
                    place(rect, origin).bg(rgb(theme.background)),
                    ViewerSection::ControlsContent as isize,
                    &self.places.controls_content,
                );
                if dark {
                    controls = controls.children(spread_rows(CONTROLS_SPREAD, scale));
                } else {
                    controls = controls.shadow(vec![box_shadow(0x00000022, 3.0, 2.0)]);
                }
                if let Some(content) = self.host.slot(FrameSlot::ControlsContent, window, cx) {
                    controls = controls.child(content);
                }
                self.layout_source_lists(rect, width, scale, cx);
                controls = controls.children(self.source_lists(rect, appearance, window, cx));
                if let Some(settings) = self.field_2d_controls(rect, appearance, scale, window, cx)
                {
                    controls = controls.child(settings);
                }
                if let Some(settings) = self.field3d_settings(rect, appearance, scale, window, cx) {
                    controls = controls.child(settings);
                }
                if let Some(settings) =
                    self.statistics_controls(rect, appearance, scale, window, cx)
                {
                    controls = controls.child(settings);
                }
                if let Some(tab) = self.canvas_tab_controls(rect, appearance, scale, window, cx) {
                    controls = controls.child(tab);
                }
                if let Some(video) = self.video_controls(rect, appearance, scale, window, cx) {
                    controls = controls.child(video);
                }
                viewer = viewer.child(probe(FrameElement::ControlsContent, controls));
            }
            if let Some(rect) = layout.rect(FrameElement::ControlsHandle) {
                let handle = focus::section(
                    place(rect, origin),
                    ViewerSection::ControlsHandle as isize,
                    &self.places.controls_handle,
                );
                viewer = viewer.child(probe(
                    FrameElement::ControlsHandle,
                    handle
                        .id("controls-handle")
                        .occlude()
                        .cursor(CursorStyle::ResizeRow)
                        .on_any_mouse_down(cx.listener(|frame, _: &MouseDownEvent, _, cx| {
                            frame.handle_down(Handle::Controls, cx)
                        }))
                        .on_click(
                            cx.listener(|frame, _, _, cx| frame.handle_click(Handle::Controls, cx)),
                        ),
                ));
            }
            if let Some(rect) = layout.rect(FrameElement::Timeline) {
                let content = match self.host.slot(FrameSlot::Timeline, window, cx) {
                    Some(content) => content,
                    None => self.timeline_element(dark, cx),
                };
                let timeline = focus::section(
                    place(rect, origin),
                    ViewerSection::Timeline as isize,
                    &self.places.timeline,
                );
                viewer = viewer.child(probe(FrameElement::Timeline, timeline.child(content)));
            }
            root = root.child(probe(FrameElement::Viewer, viewer));
        }
        if let Some(rect) = layout.rect(FrameElement::TitleBar) {
            let bar = focus::section(
                place(rect, (0.0, 0.0)),
                Section::TitleBar as isize,
                &self.places.title_bar,
            );
            let mut bar = bar.id("title-bar").on_mouse_down(
                MouseButton::Left,
                |event: &MouseDownEvent, window, _| {
                    if event.click_count == 2 {
                        window.titlebar_double_click();
                    } else {
                        window.start_window_move();
                    }
                },
            );
            if classes.title_bar_win {
                bar = bar.window_control_area(WindowControlArea::Drag);
            }
            if let Some(text_rect) = layout.rect(FrameElement::TitleBarText) {
                bar = bar.child(probe(
                    FrameElement::TitleBarText,
                    Text::new(title, title_style())
                        .color(theme.text)
                        .probe("title-bar-text")
                        .absolute()
                        .left(px((text_rect[0] - rect[0]) as f32))
                        .top(px((text_rect[1] - rect[1]) as f32))
                        .w(px(text_rect[2] as f32))
                        .h(px(text_rect[3] as f32)),
                ));
            }
            root = root.child(probe(FrameElement::TitleBar, bar));
        }
        if let Some(rect) = layout.rect(FrameElement::TitleBarMenu) {
            let menu = self.title_bar_menu(rect, window, cx);
            let section = focus::section(
                place(rect, (0.0, 0.0)),
                Section::TitleBarMenu as isize,
                &self.places.title_bar_menu,
            );
            let menu = section.occlude().child(menu);
            root = root.child(probe(FrameElement::TitleBarMenu, menu));
        }
        let controls = if satellite { &[][..] } else { &TAB_CONTROLS };
        for &element in controls {
            if let Some(button) = self.tab_control(&layout, element, blurred, cx) {
                root = root.child(button);
            }
        }
        if let Some(rect) = layout.rect(FrameElement::LoadingGlow)
            && rect[1] + rect[3] > 0.0
            && rect[2] > 0.0
        {
            root = root.child(probe(
                FrameElement::LoadingGlow,
                place(rect, (0.0, 0.0)).bg(rgb(GLOW)),
            ));
        }
        if let Some(rect) = layout.rect(FrameElement::SideBarHandle) {
            let handle = focus::section(
                place(rect, (0.0, 0.0)),
                Section::SideBarHandle as isize,
                &self.places.side_bar_handle,
            );
            root = root.child(probe(
                FrameElement::SideBarHandle,
                handle
                    .id("side-bar-handle")
                    .occlude()
                    .cursor(CursorStyle::ResizeColumn)
                    .on_any_mouse_down(cx.listener(|frame, _: &MouseDownEvent, _, cx| {
                        frame.handle_down(Handle::SideBar, cx)
                    }))
                    .on_click(
                        cx.listener(|frame, _, _, cx| frame.handle_click(Handle::SideBar, cx)),
                    ),
            ));
        }
        if let (Some(rect), Some(fps)) = (layout.rect(FrameElement::Fps), fps_text) {
            root = root.child(probe(
                FrameElement::Fps,
                Text::new(fps, fps_style())
                    .color(FPS_COLOR)
                    .absolute()
                    .left(px(rect[0] as f32))
                    .top(px(rect[1] as f32)),
            ));
        }
        let drag = self.shell.drag();
        if !drag.item_hidden()
            && let (Some((x, y)), Some(content)) = (drag.item_position(), drag.item().cloned())
        {
            let item = div()
                .absolute()
                .left(px(x as f32))
                .top(px(y as f32))
                .cursor(CursorStyle::ClosedHand);
            let item = match content {
                DragContent::Field(field) => item
                    .child(
                        Text::new(field.text.to_string_lossy(), drag_text_style(field.bold))
                            .color(theme.text),
                    )
                    .into_any_element(),
                DragContent::Tab(start) => {
                    let tab = match self.host.slot(FrameSlot::DragTab, window, cx) {
                        Some(tab) => tab,
                        None => {
                            let classes = if start.selected {
                                vec!["tab", "tab-selected"]
                            } else {
                                vec!["tab"]
                            };
                            let element = TitleElement {
                                text: start.text.clone(),
                                classes,
                            };
                            let label = element.inner_text().to_string_lossy();
                            let width =
                                text::measure(cx, &tab_bar::tab_style(), &label, scale).0 + 22.0;
                            tab_bar::tab(&element, width as f32, 0.0, true, true, dark, theme.text)
                                .into_any_element()
                        }
                    };
                    item.opacity(0.5).child(tab).into_any_element()
                }
                DragContent::Column {
                    text,
                    available,
                    width,
                } => item
                    .child(table::drag_column(
                        &text, available, width, appearance, scale, cx,
                    ))
                    .into_any_element(),
                DragContent::ListItem {
                    item: list_item,
                    width,
                } => item
                    .child(source_list::drag_item(
                        &list_item, width, blurred, appearance, cx,
                    ))
                    .into_any_element(),
            };
            root = root.child(item);
        }
        if classes.title_bar_win && !window.is_fullscreen() {
            root = root.child(caption_buttons(dark, window, cx));
        }
        root
    }
}
