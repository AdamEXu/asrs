use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use asrs_formats::export::{ExportError, ExportOptions, Format, Output, Sampling, export};
use asrs_formats::historical::{MainRequest, WorkerName};
use asrs_formats::worker::{Abort, DecodeWorker, Request, WorkerMessage, WorkerThread};
use asrs_hub::hub::{ExportReply, HubFrame, HubHost, HubInterval, HubTimeout};
use asrs_hub::live::{LiveNet, LiveRole};
use asrs_hub::tabs::SatelliteUpdate;
use asrs_js::time::{Clock, Rng, V8MathRandom};
use asrs_js::value::JsValue;
use asrs_live::nt4::TokioTransport;
use asrs_live::{NetOptions, TimerId, TimerQueue};
use asrs_log::log::{Log, LogError, SerializedLog};
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_services::fs::RealFs;
use asrs_services::historical_start::{FileOpen, Outcome, UPSTREAM_HAS_LZMA};
use asrs_ui::gpui::{App, AppContext, Context, Task, Window};

use super::frames::{FRAME_PERIOD, Frames};
use super::{Hub, HubEvent, WorkerRealm, frame_time};
use crate::live::{self, Slice};

/// The delay Blink gives `setInterval(fn, period_ms)`: WebIDL converts the timeout to a
/// `long`, truncating `1000 / 60` to 16, and never less than 1 ms.
fn interval_period(period_ms: f64) -> Duration {
    let ms = if period_ms.is_finite() && period_ms >= 1.0 {
        period_ms.trunc()
    } else {
        1.0
    };
    Duration::from_millis(ms as u64)
}

/// Blink's `TimerBase::RunInternal`: a repeating timer's next fire time is set before its
/// callback runs and stays on the original period grid, skipping beats it missed, so the
/// callback's own duration and late wakeups never make it drift. A beat less than a 20th of the
/// period away counts as missed.
fn next_fire(scheduled: Instant, now: Instant, period: Duration) -> Instant {
    let late = (now + period / 20).saturating_duration_since(scheduled);
    let beats = late.as_nanos().div_ceil(period.as_nanos()).max(1);
    scheduled + Duration::from_nanos((beats * period.as_nanos()) as u64)
}

/// `setInterval(fire, period_ms)` in the page of `cx`'s entity, until the task is dropped.
pub(crate) fn interval<T: 'static>(
    cx: &Context<'_, T>,
    period_ms: f64,
    mut fire: impl FnMut(&mut T, &mut Context<'_, T>) + 'static,
) -> Task<()> {
    let period = interval_period(period_ms);
    cx.spawn(async move |this, cx| {
        let executor = cx.background_executor().clone();
        let mut next = executor.now() + period;
        loop {
            executor
                .timer(next.saturating_duration_since(executor.now()))
                .await;
            next = next_fire(next, executor.now(), period);
            if this.update(cx, &mut fire).is_err() {
                break;
            }
        }
    })
}

struct WorkerSlot {
    id: u64,
    source: String,
    thread: WorkerThread,
    unfinished: usize,
    stuck: bool,
    _pump: Task<()>,
}

enum ExportMessage {
    Reply(ExportReply),
    Unmodeled(&'static str),
    /// The worker posted progress since the last of these; the cell holds its latest value.
    Progressed,
}

/// The worker's side of its progress messages. Upstream posts one per field and timestamp, and
/// only the newest of those waiting can be seen, so at most one waits here.
#[derive(Clone, Default)]
struct ExportProgress(Arc<Mutex<Option<f64>>>);

impl ExportProgress {
    /// Whether no earlier value was waiting.
    fn post(&self, value: f64) -> bool {
        let mut latest = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        latest.replace(value).is_none()
    }

