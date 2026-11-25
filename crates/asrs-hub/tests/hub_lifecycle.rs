#![allow(clippy::disallowed_methods)]

//! Replays `parity/specs/hub/lifecycle-*.entry.json` against `asrs_hub::hub::HubModel` and
//! compares every record with the committed `hub/lifecycle-*` vector, which
//! `oracle/src/hubSession.ts` records from upstream's real hub.ts, Tabs, Sidebar,
//! HistoricalDataSource and worker bundles in jsdom. The host here schedules work as that page
//! does (oracle/src/dom/env.ts timers and frames, workers drained oldest first after every
//! task, main's `historical-start` reply as a zero-delay timeout); decode workers are the real
//! `asrs_formats` workers on their own threads and the export worker is `asrs_formats::export`.
//!
//! A fixture with `live` also runs the live sources and the publisher against that session's
//! mock server, over the in-memory network of asrs-live's session tests, scheduled as
//! `oracle/src/live/pageLoop.ts` does, and main's RLOG socket through `RlogMain`.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

#[path = "../../asrs-live/tests/support/mod.rs"]
#[allow(dead_code, clippy::all)]
mod live_support;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use asrs_formats::export::{ExportError, ExportOptions, Format, Output, Sampling, export};
use asrs_formats::historical::{MainReply, MainRequest, WorkerName};
use asrs_formats::worker::{Abort, DecodeWorker, Request, WorkerMessage, WorkerThread};
use asrs_hub::hub::{
    ExportReply, HubFrame, HubHost, HubInterval, HubModel, HubPage, HubTimeout, HubView,
};
use asrs_hub::live::{LiveNet, LiveRole};
use asrs_hub::sidebar::{NoLayout, SidebarGeometry};
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{Clock, Rng, SetClock, V8MathRandom};
use asrs_js::value::{JsValue, ToJs};
use asrs_live::rlog::{Ipc, MainCall, MainContext, RlogMain};
use asrs_live::{ManualClock, TimerQueue, TunedValue};
use asrs_log::log::{Log, LogError, SerializedLog};
use asrs_log::realm::JsRealm;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_model::selection::Selection;
use asrs_model::tab_controller::{TabCommand, TabRenderer};
use asrs_model::tab_type::TabType;
use asrs_parity::cve1::Value;
use asrs_parity::json::{self, Json};
use asrs_parity::live::{ServerHost, Session};
use asrs_parity::vectors::{self, Record};
use live_support::{Delivery, FakeNet, SessionEvent, Trace};

const START_TIME: f64 = 1767225600000.0;
const FRAME_PERIOD: f64 = 16.0;
const WORKER_FROZEN_MS: f64 = 1_000_000_000_000.0;
const IGNORED_MAIN: [&str; 3] = ["save-state", "save-type-memory", "update-touch-bar-slider"];

fn err(message: impl Into<String>) -> String {
    message.into()
}

fn js(s: &str) -> JsString {
    JsString::from(s)
}

fn cve(value: &JsValue) -> Value {
    js_json::from_js(value)
}

fn cve_str(s: &str) -> Value {
    cve(&JsValue::String(js(s)))
}

/// `Math.random` as oracle/src/dom/env.ts seeds it.
struct DomRng(u32);

impl Rng for DomRng {
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        (t ^ (t >> 14)) as f64 / 4294967296.0
    }
}

struct PageClock(Rc<Cell<f64>>);

impl Clock for PageClock {
    fn now(&self) -> f64 {
        self.0.get()
    }
}

/// jsdom with oracle/src/dom/layout.json: no rule reaches sidebar rows, so each is 0x0.
struct View {
    sidebar: NoLayout,
}

impl HubView for View {
    fn inner_height(&self) -> f64 {
        650.0
    }

    fn renderer_top(&self, _show_timeline: bool) -> f64 {
        92.0
    }

    fn dark(&self) -> bool {
        false
    }

    fn sidebar_geometry(&mut self) -> &mut dyn SidebarGeometry {
        &mut self.sidebar
    }

    fn source_list_dom(&mut self) -> &mut dyn asrs_model::source_list::SourceListDom {
        asrs_model::source_list::Undisplayed::dom()
    }

    fn tab_renderer(
        &self,
        kind: TabType,
        preferences: Option<&Preferences>,
        _is_battery: bool,
    ) -> Option<Box<dyn TabRenderer<HubPage>>> {
        (kind == TabType::Field3d).then(|| {
            Box::new(Field3dStub {
                implementation: preferences.is_some(),
                state: JsValue::Null,
            }) as Box<dyn TabRenderer<HubPage>>
        })
    }
}

/// `Field3dRenderer` over oracle/src/dom/field3dStub.ts, which saves what it restored.
struct Field3dStub {
    implementation: bool,
    state: JsValue,
}

impl TabRenderer<HubPage> for Field3dStub {
    fn save_state(&self, _: &HubPage) -> JsValue {
        match self.implementation {
            true => self.state.clone(),
            false => JsValue::Null,
        }
    }

    fn restore_state(&mut self, _: &mut HubPage, state: &JsValue) -> Result<(), JsError> {
        if !state.is_null() {
            self.state = state.clone();
        }
        Ok(())
    }

