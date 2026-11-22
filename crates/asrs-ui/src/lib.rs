#![allow(clippy::disallowed_methods)]

pub mod gpui {
    pub use gpui_kit::{
        AnyElement, AnyWindowHandle, App, AppContext, Application, AsyncApp, Bounds, BoxShadow,
        ClickEvent, Context, Corners, CursorStyle, DispatchPhase, Div, Entity, EventEmitter,
        ExternalPaths, Global, Hsla, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
        MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Render, RenderImage, Result,
        ScrollDelta, ScrollEvent, SharedString, Stateful, StatefulInteractiveElement, Styled, Task,
        TitlebarOptions, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
        WindowOptions, application, canvas, div, linear_color_stop, linear_gradient, point, px,
        rgb, rgba, size,
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
