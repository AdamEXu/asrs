//! The hub window's root: a [`HubFrame`] over the [`Hub`] entity, which it observes, booted
//! with the messages `main.ts` sends a new hub window.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_canvas::text::{FixedAdvance, TextBackend, platform_backend};

use asrs_hub::hub::{HubModel, HubPage, HubView};
use asrs_hub::sidebar::SidebarGeometry;
use asrs_js::string::JsError;
use asrs_js::time::{Rng, SystemClock, V8MathRandom};
use asrs_js::value::{JsValue, ToJs};
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage, SatelliteToMain};
use asrs_model::preferences::Preferences;
use asrs_model::source_list::{Rect, SourceListDom};
use asrs_model::tab_controller::TabRenderer;
use asrs_model::tab_type::TabType;
use asrs_services::window_manager::Platform;
use asrs_tabs::console::controller::{ConsoleController, ConsoleRenderer};
use asrs_ui::gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, ClipboardItem, Entity, IntoElement, WeakEntity,
    Window,
};

use super::canvas_tabs::JoysticksTab;
use super::console_layout::ConsoleLayout;
use super::field3d::Field3dTab;
use super::frame::{FrameGeometry, FrameHost, FrameSlot, HubFrame};
use super::prompts;
use super::sidebar::{self, SidebarView, ViewGeometry};
use super::source_list::ListsGeometry;
use crate::hub::{FrameClock, Hub, HubConfig, HubEvent};
use crate::live;
use crate::main_process::MainProcess;
use crate::menus::bridge::{self, EffectHandler};
use crate::menus::context::{self, ContextMenu, Effect, MenuEnv};
use crate::satellite::Satellite;
use crate::windows::Windows;

struct FrameView {
    geometry: FrameGeometry,
    canvas_text: Rc<RefCell<dyn TextBackend>>,
    lists: ListsGeometry,
    sidebar: ViewGeometry,
}

impl HubView for FrameView {
    fn inner_height(&self) -> f64 {
        self.geometry.inner_height()
    }

    fn renderer_top(&self, show_timeline: bool) -> f64 {
        self.geometry.renderer_top(show_timeline)
    }

    fn large_table_box(&self) -> Option<(Rect, f64)> {
        self.geometry.large_table_box()
    }

    fn dark(&self) -> bool {
        self.geometry.dark()
    }

    fn sidebar_geometry(&mut self) -> &mut dyn SidebarGeometry {
        &mut self.sidebar
    }

    fn source_list_dom(&mut self) -> &mut dyn SourceListDom {
        &mut self.lists
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

    fn canvas_text(&self) -> Option<Rc<RefCell<dyn TextBackend>>> {
        Some(self.canvas_text.clone())
    }

    fn lay_out_console(
        &self,
        controller: Option<&mut ConsoleController>,
        renderer: &mut ConsoleRenderer,
    ) {
        lay_out_console(controller, renderer);
    }

    fn tab_renderer(
        &self,
        kind: TabType,
        preferences: Option<&Preferences>,
        is_battery: bool,
    ) -> Option<Box<dyn TabRenderer<HubPage>>> {
        view_renderer(kind, preferences, is_battery)
    }
}

/// The renderers a hub's or a satellite's view builds itself.
pub(super) fn view_renderer(
    kind: TabType,
    preferences: Option<&Preferences>,
    is_battery: bool,
) -> Option<Box<dyn TabRenderer<HubPage>>> {
    match kind {
        TabType::Field3d => Some(Box::new(Field3dTab::new(preferences, is_battery))),
        TabType::Joysticks => Some(Box::new(JoysticksTab::default())),
        _ => None,
    }
}

pub(super) fn lay_out_console(
    controller: Option<&mut ConsoleController>,
    renderer: &mut ConsoleRenderer,
) {
    let layout = ConsoleLayout::default();
    if let Some(controller) = controller {
        controller.set_geometry(Box::new(layout.clone()));
    }
    renderer.set_geometry(Box::new(layout));
}

pub(super) fn canvas_text() -> Rc<RefCell<dyn TextBackend>> {
    let text: Box<dyn TextBackend> = platform_backend().unwrap_or_else(|| Box::new(FixedAdvance));
    Rc::new(RefCell::new(text))
}

/// The `Math.random()` calls upstream's hub bundle makes while its imports evaluate, before
/// hub.ts draws its first UUID: 19 three.js objects built at module scope (4 draws each in
/// `MathUtils.generateUUID`) and bignumber.js's `BigNumber.random` setup.
const IMPORT_RANDOM_DRAWS: usize = 77;

pub struct HubEntityHost {
    hub: Entity<Hub>,
    geometry: FrameGeometry,
    lists: ListsGeometry,
    sidebar_geometry: ViewGeometry,
    sidebar: Entity<SidebarView>,
}

/// `os.release()`.
fn os_release() -> String {
    #[cfg(windows)]
    return asrs_ui::platform::os_release();
    #[cfg(not(windows))]
    std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim_end_matches('\n').to_owned())
        .unwrap_or_default()
}