    fn take(&self) -> Option<f64> {
        let mut latest = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        latest.take()
    }
}

enum PageTimer {
    Interval(HubInterval),
    Timeout(HubTimeout),
}

pub(super) struct HostState {
    clock: Rc<dyn Clock>,
    pub(super) frame_origin: Instant,
    worker_realm: WorkerRealm,
    temp_dir: String,
    timers: TimerQueue,
    timer_tasks: HashMap<TimerId, PageTimer>,
    timer_due: Option<f64>,
    timer_alarm: Option<Task<()>>,
    pub(super) frames: Frames,
    frame_callbacks: Vec<HubFrame>,
    next_worker: u64,
    workers: Vec<WorkerSlot>,
    terminated: Vec<WorkerThread>,
    exports: Vec<(u64, Task<()>)>,
    posted: (SyncSender<()>, Receiver<()>),
    live_pumps: HashMap<LiveRole, Task<()>>,
    live_timer: live::Alarm,
}

impl HostState {
    pub(super) fn new(
        window: Option<&Window>,
        clock: Rc<dyn Clock>,
        worker_realm: WorkerRealm,
        temp_dir: String,
        cx: &mut Context<'_, Hub>,
    ) -> Self {
        let frame_origin = cx.background_executor().now();
        let timer_clock = live::ExecutorClock::new(frame_origin, cx);
        HostState {
            clock,
            frame_origin,
            worker_realm,
            temp_dir,
            timers: TimerQueue::new(Rc::new(timer_clock)),
            timer_tasks: HashMap::new(),
            timer_due: None,
            timer_alarm: None,
            frames: Frames::new(window, cx),
            frame_callbacks: Vec::new(),
            next_worker: 0,
            workers: Vec::new(),
            terminated: Vec::new(),
            exports: Vec::new(),
            posted: sync_channel(1),
            live_pumps: HashMap::new(),
            live_timer: live::Alarm::default(),
        }
    }

    pub(super) fn decode_threads(&self) -> usize {
        self.workers
            .iter()
            .map(|w| &w.thread)
            .chain(&self.terminated)
            .filter(|t| !t.is_finished())
            .count()
    }

    pub(super) fn decode_threads_started(&self) -> u64 {
        self.next_worker
    }

    fn timer_clock(&self, cx: &App) -> live::ExecutorClock {
        live::ExecutorClock::new(self.frame_origin, cx)
    }

    pub(super) fn frame_requested(&self) -> bool {
        !self.frame_callbacks.is_empty()
    }

    pub(super) fn begin_task(&mut self) {
        self.timers.begin_task();
    }

    pub(super) fn posted(&self) -> SyncSender<()> {
        self.posted.0.clone()
    }

    pub(super) fn wait_for_live(&self, timeout: Duration) -> bool {
        self.posted.1.recv_timeout(timeout).is_ok()
    }

    pub(super) fn wait_for_threads(&self, timeout: Duration) -> Result<bool, RecvTimeoutError> {
        if self.exports.is_empty() && self.workers.iter().all(|w| w.unfinished == 0) {
            return Ok(false);
        }
        self.posted.1.recv_timeout(timeout).map(|()| true)
    }
}

pub(super) struct GpuiHost<'a, 'b> {
    state: &'a mut HostState,
    cx: &'a mut Context<'b, Hub>,
}

impl<'a, 'b> GpuiHost<'a, 'b> {
    pub(super) fn new(state: &'a mut HostState, cx: &'a mut Context<'b, Hub>) -> Self {
        GpuiHost { state, cx }
    }

    fn emit(&mut self, event: HubEvent) {
        self.cx.emit(event);
    }
}

impl Hub {
    pub(super) fn frame(&mut self, cx: &mut Context<'_, Self>) {
        for frame in std::mem::take(&mut self.host.frame_callbacks) {
            self.task(cx, |model, host| model.animation_frame(&frame, host));
        }
    }

    /// Sets the host's one alarm for whichever page timer is due first.
    pub(super) fn arm_timer(&mut self, cx: &mut Context<'_, Self>) {
        let due = self.host.timers.next_due();
        if self.host.timer_due == due {
            return;
        }
        self.host.timer_due = due;
        self.host.timer_alarm = due.map(|due| {
            let wait = ((due - self.host.timers.now()) * 1e6).ceil().max(0.0) as u64;
            let executor = cx.background_executor();
            let timer = (wait > 0).then(|| executor.timer(Duration::from_nanos(wait)));
            cx.spawn(async move |this, cx| {
                if let Some(timer) = timer {
                    timer.await;
                }
                let _ = this.update(cx, |hub, cx| hub.fire_timer(cx));
            })
        });
    }

    fn fire_timer(&mut self, cx: &mut Context<'_, Self>) {
        self.host.timer_due = None;
        let Some(id) = self.host.timers.fire_next() else {
            self.arm_timer(cx);
            return;
        };
        let timers = &mut self.host.timer_tasks;
        match timers.remove(&id) {
            Some(PageTimer::Interval(interval)) => {
                timers.insert(id, PageTimer::Interval(interval.clone()));
                self.run(cx, |model, host| model.fire_interval(&interval, host));
            }
            Some(PageTimer::Timeout(timeout)) => {
                self.run(cx, |model, host| model.run_timeout(timeout, host));
            }
            None => self.arm_timer(cx),
        }
    }

