use asrs_ui::gpui::{
    AnyWindowHandle, App, Bounds, Entity, Render, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, point, px, size,
};
use asrs_ui::platform::{self, NativeWindow};

use super::{Rect, WindowId, WindowKind, WindowOptions, Windows};

type Size = (f64, f64);

/// `INT_MAX`, which Electron puts in for a maximum size dimension that is not given.
const UNBOUNDED: f64 = i32::MAX as f64;

/// How much larger a framed window's bounds are than its content, where that is not the platform's
/// own to say.
fn scripted_frame(options: &WindowOptions, cx: &App) -> Size {
    #[cfg(feature = "parity")]
    if options.kind == WindowKind::Satellite
        && let Some(frame) = cx.try_global::<super::ScriptedFrame>()
    {
        return frame.size;
    }
    let _ = (options, cx);
    (0.0, 0.0)
}

fn gpui_options(options: &WindowOptions, show: bool, cx: &App) -> asrs_ui::gpui::WindowOptions {
    let frame = scripted_frame(options, cx);
    let content = size(
        px((options.width - frame.0) as f32),
        px((options.height - frame.1) as f32),
    );
    let bounds = match origin(options, cx) {
        Some((x, y)) => Bounds::new(point(px(x as f32), px(y as f32)), content),
        None => Bounds::centered(None, content, cx),
    };
    let hub = options.kind == WindowKind::Hub;
    let vibrant = hub && cfg!(target_os = "macos");
    asrs_ui::gpui::WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: hub && !cfg!(target_os = "linux"),
            traffic_light_position: None,
        }),
        app_owns_titlebar_drag: vibrant,
        window_background: if vibrant {
            WindowBackgroundAppearance::Transparent
        } else if hub && platform::has_acrylic() {
            // Transparent pixels and a DWM backdrop, whose type `install_sidebar_vibrancy` sets.
            // `Transparent` would lay an accent policy over it.
            WindowBackgroundAppearance::MicaBackdrop
        } else {
            WindowBackgroundAppearance::Opaque
        },
        kind: if options.modal && !cfg!(target_os = "macos") {
            asrs_ui::gpui::WindowKind::Dialog
        } else if options.modal {
            asrs_ui::gpui::WindowKind::Normal
        } else if options.always_on_top {
            asrs_ui::gpui::WindowKind::Floating
        } else {
            asrs_ui::gpui::WindowKind::Normal
        },
        is_resizable: options.resizable.unwrap_or(true),
        focus: true,
        show,
        app_id: cfg!(target_os = "linux").then(|| crate::IDENTITY.name.to_owned()),
        ..Default::default()
    }
}

fn origin(options: &WindowOptions, cx: &App) -> Option<(f64, f64)> {
    options
        .x
        .zip(options.y)
        .map(|(x, y)| (x.trunc(), y.trunc()))
        .or_else(|| default_origin(options, cx))
}

/// Where Electron puts a window opened without `x` and `y`: its requested size centred on the
/// primary screen, rounded on macOS (`NativeWindowMac` uses the full frame and never calls
/// `[NSWindow center]`) and in whole numbers on Linux.
fn default_origin(options: &WindowOptions, cx: &App) -> Option<(f64, f64)> {
    let whole = if cfg!(target_os = "macos") {
        f64::round
    } else if cfg!(target_os = "linux") {
        f64::floor
    } else {
        return None;
    };
    let screen = cx.primary_display()?.bounds().size;
    Some((
        whole((f64::from(f32::from(screen.width)) - options.width) / 2.0),
        whole((f64::from(f32::from(screen.height)) - options.height) / 2.0),
    ))
}

