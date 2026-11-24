#![allow(clippy::disallowed_methods)]

pub mod gpui {
    pub use gpui_kit::{
        Action, AnyElement, AnyWindowHandle, App, AppContext, Application, AsyncApp,
        BackgroundExecutor, Bounds, BoxShadow, ClickEvent, ClipboardItem, Context, Corners,
        CursorStyle, DispatchPhase, Div, Entity, EventEmitter, ExternalPaths, FocusHandle,
        Focusable, Global, HitboxBehavior, HoverListenerMode, Hsla, InteractiveElement,
        IntoElement, KeyDownEvent, KeyUpEvent, KeyboardButton, Keystroke, Modifiers, MouseButton,
        MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
        PlatformInput, Point, Render, RenderImage, Result, Rgba, ScrollDelta, ScrollEvent,
        ScrollHandle, SharedString, Stateful, StatefulInteractiveElement, Styled, Subscription,
        Task, TitlebarOptions, TouchDragEvent, TouchPhase, WeakEntity, Window,
        WindowBackgroundAppearance, WindowBounds, WindowControlArea, WindowHandle, WindowKind,
        WindowOptions, anchored, application, canvas, deferred, div, fill, linear_color_stop,
        linear_gradient, point, px, rgb, rgba, size,
    };
    #[cfg(feature = "parity")]
    pub use gpui_kit::{
        HeadlessAppContext, NoopTextSystem, TestAppContext, TouchEvent, TouchId, WindowVisibility,
    };
}

pub mod button;
pub mod canvas_view;
pub mod caption;
#[cfg(feature = "parity")]
pub mod capture;
pub mod filter;
pub mod focus;
pub mod icon;
pub mod image_icon;
pub mod input;
pub mod menu;
pub mod parity;
pub mod platform;
pub mod progress;
pub mod selection;
pub mod svg;
pub mod symbols;
pub mod text;
pub mod theme;
pub mod tooltip;
pub mod webview;

use gpui::{AnyWindowHandle, App, Entity, Render, Window, WindowOptions};

pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
    input::init(cx);
    // The kit's root paints it under every window's view, over the hub's vibrancy or acrylic.
    // Views paint their own pages.
    if !cfg!(target_os = "linux") {
        let theme = gpui_kit::component::Theme::global_mut(cx);
        theme.tokens.background = gpui_kit::transparent_black().into();
    }
    platform::init(cx);
}

pub fn open_window<V: Render>(
    options: WindowOptions,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> gpui_kit::Result<(AnyWindowHandle, Entity<V>)> {
    #[cfg(target_os = "linux")]
    {
        use gpui::AppContext as _;
        let mut built = None;
        let (handle, _) = gpui_kit::open_window(options, cx, |window, cx| {
            platform::tag_srgb_colorspace(window);
            let view = build(window, cx);
            built = Some(view.clone());
            cx.new(|_| menu::MenuGuard::new(view))
        })?;
        Ok((handle, built.expect("open_window ran its build closure")))
    }
    #[cfg(not(target_os = "linux"))]
    gpui_kit::open_window(options, cx, |window, cx| {
        platform::tag_srgb_colorspace(window);
        build(window, cx)
    })
}
