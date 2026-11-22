//! A hub model with no main process, workers or live sources behind it: what the window shows
//! until the `HubModel` entity (D07b) drives the frame. It receives the messages `main.ts`
//! sends a new hub window and nothing else.

use std::rc::Rc;

use asrs_formats::historical::{MainRequest, WorkerName};
use asrs_formats::worker::{DecodeWorker, Request};
use asrs_hub::hub::{
    HubFrame as Frame, HubHost, HubInterval, HubModel, HubTimeout, HubView, LiveSourceKind,
};
use asrs_hub::sidebar::{RowRect, RowValue, Sidebar};
use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng, SetClock, SystemClock, V8MathRandom};
use asrs_js::value::{JsValue, ToJs};
use asrs_log::log::SerializedLog;
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};

use super::frame::{FrameGeometry, FrameHost};

struct View {
    geometry: FrameGeometry,
    dark: bool,
}

impl HubView for View {
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

#[derive(Default)]
struct Host {
    outbox: Vec<NamedMessage<HubToMain>>,
}

impl HubHost for Host {
    fn now(&self) -> f64 {
        asrs_js::math::trunc(SystemClock.now())
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.outbox.push(message);
    }

    fn send_historical_request(&mut self, _request: MainRequest) {}

    fn set_timeout(&mut self, _timeout: HubTimeout) {}

    fn set_interval(&mut self, _interval: HubInterval, _period_ms: f64) {}

    fn clear_interval(&mut self, _interval: &HubInterval) {}

    fn request_animation_frame(&mut self, _frame: Frame) {}

    fn worker_realm(&mut self) -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>) {
        (
            Box::new(V8MathRandom::from_entropy()),
            Box::new(SetClock(SystemClock.now())),
        )
    }

    fn spawn_worker(&mut self, _source: &str, _name: WorkerName, _worker: Box<dyn DecodeWorker>) {}

    fn post_worker(&mut self, _source: &str, _request: Request) {}

    fn terminate_worker(&mut self, _source: &str) {}

    fn request_export(&mut self, _id: u64, _log: SerializedLog, _options: JsValue) {}

    fn console(&mut self, warn: bool, text: String) {
        if warn {
            eprintln!("asrs: {text}");
        }
    }

    fn exception(&mut self, task: &'static str, error: String) {
        eprintln!("asrs: {task}: {error}");
    }

    fn start_live_source(&mut self, _kind: LiveSourceKind, _address: &JsString) {}

    fn stop_live_source(&mut self) {}

    fn live_data(&mut self, _data: &JsValue) {}

    fn start_publisher(&mut self, _is_sim: &JsValue) {}

    fn stop_publisher(&mut self) {}
}

pub struct StandaloneHub {
    hub: HubModel,
    host: Host,
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

impl StandaloneHub {
    /// A hub window as `main.ts` boots it with no saved state: `is_beta` shows the feedback
    /// button.
    pub fn new(dark: bool, is_beta: bool) -> Self {
        let geometry = FrameGeometry::default();
        let mut host = Host::default();
        let hub = HubModel::new(
            JsRealm::new(Box::new(V8MathRandom::from_entropy())),
            Rc::new(SystemClock),
            Box::new(View {
                geometry: geometry.clone(),
                dark,
            }),
            &mut host,
        );
        let mut standalone = StandaloneHub {
            hub,
            host,
            geometry,
        };
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
        let boot = [
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
        for (name, data) in boot {
            standalone.geometry.sync(&standalone.hub);
            standalone
                .hub
                .handle_main_message(&NamedMessage::new(name, data), &mut standalone.host);
        }
        standalone.host.outbox.clear();
        standalone
    }
}

impl FrameHost for StandaloneHub {
    fn geometry(&self) -> FrameGeometry {
        self.geometry.clone()
    }

    fn hub(&self) -> &HubModel {
        &self.hub
    }

    fn hub_mut(&mut self) -> &mut HubModel {
        &mut self.hub
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.host.outbox.push(message);
    }
}
