//! The hub window's root: a [`HubFrame`] over the [`Hub`] entity, which it observes, booted
//! with the messages `main.ts` sends a new hub window.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_canvas::text::{FixedAdvance, TextBackend, platform_backend};

use asrs_hub::hub::{HubModel, HubPage, HubView};
use asrs_hub::sidebar::SidebarGeometry;
use asrs_js::time::{Rng, SystemClock, V8MathRandom};
use asrs_js::value::{JsValue, ToJs};
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_model::source_list::{Rect, SourceListDom};
use asrs_model::tab_controller::TabRenderer;
use asrs_model::tab_type::TabType;
use asrs_services::window_manager::Platform;
use asrs_ui::gpui::{
    AnyElement, AnyWindowHandle, App, AppContext, ClipboardItem, Entity, IntoElement, WeakEntity,
    Window,
};

use super::field3d::Field3dTab;
use super::frame::{FrameGeometry, FrameHost, FrameSlot, HubFrame};
use super::prompts;
use super::sidebar::{self, SidebarView, ViewGeometry};
use super::source_list::ListsGeometry;
use crate::hub::{FrameClock, Hub, HubConfig, HubEvent};
use crate::main_process::MainProcess;
use crate::menus::bridge::{self, EffectHandler};
use crate::menus::context::{self, Effect, MenuEnv};
use crate::windows::Windows;

struct FrameView {
    geometry: FrameGeometry,
    canvas_text: Rc<RefCell<dyn TextBackend>>,
    lists: ListsGeometry,
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

    fn table_box(&self) -> Option<(Rect, f64)> {
        self.geometry.table_box()
    }

    fn dark(&self) -> bool {
        self.dark
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

    fn tab_renderer(
        &self,
        kind: TabType,
        preferences: Option<&Preferences>,
        is_battery: bool,
    ) -> Option<Box<dyn TabRenderer<HubPage>>> {
        match kind {
            TabType::Field3d => Some(Box::new(Field3dTab::new(preferences, is_battery))),
            _ => None,
        }
    }
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
        let sidebar_geometry = ViewGeometry::new();
        let lists = ListsGeometry::default();
        let canvas_text: Box<dyn TextBackend> =
            platform_backend().unwrap_or_else(|| Box::new(FixedAdvance));
        let view = Box::new(FrameView {
            geometry: geometry.clone(),
            canvas_text: Rc::new(RefCell::new(canvas_text)),
            lists: lists.clone(),
            dark,
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
        let handle = window.window_handle();
        let weak = hub.downgrade();
        cx.subscribe(&hub, move |_, event, cx| match event {
            HubEvent::Console { warn: true, text } => eprintln!("asrs: {text}"),
            HubEvent::Exception { task, error } => eprintln!("asrs: {task}: {error}"),
            HubEvent::Main(message) => show_context_menu(handle, weak.clone(), dark, message, cx),
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

/// What a click in a context menu of the hub window `handle` does: messages sent back to the
/// hub, and prompts whose reply has further effects. The source list help window is not wired
/// yet.
fn menu_effects(handle: AnyWindowHandle, hub: WeakEntity<Hub>) -> EffectHandler {
    Rc::new(move |effects, cx| {
        for effect in effects {
            run_effect(effect, handle, &hub, cx);
        }
    })
}

fn run_effect(effect: Effect, handle: AnyWindowHandle, hub: &WeakEntity<Hub>, cx: &mut App) {
    if let Some(message) = effect.to_hub() {
        if let Some(hub) = hub.upgrade() {
            hub.update(cx, |hub, cx| hub.handle_main_message(message, cx));
        }
        return;
    }
    match effect {
        Effect::WriteClipboard(JsValue::String(ref text)) => {
            cx.write_to_clipboard(ClipboardItem::new_string(text.to_string_lossy()))
        }
        Effect::UpdateRecentUnits(preset) => MainProcess::update_recent_units(cx, &preset),
        Effect::Prompt(prompt) => {
            let Some(parent) = Windows::id_of(cx, handle) else {
                return;
            };
            let effects = menu_effects(handle, hub.clone());
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
        other => eprintln!("asrs: context menu effect not available yet: {other:?}"),
    }
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
    let menu = match context::for_hub_message(message, &env) {
        Ok(Some(menu)) => menu,
        Ok(None) => return,
        Err(error) => {
            eprintln!("asrs: {}: {error:?}", message.name.name());
            return;
        }
    };
    let on_effects = menu_effects(handle, hub);
    let platform = Platform::current();
    let _ = handle.update(cx, |_, window, cx| {
        bridge::popup_context(window, cx, platform, &menu, on_effects)
    });
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

    fn editable_focused(&self, cx: &App) -> bool {
        self.sidebar_geometry.search_focused()
            || FrameHost::hub(self, cx).sidebar().number_input_focused()
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