    /// Arms the host's one timer for whichever live timer is due first. A timer whose task
    /// outlasts its period is due again at once, so each firing is bounded too.
    pub(super) fn arm_live_timer(&mut self, cx: &mut Context<'_, Self>) {
        let due = self.model.next_live_timer();
        let now = self.host.timer_clock(cx).now();
        self.host.live_timer.arm(due, now, cx, |hub: &mut Hub, cx| {
            hub.host.live_timer.clear();
            let slice = Slice::start();
            hub.task(
                cx,
                |model, host| {
                    while model.fire_live_timer(host) && !slice.is_over() {}
                },
            );
        });
    }

    /// Dispatches what has arrived on `net`, one task per event, until `slice` is used up.
    fn live_events(
        &mut self,
        role: LiveRole,
        net: &Rc<RefCell<TokioTransport>>,
        slice: Slice,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        loop {
            let event = net.borrow_mut().next_event();
            let Some(event) = event else {
                return false;
            };
            self.task(cx, |model, host| model.live_event(role, &event, host));
            if slice.is_over() {
                return true;
            }
        }
    }

    fn main_reply(&mut self, outcome: Outcome, cx: &mut Context<'_, Self>) {
        if let Some(path) = outcome.recent_document {
            cx.emit(HubEvent::RecentDocument(path));
        }
        if outcome.uncaught_error {
            cx.emit(HubEvent::Exception {
                task: "main",
                error: "historical-start threw after replying".to_owned(),
            });
        }
        if let Some(reply) = outcome.reply {
            self.task(cx, |model, host| model.historical_data(&reply, host));
        }
    }

    fn worker_ready(&mut self, id: u64, cx: &mut Context<'_, Self>) {
        let Some(slot) = self.host.workers.iter_mut().find(|w| w.id == id) else {
            return;
        };
        let source = slot.source.clone();
        let message = slot.thread.try_recv();
        if let Some(WorkerMessage::Done(end)) = &message {
            slot.stuck = matches!(end, Err(Abort::NeverReturns | Abort::Unmodeled(_)));
            slot.unfinished = if slot.stuck { 0 } else { slot.unfinished - 1 };
        }
        let error = match message {
            Some(WorkerMessage::Response(response)) => {
                self.task(cx, |model, host| {
                    model.worker_message(&source, response, host)
                });
                return;
            }
            Some(WorkerMessage::Done(Err(Abort::Threw))) => "the decode worker threw".to_owned(),
            Some(WorkerMessage::Done(Err(Abort::NeverReturns))) => {
                "the decode worker never returns".to_owned()
            }
            Some(WorkerMessage::Done(Err(Abort::Unmodeled(what)))) => {
                format!("unmodeled: {what}")
            }
            Some(WorkerMessage::Done(Ok(()))) | None => return,
        };
        cx.emit(HubEvent::Exception {
            task: "worker",
            error,
        });
    }

    fn export_message(
        &mut self,
        id: u64,
        message: ExportMessage,
        progress: &ExportProgress,
        cx: &mut Context<'_, Self>,
    ) {
        match message {
            ExportMessage::Progressed => {
                if let Some(value) = progress.take() {
                    self.task(cx, |model, host| {
                        model.export_reply(id, ExportReply::Progress(value), host)
                    });
                }
            }
            ExportMessage::Reply(reply) => {
                self.task(cx, |model, host| model.export_reply(id, reply, host));
                self.host.exports.retain(|(i, _)| *i != id);
            }
            ExportMessage::Unmodeled(what) => {
                cx.emit(HubEvent::Exception {
                    task: "worker",
                    error: format!("unmodeled: {what}"),
                });
                self.host.exports.retain(|(i, _)| *i != id);
            }
        }
    }
}

/// What `exportWorker.ts` reads from `payload.options`.
fn export_options(options: &JsValue) -> Option<ExportOptions> {
    let text = |key: &str| {
        options
            .get(key)
            .and_then(JsValue::as_str)
            .map(|s| s.to_string_lossy())
    };
    Some(ExportOptions {
        format: match text("format")?.as_str() {
            "csv-table" => Format::CsvTable,
            "csv-list" => Format::CsvList,
            "wpilog" => Format::Wpilog,
            "mcap" => Format::Mcap,
            _ => return None,
        },
        sampling: match text("samplingMode")?.as_str() {
            "changes" => Sampling::Changes,
            "fixed" => Sampling::Fixed,
            "akit" => Sampling::Akit,
            _ => return None,
        },
        sampling_period: options.get("samplingPeriod")?.as_f64()?,
        prefixes: options.get("prefixes")?.as_str()?.clone(),
        include_generated: matches!(options.get("includeGenerated"), Some(JsValue::Bool(true))),
    })
}

fn run_export(
    log: SerializedLog,
    options: &JsValue,
    progress: &mut dyn FnMut(f64),
) -> ExportMessage {
    let mut realm = JsRealm::new(Box::new(V8MathRandom::from_entropy()));
    let output = match (
        Log::from_serialized(log, &mut realm),
        export_options(options),
    ) {
        (Ok(log), Some(options)) => export(&log, &options, progress),
        (Err(LogError::Proto(_) | LogError::StackOverflow), _) => Err(ExportError::Rejected),
        (Err(LogError::NeverCompletes), _) => Err(ExportError::Unported(
            "a log the export worker never finishes reading",
        )),
        (Err(LogError::Unmodeled(what)), _) => Err(ExportError::Unported(what)),
        (_, None) => Err(ExportError::Unported("export options of another shape")),
    };
    ExportMessage::Reply(ExportReply::Payload(match output {
        Ok(Output::Text(text)) => JsValue::String(text),
        Ok(Output::Bytes(bytes)) => JsValue::Bytes(bytes),
        Err(ExportError::Rejected) => return ExportMessage::Reply(ExportReply::Failed),
        Err(ExportError::Unported(what)) => return ExportMessage::Unmodeled(what),
    }))
}

impl HubHost for GpuiHost<'_, '_> {
    fn now(&self) -> f64 {
        self.state.clock.now()
    }

