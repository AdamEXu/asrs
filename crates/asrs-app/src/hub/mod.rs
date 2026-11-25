//! The hub window's page as a GPUI entity. [`Hub`] owns `asrs_hub`'s headless [`HubModel`] and
//! is its browser: timers and intervals fire as foreground tasks, animation frames are the
//! window's next frames while it has any, each decode worker is an OS thread whose messages a
//! foreground task pumps into the model, the export worker is a one-shot thread, and main's
//! `historical-start` runs `asrs_services::historical_start` on the background executor. Live
//! sources and the publisher run on the foreground over sockets of the app's one
//! [`asrs_live::LiveRuntime`]; main's RLOG socket is [`crate::live::RlogBridge`].
//!
//! Views read the model through [`Hub::model`] and observe the entity: it notifies after every
//! task the page runs, but after its frames and its saving intervals only when what they show
//! changed, and emits a [`HubEvent`] for each change a view or main reacts to.

mod frames;
mod host;

use std::rc::Rc;
use std::sync::mpsc::{RecvTimeoutError, SyncSender};
use std::time::{Duration, Instant};

use asrs_formats::historical::Status;
use asrs_hub::hub::{HubHost, HubModel, HubView, LoadingGlow};
use asrs_hub::tabs::SatelliteUpdate;
use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng, SystemClock, V8MathRandom};
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_ui::gpui::{App, BackgroundExecutor, Context, EventEmitter, Window};

use crate::repaint::Repaint;
use host::{GpuiHost, HostState};

pub(crate) use host::interval;

/// `(Math.random, Date)` for a new decode worker's realm.
pub type WorkerRealm = Box<dyn FnMut() -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>)>;

/// Milliseconds on the frame clock since `origin`: the executor's clock, which is simulated
/// under a test dispatcher, so animations stepped by it do not depend on how long frames take.
pub(crate) fn frame_time(origin: Instant, cx: &App) -> f64 {
    cx.background_executor()
        .now()
        .saturating_duration_since(origin)
        .as_secs_f64()
        * 1000.0
}

/// `Date` on the frame clock: the wall time when made, then advancing only as the executor's
/// clock does, so under a simulated dispatcher the page's timers and its animations agree.
pub struct FrameClock {
    executor: BackgroundExecutor,
    origin: Instant,
    epoch: f64,
}

impl FrameClock {
    pub fn new(cx: &App) -> Self {
        let executor = cx.background_executor().clone();
        FrameClock {
            origin: executor.now(),
            executor,
            epoch: SystemClock.now(),
        }
    }
}

impl Clock for FrameClock {
    fn now(&self) -> f64 {
        let elapsed = self.executor.now().saturating_duration_since(self.origin);
        self.epoch + elapsed.as_millis() as f64
    }
}

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
    /// `update-satellite`.
    SatelliteUpdate(SatelliteUpdate),
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
    repaint: Repaint,
    moving: bool,
}

impl EventEmitter<HubEvent> for Hub {}

impl Hub {
    /// Evaluates the page in `window`, whose frames drive the per-frame loop while it has any; on
    /// a timer otherwise.
    pub fn new(window: Option<&Window>, config: HubConfig, cx: &mut Context<'_, Self>) -> Self {
        let mut host = HostState::new(
            window,
            config.clock.clone(),
            config.worker_realm,
            config.temp_dir,
            cx,
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
            repaint: Repaint::default(),
            moving: true,
        };
        hub.repaint.changed();
        hub.after_task(cx);
        cx.notify();
        hub
    }

    pub fn model(&self) -> &HubModel {
        &self.model
    }

    /// [`HubHost::frame_time`] now: what wheel events over the page's scroll sensors are
    /// stamped with.
    pub fn frame_time(&self, cx: &App) -> f64 {
        frame_time(self.host.frame_origin, cx)
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
    /// sidebar or tabs, then sends what it queued for main and publishes what it changed.
    pub fn update<R>(
        &mut self,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut HubModel, &mut dyn HubHost) -> R,
    ) -> R {
        self.task(cx, |model, host| {
            let result = f(model, host);
            model.end_input_task(host);
            result
        })
    }

    /// Decode threads still running, terminated workers included until their thread exits.
    pub fn decode_threads(&self) -> usize {
        self.host.decode_threads()
    }

    /// Decode threads ever started, whether or not they still run.
    pub fn decode_threads_started(&self) -> u64 {
        self.host.decode_threads_started()
    }

    pub(crate) fn live_posted(&self) -> SyncSender<()> {
        self.host.posted()
    }

    /// Blocks until a live connection posts to the page.
    pub fn wait_for_live(&self, timeout: Duration) -> bool {
        self.host.wait_for_live(timeout)
    }

    /// Blocks until a decode or export thread posts to the page. False, at once, when none of
    /// them has anything left to post.
    pub fn wait_for_threads(&self, timeout: Duration) -> Result<bool, RecvTimeoutError> {
        self.host.wait_for_threads(timeout)
    }

    fn task<R>(
        &mut self,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut HubModel, &mut dyn HubHost) -> R,
    ) -> R {
        self.host.begin_task();
        self.run(cx, f)
    }

    /// The body of a task, a page timer's unless [`Hub::task`] is running it.
    fn run<R>(
        &mut self,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut HubModel, &mut dyn HubHost) -> R,
    ) -> R {
        let result = self.run_quietly(cx, f);
        self.repaint.task();
        cx.notify();
        result
    }

    /// The body of a task that shows nothing new unless the model says so.
    fn run_quietly<R>(
        &mut self,
        cx: &mut Context<'_, Self>,
        f: impl FnOnce(&mut HubModel, &mut dyn HubHost) -> R,
    ) -> R {
        let result = f(&mut self.model, &mut GpuiHost::new(&mut self.host, cx));
        self.after_task(cx);
        result
    }

    /// Whether the last frame showed something new, so the satellites it updated do too.
    pub fn moving(&self) -> bool {
        self.moving
    }

    pub(crate) fn begin_draw(&mut self) {
        self.repaint.begin_draw();
    }

    pub(crate) fn end_draw(&mut self) {
        self.repaint.end_draw();
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
        self.arm_timer(cx);
        self.arm_live_timer(cx);
    }
}
