use asrs_ui::gpui::{
    AnyWindowHandle, App, Bounds, Entity, Render, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, point, px, size,
};
use asrs_ui::platform::{self, NativeWindow};

use super::{Rect, WindowId, WindowKind, WindowOptions, Windows};

type Size = (f64, f64);

/// `INT_MAX`, which Electron puts in for a maximum size dimension that is not given.
const UNBOUNDED: f64 = i32::MAX as f64;

fn gpui_options(options: &WindowOptions, show: bool, cx: &App) -> asrs_ui::gpui::WindowOptions {
    let content = size(px(options.width as f32), px(options.height as f32));
    let bounds = match options.x.zip(options.y) {
        Some((x, y)) => Bounds::new(point(px(x.trunc() as f32), px(y.trunc() as f32)), content),
        None => Bounds::centered(None, content, cx),
    };
    let fancy = options.kind == WindowKind::Hub && cfg!(target_os = "macos");
    asrs_ui::gpui::WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: None,
            appears_transparent: fancy,
            traffic_light_position: None,
        }),
        app_owns_titlebar_drag: fancy,
        window_background: if fancy {
            WindowBackgroundAppearance::Transparent
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
        ..Default::default()
    }
}

fn rect(bounds: Bounds<asrs_ui::gpui::Pixels>) -> Rect {
    Rect {
        x: f64::from(f32::from(bounds.origin.x)),
        y: f64::from(f32::from(bounds.origin.y)),
        width: f64::from(f32::from(bounds.size.width)),
        height: f64::from(f32::from(bounds.size.height)),
    }
}

fn track<V: Render>(id: WindowId, window: &mut Window, entity: &Entity<V>, cx: &mut App) {
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
            let bounds = rect(window.bounds());
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
    (min, max)
}

/// What Electron's `BrowserWindow` constructor does that GPUI's options cannot: the
/// `hiddenInset` toolbar, the exact content size (GPUI opens macOS windows 1 px taller than
/// asked), frame-based sizes for windows without `useContentSize`, size limits,
/// `fullscreenable`, centering, then showing the window, or attaching it as a sheet to its
/// parent. GPUI's `Dialog` kind would attach it to whichever window is main, and to none while
/// the app is inactive.
fn finish(native: &NativeWindow, options: &WindowOptions, parent: Option<&NativeWindow>) {
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
    if options.fullscreenable == Some(false) {
        native.set_fullscreenable(false);
    }
    if let (true, Some(parent)) = (options.modal, parent) {
        parent.begin_sheet(native);
        return;
    }
    if options.x.is_none() || options.y.is_none() {
        native.center();
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
            track(id, window, &entity, cx);
            native = platform::native_window(window);
            entity
        })?;
    if let Some(native) = native {
        cx.spawn(async move |cx| {
            finish(&native, &options, parent.as_ref());
            handle
                .update(cx, |_, window, cx| {
                    let bounds = rect(window.bounds());
                    cx.global_mut::<Windows>().manager.set_bounds(id, bounds);
                })
                .ok();
        })
        .detach();
    }
    Ok((handle, entity))
}