    fn frame_time(&self) -> f64 {
        frame_time(self.state.frame_origin, &*self.cx)
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.emit(HubEvent::Main(message));
    }

    fn update_satellite(&mut self, update: SatelliteUpdate) {
        self.emit(HubEvent::SatelliteUpdate(update));
    }

    fn send_historical_request(&mut self, request: MainRequest) {
        let temp_dir = self.state.temp_dir.clone();
        let read = self.cx.background_spawn(async move {
            let open = FileOpen {
                fs: &RealFs,
                temp_dir: &temp_dir,
                lzma: UPSTREAM_HAS_LZMA,
            };
            open.handle(request, &mut V8MathRandom::from_entropy())
        });
        self.cx
            .spawn(async move |this, cx| {
                let outcome = read.await;
                let _ = this.update(cx, |hub, cx| hub.main_reply(outcome, cx));
            })
            .detach();
    }

    fn set_timeout(&mut self, timeout: HubTimeout) {
        let id = self.state.timers.set_timeout(0.0);
        let timer = PageTimer::Timeout(timeout);
        self.state.timer_tasks.insert(id, timer);
    }

    fn set_interval(&mut self, interval: HubInterval, period_ms: f64) {
        // Nothing here has a Touch Bar to show what it sends (#402).
        if interval == HubInterval::TouchBarSlider {
            return;
        }
        self.clear_interval(&interval);
        let id = self.state.timers.set_interval(period_ms);
        let timer = PageTimer::Interval(interval);
        self.state.timer_tasks.insert(id, timer);
    }

    fn clear_interval(&mut self, interval: &HubInterval) {
        let timers = &mut self.state.timers;
        self.state.timer_tasks.retain(|id, timer| {
            let cleared = matches!(timer, PageTimer::Interval(i) if i == interval);
            if cleared {
                timers.clear(*id);
            }
            !cleared
        });
    }

    fn request_animation_frame(&mut self, frame: HubFrame) {
        self.state.frame_callbacks.push(frame);
        if self.state.frame_callbacks.len() == 1 {
            self.state.frames.schedule(self.cx);
        }
    }

    fn worker_realm(&mut self) -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>) {
        (self.state.worker_realm)()
    }

    fn spawn_worker(&mut self, source: &str, _name: WorkerName, worker: Box<dyn DecodeWorker>) {
        let (ready, wake) = async_channel::unbounded::<()>();
        let posted = self.state.posted.0.clone();
        let thread = match WorkerThread::spawn_notifying(source, worker, move || {
            let _ = ready.try_send(());
            let _ = posted.try_send(());
        }) {
            Ok(thread) => thread,
            Err(e) => {
                self.emit(HubEvent::Exception {
                    task: "worker",
                    error: format!("cannot start a decode thread: {e}"),
                });
                return;
            }
        };
        let id = self.state.next_worker;
        self.state.next_worker += 1;
        let pump = self.cx.spawn(async move |this, cx| {
            while wake.recv().await.is_ok() {
                if this.update(cx, |hub, cx| hub.worker_ready(id, cx)).is_err() {
                    break;
                }
            }
        });
        self.state.workers.push(WorkerSlot {
            id,
            source: source.to_owned(),
            thread,
            unfinished: 0,
            stuck: false,
            _pump: pump,
        });
    }

