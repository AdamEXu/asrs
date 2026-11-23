#![allow(clippy::disallowed_methods)]

pub mod gpui {
    pub use gpui_kit::{
        AnyElement, AnyWindowHandle, App, AppContext, Application, AsyncApp, BackgroundExecutor,
        Bounds, BoxShadow, ClickEvent, ClipboardItem, Context, Corners, CursorStyle, DispatchPhase,
        Div, Entity, EventEmitter, ExternalPaths, FocusHandle, Focusable, Global, Hsla,
        InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, KeyboardButton, Keystroke,
        Modifiers, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent,
        ParentElement, Pixels, PlatformInput, Point, Render, RenderImage, Result, ScrollDelta,
        ScrollEvent, ScrollHandle, SharedString, Stateful, StatefulInteractiveElement, Styled,
        Subscription, Task, TitlebarOptions, TouchPhase, WeakEntity, Window,
        WindowBackgroundAppearance, WindowBounds, WindowHandle, WindowKind, WindowOptions,
        anchored, application, canvas, deferred, div, linear_color_stop, linear_gradient, point,
        px, rgb, rgba, size,
    };
    #[cfg(feature = "parity")]
    pub use gpui_kit::{HeadlessAppContext, NoopTextSystem};
}

pub mod button;
pub mod canvas_view;
#[cfg(feature = "parity")]
pub mod capture;
pub mod filter;
pub mod icon;
pub mod input;
pub mod menu;
pub mod parity;
pub mod platform;
pub mod svg;
pub mod symbols;
pub mod text;
pub mod theme;

use gpui::{AnyWindowHandle, App, Entity, Render, Window, WindowOptions};

pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    platform::init(cx);
}

pub fn open_window<V: Render>(
    options: WindowOptions,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> gpui_kit::Result<(AnyWindowHandle, Entity<V>)> {
    gpui_kit::open_window(options, cx, |window, cx| {
        platform::tag_srgb_colorspace(window);
        build(window, cx)
    })
}
