#![allow(clippy::disallowed_methods)]

pub mod gpui {
    pub use gpui_kit::{
        AnyWindowHandle, App, AppContext, Application, Bounds, Context, Corners, Entity,
        IntoElement, ParentElement, Pixels, Render, RenderImage, ScrollDelta, ScrollEvent, Styled,
        Window, WindowBounds, WindowOptions, application, canvas, div, px, rgb, size,
    };
}

pub mod button;
pub mod canvas_view;
#[cfg(feature = "parity")]
pub mod capture;
pub mod filter;
pub mod icon;
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