    fn aspect_ratio(&self, _: &HubPage) -> Option<f64> {
        None
    }

    fn render(&mut self, page: &mut HubPage, _: &dyn TabCommand) -> Result<(), JsError> {
        self.implementation |= page.preferences().is_some();
        Ok(())
    }
}

/// Records each value the loading glow's style takes.
const WATCH_GLOW: &str = "(() => { const read = () => [document.documentElement.style.getPropertyValue('--show-loading-glow'), document.documentElement.style.getPropertyValue('--loading-glow-progress')]; window.__glow = []; let last = JSON.stringify(read()); new MutationObserver(() => { const now = read(); if (JSON.stringify(now) !== last) { last = JSON.stringify(now); window.__glow.push(now); } }).observe(document.documentElement, { attributes: true, attributeFilter: ['style'] }); return read(); })()";

/// Records each text the title takes.
const WATCH_TITLE: &str = "(() => { const t = document.getElementsByTagName('title')[0]; window.__titles = []; let last = t.textContent; new MutationObserver(() => { if (t.textContent !== last) { last = t.textContent; window.__titles.push(last); } }).observe(t, { childList: true, characterData: true, subtree: true }); return last; })()";

enum Emit {
    Record(Vec<(&'static str, Value)>),
    /// A line of the network's trace.
    Trace(String),
}

enum Task {
    Live,
    Interval(HubInterval, f64),
    Timeout(HubTimeout),
    Main(MainReply),
}

struct Timer {
    due: f64,
    seq: u64,
    task: Task,
}

enum Work {
    Decode {
        source: String,
        thread: WorkerThread,
        pending: usize,
    },
    Export {
        id: u64,
        replies: Vec<ExportReply>,
    },
}

struct Slot {
    work: Work,
    terminated: bool,
    hung: bool,
}

struct Host {
    clock: Rc<Cell<f64>>,
    base: PathBuf,
    timers: Vec<Timer>,
    seq: u64,
    frames: Vec<HubFrame>,
    last_frame: f64,
    slots: Vec<Slot>,
    emitted: Vec<Emit>,
    unmodeled: Vec<String>,
    ignored: Vec<String>,
    live: Option<Live>,
}

/// The fixture's `live`: the network and main's side of RLOG.
struct Live {
    clock: ManualClock,
    trace: Trace,
    net: Rc<RefCell<FakeNet>>,
    rlog: RlogMain,
    rlog_timers: TimerQueue,
    armed: Option<f64>,
}

impl Live {
    fn new(session: &Session) -> Result<Self, String> {
        let clock = ManualClock::new(0.0);
        let trace = Trace::default();
        let host = ServerHost::new(session)?;
        let net = FakeNet::new(host, clock.clone(), session.latency_ms, trace.clone());
        let mut rlog_timers = TimerQueue::new(Rc::new(clock.clone()));
        Ok(Live {
            clock,
            trace,
            net: Rc::new(RefCell::new(net)),
            rlog: RlogMain::new(&mut rlog_timers),
            rlog_timers,
            armed: None,
        })
    }

    fn rlog<R>(&mut self, f: impl FnOnce(&mut RlogMain, &mut MainContext<'_>) -> R) -> R {
        let net = &mut *self.net.borrow_mut();
        let result = f(
            &mut self.rlog,
            &mut MainContext {
                timers: &mut self.rlog_timers,
                net,
            },
        );
        while let Some(message) = self.rlog.outbox.pop_front() {
            net.deliver(0.0, Delivery::Ipc(Ipc::ToSource(message.into_value())));
        }
        result
    }
}

impl Host {
    fn emit(&mut self, values: Vec<(&'static str, Value)>) {
        self.drain_trace();
        self.emitted.push(Emit::Record(values));
    }

    fn drain_trace(&mut self) {
        if let Some(live) = &self.live {
            self.emitted
                .extend(live.trace.take().into_iter().map(Emit::Trace));
        }
    }

    fn set_clock(&mut self, now: f64) {
        self.clock.set(now);
        if let Some(live) = &self.live {
            live.clock.set(now - START_TIME);
        }
    }

    fn add_timer(&mut self, delay: f64, task: Task) {
        let due = self.clock.get() + delay;
        let seq = self.seq;
        self.seq += 1;
        self.timers.push(Timer { due, seq, task });
    }

    fn decode_slot(&mut self, source: &str) -> Option<usize> {
        self.slots
            .iter()
            .rposition(|s| matches!(&s.work, Work::Decode { source: owner, .. } if owner == source))
    }
}

fn number(n: f64) -> Value {
    Value::number(n)
}

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

impl HubHost for Host {
    /// env.ts's `Date` takes the fractional clock, and a `Date` keeps whole milliseconds.
    fn now(&self) -> f64 {
        self.clock.get().trunc()
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        let name = message.name.name();
        if !self.ignored.iter().any(|i| i == name) {
            self.emit(vec![("main", cve_str(name)), ("data", cve(&message.data))]);
        }
        if let Some(call) = MainCall::parse(name, &message.data) {
            match &self.live {
                Some(live) => {
                    let ipc = Delivery::Ipc(Ipc::ToMain(call));
                    live.net.borrow_mut().deliver(0.0, ipc);
                }
                None => self.unmodeled.push(name.to_owned()),
            }
        }
    }

    fn send_historical_request(&mut self, request: MainRequest) {
        match request {
            MainRequest::HistoricalStart { uuid, path } => {
                self.emit(vec![
                    ("main", cve_str("historical-start")),
                    (
                        "data",
                        Value::object([
                            ("uuid", cve_str(&uuid)),
                            ("path", cve(&JsValue::String(path.clone()))),
                        ]),
                    ),
                ]);
                let target = self.base.join(path.to_string_lossy());
                let files = vec![std::fs::read(target).ok().map(Arc::from)];
                self.add_timer(
                    0.0,
                    Task::Main(MainReply {
                        uuid,
                        files,
                        error: None,
                    }),
                );
            }
            MainRequest::HistoricalStartRaw { .. } => {
                self.unmodeled.push("historical-start-raw".to_owned());
            }
        }
    }

    fn set_timeout(&mut self, timeout: HubTimeout) {
        self.add_timer(0.0, Task::Timeout(timeout));
    }

    fn set_interval(&mut self, interval: HubInterval, period_ms: f64) {
        // pageLoop.ts's timers follow Blink's rules, unlike env.ts's.
        let period_ms = match &self.live {
            Some(_) => period_ms.trunc(),
            None => period_ms,
        };
        let period = period_ms.max(1.0);
        self.add_timer(period, Task::Interval(interval, period));
    }

    fn clear_interval(&mut self, interval: &HubInterval) {
        self.timers
            .retain(|t| !matches!(&t.task, Task::Interval(i, _) if i == interval));
    }

    fn request_animation_frame(&mut self, frame: HubFrame) {
        self.frames.push(frame);
    }

    fn worker_realm(&mut self) -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>) {
        (
            Box::new(V8MathRandom::from_entropy()),
            Box::new(SetClock(WORKER_FROZEN_MS)),
        )
    }

    fn spawn_worker(&mut self, source: &str, name: WorkerName, worker: Box<dyn DecodeWorker>) {
        let thread = WorkerThread::spawn(source, worker).expect("spawn a decode thread");
        self.emit(vec![
            ("worker", number(self.slots.len() as f64)),
            ("script", cve_str(name.script())),
        ]);
        self.slots.push(Slot {
            work: Work::Decode {
                source: source.to_owned(),
                thread,
                pending: 0,
            },
            terminated: false,
            hung: false,
        });
    }

    fn post_worker(&mut self, source: &str, request: Request) {
        let Some(index) = self.decode_slot(source) else {
            return;
        };
        let record = match &request {
            Request::Start(files) => vec![
                ("post", number(index as f64)),
                ("type", cve_str("start")),
                (
                    "files",
                    Value::Array(
                        files
                            .iter()
                            .map(|f| f.as_ref().map_or(Value::Null, |f| number(f.len() as f64)))
                            .collect(),
                    ),
                ),
            ],
            Request::ParseField(key) => vec![
                ("post", number(index as f64)),
                ("type", cve_str("parseField")),
                ("key", cve(&JsValue::String(key.clone()))),
            ],
        };
        self.emit(record);
        let slot = &mut self.slots[index];
        if let Work::Decode {
            thread, pending, ..
        } = &mut slot.work
            && !slot.terminated
            && !slot.hung
        {
            *pending += 1;
            thread.post(request);
        }
    }

    fn terminate_worker(&mut self, source: &str) {
        let Some(index) = self.decode_slot(source) else {
            return;
        };
        let slot = &mut self.slots[index];
        slot.terminated = true;
        if let Work::Decode { thread, .. } = &mut slot.work {
            thread.terminate();
        }
        self.emit(vec![("terminate", number(index as f64))]);
    }

    fn request_export(&mut self, id: u64, log: SerializedLog, options: JsValue) {
        let index = self.slots.len();
        self.emit(vec![
            ("worker", number(index as f64)),
            ("script", cve_str("exportWorker.js")),
        ]);
        self.emit(vec![
            ("post", number(index as f64)),
            ("type", cve_str("request")),
        ]);
        let mut realm = JsRealm::new(Box::new(V8MathRandom::from_entropy()));
        let mut replies = Vec::new();
        let output = match (
            Log::from_serialized(log, &mut realm),
            export_options(&options),
        ) {
            (Ok(log), Some(options)) => export(&log, &options, &mut |value| {
                replies.push(ExportReply::Progress(value))
            }),
            (Err(LogError::Proto(_) | LogError::Thrown(_) | LogError::StackOverflow), _) => {
                Err(ExportError::Rejected)
            }
            (Err(LogError::NeverCompletes), _) => Err(ExportError::Unported(
                "a log the export worker never finishes reading",
            )),
            (Err(LogError::Unmodeled(what)), _) => Err(ExportError::Unported(what)),
            (_, None) => Err(ExportError::Unported("export options of another shape")),
        };
        match output {
            Ok(Output::Text(text)) => replies.push(ExportReply::Payload(JsValue::String(text))),
            Ok(Output::Bytes(bytes)) => replies.push(ExportReply::Payload(JsValue::Bytes(bytes))),
            Err(ExportError::Rejected) => replies.push(ExportReply::Failed),
            Err(ExportError::Unported(what)) => self.unmodeled.push(what.to_owned()),
        }

        self.slots.push(Slot {
            work: Work::Export { id, replies },
            terminated: false,
            hung: false,
        });
    }

    fn console(&mut self, warn: bool, text: String) {
        if warn {
            self.emit(vec![("console", cve_str("warn")), ("text", cve_str(&text))]);
        }
    }

    fn exception(&mut self, task: &'static str, error: String) {
        // hubSession.ts records a rejection as `${e.name}: ${e.message}`, env.ts a thrown string
        // as itself.
        let error = match error.strip_prefix(": ") {
            Some(_) if task == "unhandledRejection" => "undefined: undefined".to_owned(),
            Some(text) => text.to_owned(),
            None => error,
        };
        self.emit(vec![
            ("exception", cve_str(task)),
            ("error", cve_str(&error)),
        ]);
    }

    fn live_net(&mut self, _role: LiveRole) -> Result<LiveNet, String> {
        match &self.live {
            Some(live) => Ok(live.net.clone()),
            None => {
                self.unmodeled.push("a live connection".to_owned());
                Err("no network".to_owned())
            }
        }
    }

    fn timer_clock(&self) -> Rc<dyn Clock> {
        match &self.live {
            Some(live) => Rc::new(live.clock.clone()),
            None => Rc::new(ManualClock::new(0.0)),
        }
    }

    fn idle_period(&self) -> Option<f64> {
        None
    }
}

struct Runner {
    host: Host,
    hub: HubModel,
    glow: Option<Glow>,
    titles: Option<Titles>,
}

struct Titles {
    last: JsString,
    seen: Vec<JsString>,
}

/// The loading glow's style as the watching expression sees it: its value after the last task
/// and the values it changed to since the last drain.
struct Glow {
    last: [String; 2],
    seen: Vec<[String; 2]>,
}

fn glow_value(pair: &[String; 2]) -> Value {
    Value::Array(pair.iter().map(|s| cve_str(s)).collect())
}

impl Runner {
    fn read_glow(&self) -> [String; 2] {
        let loading = self.hub.loading();
        let show = match loading.show {
            None => "",
            Some(true) => "1",
            Some(false) => "0",
        };
        let progress = loading.progress.clone().unwrap_or_default();
        [show.to_owned(), progress.to_string_lossy()]
    }

    /// The end of a task, where a `MutationObserver` runs.
    fn observe(&mut self) {
        let now = self.read_glow();
        if let Some(glow) = &mut self.glow
            && now != glow.last
        {
            glow.seen.push(now.clone());
            glow.last = now;
        }
        let title = self.hub.title();
        if let Some(titles) = &mut self.titles
            && *title != titles.last
        {
            titles.seen.push(title.clone());
            titles.last = title.clone();
        }
    }

    /// pageLoop.ts `taskEnded`: the page timer is armed for what is due next.
    fn arm_live(&mut self) {
        let Some(live) = &mut self.host.live else {
            return;
        };
        let due = {
            let net = live.net.borrow();
            [
                net.host.next_time(),
                net.next_task_due(),
                live.rlog_timers.next_due(),
                self.hub.next_live_timer(),
            ]
        };
        let due = due.into_iter().flatten().reduce(f64::min);
        if due == live.armed {
            return;
        }
        live.armed = due;
        self.host.timers.retain(|t| !matches!(t.task, Task::Live));
        if let Some(due) = due {
            let due = START_TIME + due;
            let seq = self.host.seq;
            self.host.seq += 1;
            self.host.timers.push(Timer {
                due,
                seq,
                task: Task::Live,
            });
        }
    }

    /// pageLoop.ts `pump`: the server, then one delivery or timer.
    fn run_live(&mut self) -> Result<(), String> {
        let live = self.host.live.as_mut().expect("armed by a live fixture");
        live.armed = None;
        let now = live.clock.now();
        let net = live.net.clone();
        let server_due = net.borrow().host.next_time();
        if let Some(due) = server_due.filter(|due| *due <= now) {
            net.borrow_mut().server_events(due)?;
        }
        let delivery = net.borrow_mut().take_task(now);
        if let Some(delivery) = delivery {
            live.rlog_timers.begin_task();
            let events = net.borrow_mut().apply(delivery);
            for event in events {
                self.live_event(event);
            }
        } else {
            let rlog_due = live.rlog_timers.next_due().unwrap_or(f64::INFINITY);
            let page_due = self.hub.next_live_timer().unwrap_or(f64::INFINITY);
            if rlog_due <= now && rlog_due <= page_due {
                let id = live.rlog_timers.fire_next().expect("due");
                live.rlog(|main, cx| main.handle_timer(id, cx));
            } else {
                self.hub.fire_live_timer(&mut self.host);
            }
        }
        loop {
            let event = net.borrow_mut().microtasks.pop_front();
            let Some(event) = event else {
                return Ok(());
            };
            self.live_event(SessionEvent::Net(event));
        }
    }

    fn live_event(&mut self, event: SessionEvent) {
        let live = self.host.live.as_mut().expect("a live fixture");
        match event {
            SessionEvent::Net(event) => {
                let source = LiveRole::Source {
                    whole_bodies: false,
                };
                self.hub.live_event(source, &event, &mut self.host);
                self.hub
                    .live_event(LiveRole::Publisher, &event, &mut self.host);
            }
            SessionEvent::Tcp(event) => {
                live.rlog(|main, cx| main.handle_net_event(&event, cx));
            }
            SessionEvent::Ipc(Ipc::ToMain(call)) => {
                live.rlog(|main, cx| main.handle(&call, cx))
                    .expect("a port Node takes");
            }
            SessionEvent::Ipc(Ipc::ToSource(data)) => {
                let message = NamedMessage::new(MainToHub::LiveData, data);
                self.hub.handle_main_message(&message, &mut self.host);
            }
        }
    }

    /// The page between tasks: each worker message in turn, from the oldest worker that has
    /// one or is still busy.
    fn settle(&mut self) -> Result<(), String> {
        loop {
            let Some(index) = self.host.slots.iter().position(|s| {
                !s.terminated
                    && !s.hung
                    && match &s.work {
                        Work::Decode { pending, .. } => *pending > 0,
                        Work::Export { replies, .. } => !replies.is_empty(),
                    }
            }) else {
                self.host.drain_trace();
                self.arm_live();
                return Ok(());
            };
            let slot = &mut self.host.slots[index];
            match &mut slot.work {
                Work::Decode {
                    source,
                    thread,
                    pending,
                } => match thread.recv() {
                    Some(WorkerMessage::Response(response)) => {
                        let source = source.clone();
                        self.hub.worker_message(&source, response, &mut self.host);
                        self.observe();
                    }
                    Some(WorkerMessage::Done(end)) => {
                        *pending -= 1;
                        match end {
                            Ok(()) | Err(Abort::Threw) => {}
                            Err(Abort::NeverReturns) => slot.hung = true,
                            Err(Abort::Unmodeled(what)) => return Err(format!("worker: {what}")),
                        }
                    }
                    None => slot.hung = true,
                },
                Work::Export { id, replies } => {
                    let id = *id;
                    let reply = replies.remove(0);
                    if !matches!(reply, ExportReply::Progress(_)) {
                        replies.clear();
                        slot.terminated = true;
                        self.host.emit(vec![("terminate", number(index as f64))]);
                    }
                    self.hub.export_reply(id, reply, &mut self.host);
                    self.observe();
                }
            }
        }
    }

    fn run_timer(&mut self, timer: Timer) -> Result<(), String> {
        match timer.task {
            Task::Live => self.run_live()?,
            Task::Interval(interval, period) => {
                self.host.timers.push(Timer {
                    due: timer.due + period,
                    seq: self.host.seq,
                    task: Task::Interval(interval.clone(), period),
                });
                self.host.seq += 1;
                self.hub.fire_interval(&interval, &mut self.host);
            }
            Task::Timeout(timeout) => self.hub.run_timeout(timeout, &mut self.host),
            Task::Main(reply) => self.hub.historical_data(&reply, &mut self.host),
        }
        self.observe();
        self.settle()
    }

    /// oracle/src/dom/env.ts `advance`: timers and frames in time order, timers first on a tie.
    fn advance(&mut self, ms: f64) -> Result<(), String> {
        let end = self.host.clock.get() + ms;
        loop {
            let next = self
                .host
                .timers
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.due.total_cmp(&b.due).then(a.seq.cmp(&b.seq)))
                .map(|(i, t)| (i, t.due));
            let frame_at = self.host.last_frame + FRAME_PERIOD;
            match next {
                Some((i, due)) if due <= end && due <= frame_at => {
                    self.host.set_clock(self.host.clock.get().max(due));
                    let timer = self.host.timers.remove(i);
                    self.run_timer(timer)?;
                }
                _ if frame_at <= end => {
                    self.host.set_clock(frame_at);
                    self.host.last_frame = frame_at;
                    for frame in std::mem::take(&mut self.host.frames) {
                        self.hub.animation_frame(&frame, &mut self.host);
                    }
                    self.observe();
                    self.settle()?;
                }
                _ => {
                    self.host.set_clock(end);
                    return Ok(());
                }
            }
        }
    }

    fn main(&mut self, name: &str, data: JsValue) -> Result<(), String> {
        let name =
            MainToHub::from_name(name).ok_or_else(|| err(format!("unknown message {name}")))?;
        self.hub
            .handle_main_message(&NamedMessage::new(name, data), &mut self.host);
        self.observe();
        self.settle()
    }

    fn eval(&mut self, expr: &str) -> Result<Value, String> {
        let flag = |v: Option<bool>| match v {
            None => "",
            Some(true) => "1",
            Some(false) => "0",
        };
        let strings = |items: Vec<JsString>| {
            Value::Array(
                items
                    .into_iter()
                    .map(|k| cve(&JsValue::String(k)))
                    .collect(),
            )
        };
        Ok(match expr {
            "document.getElementsByTagName('title')[0].textContent" => {
                cve(&JsValue::String(self.hub.title().clone()))
            }
            "[document.documentElement.style.getPropertyValue('--show-loading-glow'), document.documentElement.style.getPropertyValue('--loading-glow-progress')]" =>
            {
                let loading = self.hub.loading();
                let progress = loading.progress.clone().unwrap_or_default();
                Value::Array(vec![
                    cve_str(flag(loading.show)),
                    cve(&JsValue::String(progress)),
                ])
            }
            "[document.documentElement.style.getPropertyValue('--side-bar-width'), document.documentElement.style.getPropertyValue('--show-side-bar')]" =>
            {
                let sidebar = self.hub.sidebar();
                let width = asrs_js::number::to_string(sidebar.applied_width());
                Value::Array(vec![
                    cve_str(&format!("{width}px")),
                    cve_str(flag(Some(sidebar.sidebar_shown()))),
                ])
            }
            WATCH_GLOW => {
                let last = self.read_glow();
                let value = glow_value(&last);
                self.glow = Some(Glow {
                    last,
                    seen: Vec::new(),
                });
                value
            }
            "window.__glow.splice(0)" => {
                let glow = self.glow.as_mut().ok_or_else(|| err("no watched glow"))?;
                Value::Array(glow.seen.drain(..).map(|pair| glow_value(&pair)).collect())
            }
            WATCH_TITLE => {
                let last = self.hub.title().clone();
                self.titles = Some(Titles {
                    last: last.clone(),
                    seen: Vec::new(),
                });
                cve(&JsValue::String(last))
            }
            "window.__titles.splice(0)" => {
                let titles = self
                    .titles
                    .as_mut()
                    .ok_or_else(|| err("no watched title"))?;
                strings(std::mem::take(&mut titles.seen))
            }
            "window.tuner !== null && window.tuner.hasTunableFields()" => {
                Value::Bool(self.hub.with_sidebar(|_, env| env.has_tunable_fields()))
            }
            "window.log.getFieldKeys()" => strings(self.hub.page().log().get_field_keys()),
            "window.log.getTimestampRange()" => {
                let [a, b] = self.hub.page().log().get_timestamp_range();
                Value::Array(vec![number(a), number(b)])
            }
            "[...window.getLoadingFields()].sort()" => {
                let mut loading: Vec<JsString> = self.hub.loading_fields().into_iter().collect();
                loading.sort_by(|a, b| a.units().cmp(b.units()));
                strings(loading)
            }
            "[...document.body.classList].filter((c) => c.startsWith('fancy-'))" => {
                Value::Array(self.hub.body_classes().iter().map(|c| cve_str(c)).collect())
            }
            "[document.documentElement.style.getPropertyValue('--show-update-button'), document.documentElement.style.getPropertyValue('--show-feedback-button')]" => {
                Value::Array(vec![
                    cve_str(flag(self.hub.update_button())),
                    cve_str(flag(self.hub.feedback_button())),
                ])
            }
            other => return Err(format!("eval {other} is not modeled")),
        })
    }

    /// `window.tuner.publish(key, value)` and `window.tuner.unpublish(key)` with JSON arguments,
    /// each in its own task.
    fn tune(&mut self, expr: &str) -> Result<(), String> {
        let call = expr
            .strip_prefix("window.tuner.")
            .and_then(|rest| rest.strip_suffix(')'))
            .and_then(|rest| rest.split_once('('));
        let Some((method, args)) = call else {
            return Err(format!("eval {expr} is not modeled"));
        };
        let args = json::parse(&format!("[{args}]")).map_err(|e| e.to_string())?;
        let args = args.as_array().ok_or_else(|| err("no arguments"))?;
        let key = to_js(&args[0])?.into_string();
        let key = key.ok_or_else(|| err("a tuned key is a string"))?;
        let thrown = match (method, args.get(1).map(to_js).transpose()?) {
            ("publish", Some(JsValue::Number(n))) => {
                let value = TunedValue::Number(n);
                self.hub.with_sidebar(|_, env| env.publish(&key, value))
            }
            ("publish", Some(JsValue::Bool(b))) => {
                let value = TunedValue::Boolean(b);
                self.hub.with_sidebar(|_, env| env.publish(&key, value))
            }
            ("unpublish", None) => self.hub.with_sidebar(|_, env| env.unpublish(&key)),
            _ => return Err(format!("eval {expr} is not modeled")),
        };
        if let Err(thrown) = thrown {
            self.host.emit(vec![
                ("exception", cve_str("eval")),
                ("error", cve(&JsValue::String(thrown.0))),
            ]);
        }
        self.observe();
        self.settle()
    }

    fn capture(&mut self, kind: &Json) -> Result<(), String> {
        let name = kind.as_str().map(|s| s.to_string_lossy());
        let record = match name.as_deref() {
            Some("selection") => vec![(
                "selection",
                selection_value(self.hub.page_mut().selection_mut()),
            )],
            Some("state") => vec![("state", cve(&self.hub.save_state()))],
            Some(other) => return Err(format!("capture kind {other} is not modeled")),
            None => {
                let is_eval = kind
                    .get("kind")
                    .and_then(Json::as_str)
                    .is_some_and(|k| k.to_string_lossy() == "eval");
                let expr = kind.get("expr").and_then(Json::as_str);
                match expr {
                    Some(expr) if is_eval => {
                        let expr = expr.to_string_lossy();
                        let value = self.eval(&expr)?;
                        vec![("eval", cve_str(&expr)), ("value", value)]
                    }
                    _ => return Err(format!("capture {kind:?} is not modeled")),
                }
            }
        };
        self.host.emit(record);
        Ok(())
    }
}

fn selection_value(sel: &mut asrs_model::selection_impl::SelectionImpl) -> Value {
    let nullable = |t: Option<f64>| t.map_or(Value::Null, Value::number);
    let pair = |p: Option<[f64; 2]>| {
        p.map_or(Value::Null, |[a, b]| {
            Value::Array(vec![number(a), number(b)])
        })
    };
    let controls = sel.controls();
    let range = sel.get_timeline_range();
    Value::object([
        ("mode", number(sel.get_mode().number())),
        ("hoveredTime", nullable(sel.get_hovered_time())),
        ("selectedTime", nullable(sel.get_selected_time())),
        ("renderTime", nullable(sel.get_render_time())),
        ("currentLiveTime", nullable(sel.get_current_live_time())),
        ("grabZoomRange", pair(sel.get_grab_zoom_range())),
        (
            "timelineRange",
            Value::Array(vec![number(range[0]), number(range[1])]),
        ),
        (
            "timelineIsMaxZoom",
            Value::Bool(sel.get_timeline_is_max_zoom()),
        ),
        (
            "buttons",
            Value::object([
                ("play", Value::Bool(controls.play_hidden)),
                ("pause", Value::Bool(controls.pause_hidden)),
                ("lock", Value::Bool(controls.lock_hidden)),
                ("unlock", Value::Bool(controls.unlock_hidden)),
                (
                    "showLockButtons",
                    cve_str(match controls.show_lock_buttons {
                        None => "",
                        Some(true) => "1",
                        Some(false) => "0",
                    }),
                ),
            ]),
        ),
    ])
}

fn to_js(json: &Json) -> Result<JsValue, String> {
    js_json::to_js(&Value::from_json_lenient(json))
}

fn replay(spec: &Path) -> Result<(Vec<Emit>, Vec<String>), String> {
    let text = std::fs::read_to_string(spec).map_err(|e| e.to_string())?;
    let spec_json = json::parse(&text).map_err(|e| e.to_string())?;
    let fixture = spec_json
        .get("fixture")
        .ok_or_else(|| err("spec has no fixture"))?;
    for key in [
        "boot",
        "preferences",
        "assets",
        "version",
        "window",
        "startTime",
    ] {
        if fixture.get(key).is_some() {
            return Err(format!("fixture key {key} is not modeled"));
        }
    }
    let seed = fixture.get("seed").and_then(Json::as_f64).unwrap_or(1.0) as u32;
    let clock = Rc::new(Cell::new(START_TIME));
    let mut host = Host {
        clock: clock.clone(),
        base: spec.parent().unwrap().to_path_buf(),
        timers: Vec::new(),
        seq: 0,
        frames: Vec::new(),
        last_frame: START_TIME,
        slots: Vec::new(),
        emitted: Vec::new(),
        unmodeled: Vec::new(),
        ignored: match fixture.get("ignoreMain").and_then(Json::as_array) {
            Some(names) => names
                .iter()
                .map(|n| n.as_str().map(|s| s.to_string_lossy()))
                .collect::<Option<_>>()
                .ok_or_else(|| err("ignoreMain lists names"))?,
            None => IGNORED_MAIN.map(str::to_owned).into(),
        },
        live: None,
    };
    let mut rng = DomRng(seed);
    // One `Math.random()` happens while hub.ts's imports evaluate, before the hub draws any
    // UUID (the same draw dom_tabs.rs makes).
    rng.next_f64();
    let realm = JsRealm::new(Box::new(rng));
    let hub = HubModel::new(
        realm,
        Rc::new(PageClock(clock)),
        Box::new(View { sidebar: NoLayout }),
        &mut host,
    );
    let intervals = host.timers.iter().filter_map(|t| match &t.task {
        Task::Interval(interval, period) => Some((interval.clone(), *period)),
        _ => None,
    });
    assert_eq!(
        intervals.take(3).collect::<Vec<_>>(),
        [
            (HubInterval::SidebarTuning, 250.0),
            (HubInterval::SaveState, 250.0),
            (HubInterval::SaveTypeMemory, 1000.0),
        ],
        "the order of hub.ts's constructors"
    );
    let mut runner = Runner {
        host,
        hub,
        glow: None,
        titles: None,
    };

    let version = asrs_js::ops::object([
        ("platform", JsValue::string("darwin")),
        ("platformRelease", JsValue::string("25.0.0")),
        ("platformArch", JsValue::string("arm64")),
        (
            "appVersion",
            JsValue::string(asrs_model::build_constants::LITE_VERSION),
        ),
    ]);
    if let Some(live) = fixture.get("live") {
        let session = Session::parse(&live.to_compact())?;
        runner.host.live = Some(Live::new(&session)?);
    }
    let boot: Vec<(&str, JsValue)> = vec![
        ("set-assets", JsValue::Null),
        ("set-fullscreen", JsValue::Bool(false)),
        ("set-battery", JsValue::Bool(false)),
        ("set-version", version),
        ("show-update-button", JsValue::Bool(false)),
        ("show-feedback-button", JsValue::Bool(false)),
        ("show-when-ready", JsValue::Undefined),
        ("set-preferences", ToJs::to_js(&Preferences::default())),
        ("set-active-satellites", JsValue::Array(Vec::new())),
    ];
    for (name, data) in boot {
        runner.main(name, data)?;
    }

    let steps = fixture
        .get("steps")
        .and_then(Json::as_array)
        .ok_or_else(|| err("fixture has no steps"))?;
    for (index, step) in steps.iter().enumerate() {
        if let Some(name) = step.get("main").and_then(Json::as_str) {
            let data = match step.get("data") {
                Some(data) => to_js(data)?,
                None => JsValue::Undefined,
            };
            runner.main(&name.to_string_lossy(), data)?;
        } else if let Some(expr) = step.get("eval").and_then(Json::as_str) {
            runner.tune(&expr.to_string_lossy())?;
        } else if let Some(ms) = step.get("advance").and_then(Json::as_f64) {
            runner.advance(ms)?;
        } else if let Some(frames) = step.get("frames").and_then(Json::as_f64) {
            runner.advance(frames * FRAME_PERIOD)?;
        } else if let Some(kinds) = step.get("capture").and_then(Json::as_array) {
            let label = match step.get("label") {
                Some(label) => Value::from_json_lenient(label),
                None => number(index as f64),
            };
            let time = number(runner.host.clock.get() - START_TIME);
            runner.host.emit(vec![("capture", label), ("time", time)]);
            for kind in kinds {
                runner.capture(kind)?;
            }
        } else {
            return Err(format!("step {index} is not modeled"));
        }
    }
    runner.host.drain_trace();
    let unmodeled = std::mem::take(&mut runner.host.unmodeled);
    Ok((std::mem::take(&mut runner.host.emitted), unmodeled))
}

fn record_keys(record: &Record) -> Vec<String> {
    match &record.json {
        Json::Object(pairs) => pairs.iter().map(|(k, _)| k.to_string_lossy()).collect(),
        _ => Vec::new(),
    }
}

fn compare(spec: &Path, id: &str) -> Result<usize, String> {
    let (emitted, unmodeled) = replay(spec)?;
    if !unmodeled.is_empty() {
        return Err(format!(
            "the fixture reaches unmodeled host work: {unmodeled:?}"
        ));
    }
    let records = vectors::load(id)?;
    for (i, record) in records.iter().enumerate() {
        let emit = emitted.get(i).ok_or_else(|| {
            format!(
                "line {}: the port emitted nothing for {}",
                record.line, record.text
            )
        })?;
        let emit = match emit {
            Emit::Record(values) => values,
            Emit::Trace(line) if *line == record.text => continue,
            Emit::Trace(line) => {
                return Err(format!(
                    "line {}: upstream {} but the port's network saw {line}",
                    record.line, record.text
                ));
            }
        };
        let keys: Vec<String> = emit.iter().map(|(k, _)| (*k).to_owned()).collect();
        if keys != record_keys(record) {
            return Err(format!(
                "line {}: upstream {} but the port emitted keys {keys:?}",
                record.line, record.text
            ));
        }
        for (key, value) in emit {
            let expected = record.value(key).map_err(|e| e.0)?;
            if !vectors::values_match(&expected, value) {
                let report = asrs_parity::diff::report(&expected, value)
                    .unwrap_or_else(|| "encodings differ".to_owned());
                return Err(format!("line {} ({key}): {report}", record.line));
            }
        }
    }
    if emitted.len() > records.len() {
        return Err(format!(
            "the port emitted {} records past the vector's {}",
            emitted.len() - records.len(),
            records.len()
        ));
    }
    Ok(records.len())
}

#[test]
fn hub_lifecycle_sessions() {
    let dir = vectors::workspace_root().join("parity/specs/hub");
    let mut specs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("lifecycle-") && n.ends_with(".entry.json"))
        })
        .collect();
    specs.sort();
    assert!(!specs.is_empty(), "no hub lifecycle specs");
    let mut failures = Vec::new();
    let mut compared = 0;
    for spec in &specs {
        let name = spec
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .trim_end_matches(".entry.json");
        match compare(spec, &format!("hub/{name}")) {
            Ok(n) => compared += n,
            Err(e) => failures.push(format!("hub/{name}: {e}")),
        }
    }
    eprintln!(
        "hub lifecycle: {} sessions, {compared} records compared",
        specs.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
