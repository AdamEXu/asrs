//! `main.ts`'s windows on GPUI: every window opens through here, from the options and placement
//! `asrs_services::window_manager` gives it, and its focus, moves and closing feed back into
//! that model, which keeps the hub list in most-recently-focused order.

pub mod chrome;
mod open;

pub use asrs_services::window_manager::{SatellitePlacement, WindowId, WindowKind, WindowOptions};
pub use asrs_services::windows::Rect;
pub use asrs_ui::platform::dialog::{
    FileFilter, MessageBoxOptions, MessageBoxResult, MessageBoxType, OpenDialogOptions,
    SaveDialogOptions, message_box, open_dialog, save_dialog,
};

use asrs_services::window_manager::{Opened, WindowManager};
use asrs_ui::gpui::{AnyWindowHandle, App, Entity, Global, Render, Window};

use crate::main_process::MainProcess;

pub struct Windows {
    manager: WindowManager,
    handles: Vec<(WindowId, AnyWindowHandle)>,
    /// Open modal prompts with their parents.
    modals: Vec<(WindowId, WindowId)>,
    /// Windows that have not been shown yet.
    hidden: Vec<WindowId>,
    on_closed: Vec<(WindowId, OnClosed)>,
}

type OnClosed = Box<dyn FnOnce(&mut App)>;

/// Stands in for the platform's frame where windows have none: how much larger a framed window's
/// bounds are than its content.
#[cfg(feature = "parity")]
pub struct ScriptedFrame {
    pub size: (f64, f64),
}

#[cfg(feature = "parity")]
impl Global for ScriptedFrame {}

impl Default for Windows {
    fn default() -> Self {
        Self {
            manager: WindowManager::new(),
            handles: Vec::new(),
            modals: Vec::new(),
            hidden: Vec::new(),
            on_closed: Vec::new(),
        }
    }
}

impl Global for Windows {}

pub fn init(cx: &mut App) {
    cx.set_global(Windows::default());
    cx.on_window_closed(|cx, closed| {
        let windows = cx.global_mut::<Windows>();
        if let Some(i) = windows
            .handles
            .iter()
            .position(|(_, h)| h.window_id() == closed)
        {
            let (id, _) = windows.handles.remove(i);
            let kind = windows.manager.kind(id);
            windows.manager.closed(id);
            windows.modals.retain(|(modal, _)| *modal != id);
            windows.hidden.retain(|hidden| *hidden != id);
            let listeners = std::mem::take(&mut windows.on_closed);
            let (closed, open): (Vec<_>, Vec<_>) =
                listeners.into_iter().partition(|(window, _)| *window == id);
            windows.on_closed = open;
            close_modals(cx, id);
            if let Some(WindowKind::Licenses | WindowKind::SourceListHelp) = kind {
                MainProcess::download_stop(cx);
            }
            for (_, listener) in closed {
                listener(cx);
            }
        }
    })
    .detach();
}

impl Windows {
    pub fn manager(cx: &App) -> &WindowManager {
        &cx.global::<Windows>().manager
    }

