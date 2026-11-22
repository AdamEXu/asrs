//! The hub window's root: a [`HubFrame`] over the [`Hub`] entity, which it observes. Until
//! main (P) exists, the window is booted with the messages `main.ts` sends a new hub window,
//! and what the hub sends main is dropped.

use asrs_hub::hub::{HubModel, HubView};
use asrs_hub::sidebar::{RowRect, RowValue, Sidebar};
use asrs_js::time::{Rng, SystemClock, V8MathRandom};
use asrs_js::value::{JsValue, ToJs};
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_ui::gpui::{App, AppContext, Entity, Window};

use super::frame::{FrameGeometry, FrameHost, HubFrame};
use crate::hub::{Hub, HubConfig, HubEvent};

struct FrameView {
    geometry: FrameGeometry,
    dark: bool,
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

    fn can_scroll_into_view(&self) -> bool {
        true
    }

    fn sidebar_rects(&self, sidebar: &Sidebar) -> Vec<RowRect> {
        vec![RowRect::default(); sidebar.rows().len()]
    }

    fn measure_value(&self, _value: &RowValue) -> f64 {
        0.0
    }
}

/// The `Math.random()` calls upstream's hub bundle makes while its imports evaluate, before
/// hub.ts draws its first UUID: 19 three.js objects built at module scope (4 draws each in
/// `MathUtils.generateUUID`) and bignumber.js's `BigNumber.random` setup.
const IMPORT_RANDOM_DRAWS: usize = 77;

pub struct HubEntityHost {
    hub: Entity<Hub>,
    geometry: FrameGeometry,
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
        let preferences = asrs_model::preferences::Preferences::default().to_js();
        Self::open_with(
            window,
            dark,
            boot_messages(JsValue::Null, is_beta, preferences, files),
            None,
            cx,
        )
    }

    /// The hub window's root view, booted with `boot` in place of main's messages. With
    /// `random_seed`, the page's and its workers' `Math.random` are V8's under
    /// `--random-seed=<seed>`.
    pub fn open_with(
        window: &Window,
        dark: bool,
        boot: Vec<NamedMessage<MainToHub>>,
        random_seed: Option<u64>,
        cx: &mut App,
    ) -> Entity<HubFrame<HubEntityHost>> {
        let geometry = FrameGeometry::default();
        let view = Box::new(FrameView {
            geometry: geometry.clone(),
            dark,
        });
        let mut config = HubConfig::new(view);
        if let Some(seed) = random_seed {
            let mut random = V8MathRandom::from_seed(seed);
            for _ in 0..IMPORT_RANDOM_DRAWS {
                random.next_f64();
            }
            config.realm = JsRealm::new(Box::new(random));
            config.worker_realm = Box::new(move || {
                (
                    Box::new(V8MathRandom::from_seed(seed)),
                    Box::new(SystemClock),
                )
            });
        }
        let hub = cx.new(|cx| Hub::new(Some(window), config, cx));
        cx.subscribe(&hub, |_, event, _| match event {
            HubEvent::Console { warn: true, text } => eprintln!("asrs: {text}"),
            HubEvent::Exception { task, error } => eprintln!("asrs: {task}: {error}"),
            _ => {}
        })
        .detach();
        hub.update(cx, |hub, cx| {
            for message in boot {
                geometry.sync(hub.model());
                hub.handle_main_message(message, cx);
            }
        });
        cx.new(|cx| {
            cx.observe(&hub, |_, _, cx| cx.notify()).detach();
            HubFrame::new(HubEntityHost { hub, geometry }, cx)
        })
    }

    pub fn hub(&self) -> &Entity<Hub> {
        &self.hub
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
}