    fn post_worker(&mut self, source: &str, request: Request) {
        if let Some(slot) = self.state.workers.iter_mut().rfind(|w| w.source == source) {
            slot.unfinished += usize::from(!slot.stuck);
            slot.thread.post(request);
        }
    }

    fn terminate_worker(&mut self, source: &str) {
        let Some(index) = self.state.workers.iter().rposition(|w| w.source == source) else {
            return;
        };
        let mut slot = self.state.workers.remove(index);
        slot.thread.terminate();
        self.state.terminated.retain(|t| !t.is_finished());
        self.state.terminated.push(slot.thread);
    }

    fn request_export(&mut self, id: u64, log: SerializedLog, options: JsValue) {
        let (send, receive) = async_channel::unbounded();
        let progress = ExportProgress::default();
        let worker = progress.clone();
        let posted = self.state.posted.0.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("export-{id}"))
            .spawn(move || {
                let end = run_export(log, &options, &mut |value| {
                    if worker.post(value) {
                        let _ = send.send_blocking(ExportMessage::Progressed);
                        let _ = posted.try_send(());
                    }
                });
                let _ = send.send_blocking(end);
                let _ = posted.try_send(());
            });
        if let Err(e) = spawned {
            self.emit(HubEvent::Exception {
                task: "worker",
                error: format!("cannot start the export thread: {e}"),
            });
            return;
        }
        let pump = self.cx.spawn(async move |this, cx| {
            while let Ok(message) = receive.recv().await {
                if this
                    .update(cx, |hub, cx| hub.export_message(id, message, &progress, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.state.exports.push((id, pump));
    }

    fn console(&mut self, warn: bool, text: String) {
        self.emit(HubEvent::Console { warn, text });
    }

    fn exception(&mut self, task: &'static str, error: String) {
        self.emit(HubEvent::Exception { task, error });
    }

    fn live_net(&mut self, role: LiveRole) -> Result<LiveNet, String> {
        let handle = live::runtime(self.cx)?;
        let (wake, ready) = async_channel::bounded::<()>(1);
        let posted = self.state.posted.0.clone();
        let options = NetOptions {
            handle,
            notify: Arc::new(move || {
                let _ = wake.try_send(());
                let _ = posted.try_send(());
            }),
            dial: live::dial(self.cx),
        };
        let whole_bodies = role == LiveRole::Source { whole_bodies: true };
        let net = Rc::new(RefCell::new(if whole_bodies {
            TokioTransport::with_bodies(options)
        } else {
            TokioTransport::new(options)
        }));
        let weak = Rc::downgrade(&net);
        let pump = self.cx.spawn(async move |this, cx| {
            live::pump(ready, cx, |slice, cx| {
                let net = weak.upgrade()?;
                this.update(cx, |hub, cx| hub.live_events(role, &net, slice, cx))
                    .ok()
            })
            .await;
        });
        self.state.live_pumps.insert(role, pump);
        Ok(net)
    }

    fn timer_clock(&self) -> Rc<dyn Clock> {
        Rc::new(self.state.timer_clock(self.cx))
    }

    fn idle_period(&self) -> Option<f64> {
        Some(FRAME_PERIOD.as_secs_f64() * 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use asrs_log::log::proto_decoder::SerializedProtoDecoder;

    use super::*;

    #[test]
    fn an_export_whose_log_replay_throws_fails() {
        let anon = b"\x22\x11\x0a\x04Anon\x12\x09\x0a\x01x\x18\x01\x20\x01\x28\x05".to_vec();
        let mut log = Log::new(true).to_serialized();
        log.proto_decoder = SerializedProtoDecoder(vec![anon.clone()]);
        let once = run_export(log.clone(), &JsValue::Null, &mut |_| {});
        assert!(matches!(once, ExportMessage::Unmodeled(_)));
        log.proto_decoder = SerializedProtoDecoder(vec![anon.clone(), anon]);
        let reply = run_export(log, &JsValue::Null, &mut |_| {});
        assert!(matches!(reply, ExportMessage::Reply(ExportReply::Failed)));
    }

    #[test]
    fn intervals_stay_on_their_period_grid() {
        let period = interval_period(250.0);
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        assert_eq!(next_fire(ms(250), ms(250), period), ms(500));
        assert_eq!(next_fire(ms(250), ms(280), period), ms(500));
        assert_eq!(next_fire(ms(250), ms(485), period), ms(500));
        assert_eq!(next_fire(ms(250), ms(490), period), ms(750));
        assert_eq!(next_fire(ms(250), ms(790), period), ms(1000));
        assert_eq!(interval_period(1000.0 / 60.0), Duration::from_millis(16));
    }
}