/// `window.getBounds()`.
fn bounds(window: &Window) -> Rect {
    let (x, y, width, height) = platform::window_bounds(window).unwrap_or_else(|| {
        let b = window.bounds();
        let f = |p: asrs_ui::gpui::Pixels| f64::from(f32::from(p));
        (
            f(b.origin.x),
            f(b.origin.y),
            f(b.size.width),
            f(b.size.height),
        )
    });
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn track<V: Render>(
    id: WindowId,
    frame: Size,
    window: &mut Window,
    entity: &Entity<V>,
    cx: &mut App,
) {
    let bounds = move |window: &Window| {
        let content = bounds(window);
        Rect {
            width: content.width + frame.0,
            height: content.height + frame.1,
            ..content
        }
    };
    let opened = bounds(window);
    cx.global_mut::<Windows>().manager.set_bounds(id, opened);
    entity.update(cx, |_, cx| {
        cx.observe_window_activation(window, move |_, window, cx| {
            let manager = &mut cx.global_mut::<Windows>().manager;
            if window.is_window_active() {
                manager.focus(id);
            } else {
                manager.blur(id);
            }
        })
        .detach();
        cx.observe_window_bounds(window, move |_, window, cx| {
            let bounds = bounds(window);
            cx.global_mut::<Windows>().manager.set_bounds(id, bounds);
        })
        .detach();
    });
}

/// Electron's size constraints for the window, converted to content sizes. A constraint with a
/// zero dimension is `IsEmpty()` there and is not applied.
fn content_limits(native: &NativeWindow, options: &WindowOptions) -> (Option<Size>, Option<Size>) {
    let content = |(w, h): (f64, f64)| {
        if options.use_content_size {
            (w, h)
        } else {
            native.content_size_for_frame(w, h)
        }
    };
    let min = (
        options.min_width.unwrap_or(0.0),
        options.min_height.unwrap_or(0.0),
    );
    let min = (min.0 > 0.0 && min.1 > 0.0).then(|| content(min));
    let max = options.max_width.map(|w| content((w, UNBOUNDED)));
    if cfg!(target_os = "linux") && options.resizable == Some(false) {
        let size = (options.width, options.height);
        return (Some(size), Some(size));
    }
    (min, max)
}

/// What Electron's `BrowserWindow` constructor does that GPUI's options cannot: the
/// `hiddenInset` toolbar, the exact content size (GPUI opens macOS windows 1 px taller than
/// asked), frame-based sizes for windows without `useContentSize`, size limits, the place an X11
/// window manager would choose itself, `fullscreenable`, then showing the window, or attaching it
/// as a sheet to its parent. GPUI's `Dialog` kind would attach it to whichever window is main, and
/// to none while the app is inactive.
fn finish(
    native: &NativeWindow,
    options: &WindowOptions,
    origin: Option<(f64, f64)>,
    parent: Option<&NativeWindow>,
) {
    if options.kind == WindowKind::Hub {
        native.hidden_inset_titlebar();
    }
    let (width, height) = if options.use_content_size {
        (options.width, options.height)
    } else {
        native.content_size_for_frame(options.width, options.height)
    };
    native.set_content_size(width, height);
    let (min, max) = content_limits(native, options);
    native.set_content_size_limits(min, max);
    match origin {
        Some((x, y)) => native.set_content_origin(x, y),
        None => native.center(parent),
    }
    if options.fullscreenable == Some(false) {
        native.set_fullscreenable(false);
    }
    if let (true, Some(parent)) = (options.modal, parent) {
        parent.begin_sheet(native);
        return;
    }
    native.show(true);
}

pub(super) fn open<V: Render>(
    cx: &mut App,
    id: WindowId,
    options: WindowOptions,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> asrs_ui::gpui::Result<(AnyWindowHandle, Entity<V>)> {
    let deferred = cfg!(target_os = "macos");
    let frame = scripted_frame(&options, cx);
    let origin = origin(&options, cx);
    let parent = options
        .parent
        .and_then(|parent| Windows::handle(cx, parent))
        .and_then(|handle| {
            handle
                .update(cx, |_, window, _| platform::native_window(window))
                .ok()
                .flatten()
        });
    let mut native = None;
    let (handle, entity) =
        asrs_ui::open_window(gpui_options(&options, !deferred, cx), cx, |window, cx| {
            let entity = build(window, cx);
            if options.kind == WindowKind::Hub {
                platform::install_sidebar_vibrancy(window);
            }
            if options.always_on_top {
                platform::keep_visible_when_inactive(window);
            }
            track(id, frame, window, &entity, cx);
            native = platform::native_window(window);
            entity
        })?;
    if let Some(native) = native {
        cx.global_mut::<Windows>().hidden.push(id);
        cx.spawn(async move |cx| {
            finish(&native, &options, origin, parent.as_ref());
            handle
                .update(cx, |_, window, cx| {
                    let bounds = bounds(window);
                    let windows = cx.global_mut::<Windows>();
                    windows.manager.set_bounds(id, bounds);
                    windows.hidden.retain(|hidden| *hidden != id);
                })
                .ok();
        })
        .detach();
    }
    Ok((handle, entity))
}