    pub fn handle(cx: &App, id: WindowId) -> Option<AnyWindowHandle> {
        let windows = cx.global::<Windows>();
        windows
            .handles
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, h)| *h)
    }

    /// The open windows, oldest first.
    pub fn ids(cx: &App) -> Vec<WindowId> {
        let windows = cx.global::<Windows>();
        windows.handles.iter().map(|(id, _)| *id).collect()
    }

    pub fn id(cx: &App, window: &Window) -> Option<WindowId> {
        Self::id_of(cx, window.window_handle())
    }

    pub fn id_of(cx: &App, handle: AnyWindowHandle) -> Option<WindowId> {
        let windows = cx.try_global::<Windows>()?;
        windows
            .handles
            .iter()
            .find(|(_, h)| h.window_id() == handle.window_id())
            .map(|(id, _)| *id)
    }

    /// Registers a hub window a parity scenario opened headless, so it can parent prompts and
    /// take the application menu's clicks.
    #[cfg(feature = "parity")]
    pub fn adopt_hub(cx: &mut App, handle: AnyWindowHandle, bounds: Rect) -> WindowId {
        let windows = cx.global_mut::<Windows>();
        let (id, _) = windows.manager.create_hub(None);
        windows.handles.push((id, handle));
        windows.manager.set_bounds(id, bounds);
        windows.manager.focus(id);
        id
    }

    /// The open window of this kind a parity scenario drives, counted in opening order.
    #[cfg(feature = "parity")]
    pub fn of_kind(cx: &App, kind: WindowKind, index: usize) -> Option<AnyWindowHandle> {
        let windows = cx.global::<Windows>();
        windows
            .handles
            .iter()
            .filter(|(id, _)| windows.manager.kind(*id) == Some(kind))
            .nth(index)
            .map(|(_, h)| *h)
    }

    /// `window.isVisible()`.
    pub fn is_visible(cx: &mut App, id: WindowId) -> bool {
        if cx.global::<Windows>().hidden.contains(&id) {
            return false;
        }
        let Some(handle) = Self::handle(cx, id) else {
            return false;
        };
        handle
            .update(cx, |_, window, _| {
                asrs_ui::platform::native_window(window).is_none_or(|native| native.is_visible())
            })
            .unwrap_or(false)
    }

    /// `window.once("closed", listener)`.
    pub fn on_closed(cx: &mut App, id: WindowId, listener: impl FnOnce(&mut App) + 'static) {
        let windows = cx.global_mut::<Windows>();
        windows.on_closed.push((id, Box::new(listener)));
    }

    /// `BrowserWindow.getFocusedWindow()`.
    pub fn focused(cx: &App) -> Option<AnyWindowHandle> {
        Self::handle(cx, Self::manager(cx).focused()?)
    }

    /// `hubWindows`: most recently focused first.
    pub fn hubs(cx: &App) -> Vec<AnyWindowHandle> {
        Self::manager(cx)
            .hubs()
            .iter()
            .filter_map(|id| Self::handle(cx, *id))
            .collect()
    }
}

fn close_modals(cx: &mut App, parent: WindowId) {
    let modals = cx.global::<Windows>().modals.clone();
    for (modal, _) in modals.into_iter().filter(|(_, p)| *p == parent) {
        close(cx, modal);
    }
}

/// `window.destroy()`. A modal prompt is detached from its parent first, and a parent's prompts
/// close with it.
pub fn close(cx: &mut App, id: WindowId) {
    close_modals(cx, id);
    let Some(handle) = Windows::handle(cx, id) else {
        return;
    };
    let native = handle
        .update(cx, |_, window, _| asrs_ui::platform::native_window(window))
        .ok()
        .flatten();
    cx.spawn(async move |cx| {
        if let Some(native) = native {
            native.end_sheet();
        }
        handle
            .update(cx, |_, window, _| window.remove_window())
            .ok();
    })
    .detach();
}

/// `window.setAspectRatio(lock)`, then `window.setContentSize(width, height)`, for the size
/// `resize` computes from `window.getContentSize()`. Not a window's own task on macOS, where
/// AppKit answers a resize by notifying GPUI.
pub fn set_aspect_ratio(
    cx: &mut App,
    id: WindowId,
    lock: f64,
    resize: impl FnOnce((f64, f64)) -> Option<(f64, f64)> + 'static,
) {
    let Some(handle) = Windows::handle(cx, id) else {
        return;
    };
    cx.spawn(async move |cx| {
        let native = handle.update(cx, |_, window, cx| {
            asrs_ui::platform::set_aspect_ratio(window, lock);
            let native = asrs_ui::platform::native_window(window);
            if native.is_none() {
                let size = window.viewport_size();
                let (width, height) = (f32::from(size.width), f32::from(size.height));
                if let Some((width, height)) = resize((f64::from(width), f64::from(height))) {
                    resize_window(window, width, height, cx);
                }
                return None;
            }
            native.map(|native| (native, resize))
        });
        if let Ok(Some((native, resize))) = native
            && let Some((width, height)) = resize(native.content_size())
        {
            let min = native.content_size_for_frame(SATELLITE_MIN.0, SATELLITE_MIN.1);
            native.set_content_size(width.max(min.0), height.max(min.1));
        }
    })
    .detach();
}

