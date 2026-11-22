//! The hub window's page as a GPUI entity. [`Hub`] owns `asrs_hub`'s headless [`HubModel`] and
//! is its browser: timers and intervals are foreground tasks on executor timers, animation
//! frames are the window's next frames, each decode worker is an OS thread whose messages a
//! foreground task pumps into the model, the export worker is a one-shot thread, and main's
//! `historical-start` runs `asrs_services::historical_start` on the background executor.
//!
//! Views read the model through [`Hub::model`] and observe the entity: it notifies after every
//! task the page runs and emits a [`HubEvent`] for each change a view or main reacts to.

mod host;

use std::rc::Rc;

use asrs_formats::historical::Status;
use asrs_hub::hub::{HubHost, HubModel, HubView, LoadingGlow};
use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng, SystemClock, V8MathRandom};
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_ui::gpui::{Context, EventEmitter, Window};

use host::{GpuiHost, HostState};

/// `(Math.random, Date)` for a new decode worker's realm.
pub type WorkerRealm = Box<dyn FnMut() -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>)>;

pub struct HubConfig {
    /// What the page reads from its layout.
    pub view: Box<dyn HubView>,
    /// The page's `Math.random`.
    pub realm: JsRealm,
    /// The page's `Date`.
    pub clock: Rc<dyn Clock>,
    pub worker_realm: WorkerRealm,
    /// `app.getPath("temp")`, where `historical-start-raw` writes its child files.
    pub temp_dir: String,
}

impl HubConfig {
    pub fn new(view: Box<dyn HubView>) -> Self {
        HubConfig {
            view,
            realm: JsRealm::new(Box::new(V8MathRandom::from_entropy())),
            clock: Rc::new(SystemClock),
            worker_realm: Box::new(|| {
                (
                    Box::new(V8MathRandom::from_entropy()),
                    Box::new(SystemClock),
                )
            }),
            temp_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum HubEvent {
    /// [`HubModel::title`] changed.
    Title,
    /// [`HubModel::loading`] changed.
    Loading,
    /// A historical source's status callback ran.
    SourceStatus {
        source: usize,
        status: Status,
    },
    /// A message for main, other than `historical-start`, which the hub answers itself.
    Main(NamedMessage<HubToMain>),
    /// `app.addRecentDocument(path)` after `historical-start`.
    RecentDocument(JsString),
    Console {
        warn: bool,
        text: String,
    },
    /// A throw the page goes on from, or a worker or main handler that failed where upstream's
    /// would have reported an error.
    Exception {
        task: &'static str,
        error: String,
    },
}

pub struct Hub {
    model: HubModel,
    host: HostState,
    title: JsString,
    loading: LoadingGlow,
}

impl EventEmitter<HubEvent> for Hub {}

impl Hub {
    /// Evaluates the page in `window`, whose frames drive the per-frame loop; without a window
    /// frames run on a 60 Hz timer.
    pub fn new(window: Option<&Window>, config: HubConfig, cx: &mut Context<'_, Self>) -> Self {
        let mut host = HostState::new(
            window.map(Window::window_handle),
            config.clock.clone(),
            config.worker_realm,
            config.temp_dir,
        );
        let model = HubModel::new(
            config.realm,
            config.clock,
            config.view,
            &mut GpuiHost::new(&mut host, cx),
        );
        let mut hub = Hub {
            title: model.title().clone(),
            loading: model.loading().clone(),
            model,
            host,
        };
        hub.after_task(cx);
        hub
    }

    pub fn model(&self) -> &HubModel {
        &self.model
    }

    /// `handleMainMessage`. `historical-data` is the hub's own business and is ignored here.
    pub fn handle_main_message(
        &mut self,
        message: NamedMessage<MainToHub>,
        cx: &mut Context<'_, Self>,
    ) {
        if message.name == MainToHub::HistoricalData {
            return;
        }
        self.task(cx, |model, host| model.handle_main_message(&message, host));
    }

    /// Runs `f` as one page task, such as an input handler a view dispatches into the
    /// sidebar or tabs, and then publishes what it changed.
    pub fn update<R>(
        &mut self,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut HubModel, &mut dyn HubHost) -> R,
    ) -> R {
        self.task(cx, f)
    }

    /// Decode threads still running, terminated workers included until their thread exits.
    pub fn decode_threads(&self) -> usize {
        self.host.decode_threads()
    }

    fn task<R>(
        &mut self,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut HubModel, &mut dyn HubHost) -> R,
    ) -> R {
        let result = f(&mut self.model, &mut GpuiHost::new(&mut self.host, cx));
        self.after_task(cx);
        result
    }

    fn after_task(&mut self, cx: &mut Context<'_, Self>) {
        for (source, status) in self.model.take_status_changes() {
            cx.emit(HubEvent::SourceStatus { source, status });
        }
        if *self.model.title() != self.title {
            self.title = self.model.title().clone();
            cx.emit(HubEvent::Title);
        }
        if *self.model.loading() != self.loading {
            self.loading = self.model.loading().clone();
            cx.emit(HubEvent::Loading);
        }
        cx.notify();
    }
}
