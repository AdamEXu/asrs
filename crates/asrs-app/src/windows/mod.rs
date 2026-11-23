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

pub struct Windows {
    manager: WindowManager,
    handles: Vec<(WindowId, AnyWindowHandle)>,
    /// Open modal prompts with their parents.
    modals: Vec<(WindowId, WindowId)>,
}

impl Default for Windows {
    fn default() -> Self {
        Self {
            manager: WindowManager::new(),
            handles: Vec::new(),
            modals: Vec::new(),
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
            windows.manager.closed(id);
            windows.modals.retain(|(modal, _)| *modal != id);
            close_modals(cx, id);
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
    pub fn adopt_hub(cx: &mut App, handle: AnyWindowHandle) -> WindowId {
        let windows = cx.global_mut::<Windows>();
        let (id, _) = windows.manager.create_hub(None);
        windows.handles.push((id, handle));
        windows.manager.focus(id);
        id
    }

    /// The open window of this kind a parity scenario drives.
    #[cfg(feature = "parity")]
    pub fn of_kind(cx: &App, kind: WindowKind) -> Option<AnyWindowHandle> {
        let windows = cx.global::<Windows>();
        windows
            .handles
            .iter()
            .find(|(id, _)| windows.manager.kind(*id) == Some(kind))
            .map(|(_, h)| *h)
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
