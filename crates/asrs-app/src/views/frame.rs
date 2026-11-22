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
use asrs_model::scroll_sensor::{ScrollSensor, Wheel};
use asrs_model::selection_impl::ControlButton;
use asrs_ui::button::IconButton;
use asrs_ui::canvas_view::geometry::mouse_client;
use asrs_ui::canvas_view::{CanvasMetrics, CanvasState, canvas_view, with_text_backend};
use asrs_ui::gpui::{
    AnyElement, App, AppContext, BoxShadow, ClickEvent, Context, CursorStyle, DispatchPhase,
    Entity, ExternalPaths, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Render, ScrollDelta, ScrollEvent, SharedString,
    StatefulInteractiveElement, Styled, Window, canvas, div, point, px, rgb, rgba,
};
use asrs_ui::icon::IconFit;
use asrs_ui::parity::{Probed, probed};
use asrs_ui::platform;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, Text, TextStyle};
use asrs_ui::theme::{Appearance, FONT_FAMILY, IconTint, MONOSPACE_FAMILY};

use super::line_graph::LineGraphViews;
use super::source_list::{self, ListsGeometry};
use super::tab_bar::{self, TabBarGeometry};

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
    /// The hub's frame clock (ms), which its scroll sensors animate on.
    fn frame_time(&self, cx: &App) -> f64;
    /// Runs `f`, the frame's handler for an input event, as one page task.
    fn input<R>(&mut self, cx: &mut App, f: impl FnOnce(&mut HubModel) -> R) -> R;
    fn send_main_message(&mut self, message: NamedMessage<HubToMain>, cx: &mut App);
    /// The layout the hub model's `HubView` answers source list reads from.
    fn source_list_geometry(&self) -> ListsGeometry;
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
}

impl FrameGeometry {
    pub fn tab_bar(&self) -> &RefCell<TabBarGeometry> {
        &self.1
    }

