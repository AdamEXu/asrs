#![allow(clippy::disallowed_methods)]

pub mod gpui {
    pub use gpui_kit::{
        AnyWindowHandle, App, AppContext, Application, Bounds, Context, Entity, IntoElement,
        ParentElement, Render, Styled, Window, WindowBounds, WindowOptions, application, div, px,
        rgb, size,
    };
}

#[cfg(feature = "parity")]
pub mod capture;

use gpui::{AnyWindowHandle, App, Entity, Render, Window, WindowOptions};

pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
}

pub fn open_window<V: Render>(
    options: WindowOptions,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> gpui_kit::Result<(AnyWindowHandle, Entity<V>)> {
    gpui_kit::open_window(options, cx, build)
}
