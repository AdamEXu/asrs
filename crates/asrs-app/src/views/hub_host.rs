//! The hub window's root: a [`HubFrame`] over the [`Hub`] entity, which it observes. Until
//! main (P) exists, the window is booted with the messages `main.ts` sends a new hub window,
//! and what the hub sends main is dropped.

use asrs_hub::hub::{HubModel, HubView};
use asrs_hub::sidebar::SidebarGeometry;
use asrs_js::value::{JsValue, ToJs};
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::source_list::Rect;
use asrs_ui::gpui::{AnyElement, App, AppContext, Entity, IntoElement, Window};

use super::frame::{FrameGeometry, FrameHost, FrameSlot, HubFrame};
use super::sidebar::{self, SidebarView, ViewGeometry};
use crate::hub::{Hub, HubConfig, HubEvent};

struct FrameView {
    geometry: FrameGeometry,
    dark: bool,
    sidebar: ViewGeometry,
}

impl HubView for FrameView {
    fn inner_height(&self) -> f64 {
        self.geometry.inner_height()
    }

    fn renderer_top(&self, show_timeline: bool) -> f64 {
        self.geometry.renderer_top(show_timeline)
    }

    fn dark(&self) -> bool {
        self.dark
    }

    fn sidebar_geometry(&mut self) -> &mut dyn SidebarGeometry {
        &mut self.sidebar
    }

    fn title_rect(&self, index: usize) -> Rect {
        self.geometry.tab_bar().borrow().title_rect(index)
    }

    fn overlay_rect(&self) -> Rect {
        self.geometry.tab_bar().borrow().overlay_rect()
    }

    fn add_button_rect(&self) -> Rect {
        self.geometry.tab_bar().borrow().add_button_rect()
    }

    fn tab_bar_scroll_left(&self) -> f64 {
        self.geometry.tab_bar().borrow().scroll_left
    }

    fn set_tab_bar_scroll_left(&self, value: f64) {
        self.geometry
            .tab_bar()
            .borrow_mut()
            .assign_scroll_left(value);
    }

    fn tab_bar_scroll_width(&self) -> f64 {
        self.geometry.tab_bar().borrow().scroll_width()
    }

    fn tab_bar_client_width(&self) -> f64 {
        self.geometry.tab_bar().borrow().client_width()
    }
}

pub struct HubEntityHost {
    hub: Entity<Hub>,
    geometry: FrameGeometry,
    sidebar_geometry: ViewGeometry,
    sidebar: Entity<SidebarView>,
}

/// `os.release()`.
fn os_release() -> String {
    if cfg!(unix) {
        std::process::Command::new("uname")
            .arg("-r")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim_end_matches('\n').to_owned())
            .unwrap_or_default()
    } else {
        String::new()
    }
}

/// `process.platform`.
fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// What `main.ts` sends a new hub window with no saved state: `is_beta` shows the feedback
/// button, and `files` (`firstOpenPath`) are opened once the page is up.
fn boot_messages(is_beta: bool, files: Vec<String>) -> Vec<NamedMessage<MainToHub>> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    };
    let version = asrs_js::ops::object([
        ("platform", JsValue::string(platform())),
        ("platformRelease", JsValue::string(&os_release())),
        ("platformArch", JsValue::string(arch)),
        (
            "appVersion",
            JsValue::string(asrs_model::build_constants::VERSION),
        ),
    ]);
    let mut messages = vec![
        (MainToHub::SetAssets, JsValue::Null),
        (MainToHub::SetFullscreen, JsValue::Bool(false)),
        (MainToHub::SetBattery, JsValue::Bool(false)),
        (MainToHub::SetVersion, version),
        (MainToHub::ShowUpdateButton, JsValue::Bool(false)),
        (MainToHub::ShowFeedbackButton, JsValue::Bool(is_beta)),
        (MainToHub::ShowWhenReady, JsValue::Undefined),
        (
            MainToHub::SetPreferences,
            asrs_model::preferences::Preferences::default().to_js(),
        ),
        (MainToHub::SetActiveSatellites, JsValue::Array(Vec::new())),
    ];
    if !files.is_empty() {
        let files = files.iter().map(|f| JsValue::string(f)).collect();
        messages.push((
            MainToHub::OpenFiles,
            asrs_js::ops::object([
                ("files", JsValue::Array(files)),
                ("merge", JsValue::Bool(false)),
            ]),
        ));
    }
    messages
        .into_iter()
        .map(|(name, data)| NamedMessage::new(name, data))
        .collect()
}

impl HubEntityHost {
    /// The hub window's root view: a new [`Hub`] evaluated in `window` and booted as `main.ts`
    /// boots it.
    pub fn open(
        window: &Window,
        dark: bool,
        is_beta: bool,
        files: Vec<String>,
        cx: &mut App,
    ) -> Entity<HubFrame<HubEntityHost>> {
        let geometry = FrameGeometry::default();
        let sidebar_geometry = ViewGeometry::new();
        let view = Box::new(FrameView {
            geometry: geometry.clone(),
            dark,
            sidebar: sidebar_geometry.clone(),
        });
        let hub = cx.new(|cx| Hub::new(Some(window), HubConfig::new(view), cx));
        let handle = window.window_handle();
        cx.subscribe(&hub, move |_, event, cx| match event {
            HubEvent::Console { warn: true, text } => eprintln!("asrs: {text}"),
            HubEvent::Exception { task, error } => eprintln!("asrs: {task}: {error}"),
            HubEvent::Main(message) => {
                let _ = handle.update(cx, |_, window, cx| {
                    crate::menus::context::handle(message, window, cx)
                });
            }
            _ => {}
        })
        .detach();
        let sidebar = cx.new(|_| SidebarView::new(hub.clone(), &sidebar_geometry));
        hub.update(cx, |hub, cx| {
            for message in boot_messages(is_beta, files) {
                geometry.sync(hub.model());
                hub.handle_main_message(message, cx);
            }
        });
        cx.new(|cx| {
            cx.observe(&hub, |_, _, cx| cx.notify()).detach();
            cx.observe(&sidebar, |_, _, cx| cx.notify()).detach();
            HubFrame::new(
                HubEntityHost {
                    hub,
                    geometry,
                    sidebar_geometry,
                    sidebar,
                },
                cx,
            )
        })
    }

    pub fn hub(&self) -> &Entity<Hub> {
        &self.hub
    }

    pub fn sidebar(&self) -> &Entity<SidebarView> {
        &self.sidebar
    }
}

impl FrameHost for HubEntityHost {
    fn geometry(&self) -> FrameGeometry {
        self.geometry.clone()
    }

    fn hub<'a>(&'a self, cx: &'a App) -> &'a HubModel {
        self.hub.read(cx).model()
    }

    fn input<R>(&mut self, cx: &mut App, f: impl FnOnce(&mut HubModel) -> R) -> R {
        self.hub
            .update(cx, |hub, cx| hub.update(cx, |model, _| f(model)))
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>, cx: &mut App) {
        self.hub.update(cx, |hub, cx| {
            hub.update(cx, |_, host| host.send_main_message(message))
        });
    }

    fn slot(&mut self, slot: FrameSlot, _window: &mut Window, _cx: &mut App) -> Option<AnyElement> {
        match slot {
            FrameSlot::SideBar => {
                sidebar::set_side_bar(&self.sidebar_geometry, self.geometry.side_bar());
                Some(self.sidebar.clone().into_any_element())
            }
            _ => None,
        }
    }
}