    pub fn sync(&self, hub: &HubModel) {
        self.0.borrow_mut().classes = BodyClasses::from_names(hub.body_classes().iter().copied());
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

    /// `div.side-bar`'s box as last laid out.
    pub fn side_bar(&self) -> [f64; 4] {
        self.0.borrow().side_bar
    }
}

pub struct HubFrame<H: FrameHost> {
    host: H,
    shell: HubShell,
    started: Instant,
    geometry: FrameGeometry,
    timeline: Timeline,
    timeline_sensor: ScrollSensor,
    timeline_canvas: Option<Entity<CanvasState>>,
    timeline_rect: Option<[f64; 4]>,
    timeline_hovered: bool,
    pub(super) line_graph: LineGraphViews,
}

/// The timeline overlay's view of one pointer position: the client x relative to the overlay
/// and whether the pointer is over it.
struct TimelinePointer {
    x: f64,
    on_overlay: bool,
    hidden: bool,
    target: ScrollTarget,
}

pub(super) fn wheel_of(event: &ScrollEvent) -> Wheel {
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

fn place(rect: [f64; 4], origin: (f64, f64)) -> asrs_ui::gpui::Div {
    div()
        .absolute()
        .left(px((rect[0] - origin.0) as f32))
        .top(px((rect[1] - origin.1) as f32))
        .w(px(rect[2] as f32))
        .h(px(rect[3] as f32))
}

impl<H: FrameHost> HubFrame<H> {
    pub fn new(host: H, cx: &App) -> Self {
        let geometry = host.geometry();
        geometry.sync(host.hub(cx));
        HubFrame {
            host,
            shell: HubShell::default(),
            started: Instant::now(),
            geometry,
            timeline: Timeline::new(Rc::new(asrs_js::time::SystemClock)),
            timeline_sensor: ScrollSensor::default(),
            timeline_canvas: None,
            timeline_rect: None,
            timeline_hovered: false,
            line_graph: LineGraphViews::default(),
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

    fn now(&self) -> f64 {
        asrs_js::math::trunc(asrs_js::time::Clock::now(&asrs_js::time::SystemClock))
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
        let now = self.now();
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

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        self.source_list_mouse_move(x, y, cx);
        let (shell, geometry) = (&mut self.shell, &self.geometry);
        let result = self.host.input(cx, |hub| {
            geometry.sync(hub);
            shell.window_mouse_move(hub, x, y, &mut |hub| geometry.sync(hub))
        });
        if let Err(e) = result {
            eprintln!("asrs: {e}");
        }
        self.line_graph_mouse_move(x, y, event.modifiers.shift, cx);
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

    fn mouse_up(&mut self, event: &MouseUpEvent, cx: &mut Context<'_, Self>) {
        let x = mouse_client(f32::from(event.position.x));
        let y = mouse_client(f32::from(event.position.y));
        let shell = &mut self.shell;
        if let Err(e) = self.host.input(cx, |hub| shell.window_mouse_up(hub)) {
            eprintln!("asrs: {e}");
        }
        self.line_graph_mouse_up(x, event.modifiers.shift, cx);
        if let Some(p) = self.timeline_pointer(x, y, cx) {
            let timeline = &mut self.timeline;
            self.host.input(cx, |hub| {
                timeline.mouse_up(p.on_overlay, p.hidden, hub.page_mut().selection_mut())
            });
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

    /// One `Timeline.periodic()`, after the frame's scroll step for the timeline's overlay.
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
        if height != 0.0 {
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
        let rect = layout.rect(element)?;
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
        let symbol = Symbol::named(symbol)?;
        let entity = cx.entity().downgrade();
        let button = IconButton::new(element.selector(), symbol)
            .tint(tint)
            .fit(fit)
            .blurred(blurred)
            .on_click(move |_, _, cx| {
                let _ = entity.update(cx, |frame, cx| {
                    frame.tab_control_click(element, cx);
                    cx.notify();
                });
            });
        Some(probe(element, place(rect, (0.0, 0.0)).child(button)).into_any_element())
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
    fn timeline_element(&mut self, dark: bool, cx: &mut Context<'_, Self>) -> AnyElement {
        let state = self
            .timeline_canvas
            .get_or_insert_with(|| cx.new(CanvasState::new))
            .clone();
        let entity = cx.entity().downgrade();
        let painter = move |metrics: &CanvasMetrics, cx: &mut App| {
            entity
                .update(cx, |frame, cx| frame.paint_timeline(metrics, dark, cx))
                .ok()
                .flatten()
        };
        div()
            .id("timeline-scroll")
            .size_full()
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
        let appearance = Appearance::of(window, cx);
        let theme = appearance.theme();
        let dark = appearance == Appearance::Dark;
        let size = window.viewport_size();
        let (width, height) = (
            f64::from(f32::from(size.width)),
            f64::from(f32::from(size.height)),
        );
        let scale = window.scale_factor();

        let hub = self.host.hub(cx);
        let show_fps = hub.show_fps();
        let shadow_shown = hub.sidebar().shadow_shown();
        let title = SharedString::from(hub.title().to_string_lossy());
        let blurred = !hub.is_focused();
        let now = self.started.elapsed().as_secs_f64() * 1000.0;
        self.shell.fps_mut().frame(now, show_fps);
        self.line_graph_timers(cx);
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
        let layout = FrameLayout::compute(&inputs);
        self.geometry.set_inner_height(height);
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
        let window_listeners = canvas(
            |_, _, _| {},
            move |_, _, window, _| {
                let frame = entity.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = frame.update(cx, |frame, cx| frame.mouse_move(event, cx));
                    }
                });
                window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = entity.update(cx, |frame, cx| frame.mouse_up(event, cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();
        let mut root = div()
            .id("hub-frame")
            .relative()
            .size_full()
            .child(window_listeners)
            .on_drop(
                cx.listener(|frame, paths: &ExternalPaths, _, cx| frame.drop_files(paths, cx)),
            );
        root = match self.shell.cursor() {
            BodyCursor::ColResize => root.cursor(CursorStyle::ResizeColumn),
            BodyCursor::RowResize => root.cursor(CursorStyle::ResizeRow),
            BodyCursor::Grabbing => root.cursor(CursorStyle::ClosedHand),
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
            let mut side_bar = place(rect, (0.0, 0.0)).overflow_hidden();
            if let Some(content) = self.host.slot(FrameSlot::SideBar, window, cx) {
                side_bar = side_bar.child(content);
            }
            root = root.child(probe(FrameElement::SideBar, side_bar));
        }
        if let Some(viewer_rect) = layout.rect(FrameElement::Viewer) {
            let origin = (viewer_rect[0], viewer_rect[1]);
            let mut viewer = place(viewer_rect, (0.0, 0.0)).overflow_hidden();
            if let Some(rect) = layout.rect(FrameElement::TabBar) {
                let content = match self.host.slot(FrameSlot::TabBar, window, cx) {
                    Some(content) => content,
                    None => {
                        let geometry = self.geometry.tab_bar().borrow();
                        tab_bar::titles(&geometry, &titles, dark, theme.text)
                    }
                };
                viewer = viewer.child(probe(
                    FrameElement::TabBar,
                    place(rect, origin).child(content),
                ));
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
                viewer = viewer.child(
                    place(rect, origin)
                        .id("tab-bar-scroll")
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
                let mut renderer = place(rect, origin);
                if let Some(content) = self.host.slot(FrameSlot::RendererContent, window, cx) {
                    renderer = renderer.child(content);
                }
                if let Some(graph) = self.line_graph_element(rect, appearance, scale, window, cx) {
                    renderer = renderer.child(graph);
                }
                viewer = viewer.child(probe(FrameElement::RendererContent, renderer));
            }
            if let Some(rect) = layout.rect(FrameElement::ControlsContent) {
                let shadow = if dark {
                    box_shadow(0x000000ff, 0.0, 0.7)
                } else {
                    box_shadow(0x00000022, 3.0, 2.0)
                };
                let mut controls = place(rect, origin)
                    .bg(rgb(theme.background))
                    .shadow(vec![shadow]);
                if let Some(content) = self.host.slot(FrameSlot::ControlsContent, window, cx) {
                    controls = controls.child(content);
                }
                self.layout_source_lists(rect, width, scale, cx);
                controls = controls.children(self.source_lists(rect, appearance, cx));
                viewer = viewer.child(probe(FrameElement::ControlsContent, controls));
            }
            if let Some(rect) = layout.rect(FrameElement::ControlsHandle) {
                viewer = viewer.child(probe(
                    FrameElement::ControlsHandle,
                    place(rect, origin)
                        .id("controls-handle")
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
                viewer = viewer.child(probe(
                    FrameElement::Timeline,
                    place(rect, origin).child(content),
                ));
            }
            root = root.child(probe(FrameElement::Viewer, viewer));
        }
        if let Some(rect) = layout.rect(FrameElement::TitleBar) {
            let mut bar = place(rect, (0.0, 0.0)).id("title-bar").on_mouse_down(
                MouseButton::Left,
                |event: &MouseDownEvent, window, _| {
                    if event.click_count == 2 {
                        window.titlebar_double_click();
                    } else {
                        window.start_window_move();
                    }
                },
            );
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
        for element in [
            FrameElement::Update,
            FrameElement::Feedback,
            FrameElement::Play,
            FrameElement::Pause,
            FrameElement::Lock,
            FrameElement::Unlock,
            FrameElement::Close,
            FrameElement::Popup,
            FrameElement::AddTab,
        ] {
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
            root = root.child(probe(
                FrameElement::SideBarHandle,
                place(rect, (0.0, 0.0))
                    .id("side-bar-handle")
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
                            tab_bar::tab(&element, width, true, true, dark, theme.text)
                                .into_any_element()
                        }
                    };
                    item.opacity(0.5).child(tab).into_any_element()
                }
                DragContent::ListItem {
                    item: list_item,
                    width,
                } => item
                    .child(source_list::drag_item(&list_item, width, appearance))
                    .into_any_element(),
            };
            root = root.child(item);
        }
        root
    }
}