/// `createSatellite`'s `minWidth` and `minHeight`, which `setContentSize` keeps to.
const SATELLITE_MIN: (f64, f64) = (200.0, 100.0);

fn resize_window(window: &mut Window, width: f64, height: f64, cx: &mut App) {
    window.resize(asrs_ui::gpui::size(
        asrs_ui::gpui::px(width as f32),
        asrs_ui::gpui::px(height as f32),
    ));
    // The headless platform resizes its windows without the notification real ones send.
    #[cfg(feature = "parity")]
    if cx.has_global::<ScriptedFrame>() {
        window.bounds_changed(cx);
    }
    let _ = cx;
}

pub type Opening<V> = asrs_ui::gpui::Result<(WindowId, Entity<V>)>;

fn open_new<V: Render>(
    cx: &mut App,
    id: WindowId,
    options: WindowOptions,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Opening<V> {
    match open::open(cx, id, options, build) {
        Ok((handle, entity)) => {
            cx.global_mut::<Windows>().handles.push((id, handle));
            Ok((id, entity))
        }
        Err(e) => {
            cx.global_mut::<Windows>().manager.closed(id);
            Err(e)
        }
    }
}

/// `createHubWindow(state)`.
pub fn open_hub<V: Render>(
    cx: &mut App,
    state: Option<Rect>,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Opening<V> {
    let (id, options) = cx.global_mut::<Windows>().manager.create_hub(state);
    open_new(cx, id, options, build)
}

/// `createSatellite`.
pub fn open_satellite<V: Render>(
    cx: &mut App,
    uuid: &str,
    placement: SatellitePlacement,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Opening<V> {
    let (id, options) = cx
        .global_mut::<Windows>()
        .manager
        .create_satellite(uuid, placement);
    open_new(cx, id, options, build)
}

/// A modal prompt as a sheet on `parent`. GPUI attaches sheets to the main window, which is the
/// parent whenever the prompt answers an action in it, as all of upstream's do.
pub fn open_modal<V: Render>(
    cx: &mut App,
    kind: WindowKind,
    parent: WindowId,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Opening<V> {
    let (id, options) = cx
        .global_mut::<Windows>()
        .manager
        .create_modal(kind, parent);
    let opened = open_new(cx, id, options, build)?;
    cx.global_mut::<Windows>().modals.push((id, parent));
    Ok(opened)
}

/// `openSourceListHelp`.
pub fn open_source_list_help<V: Render>(
    cx: &mut App,
    parent: WindowId,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> Opening<V> {
    let (id, options) = cx
        .global_mut::<Windows>()
        .manager
        .open_source_list_help(parent);
    open_new(cx, id, options, build)
}

/// What opening a singleton window did: `None` when it already existed and was focused.
pub type SingletonOpening<V> = asrs_ui::gpui::Result<Option<(WindowId, Entity<V>)>>;

fn singleton<V: Render>(
    cx: &mut App,
    open: impl FnOnce(&mut WindowManager) -> Opened,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> SingletonOpening<V> {
    match open(&mut cx.global_mut::<Windows>().manager) {
        Opened::New(id, options) => open_new(cx, id, options, build).map(Some),
        Opened::Focus(id) => {
            if let Some(handle) = Windows::handle(cx, id) {
                handle.update(cx, |_, window, _| window.activate_window())?;
            }
            Ok(None)
        }
    }
}

/// `openPreferences`.
pub fn open_preferences<V: Render>(
    cx: &mut App,
    parent: WindowId,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> SingletonOpening<V> {
    singleton(cx, |m| m.open_preferences(parent), build)
}

/// `openDownload`.
pub fn open_download<V: Render>(
    cx: &mut App,
    parent: WindowId,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> SingletonOpening<V> {
    singleton(cx, |m| m.open_download(parent), build)
}

/// `openLicenses`.
pub fn open_licenses<V: Render>(
    cx: &mut App,
    parent: WindowId,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> SingletonOpening<V> {
    singleton(cx, |m| m.open_licenses(parent), build)
}
