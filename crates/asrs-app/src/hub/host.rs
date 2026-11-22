use std::collections::{HashMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use asrs_formats::export::{ExportError, ExportOptions, Format, Output, Sampling, export};
use asrs_formats::historical::{MainRequest, WorkerName};
use asrs_formats::worker::{Abort, DecodeWorker, Request, WorkerMessage, WorkerThread};
use asrs_hub::hub::{ExportReply, HubFrame, HubHost, HubInterval, HubTimeout, LiveSourceKind};
use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng, V8MathRandom};
use asrs_js::value::JsValue;
use asrs_log::log::{Log, SerializedLog};
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_services::fs::RealFs;
use asrs_services::historical_start::{FileOpen, Outcome, UPSTREAM_HAS_LZMA};
use asrs_ui::gpui::{AnyWindowHandle, AppContext, Context, Task};

use super::{Hub, HubEvent, WorkerRealm};

/// `requestAnimationFrame` without a window.
const FRAME_PERIOD: Duration = Duration::from_micros(16_667);

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
/// callback's own duration and late wakeups never make it drift.
fn next_fire(scheduled: Instant, now: Instant, period: Duration) -> Instant {
    let late = now.saturating_duration_since(scheduled).as_nanos() % period.as_nanos();
    now + period - Duration::from_nanos(late as u64)
}

struct WorkerSlot {
    id: u64,
    source: String,
    thread: WorkerThread,
    _pump: Task<()>,
}