/// `process.platform`.
fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// What `main.ts` sends a new hub window with no saved state: `assets` are main's loaded
/// assets (`null` before the first load), `is_beta` shows the feedback button, `preferences` are
/// main's merged preferences, and `files` (`firstOpenPath`) are opened once the page is up.
pub fn boot_messages(
    assets: JsValue,
    is_beta: bool,
    preferences: JsValue,
    files: Vec<String>,
) -> Vec<NamedMessage<MainToHub>> {
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
        (MainToHub::SetAssets, assets),
        (MainToHub::SetFullscreen, JsValue::Bool(false)),
        (MainToHub::SetBattery, JsValue::Bool(false)),
        (MainToHub::SetVersion, version),
        (MainToHub::ShowUpdateButton, JsValue::Bool(false)),
        (MainToHub::ShowFeedbackButton, JsValue::Bool(is_beta)),
        (MainToHub::ShowWhenReady, JsValue::Undefined),
        (MainToHub::SetPreferences, preferences),
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
        let preferences = Preferences::default().to_js();
        Self::open_with(
            window,
            dark,
            boot_messages(JsValue::Null, is_beta, preferences, files),
            None,
            cx,
        )
    }

    /// The hub window's root view, booted with `boot` in place of main's messages. With
    /// `scenario_seed`, the page runs as a parity scenario's: its and its workers' `Math.random`
    /// are V8's under `--random-seed=<seed>`, and its `Date` is a [`FrameClock`].
    pub fn open_with(
        window: &Window,
        dark: bool,
        boot: Vec<NamedMessage<MainToHub>>,
        scenario_seed: Option<u64>,
        cx: &mut App,
    ) -> Entity<HubFrame<HubEntityHost>> {
        let geometry = FrameGeometry::default();
        geometry.set_dark(dark);
        let sidebar_geometry = ViewGeometry::new();
        let lists = ListsGeometry::default();
        let view = Box::new(FrameView {
            geometry: geometry.clone(),
            canvas_text: canvas_text(),
            lists: lists.clone(),
            sidebar: sidebar_geometry.clone(),
        });
        let mut config = HubConfig::new(view);
        if let Some(seed) = scenario_seed {
            let mut random = V8MathRandom::from_seed(seed);
            for _ in 0..IMPORT_RANDOM_DRAWS {
                random.next_f64();
            }
            config.realm = JsRealm::new(Box::new(random));
            config.clock = Rc::new(FrameClock::new(cx));
            config.worker_realm = Box::new(move || {
                (
                    Box::new(V8MathRandom::from_seed(seed)),
                    Box::new(SystemClock),
                )
            });
        }
        let hub = cx.new(|cx| Hub::new(Some(window), config, cx));
        live::attach(&hub, cx);
        let handle = window.window_handle();
        let weak = hub.downgrade();
        let scheme = geometry.clone();
        cx.subscribe(&hub, move |_, event, cx| match event {
            HubEvent::Console { warn: true, text } => eprintln!("asrs: {text}"),
            HubEvent::Exception { task, error } => eprintln!("asrs: {task}: {error}"),
            HubEvent::Main(message) => {
                show_context_menu(handle, weak.clone(), scheme.dark(), message, cx);
            }
            _ => {}
        })
        .detach();
        let sidebar = cx.new(|_| SidebarView::new(hub.clone(), &sidebar_geometry));
        hub.update(cx, |hub, cx| {
            for message in boot {
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
                    lists,
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

/// `sendMessage(window, ..)` to the window a menu popped up for.
type Send = Rc<dyn Fn(&Effect, &mut App)>;

/// What a click in a context menu of the window `handle` does: messages `send` sends back to it,
/// prompts whose reply has further effects, and the source list help window.
fn menu_effects(handle: AnyWindowHandle, send: Send) -> EffectHandler {
    Rc::new(move |effects, cx| {
        for effect in effects {
            run_effect(effect, handle, &send, cx);
        }
    })
}

fn run_effect(effect: Effect, handle: AnyWindowHandle, send: &Send, cx: &mut App) {
    match effect {
        Effect::Send { .. } => send(&effect, cx),
        Effect::WriteClipboard(JsValue::String(ref text)) => {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string_lossy()))
        }
        Effect::UpdateRecentUnits(preset) => MainProcess::update_recent_units(cx, &preset),
        Effect::Prompt(prompt) => {
            let Some(parent) = Windows::id_of(cx, handle) else {
                return;
            };
            let effects = menu_effects(handle, send.clone());
            let request = prompt.clone();
            let opened = prompts::ask(cx, parent, &request, move |value, cx| {
                match prompt.reply(value) {
                    Ok(replied) => effects(replied, cx),
                    Err(error) => eprintln!("asrs: {:?}: {error:?}", prompt.window()),
                }
            });
            if let Err(error) = opened {
                eprintln!("asrs: opening a prompt: {error}");
            }
        }
        Effect::OpenSourceListHelp(config) => {
            if let Some(parent) = Windows::id_of(cx, handle) {
                MainProcess::open_source_list_help(cx, parent, &config);
            }
        }
        other => eprintln!("asrs: context menu effect not available yet: {other:?}"),
    }
}

fn send_to_hub(hub: WeakEntity<Hub>) -> Send {
    Rc::new(move |effect, cx| {
        if let (Some(message), Some(hub)) = (effect.to_hub(), hub.upgrade()) {
            hub.update(cx, |hub, cx| hub.handle_main_message(message, cx));
        }
    })
}

fn send_to_satellite(satellite: WeakEntity<Satellite>) -> Send {
    Rc::new(move |effect, cx| {
        if let (Some(message), Some(satellite)) = (effect.to_satellite(), satellite.upgrade()) {
            satellite.update(cx, |satellite, cx| {
                satellite.handle_main_message(&message, cx)
            });
        }
    })
}

/// Pops up `menu`, the context menu main answers a message of the window's page with, if any.
fn popup(
    handle: AnyWindowHandle,
    what: &str,
    menu: Result<Option<ContextMenu>, JsError>,
    send: Send,
    cx: &mut App,
) {
    let menu = match menu {
        Ok(Some(menu)) => menu,
        Ok(None) => return,
        Err(error) => {
            eprintln!("asrs: {what}: {error:?}");
            return;
        }
    };
    let on_effects = menu_effects(handle, send);
    let platform = Platform::current();
    let _ = handle.update(cx, |_, window, cx| {
        bridge::popup_context(window, cx, platform, &menu, on_effects)
    });
}

/// The context menu a satellite's message handler pops up for `message`, if any.
pub(crate) fn show_satellite_menu(
    handle: AnyWindowHandle,
    satellite: WeakEntity<Satellite>,
    message: &NamedMessage<SatelliteToMain>,
    cx: &mut App,
) {
    let menu = context::for_satellite_message(message);
    let name = message.name.name();
    popup(handle, name, menu, send_to_satellite(satellite), cx);
}

/// The context menu `main.ts` pops up for `message`, if any.
fn show_context_menu(
    handle: AnyWindowHandle,
    hub: WeakEntity<Hub>,
    dark: bool,
    message: &NamedMessage<HubToMain>,
    cx: &mut App,
) {
    let recent_units = match message.name {
        HubToMain::AskEditAxis => match MainProcess::recent_units(cx) {
            Ok(recents) => recents,
            Err(error) => {
                eprintln!("asrs: {}: {error}", message.name.name());
                return;
            }
        },
        _ => None,
    };
    let env = MenuEnv { dark, recent_units };
    let menu = context::for_hub_message(message, &env);
    popup(handle, message.name.name(), menu, send_to_hub(hub), cx);
}

impl FrameHost for HubEntityHost {
    fn geometry(&self) -> FrameGeometry {
        self.geometry.clone()
    }

    fn source_list_geometry(&self) -> ListsGeometry {
        self.lists.clone()
    }

    fn frame_time(&self, cx: &App) -> f64 {
        self.hub.read(cx).frame_time(cx)
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

    fn take_touched_field(&mut self) -> Option<usize> {
        sidebar::take_touched_field(&self.sidebar_geometry)
    }

    fn set_menu_buttons(&mut self, rects: Vec<[f64; 4]>) {
        sidebar::set_menu_buttons(&self.sidebar_geometry, rects);
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
