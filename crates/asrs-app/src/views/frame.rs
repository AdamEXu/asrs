//! The hub window's frame (`www/hub.html` and the shell rules of `www/hub.css`): backgrounds,
//! title bar, tab control buttons, loading glow, resize handles, FPS counter, drag item and
//! file drop. Every box comes from `asrs_hub::frame::FrameLayout`; the views that fill the
//! sidebar, tab bar, renderer, controls and timeline plug into [`FrameSlot`]s.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use asrs_hub::frame::{
    BodyClasses, BodyCursor, DragContent, FrameElement, FrameInputs, FrameLayout, Handle, HubShell,
    client_coordinate, renderer_top,
};
use asrs_hub::hub::HubModel;
use asrs_js::string::JsString;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::selection_impl::ControlButton;
use asrs_model::source_list::Rect;
use asrs_ui::button::IconButton;
use asrs_ui::gpui::{
    AnyElement, App, BoxShadow, Context, CursorStyle, DispatchPhase, ExternalPaths,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div,
    point, px, rgb, rgba,
};
use asrs_ui::icon::IconFit;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, Text, TextStyle};
use asrs_ui::theme::{Appearance, FONT_FAMILY, IconTint, MONOSPACE_FAMILY};

/// The boxes of the frame elements a scenario probe selector (`.title-bar`) names, as
/// `getBoundingClientRect()` gives them for each rendered match; `None` when the selector names
/// no frame element.
pub fn probe_rects(layout: &FrameLayout, selector: &str) -> Option<Vec<[f64; 4]>> {
    let element = FrameElement::ALL.into_iter().find(|e| {
        let full = e.selector();
        full == selector || full.find('.').is_some_and(|dot| &full[dot..] == selector)
    })?;
    Some(layout.rect(element).into_iter().collect())
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
    /// Runs `f`, the frame's handler for an input event, as one page task.
    fn input<R>(&mut self, cx: &mut App, f: impl FnOnce(&mut HubModel) -> R) -> R;
    fn send_main_message(&mut self, message: NamedMessage<HubToMain>, cx: &mut App);
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
pub struct FrameGeometry(Rc<RefCell<GeometryState>>);

#[derive(Debug, Default)]
struct GeometryState {
    classes: BodyClasses,
    inner_height: f64,
    side_bar: [f64; 4],
}

impl FrameGeometry {
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

    fn mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<'_, Self>) {
        let x = client_coordinate(f64::from(f32::from(event.position.x)));
        let y = client_coordinate(f64::from(f32::from(event.position.y)));
        let (shell, geometry) = (&mut self.shell, &self.geometry);
        let result = self.host.input(cx, |hub| {
            geometry.sync(hub);
            shell.window_mouse_move(hub, x, y, &mut |hub| geometry.sync(hub))
        });
        if let Err(e) = result {
            eprintln!("asrs: {e}");
        }
        if !matches!(self.shell.cursor(), BodyCursor::Unset | BodyCursor::Initial) {
            cx.notify();
        }
    }

    fn mouse_up(&mut self, cx: &mut Context<'_, Self>) {
        let shell = &mut self.shell;
        self.host.input(cx, |hub| shell.window_mouse_up(hub));
        cx.notify();
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
                    frame.tab_control_click(element, rect, cx);
                    cx.notify();
                });
            });
        Some(place(rect, (0.0, 0.0)).child(button).into_any_element())
    }

    fn tab_control_click(&mut self, element: FrameElement, rect: [f64; 4], cx: &mut App) {
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
                    hub.tabs_mut().0.add_button(&Rect {
                        x: rect[0],
                        y: rect[1],
                        width: rect[2],
                        height: rect[3],
                    })
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
                window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                    if phase == DispatchPhase::Bubble {
                        let _ = entity.update(cx, |frame, cx| frame.mouse_up(cx));
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
            root = root.child(bg);
        }
        if let Some(rect) = layout.rect(FrameElement::SideBarShadow)
            && shadow_shown
        {
            let shadow = if dark {
                box_shadow(0x000000ff, 0.0, 0.7)
            } else {
                box_shadow(0x777777ff, 3.0, 0.0)
            };
            root = root.child(place(rect, (0.0, 0.0)).shadow(vec![shadow]));
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
            root = root.child(bg);
        }
        if let Some(rect) = layout.rect(FrameElement::SideBar) {
            self.geometry.0.borrow_mut().side_bar = rect;
            let mut side_bar = place(rect, (0.0, 0.0)).overflow_hidden();
            if let Some(content) = self.host.slot(FrameSlot::SideBar, window, cx) {
                side_bar = side_bar.child(content);
            }
            root = root.child(side_bar);
        }
        if let Some(viewer_rect) = layout.rect(FrameElement::Viewer) {
            let origin = (viewer_rect[0], viewer_rect[1]);
            let mut viewer = place(viewer_rect, (0.0, 0.0)).overflow_hidden();
            if let Some(rect) = layout.rect(FrameElement::TabBar)
                && let Some(content) = self.host.slot(FrameSlot::TabBar, window, cx)
            {
                viewer = viewer.child(place(rect, origin).child(content));
            }
            if let Some(rect) = layout.rect(FrameElement::RendererContent) {
                let mut renderer = place(rect, origin);
                if let Some(content) = self.host.slot(FrameSlot::RendererContent, window, cx) {
                    renderer = renderer.child(content);
                }
                viewer = viewer.child(renderer);
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
                viewer = viewer.child(controls);
            }
            if let Some(rect) = layout.rect(FrameElement::ControlsHandle) {
                viewer = viewer.child(
                    place(rect, origin)
                        .id("controls-handle")
                        .cursor(CursorStyle::ResizeRow)
                        .on_any_mouse_down(cx.listener(|frame, _: &MouseDownEvent, _, cx| {
                            frame.handle_down(Handle::Controls, cx)
                        }))
                        .on_click(
                            cx.listener(|frame, _, _, cx| frame.handle_click(Handle::Controls, cx)),
                        ),
                );
            }
            if let Some(rect) = layout.rect(FrameElement::Timeline)
                && let Some(content) = self.host.slot(FrameSlot::Timeline, window, cx)
            {
                viewer = viewer.child(place(rect, origin).child(content));
            }
            root = root.child(viewer);
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
                bar = bar.child(
                    Text::new(title, title_style())
                        .color(theme.text)
                        .probe("title-bar-text")
                        .absolute()
                        .left(px((text_rect[0] - rect[0]) as f32))
                        .top(px((text_rect[1] - rect[1]) as f32))
                        .w(px(text_rect[2] as f32))
                        .h(px(text_rect[3] as f32)),
                );
            }
            root = root.child(bar);
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
            root = root.child(place(rect, (0.0, 0.0)).bg(rgb(GLOW)));
        }
        if let Some(rect) = layout.rect(FrameElement::SideBarHandle) {
            root = root.child(
                place(rect, (0.0, 0.0))
                    .id("side-bar-handle")
                    .cursor(CursorStyle::ResizeColumn)
                    .on_any_mouse_down(cx.listener(|frame, _: &MouseDownEvent, _, cx| {
                        frame.handle_down(Handle::SideBar, cx)
                    }))
                    .on_click(
                        cx.listener(|frame, _, _, cx| frame.handle_click(Handle::SideBar, cx)),
                    ),
            );
        }
        if let (Some(rect), Some(fps)) = (layout.rect(FrameElement::Fps), fps_text) {
            root = root.child(
                Text::new(fps, fps_style())
                    .color(FPS_COLOR)
                    .absolute()
                    .left(px(rect[0] as f32))
                    .top(px(rect[1] as f32)),
            );
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
                DragContent::Tab(_) => match self.host.slot(FrameSlot::DragTab, window, cx) {
                    Some(tab) => item.opacity(0.5).child(tab).into_any_element(),
                    None => item.into_any_element(),
                },
            };
            root = root.child(item);
        }
        root
    }
}