enum ExportMessage {
    Reply(ExportReply),
    Unmodeled(&'static str),
}

pub(super) struct HostState {
    window: Option<AnyWindowHandle>,
    clock: Rc<dyn Clock>,
    worker_realm: WorkerRealm,
    temp_dir: String,
    intervals: HashMap<HubInterval, Task<()>>,
    timeouts: VecDeque<HubTimeout>,
    frames: Vec<HubFrame>,
    frame_scheduled: bool,
    next_worker: u64,
    workers: Vec<WorkerSlot>,
    terminated: Vec<WorkerThread>,
    exports: Vec<(u64, Task<()>)>,
}

impl HostState {
    pub(super) fn new(
        window: Option<AnyWindowHandle>,
        clock: Rc<dyn Clock>,
        worker_realm: WorkerRealm,
        temp_dir: String,
    ) -> Self {
        HostState {
            window,
            clock,
            worker_realm,
            temp_dir,
            intervals: HashMap::new(),
            timeouts: VecDeque::new(),
            frames: Vec::new(),
            frame_scheduled: false,
            next_worker: 0,
            workers: Vec::new(),
            terminated: Vec::new(),
            exports: Vec::new(),
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

    fn unmodeled(&mut self, task: &'static str, what: &str) {
        self.emit(HubEvent::Exception {
            task,
            error: format!("unmodeled: {what}"),
        });
    }

    fn schedule_frame(&mut self) {
        let this = self.cx.weak_entity();
        match self.state.window {
            Some(window) => self.cx.defer(move |cx| {
                let _ = window.update(cx, |_, window, _| {
                    window.on_next_frame(move |_, cx| {
                        let _ = this.update(cx, |hub, cx| hub.frame(cx));
                    });
                });
            }),
            None => self
                .cx
                .spawn(async move |this, cx| {
                    cx.background_executor().timer(FRAME_PERIOD).await;
                    let _ = this.update(cx, |hub, cx| hub.frame(cx));
                })
                .detach(),
        }
    }
}

impl Hub {
    fn frame(&mut self, cx: &mut Context<'_, Self>) {
        self.host.frame_scheduled = false;
        for frame in std::mem::take(&mut self.host.frames) {
            self.task(cx, |model, host| model.animation_frame(&frame, host));
        }
    }

    fn run_timeout(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(timeout) = self.host.timeouts.pop_front() {
            self.task(cx, |model, host| model.run_timeout(timeout, host));
        }
    }

    fn fire_interval(&mut self, interval: &HubInterval, cx: &mut Context<'_, Self>) {
        self.task(cx, |model, host| model.fire_interval(interval, host));
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
        let Some(slot) = self.host.workers.iter().find(|w| w.id == id) else {
            return;
        };
        let source = slot.source.clone();
        let error = match slot.thread.try_recv() {
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

    fn export_message(&mut self, id: u64, message: ExportMessage, cx: &mut Context<'_, Self>) {
        match message {
            ExportMessage::Reply(reply) => {
                let done = !matches!(reply, ExportReply::Progress(_));
                self.task(cx, |model, host| model.export_reply(id, reply, host));
                if done {
                    self.host.exports.retain(|(i, _)| *i != id);
                }
            }
            ExportMessage::Unmodeled(what) => cx.emit(HubEvent::Exception {
                task: "worker",
                error: format!("unmodeled: {what}"),
            }),
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

fn run_export(log: SerializedLog, options: &JsValue) -> Vec<ExportMessage> {
    let mut realm = JsRealm::new(Box::new(V8MathRandom::from_entropy()));
    let output = match (
        Log::from_serialized(log, &mut realm),
        export_options(options),
    ) {
        (Ok(log), Some(options)) => export(&log, &options),
        (Err(_), _) => Err(ExportError::Unported("a log the export worker cannot read")),
        (_, None) => Err(ExportError::Unported("export options of another shape")),
    };
    let payload = |value| {
        vec![
            ExportMessage::Reply(ExportReply::Progress(1.0)),
            ExportMessage::Reply(ExportReply::Payload(value)),
        ]
    };
    match output {
        Ok(Output::Text(text)) => payload(JsValue::String(text)),
        Ok(Output::Bytes(bytes)) => payload(JsValue::Bytes(bytes)),
        Err(ExportError::Rejected) => vec![ExportMessage::Reply(ExportReply::Failed)],
        Err(ExportError::Unported(what)) => vec![ExportMessage::Unmodeled(what)],
    }
}

impl HubHost for GpuiHost<'_, '_> {
    fn now(&self) -> f64 {
        self.state.clock.now()
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.emit(HubEvent::Main(message));
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
        self.state.timeouts.push_back(timeout);
        self.cx
            .spawn(async move |this, cx| {
                let _ = this.update(cx, |hub, cx| hub.run_timeout(cx));
            })
            .detach();
    }

    fn set_interval(&mut self, interval: HubInterval, period_ms: f64) {
        let period = interval_period(period_ms);
        let key = interval.clone();
        let task = self.cx.spawn(async move |this, cx| {
            let executor = cx.background_executor().clone();
            let mut next = executor.now() + period;
            loop {
                executor
                    .timer(next.saturating_duration_since(executor.now()))
                    .await;
                next = next_fire(next, executor.now(), period);
                if this
                    .update(cx, |hub, cx| hub.fire_interval(&key, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.state.intervals.insert(interval, task);
    }

    fn clear_interval(&mut self, interval: &HubInterval) {
        self.state.intervals.remove(interval);
    }

    fn request_animation_frame(&mut self, frame: HubFrame) {
        self.state.frames.push(frame);
        if !self.state.frame_scheduled {
            self.state.frame_scheduled = true;
            self.schedule_frame();
        }
    }

    fn worker_realm(&mut self) -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>) {
        (self.state.worker_realm)()
    }

    fn spawn_worker(&mut self, source: &str, _name: WorkerName, worker: Box<dyn DecodeWorker>) {
        let (ready, wake) = async_channel::unbounded::<()>();
        let thread = match WorkerThread::spawn_notifying(source, worker, move || {
            let _ = ready.try_send(());
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
            _pump: pump,
        });
    }

    fn post_worker(&mut self, source: &str, request: Request) {
        if let Some(slot) = self.state.workers.iter().rfind(|w| w.source == source) {
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
        let spawned = std::thread::Builder::new()
            .name(format!("export-{id}"))
            .spawn(move || {
                for message in run_export(log, &options) {
                    let _ = send.send_blocking(message);
                }
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
                    .update(cx, |hub, cx| hub.export_message(id, message, cx))
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

    fn start_live_source(&mut self, kind: LiveSourceKind, _address: &JsString) {
        self.unmodeled("live", &format!("a {kind:?} live source"));
    }

    fn stop_live_source(&mut self) {}

    fn live_data(&mut self, _data: &JsValue) {}

    fn start_publisher(&mut self, _is_sim: &JsValue) {
        self.unmodeled("live", "the NT4 publisher");
    }

    fn stop_publisher(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intervals_stay_on_their_period_grid() {
        let period = interval_period(250.0);
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        assert_eq!(next_fire(ms(250), ms(250), period), ms(500));
        assert_eq!(next_fire(ms(250), ms(280), period), ms(500));
        assert_eq!(next_fire(ms(250), ms(790), period), ms(1000));
        assert_eq!(interval_period(1000.0 / 60.0), Duration::from_millis(16));
    }
}
