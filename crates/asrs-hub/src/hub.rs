//! `src/hub/hub.ts` and `src/hub/WorkerManager.ts` as a headless model: the hub's `Log`,
//! selection, sidebar and tabs, the historical sources with their loading progress, the live
//! and publish orchestration, export, the window title and the main-message dispatch.
//!
//! Everything upstream reaches through browser APIs goes through [`HubHost`], passed into each
//! call: main messages, timers, animation frames, decode workers, the export worker and the
//! live transports. The host decides when each task runs and hands it back through the
//! matching method. What the page reads from its layout comes from [`HubView`].
//!
//! Lite, XR and the drag item are out of scope here; drags are `crate::drag`.

use std::cell::{Ref, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

use asrs_formats::historical::{
    HistoricalDataSource, MainReply, MainRequest, PageContext, SourceEvent, SourceHost, Status,
    Timeout, WorkerName,
};
use asrs_formats::worker::{Abort, DecodeWorker, Request, Response};
use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{Clock, Rng};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::log::log_key_utils::MERGE_PREFIX;
use asrs_log::log::robot_state::{AKIT_TIMESTAMP_KEYS, get_enabled_data};
use asrs_log::log::{Log, SerializedLog};
use asrs_log::realm::JsRealm;
use asrs_log::util::{calc_mock_progress, clamp_value, scale_value};
use asrs_model::assets::AdvantageScopeAssets;
use asrs_model::build_constants::{DISTRIBUTION, Distribution, IS_LITE};
use asrs_model::hub_state::SidebarState;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_model::selection::{Selection, TimeSupplier};
use asrs_model::selection_impl::{SelectionContext, SelectionImpl, SharedLog};
use asrs_model::source_list_config::{SourceListTypeMemory, SourceListTypeMemoryEntry};
use asrs_model::tab_type::TabType;
use asrs_tabs::cx::TabCx;
use asrs_tabs::field_2d::Field2dRenderer;
use asrs_tabs::line_graph::LineGraphController;
use asrs_tabs::list_cx::{ListParts, SourceListCx};
use asrs_tabs::statistics::StatisticsCx;
use asrs_tabs::video::{VideoController, VideoCx};

use crate::factory::HubTabFactory;
use crate::sidebar::{EventOutcome, RowRect, RowValue, Sidebar, SidebarEnv, Tuner};
use crate::tabs::{Tabs, TabsError, TabsHost};

pub const STATE_SAVE_PERIOD_MS: f64 = 250.0;
pub const TYPE_MEMORY_SAVE_PERIOD_MS: f64 = 1000.0;
/// `1000 / 60`, as `setInterval` receives it.
pub const MOCK_PROGRESS_PERIOD_MS: f64 = 1000.0 / 60.0;
pub const TOUCH_BAR_PERIOD_MS: f64 = 1000.0 / 60.0;
pub const SOURCE_INTERVAL_MS: f64 = 50.0;

const APP_NAME: &str = "AdvantageScope";

/// A repeating timer the hub sets. Each is cleared by the same value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum HubInterval {
    SaveState,
    SaveTypeMemory,
    TouchBarSlider,
    /// The 60 Hz mock progress of one `start-publish` or `prepare-export` handler.
    MockProgress(u64),
    /// A historical source's 50 ms field-request interval.
    Source(String),
}

/// A one-shot `setTimeout(.., 0)`.
pub enum HubTimeout {
    Source(Timeout),
}

/// A `requestAnimationFrame` callback. Each runs once; the hub requests the next one itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HubFrame {
    /// `Tabs`' per-frame loop.
    Tabs,
    /// `show-when-ready`.
    Show,
    /// A historical source's mock progress.
    Source(String),
}

/// `liveSource = new ...` in `startLive`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveSourceKind {
    Nt4,
    Nt4AdvantageKit,
    Nt4Systemcore,
    Nt4DriverStation,
    Phoenix,
    Rlog,
    FtcDashboard,
}

/// `LiveDataSourceStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveStatus {
    Waiting,
    Connecting,
    Active,
    Error,
    Stopped,
}

/// `NT4PublisherStatus`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublisherStatus {
    Connecting,
    Active,
    Stopped,
}

/// What a `WorkerManager.request` resolves or rejects with.
pub enum ExportReply {
    Progress(f64),
    Payload(JsValue),
    Failed,
}

/// The browser around the hub.
pub trait HubHost {
    /// `new Date().getTime()`.
    fn now(&self) -> f64;
    fn send_main_message(&mut self, message: NamedMessage<HubToMain>);
    /// `historical-start` and `historical-start-raw`, whose payloads hold bytes.
    fn send_historical_request(&mut self, request: MainRequest);
    fn set_timeout(&mut self, timeout: HubTimeout);
    /// `period_ms` is what upstream passes; Blink converts it to a whole `long` number of
    /// milliseconds (16 for `1000 / 60`) and the oracle's fake timers do not.
    fn set_interval(&mut self, interval: HubInterval, period_ms: f64);
    fn clear_interval(&mut self, interval: &HubInterval);
    fn request_animation_frame(&mut self, frame: HubFrame);
    fn worker_realm(&mut self) -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>);
    fn spawn_worker(&mut self, source: &str, name: WorkerName, worker: Box<dyn DecodeWorker>);
    fn post_worker(&mut self, source: &str, request: Request);
    fn terminate_worker(&mut self, source: &str);
    /// `WorkerManager.request("../bundles/exportWorker.js", { options, log })`; the reply
    /// comes back through [`HubModel::export_reply`] with the same id.
    fn request_export(&mut self, id: u64, log: SerializedLog, options: JsValue);
    /// `console.warn` and `console.log`.
    fn console(&mut self, warn: bool, text: String);
    /// A throw out of a task the page goes on from, as the browser reports it.
    fn exception(&mut self, task: &'static str, error: String);
    /// Starts `kind` connected to `address` in place of the stopped previous source. Its
    /// status and output come back through [`HubModel::live_status`] and
    /// [`HubModel::live_output`]; the host also sets `window.tuner` from it with
    /// [`HubModel::set_tuner`].
    fn start_live_source(&mut self, kind: LiveSourceKind, address: &JsString);
    fn stop_live_source(&mut self);
    fn live_data(&mut self, data: &JsValue);
    /// `new NT4Publisher(isSim, ...)`, replacing any running one; its status comes back
    /// through [`HubModel::publisher_status`].
    fn start_publisher(&mut self, is_sim: &JsValue);
    fn stop_publisher(&mut self);
}

/// What the page reads from its layout and environment.
pub trait HubView {
    fn inner_height(&self) -> f64;
    /// `RENDERER_CONTENT.getBoundingClientRect().top` while `--show-timeline` is `show_timeline`.
    fn renderer_top(&self, show_timeline: bool) -> f64;
    /// `prefers-color-scheme: dark`.
    fn dark(&self) -> bool;
    /// Whether `scrollIntoViewIfNeeded` exists.
    fn can_scroll_into_view(&self) -> bool;
    /// `.field-item` rects in display order.
    fn sidebar_rects(&self, sidebar: &Sidebar) -> Vec<RowRect>;
    /// `clientWidth` of a displayed `.field-value` showing `value`.
    fn measure_value(&self, value: &RowValue) -> f64;
}

/// The `title` element's text after `setWindowTitle`: upstream `htmlEncode`s the name and
/// status into its `innerHTML`, which the parser decodes back.
fn window_title(name: &JsString, status: Option<&str>) -> JsString {
    let mut title = name.clone();
    if let Some(status) = status.filter(|s| !s.is_empty()) {
        title.push_str(&JsString::from(" ("));
        title.push_str(&JsString::from(status));
        title.push_str(&JsString::from(")"));
    }
    title.push_str(&JsString::from(" \u{2014} "));
    title.push_str(&JsString::from(APP_NAME));
    if IS_LITE {
        title.push_str(&JsString::from(" Lite"));
    }
    if DISTRIBUTION == Distribution::LiteDs {
        title.push_str(&JsString::from(" (DS)"));
    }
    title
}

/// The root element's `--show-loading-glow` and `--loading-glow-progress` properties, `None`
/// until first set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadingGlow {
    pub show: Option<bool>,
    pub progress: Option<JsString>,
}

/// `set-version`'s fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Platform {
    pub platform: JsString,
    pub release: JsString,
    pub arch: JsValue,
    pub app_version: JsValue,
}

/// The `window.*` state tabs and the sidebar read.
pub struct HubPage {
    log: SharedLog,
    selection: SelectionImpl,
    preferences: Option<Preferences>,
    assets: Option<AdvantageScopeAssets>,
    assets_raw: JsValue,
    type_memory: SourceListTypeMemory,
    /// `window.typeMemory` as restored, which may hold entries of other shapes.
    type_memory_raw: JsValue,
    /// `type_memory` when last written into `type_memory_raw`.
    type_memory_synced: SourceListTypeMemory,
    realm: JsRealm,
    clock: Rc<dyn Clock>,
    view: Box<dyn HubView>,
    outbox: Vec<NamedMessage<HubToMain>>,
    open_xr: Vec<JsValue>,
}

impl HubPage {
    pub fn log(&self) -> Ref<'_, Log> {
        self.log.borrow()
    }

    pub fn shared_log(&self) -> &SharedLog {
        &self.log
    }

    pub fn selection(&self) -> &SelectionImpl {
        &self.selection
    }

    pub fn selection_mut(&mut self) -> &mut SelectionImpl {
        &mut self.selection
    }

    pub fn preferences(&self) -> Option<&Preferences> {
        self.preferences.as_ref()
    }

    pub fn assets(&self) -> Option<&AdvantageScopeAssets> {
        self.assets.as_ref()
    }

    pub fn type_memory(&self) -> &SourceListTypeMemory {
        &self.type_memory
    }

    /// `window.typeMemory = data`. Source lists read the entries of the shape they write.
    fn restore_type_memory(&mut self, data: &JsValue) {
        self.type_memory = type_memory_entries(data);
        self.type_memory_synced = self.type_memory.clone();
        self.type_memory_raw = data.clone();
    }

    /// `window.typeMemory`: the restored value with the entries source lists wrote since.
    fn type_memory_value(&mut self) -> Result<JsValue, &'static str> {
        for (id, entries) in self.type_memory.iter() {
            for (key, entry) in entries.iter() {
                let synced = self
                    .type_memory_synced
                    .get_key(id)
                    .and_then(|e| e.get_key(key));
                if synced == Some(entry) {
                    continue;
                }
                let JsValue::Object(ids) = &mut self.type_memory_raw else {
                    return Err("type memory written into a value that is not an object");
                };
                if ids.get_key(id).is_none() {
                    ids.insert(id.clone(), JsValue::Object(JsObject::new()));
                }
                let Some(JsValue::Object(raw)) = ids.get_units_mut(id.units()) else {
                    return Err("type memory written under an id that is not an object");
                };
                raw.insert(key.clone(), entry.to_js());
            }
        }
        self.type_memory_synced = self.type_memory.clone();
        Ok(self.type_memory_raw.clone())
    }

    pub fn view(&self) -> &dyn HubView {
        &*self.view
    }
}

impl TabCx for HubPage {
    fn tab_log(&self) -> Ref<'_, Log> {
        self.log.borrow()
    }

    fn tab_selection(&mut self) -> &mut dyn Selection {
        &mut self.selection
    }

    fn tab_preferences(&self) -> Option<&Preferences> {
        self.preferences.as_ref()
    }

    fn tab_preferences_mut(&mut self) -> Option<&mut Preferences> {
        self.preferences.as_mut()
    }

    fn tab_assets(&self) -> Option<&AdvantageScopeAssets> {
        self.assets.as_ref()
    }

    fn tab_rng(&mut self) -> &mut dyn Rng {
        self.realm.random.as_mut()
    }

    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.outbox.push(message);
    }

    fn send_open_xr(&mut self, uuid: JsValue) {
        self.open_xr.push(uuid);
    }
}

impl SourceListCx for HubPage {
    fn tab_dark(&self) -> bool {
        self.view.dark()
    }

    fn list_parts(&mut self) -> ListParts<'_> {
        ListParts {
            log: self.log.borrow(),
            type_memory: &mut self.type_memory,
            selection: &mut self.selection,
            dark: self.view.dark(),
        }
    }
}

impl StatisticsCx for HubPage {
    fn now(&self) -> f64 {
        self.clock.now()
    }
}

impl VideoCx for HubPage {
    fn now(&self) -> f64 {
        self.clock.now()
    }
}

impl TabsHost for HubPage {
    fn inner_height(&self) -> f64 {
        self.view.inner_height()
    }

    fn renderer_top(&self, show_timeline: bool) -> f64 {
        self.view.renderer_top(show_timeline)
    }
}

struct SourceEntry {
    source: HistoricalDataSource,
    path: JsString,
    progress: Option<f64>,
    progress_included: bool,
    /// `originalTimelineRange` and `originalTimelineIsMaxZoom` of the `startHistorical` call.
    original_timeline: Option<([f64; 2], bool)>,
}

/// An `async` handler waiting on `loadAllFields()`.
enum Pending {
    Publish {
        interval: HubInterval,
        is_sim: JsValue,
        /// Resolutions still awaited; 0 once queued for the next microtask checkpoint.
        waiting: usize,
    },
    Export {
        interval: HubInterval,
        mock: Rc<RefCell<f64>>,
        options: JsValue,
        path: JsValue,
        waiting: usize,
    },
}

struct MockProgress {
    start: f64,
    max_percent: f64,
    value: Rc<RefCell<f64>>,
}

struct ExportRequest {
    id: u64,
    interval: HubInterval,
    mock: Rc<RefCell<f64>>,
    path: JsValue,
}

/// Routes a source's host calls to the hub's host and keeps its events for the hub.
struct SourceAdapter<'a> {
    host: &'a mut dyn HubHost,
    active_fields: &'a [JsString],
    events: Vec<SourceEvent>,
}

impl SourceHost for SourceAdapter<'_> {
    fn now(&self) -> f64 {
        self.host.now()
    }

    fn send_main_message(&mut self, request: MainRequest) {
        self.host.send_historical_request(request);
    }

    fn set_timeout(&mut self, timeout: Timeout) {
        self.host.set_timeout(HubTimeout::Source(timeout));
    }

    fn set_interval(&mut self, source: &str) {
        self.host
            .set_interval(HubInterval::Source(source.to_owned()), SOURCE_INTERVAL_MS);
    }

    fn clear_interval(&mut self, source: &str) {
        self.host
            .clear_interval(&HubInterval::Source(source.to_owned()));
    }

    fn request_animation_frame(&mut self, source: &str) {
        self.host
            .request_animation_frame(HubFrame::Source(source.to_owned()));
    }

    fn worker_realm(&mut self) -> (Box<dyn Rng + Send>, Box<dyn Clock + Send>) {
        self.host.worker_realm()
    }

    fn spawn_worker(&mut self, source: &str, name: WorkerName, worker: Box<dyn DecodeWorker>) {
        self.host.spawn_worker(source, name, worker);
    }

    fn post_worker(&mut self, source: &str, request: Request) {
        self.host.post_worker(source, request);
    }

    fn terminate_worker(&mut self, source: &str) {
        self.host.terminate_worker(source);
    }

    fn active_fields(&mut self) -> Vec<JsString> {
        self.active_fields.to_vec()
    }

    fn event(&mut self, event: SourceEvent) {
        self.events.push(event);
    }
}

pub struct HubModel {
    page: HubPage,
    tabs: Tabs<HubPage>,
    sidebar: Sidebar,
    filenames: Rc<RefCell<Vec<JsString>>>,
    historical: Vec<SourceEntry>,
    platform: Platform,
    body_classes: Vec<&'static str>,
    fullscreen: bool,
    focused: bool,
    battery: bool,
    fps: bool,
    update_button: Option<bool>,
    feedback_button: Option<bool>,
    title: JsString,
    loading: LoadingGlow,
    exporting: bool,
    log_friendly_name: Option<JsString>,
    live_active: bool,
    live_connected: bool,
    live_address: JsString,
    /// Whether `liveSource` is set; upstream never clears it, so a stopped source still gets
    /// `live-data` and `stop()` calls.
    has_live_source: bool,
    tuner: Option<Box<dyn Tuner>>,
    next_mock: u64,
    mocks: Vec<(HubInterval, MockProgress)>,
    pending: Vec<Pending>,
    next_request: u64,
    exports: Vec<ExportRequest>,
}

fn num(n: f64) -> JsValue {
    JsValue::Number(n)
}

fn error_message(title: &str, content: &str) -> NamedMessage<HubToMain> {
    let mut o = JsObject::new();
    o.insert("title", JsValue::string(title));
    o.insert("content", JsValue::string(content));
    NamedMessage::new(HubToMain::Error, JsValue::Object(o))
}

fn plural(count: usize) -> &'static str {
    if count != 1 { "s" } else { "" }
}

fn truthy(value: &JsValue) -> bool {
    match value {
        JsValue::Undefined | JsValue::Null => false,
        JsValue::Bool(b) => *b,
        JsValue::Number(n) => *n != 0.0 && !n.is_nan(),
        JsValue::String(s) => !s.is_empty(),
        _ => true,
    }
}

fn file_name(path: &JsString, platform: &JsString) -> JsString {
    let separator = if *platform == JsString::from("win32") {
        u16::from(b'\\')
    } else {
        u16::from(b'/')
    };
    let units = path.units();
    let start = units
        .iter()
        .rposition(|&u| u == separator)
        .map_or(0, |i| i + 1);
    JsString::from_units(units[start..].to_vec())
}

impl HubModel {
    /// Module evaluation of `hub.ts`: `window.selection`, `window.sidebar` and `window.tabs`
    /// in that order, then the state-save, type-memory and touch-bar intervals. `realm` is the
    /// page's `Math.random`, shared by every `createUUID()` on the page.
    pub fn new(
        realm: JsRealm,
        clock: Rc<dyn Clock>,
        view: Box<dyn HubView>,
        host: &mut dyn HubHost,
    ) -> Self {
        let log: SharedLog = Rc::new(RefCell::new(Log::new(true)));
        let selection = SelectionImpl::new(log.clone(), clock.clone());
        let filenames = Rc::new(RefCell::new(Vec::new()));
        let names = filenames.clone();
        let sidebar = Sidebar::new(host.now(), move || names.borrow().clone());
        let mut page = HubPage {
            log,
            selection,
            preferences: None,
            assets: None,
            assets_raw: JsValue::Null,
            type_memory: SourceListTypeMemory::new(),
            type_memory_raw: JsValue::Object(JsObject::new()),
            type_memory_synced: SourceListTypeMemory::new(),
            realm,
            clock,
            view,
            outbox: Vec::new(),
            open_xr: Vec::new(),
        };
        let tabs = Tabs::new(Box::new(HubTabFactory::new()), &mut page);
        host.request_animation_frame(HubFrame::Tabs);
        host.set_interval(HubInterval::SaveState, STATE_SAVE_PERIOD_MS);
        host.set_interval(HubInterval::SaveTypeMemory, TYPE_MEMORY_SAVE_PERIOD_MS);
        host.set_interval(HubInterval::TouchBarSlider, TOUCH_BAR_PERIOD_MS);
        let mut hub = HubModel {
            page,
            tabs,
            sidebar,
            filenames,
            historical: Vec::new(),
            platform: Platform::default(),
            body_classes: Vec::new(),
            fullscreen: false,
            focused: true,
            battery: false,
            fps: false,
            update_button: None,
            feedback_button: None,
            title: JsString::from(APP_NAME),
            loading: LoadingGlow::default(),
            exporting: false,
            log_friendly_name: None,
            live_active: false,
            live_connected: false,
            live_address: JsString::new(),
            has_live_source: false,
            tuner: None,
            next_mock: 0,
            mocks: Vec::new(),
            pending: Vec::new(),
            next_request: 0,
            exports: Vec::new(),
        };
        hub.flush(host);
        hub
    }

    pub fn page(&self) -> &HubPage {
        &self.page
    }

    pub fn page_mut(&mut self) -> &mut HubPage {
        &mut self.page
    }

    pub fn tabs(&self) -> &Tabs<HubPage> {
        &self.tabs
    }

    /// The tabs with the page they run against.
    pub fn tabs_mut(&mut self) -> (&mut Tabs<HubPage>, &mut HubPage) {
        (&mut self.tabs, &mut self.page)
    }

    pub fn sidebar(&self) -> &Sidebar {
        &self.sidebar
    }

    /// Runs `f` with the sidebar and the environment its handlers read.
    pub fn with_sidebar<R>(&mut self, f: impl FnOnce(&mut Sidebar, &mut SidebarEnv<'_>) -> R) -> R {
        let loading = self.loading_fields();
        let page = &mut self.page;
        let view = &*page.view;
        let measure = |value: &RowValue| view.measure_value(value);
        let mut env = SidebarEnv {
            log: &page.log,
            selection: &mut page.selection,
            tuner: self.tuner.as_deref(),
            dark_theme: view.dark(),
            inner_height: view.inner_height(),
            can_scroll_into_view: view.can_scroll_into_view(),
            loading_fields: &loading,
            measure_value: &measure,
        };
        f(&mut self.sidebar, &mut env)
    }

    /// The `title` element's text (hub.html starts with the app name).
    pub fn title(&self) -> &JsString {
        &self.title
    }

    pub fn loading(&self) -> &LoadingGlow {
        &self.loading
    }

    /// The `fancy-*` classes on `body`, in `classList` order.
    pub fn body_classes(&self) -> &[&'static str] {
        &self.body_classes
    }

    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    /// Whether buttons are drawn unblurred.
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn is_battery(&self) -> bool {
        self.battery
    }

    pub fn show_fps(&self) -> bool {
        self.fps
    }

    /// `--show-update-button` and `--show-feedback-button`, `None` until first set.
    pub fn update_button(&self) -> Option<bool> {
        self.update_button
    }

    pub fn feedback_button(&self) -> Option<bool> {
        self.feedback_button
    }

    pub fn is_exporting(&self) -> bool {
        self.exporting
    }

    pub fn is_live_active(&self) -> bool {
        self.live_active
    }

    pub fn historical_paths(&self) -> impl Iterator<Item = &JsString> {
        self.historical.iter().map(|e| &e.path)
    }

    /// `window.tuner`, set by the host once a live source connects.
    pub fn set_tuner(&mut self, tuner: Option<Box<dyn Tuner>>) {
        self.tuner = tuner;
    }

    /// `window.getLoadingFields()`.
    pub fn loading_fields(&self) -> HashSet<JsString> {
        let mut out = HashSet::new();
        for entry in &self.historical {
            out.extend(entry.source.get_loading_fields());
        }
        out
    }

    fn set_window_title(&mut self, name: &JsString, status: Option<&str>) {
        self.title = window_title(name, status);
    }

    fn set_loading(&mut self, progress: Option<f64>) {
        self.loading.show = Some(progress.is_some());
        if let Some(progress) = progress {
            self.loading.progress = Some(JsString::from(number::to_string(progress).as_str()));
        }
    }

    fn set_exporting(&mut self, exporting: bool, host: &mut dyn HubHost) {
        if exporting != self.exporting {
            self.exporting = exporting;
            host.send_main_message(NamedMessage::new(
                HubToMain::SetExporting,
                JsValue::Bool(exporting),
            ));
        }
    }

    fn toggle_class(&mut self, class: &'static str, on: bool) {
        if on {
            if !self.body_classes.contains(&class) {
                self.body_classes.push(class);
            }
        } else {
            self.body_classes.retain(|c| *c != class);
        }
    }

    /// `updateFancyWindow()`.
    fn update_fancy_window(&mut self) {
        let platform = self.platform.platform.clone();
        let is = |name: &str| platform == JsString::from(name);
        let release = self.platform.release.to_string_lossy();
        let split: Vec<&str> = release.split('.').collect();
        let release_number = |s: Option<&&str>| {
            s.map_or(f64::NAN, |s| number::to_number(JsString::from(*s).units()))
        };
        self.toggle_class("fancy-title-bar-mac", is("darwin") && !self.fullscreen);
        self.toggle_class("fancy-title-bar-win", is("win32"));
        self.toggle_class("fancy-title-bar-linux", is("linux"));
        self.toggle_class("fancy-title-bar-lite", is("lite"));
        if is("darwin") {
            self.toggle_class("fancy-side-bar-mac", true);
            self.toggle_class(
                "fancy-side-bar-mac-27",
                release_number(split.first()) >= 27.0,
            );
        } else {
            self.toggle_class("fancy-side-bar-mac", false);
        }
        self.toggle_class(
            "fancy-side-bar-win",
            is("win32") && release_number(split.last()) >= 22621.0,
        );
    }

    /// Sends what the page queued during a task: the tabs', the selection's and the
    /// sidebar's main messages.
    fn flush(&mut self, host: &mut dyn HubHost) {
        for message in self.tabs.take_messages() {
            host.send_main_message(message);
        }
        for message in std::mem::take(&mut self.page.outbox) {
            host.send_main_message(message);
        }
        for message in self.page.selection.take_messages() {
            host.send_main_message(message);
        }
        for (name, data) in self.sidebar.take_main_messages() {
            match HubToMain::from_name(&name.to_string_lossy()) {
                Some(name) => host.send_main_message(NamedMessage::new(name, data)),
                None => host.exception("sidebar", format!("unknown message {name:?}")),
            }
        }
        self.sync_selection_context();
    }

    fn sync_selection_context(&mut self) {
        let selected = self
            .tabs
            .selected_index()
            .and_then(|i| self.tabs.tab(i))
            .and_then(|t| t.tab_type());
        let unlocked_video = self
            .tabs
            .selected_index()
            .and_then(|i| self.tabs.tab(i))
            .and_then(|t| {
                t.controller()
                    .downcast_ref::<VideoController>()
                    .map(|v| !v.is_locked())
            })
            .unwrap_or(false);
        let context = SelectionContext {
            platform: self.platform.platform.to_string_lossy(),
            selected_tab_type: selected.unwrap_or(TabType::LineGraph),
            unlocked_video_selected: unlocked_video,
        };
        self.page.selection.set_context(context);
    }

    fn sidebar_refresh(&mut self) -> Result<(), TabsError> {
        match self.with_sidebar(|sidebar, env| sidebar.refresh(env, false)) {
            Ok(()) => Ok(()),
            Err(outcome) => sidebar_result(outcome),
        }
    }

    fn tabs_refresh(&mut self) -> Result<(), TabsError> {
        self.tabs.refresh(&mut self.page)
    }

    fn active_fields(&mut self) -> Vec<JsString> {
        let mut fields = self.tabs.active_fields(&self.page, &self.page.log.borrow());
        let rects = self.page.view.sidebar_rects(&self.sidebar);
        let sidebar = self.with_sidebar(|sidebar, env| sidebar.get_active_fields(env, &rects));
        fields.extend(sidebar);
        fields
    }

    /// Runs `f` on historical source `index` and then hands the events it raised to the
    /// callbacks upstream passes to `openFile`, which upstream runs inside the source; here
    /// they borrow the `Log` the source writes, so they run after it. Only their `refresh`
    /// calls can throw, and a throw ends the upstream handler: the events after it are
    /// dropped, and the `loadAllFields()` resolutions among them stay queued on the source.
    /// The source's own state is not rolled back; the one change a handler makes after a
    /// refresh, `updateFieldRequest`'s status, never follows a throwing one, since every
    /// handler that changes the field keys refreshes before it returns.
    fn with_source<R>(
        &mut self,
        index: usize,
        active_fields: &[JsString],
        host: &mut dyn HubHost,
        f: impl FnOnce(&mut HistoricalDataSource, &mut PageContext<'_>) -> R,
    ) -> Result<R, TabsError> {
        let mut adapter = SourceAdapter {
            host,
            active_fields,
            events: Vec::new(),
        };
        let result = {
            let log = self.page.log.clone();
            let mut log = log.borrow_mut();
            let mut cx = PageContext {
                log: &mut log,
                realm: &mut self.page.realm,
                host: &mut adapter,
            };
            f(&mut self.historical[index].source, &mut cx)
        };
        let mut events = adapter.events.into_iter();
        while let Some(event) = events.next() {
            if let Err(e) = self.source_event(index, event, host) {
                let unresolved = events
                    .filter(|e| matches!(e, SourceEvent::AllFieldsLoaded))
                    .count();
                if let Some(entry) = self.historical.get_mut(index) {
                    entry.source.requeue_load_all(unresolved);
                }
                return Err(e);
            }
        }
        Ok(result)
    }

    fn update_loading(&mut self) {
        if self.historical.iter().all(|e| e.progress.is_none()) {
            for entry in &mut self.historical {
                entry.progress_included = false;
            }
        }
        let mut total = 0.0;
        let mut count = 0usize;
        for entry in &self.historical {
            if !entry.progress_included {
                continue;
            }
            total += entry.progress.unwrap_or(1.0);
            count += 1;
        }
        if count == 0 {
            self.set_loading(None);
        } else {
            self.set_loading(Some(total / count as f64));
        }
    }

    fn source_event(
        &mut self,
        index: usize,
        event: SourceEvent,
        host: &mut dyn HubHost,
    ) -> Result<(), TabsError> {
        match event {
            SourceEvent::Status {
                status,
                custom_error,
            } => self.source_status(index, status, custom_error, host),
            SourceEvent::Progress(progress) => {
                if let Some(entry) = self.historical.get_mut(index) {
                    entry.progress = Some(progress);
                }
                self.update_loading();
            }
            SourceEvent::Refresh { has_new_fields } => {
                self.sidebar_refresh()?;
                if has_new_fields {
                    self.tabs_refresh()?;
                }
            }
            SourceEvent::AllFieldsLoaded => self.all_fields_loaded(index),
        }
        Ok(())
    }

    fn source_status(
        &mut self,
        index: usize,
        status: Status,
        custom_error: Option<JsString>,
        host: &mut dyn HubHost,
    ) {
        let name = if self.historical.len() == 1 {
            file_name(&self.historical[0].path, &self.platform.platform)
        } else {
            JsString::from(
                format!(
                    "{} Log Files",
                    number::to_string(self.historical.len() as f64)
                )
                .as_str(),
            )
        };
        self.log_friendly_name = Some(name.clone());
        match status {
            Status::Reading | Status::DecodingInitial => {
                self.set_window_title(&name, Some("Loading"));
            }
            Status::DecodingField | Status::Idle => {
                self.set_window_title(&name, None);
                if let Some(entry) = self.historical.get_mut(index) {
                    entry.progress = None;
                }
                self.update_loading();
                let original = self
                    .historical
                    .get_mut(index)
                    .and_then(|e| e.original_timeline.take());
                if let Some((range, max_zoom)) = original {
                    let selection = &mut self.page.selection;
                    selection.set_timeline_range(range, max_zoom);
                    let new_range = selection.get_timeline_range();
                    if selection.get_timeline_is_max_zoom() && range[0] >= 0.0 && new_range[0] < 0.0
                    {
                        selection.set_timeline_range([0.0, new_range[1]], false);
                    }
                }
            }
            Status::Error => {
                self.set_window_title(&name, Some("Error"));
                if let Some(entry) = self.historical.get_mut(index) {
                    entry.progress = None;
                }
                self.update_loading();
                let is_csv = self
                    .historical
                    .get(index)
                    .is_some_and(|e| e.path.ends_with(&JsString::from(".csv"), None));
                let mut message =
                    JsString::from("There was a problem while reading the log file. ");
                message.push_str(&JsString::from(if is_csv {
                    "Please check the documentation for more information on the required format of CSV files."
                } else {
                    "Please try again."
                }));
                if let Some(custom) = custom_error {
                    message = custom;
                }
                let mut o = JsObject::new();
                o.insert("title", JsValue::string("Failed to open log"));
                o.insert("content", JsValue::String(message));
                host.send_main_message(NamedMessage::new(HubToMain::Error, JsValue::Object(o)));
            }
            Status::Waiting | Status::Stopped => {}
        }
    }

    fn sync_filenames(&mut self) {
        let platform = self.platform.platform.clone();
        *self.filenames.borrow_mut() = self
            .historical
            .iter()
            .map(|e| file_name(&e.path, &platform))
            .collect();
    }

    fn stop_historical(&mut self, host: &mut dyn HubHost) -> Result<(), TabsError> {
        for index in 0..self.historical.len() {
            self.with_source(index, &[], host, |source, cx| source.stop(cx))?;
        }
        self.historical.clear();
        self.sync_filenames();
        Ok(())
    }

    /// `startHistorical(path, clear, merge)`.
    fn start_historical(
        &mut self,
        path: &JsString,
        clear: bool,
        merge: bool,
        host: &mut dyn HubHost,
    ) -> Result<(), TabsError> {
        let clear = clear || !merge;
        let mut original_timeline = None;
        if clear {
            self.stop_historical(host)?;
            let selection = &mut self.page.selection;
            original_timeline = Some((
                selection.get_timeline_range(),
                selection.get_timeline_is_max_zoom(),
            ));
            let log: SharedLog = Rc::new(RefCell::new(Log::new(true)));
            self.page.log = log.clone();
            self.page.selection.set_log(log);
            self.sidebar_refresh()?;
            self.tabs_refresh()?;
        }

        if self.has_live_source {
            host.stop_live_source();
        }
        self.tuner = None;
        self.live_active = false;
        self.live_connected = false;
        self.set_loading(None);

        let source = HistoricalDataSource::new(&mut self.page.realm);
        self.historical.push(SourceEntry {
            source,
            path: path.clone(),
            progress: Some(0.0),
            progress_included: true,
            original_timeline,
        });
        self.sync_filenames();
        let index = self.historical.len() - 1;
        let prefix = if merge {
            let mut prefix = JsString::from("/");
            prefix.push_str(&JsString::from(MERGE_PREFIX));
            prefix.push_str(&JsString::from(number::to_string(index as f64).as_str()));
            prefix
        } else {
            JsString::new()
        };
        self.with_source(index, &[], host, |source, cx| {
            source.open_file(path, &prefix, cx)
        })
    }

    fn route(&self, uuid: &str) -> Option<usize> {
        self.historical.iter().position(|e| e.source.contains(uuid))
    }

    /// Runs a `setTimeout` the hub set.
    pub fn run_timeout(&mut self, timeout: HubTimeout, host: &mut dyn HubHost) {
        match timeout {
            HubTimeout::Source(timeout) => {
                if let Some(index) = self.route(timeout.source()) {
                    let result = self.with_source(index, &[], host, |source, cx| {
                        source.run_timeout(timeout, cx)
                    });
                    uncaught("setTimeout", result, host);
                }
            }
        }
        self.microtasks(host);
        self.flush(host);
    }

    /// Fires one interval the hub set.
    pub fn fire_interval(&mut self, interval: &HubInterval, host: &mut dyn HubHost) {
        match interval {
            HubInterval::SaveState => {
                let state = self.save_state();
                host.send_main_message(NamedMessage::new(HubToMain::SaveState, state));
            }
            HubInterval::SaveTypeMemory => match self.page.type_memory_value() {
                Ok(memory) => {
                    host.send_main_message(NamedMessage::new(HubToMain::SaveTypeMemory, memory))
                }
                Err(what) => host.exception("setInterval", format!("unmodeled: {what}")),
            },
            HubInterval::TouchBarSlider => {
                if self.platform.platform == JsString::from("darwin") {
                    let mut range = self.page.log.borrow().get_timestamp_range();
                    if let Some(live) = self.page.selection.get_current_live_time() {
                        range[1] = live;
                    }
                    let selected = self.page.selection.get_selected_time().unwrap_or(range[0]);
                    let percent = clamp_value(scale_value(selected, range, [0.0, 1.0]), 0.0, 1.0);
                    host.send_main_message(NamedMessage::new(
                        HubToMain::UpdateTouchBarSlider,
                        num(percent),
                    ));
                }
            }
            HubInterval::MockProgress(_) => {
                let now = host.now();
                if let Some((_, mock)) = self.mocks.iter().find(|(i, _)| i == interval) {
                    let value = calc_mock_progress((now - mock.start) / 1000.0, mock.max_percent);
                    *mock.value.borrow_mut() = value;
                    self.set_loading(Some(value));
                }
            }
            HubInterval::Source(uuid) => {
                if let Some(index) = self.route(uuid) {
                    let active = self.active_fields();
                    let uuid = uuid.clone();
                    let result = self.with_source(index, &active, host, |source, cx| {
                        source.fire_interval(&uuid, cx)
                    });
                    uncaught("setInterval", result, host);
                }
            }
        }
        self.microtasks(host);
        self.flush(host);
    }

    /// Runs one animation-frame callback the hub requested.
    pub fn animation_frame(&mut self, frame: &HubFrame, host: &mut dyn HubHost) {
        match frame {
            HubFrame::Tabs => {
                let out = self.tabs.periodic(&mut self.page);
                for update in out.satellite_updates {
                    host.send_main_message(update.to_message());
                }
                match out.error {
                    Some(e) => host.exception("requestAnimationFrame", format!("{e}")),
                    None => host.request_animation_frame(HubFrame::Tabs),
                }
            }
            HubFrame::Show => host.send_main_message(NamedMessage::without_data(HubToMain::Show)),
            HubFrame::Source(uuid) => {
                if let Some(index) = self.route(uuid) {
                    let uuid = uuid.clone();
                    let result = self.with_source(index, &[], host, |source, cx| {
                        source.animation_frame(&uuid, cx)
                    });
                    uncaught("requestAnimationFrame", result, host);
                }
            }
        }
        self.microtasks(host);
        self.flush(host);
    }

    /// One decode-worker message for the source `source`.
    pub fn worker_message(&mut self, source: &str, response: Response, host: &mut dyn HubHost) {
        if let Some(index) = self.route(source) {
            let result = self.with_source(index, &[], host, |root, cx| {
                root.worker_message(source, response, cx)
            });
            match result {
                Ok(Ok(())) => {}
                Ok(Err(Abort::Threw)) => host.exception("worker", "threw".to_owned()),
                Ok(Err(Abort::NeverReturns)) => {
                    host.exception("worker", "the page realm never returns".to_owned())
                }
                Ok(Err(Abort::Unmodeled(what))) => {
                    host.exception("worker", format!("unmodeled: {what}"))
                }
                Err(e) => host.exception("worker", format!("{e}")),
            }
        }
        self.microtasks(host);
        self.flush(host);
    }

    /// `historical-data`: main's reply to a source's read, handed to every source.
    pub fn historical_data(&mut self, reply: &MainReply, host: &mut dyn HubHost) {
        for index in 0..self.historical.len() {
            let result = self.with_source(index, &[], host, |source, cx| {
                source.handle_main_message(reply, cx)
            });
            if let Err(e) = result {
                host.exception("setTimeout", format!("{e}"));
                break;
            }
        }
        self.microtasks(host);
        self.flush(host);
    }

    fn all_fields_loaded(&mut self, index: usize) {
        for pending in &mut self.pending {
            match pending {
                Pending::Publish { waiting, .. } if index == 0 && *waiting > 0 => {
                    *waiting -= 1;
                    return;
                }
                Pending::Export { waiting, .. } if *waiting > 0 => {
                    *waiting -= 1;
                    return;
                }
                _ => {}
            }
        }
    }

    /// The microtask checkpoint after a task: `async` handlers whose `await` resolved go on.
    fn microtasks(&mut self, host: &mut dyn HubHost) {
        while let Some(i) = self.pending.iter().position(|p| match p {
            Pending::Publish { waiting, .. } | Pending::Export { waiting, .. } => *waiting == 0,
        }) {
            match self.pending.remove(i) {
                Pending::Publish {
                    interval, is_sim, ..
                } => {
                    self.clear_mock(&interval, host);
                    self.start_publisher(&is_sim, host);
                }
                Pending::Export {
                    interval,
                    mock,
                    options,
                    path,
                    ..
                } => self.request_export(interval, mock, options, path, host),
            }
        }
    }

    fn start_mock(
        &mut self,
        max_percent: f64,
        host: &mut dyn HubHost,
    ) -> (HubInterval, Rc<RefCell<f64>>) {
        let interval = HubInterval::MockProgress(self.next_mock);
        self.next_mock += 1;
        let value = Rc::new(RefCell::new(0.0));
        self.mocks.push((
            interval.clone(),
            MockProgress {
                start: host.now(),
                max_percent,
                value: value.clone(),
            },
        ));
        host.set_interval(interval.clone(), MOCK_PROGRESS_PERIOD_MS);
        (interval, value)
    }

    fn clear_mock(&mut self, interval: &HubInterval, host: &mut dyn HubHost) {
        if let Some(i) = self.mocks.iter().position(|(m, _)| m == interval) {
            self.mocks.remove(i);
            host.clear_interval(interval);
        }
    }

    fn start_publisher(&mut self, is_sim: &JsValue, host: &mut dyn HubHost) {
        host.stop_publisher();
        host.start_publisher(is_sim);
    }

    fn request_export(
        &mut self,
        interval: HubInterval,
        mock: Rc<RefCell<f64>>,
        options: JsValue,
        path: JsValue,
        host: &mut dyn HubHost,
    ) {
        let id = self.next_request;
        self.next_request += 1;
        let log = self.page.log.borrow().to_serialized();
        self.exports.push(ExportRequest {
            id,
            interval,
            mock,
            path,
        });
        host.request_export(id, log, options);
    }

    /// A message from the export worker of request `id`.
    pub fn export_reply(&mut self, id: u64, reply: ExportReply, host: &mut dyn HubHost) {
        let Some(i) = self.exports.iter().position(|e| e.id == id) else {
            return;
        };
        match reply {
            ExportReply::Progress(progress) => {
                let interval = self.exports[i].interval.clone();
                self.clear_mock(&interval, host);
                let mock = *self.exports[i].mock.borrow();
                self.set_loading(Some(scale_value(progress, [0.0, 1.0], [mock, 1.0])));
            }
            ExportReply::Payload(content) => {
                let request = self.exports.remove(i);
                let mut o = JsObject::new();
                o.insert("path", request.path);
                o.insert("content", content);
                host.send_main_message(NamedMessage::new(
                    HubToMain::WriteExport,
                    JsValue::Object(o),
                ));
                self.set_exporting(false, host);
            }
            ExportReply::Failed => {
                self.exports.remove(i);
                host.send_main_message(error_message(
                    "Failed to export data",
                    "There was a problem while converting to the export format. Please try again.",
                ));
                self.set_loading(None);
                self.set_exporting(false, host);
            }
        }
        self.flush(host);
    }

    /// `startLive(sourceType)`.
    fn start_live(
        &mut self,
        source_type: &JsValue,
        host: &mut dyn HubHost,
    ) -> Result<(), TabsError> {
        self.stop_historical(host)?;
        if self.has_live_source {
            host.stop_live_source();
        }
        host.stop_publisher();
        self.live_active = true;
        self.set_loading(None);

        let Some(preferences) = self.page.preferences.as_ref() else {
            return Ok(());
        };
        let is =
            |name: &str| matches!(source_type, JsValue::String(s) if *s == JsString::from(name));
        let kind = if is("ds") {
            Some(LiveSourceKind::Nt4DriverStation)
        } else {
            match preferences.live_mode.as_str() {
                "nt4" => Some(LiveSourceKind::Nt4),
                "nt4-akit" => Some(LiveSourceKind::Nt4AdvantageKit),
                "nt4-systemcore" => Some(LiveSourceKind::Nt4Systemcore),
                "phoenix" => Some(LiveSourceKind::Phoenix),
                "rlog" => Some(LiveSourceKind::Rlog),
                "ftcdashboard" => Some(LiveSourceKind::FtcDashboard),
                _ => None,
            }
        };
        let address = if is("sim") || is("ds") {
            JsString::from("127.0.0.1")
        } else {
            preferences.robot_address_for(&self.platform.platform.to_string_lossy())
        };
        let Some(kind) = kind else {
            return Err(TabsError::Thrown(JsError::new(
                "TypeError",
                "Cannot read properties of null (reading 'connect')",
            )));
        };
        self.live_address = address.clone();
        self.has_live_source = true;
        host.start_live_source(kind, &address);
        Ok(())
    }

    /// The live source's status callback.
    pub fn live_status(&mut self, status: LiveStatus, host: &mut dyn HubHost) {
        let address = self.live_address.clone();
        match status {
            LiveStatus::Connecting => self.set_window_title(&address, Some("Searching")),
            LiveStatus::Active => self.set_window_title(&address, None),
            LiveStatus::Error => {
                self.set_window_title(&address, Some("Error"));
                host.send_main_message(error_message(
                    "Problem with live source",
                    "There was a problem while connecting to the live source. Please check your connection settings and try again.",
                ));
            }
            LiveStatus::Waiting | LiveStatus::Stopped => {}
        }
        if status != LiveStatus::Active {
            self.page.selection.set_live_disconnected();
        }
        self.flush(host);
    }

    /// The live source's output callback: its `Log` becomes `window.log`.
    pub fn live_output(
        &mut self,
        log: SharedLog,
        time_supplier: TimeSupplier,
        host: &mut dyn HubHost,
    ) {
        self.live_connected = true;
        self.page.log = log.clone();
        self.page.selection.set_log(log);
        self.page.selection.set_live_connected(time_supplier);
        let result = self.sidebar_refresh().and_then(|()| self.tabs_refresh());
        uncaught("live", result, host);
        self.flush(host);
    }

    /// The publisher's status callback.
    pub fn publisher_status(&mut self, status: PublisherStatus, host: &mut dyn HubHost) {
        let Some(name) = self.log_friendly_name.clone() else {
            return;
        };
        match status {
            PublisherStatus::Connecting => self.set_window_title(&name, Some("Searching")),
            PublisherStatus::Active => self.set_window_title(&name, Some("Publishing")),
            PublisherStatus::Stopped => self.set_window_title(&name, None),
        }
        self.flush(host);
    }

    /// `saveState()`.
    pub fn save_state(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("sidebar", self.sidebar.save_state().to_js());
        o.insert("tabs", self.tabs.save_state(&self.page));
        JsValue::Object(o)
    }

    /// `restoreState(state)`.
    fn restore_state(&mut self, state: &JsValue) -> Result<(), TabsError> {
        let sidebar = state.get("sidebar").cloned().unwrap_or_default();
        let Some(sidebar) = SidebarState::from_js(&sidebar) else {
            return Err(TabsError::Unmodeled(
                "restore-state with a sidebar of another shape",
            ));
        };
        let outcome = self.with_sidebar(|s, env| s.restore_state(env, &sidebar));
        sidebar_result(outcome)?;
        let tabs = state.get("tabs").cloned().unwrap_or_default();
        self.tabs.restore_state(&mut self.page, &tabs)
    }

    /// `set-assets`.
    fn set_assets(&mut self, data: &JsValue, host: &mut dyn HubHost) -> Result<(), TabsError> {
        if asrs_js::json::stringify(&self.page.assets_raw) == asrs_js::json::stringify(data) {
            return Ok(());
        }
        let alert = assets_alert(&self.page.assets_raw, data);
        self.page.assets = match data {
            JsValue::Null | JsValue::Undefined => None,
            _ => Some(AdvantageScopeAssets::from_js(data)),
        };
        self.page.assets_raw = data.clone();
        self.tabs.new_assets(&mut self.page)?;
        if let Some(warning) = alert.warning {
            host.console(true, warning);
        }
        if let Some(error) = alert.error {
            host.send_main_message(error);
        }
        if alert.recovered {
            host.console(false, "All assets loaded successfully".to_owned());
        }
        Ok(())
    }

    /// `zoom-enabled`.
    fn zoom_enabled(&mut self) {
        let (range, enabled) = {
            let log = self.page.log.borrow();
            (log.get_timestamp_range(), get_enabled_data(&log))
        };
        let mut range = range;
        let first_enable = enabled
            .as_ref()
            .and_then(|d| d.values.iter().position(|v| *v));
        if let (Some(first), Some(data)) = (first_enable, enabled.as_ref()) {
            range[0] = data.timestamps[first];
            let mut last_disable = data.values.iter().rposition(|v| !*v);
            if data.values.last() == Some(&true) {
                last_disable = None;
            }
            if last_disable.is_some_and(|l| l < first) {
                last_disable = None;
            }
            if let Some(last) = last_disable {
                range[1] = data.timestamps[last];
            }
        }
        self.page.selection.unlock();
        self.page.selection.set_timeline_range(range, false);
    }

    /// `handleMainMessage(message)` for every message without a binary payload.
    pub fn handle_main_message(
        &mut self,
        message: &NamedMessage<MainToHub>,
        host: &mut dyn HubHost,
    ) {
        let result = self.dispatch(message, host);
        if let Err(e) = result {
            host.exception("unhandledRejection", format!("{e}"));
        }
        self.microtasks(host);
        self.flush(host);
    }

    fn dispatch(
        &mut self,
        message: &NamedMessage<MainToHub>,
        host: &mut dyn HubHost,
    ) -> Result<(), TabsError> {
        let data = &message.data;
        let field = |key: &str| data.get(key).cloned().unwrap_or_default();
        match message.name {
            MainToHub::ShowWhenReady => host.request_animation_frame(HubFrame::Show),
            MainToHub::RestoreState => self.restore_state(data)?,
            MainToHub::RestoreTypeMemory => self.page.restore_type_memory(data),
            MainToHub::SetFullscreen => {
                self.fullscreen = truthy(data);
                self.update_fancy_window();
            }
            MainToHub::SetFocused => self.focused = truthy(data),
            MainToHub::SetBattery => self.battery = truthy(data),
            MainToHub::SetVersion => {
                let text = |key: &str| match data.get(key) {
                    Some(JsValue::String(s)) => s.clone(),
                    _ => JsString::new(),
                };
                self.platform = Platform {
                    platform: text("platform"),
                    release: text("platformRelease"),
                    arch: field("platformArch"),
                    app_version: field("appVersion"),
                };
                self.sync_filenames();
                self.update_fancy_window();
            }
            MainToHub::SetPreferences => {
                let mut preferences = Preferences::default();
                preferences
                    .merge(data)
                    .map_err(|_| TabsError::Unmodeled("set-preferences of another shape"))?;
                self.page.preferences = Some(preferences);
            }
            MainToHub::SetAssets => self.set_assets(data, host)?,
            MainToHub::SetActiveSatellites => {
                let uuids = data.as_array().map(<[JsValue]>::to_vec).unwrap_or_default();
                self.tabs.set_active_satellites(uuids);
            }
            MainToHub::CallSelectionSetter => self.call_selection_setter(data),
            MainToHub::ZoomEnabled => self.zoom_enabled(),
            MainToHub::ShowUpdateButton => self.update_button = Some(truthy(data)),
            MainToHub::ShowFeedbackButton => self.feedback_button = Some(truthy(data)),
            MainToHub::HistoricalData => {
                return Err(TabsError::Unmodeled(
                    "historical-data goes through HubModel::historical_data",
                ));
            }
            MainToHub::LiveData => {
                if self.has_live_source {
                    host.live_data(data);
                }
            }
            MainToHub::OpenFiles => self.open_files(data, host)?,
            MainToHub::StartLive => {
                if self.exporting {
                    host.send_main_message(error_message(
                        "Cannot connect",
                        "Please wait for the export to finish, then try again.",
                    ));
                } else {
                    self.start_live(data, host)?;
                }
            }
            MainToHub::StartPublish => self.start_publish(data, host)?,
            MainToHub::StopPublish => host.stop_publisher(),
            MainToHub::SetPlaybackOptions => {
                let speed = field("speed").as_f64().unwrap_or(f64::NAN);
                self.page.selection.set_playback_speed(speed);
                self.page
                    .selection
                    .set_playback_looping(truthy(&field("looping")));
            }
            MainToHub::ToggleSidebar => self.sidebar.toggle_visible(),
            MainToHub::ToggleControls => self.tabs.toggle_controls_visible(&self.page)?,
            MainToHub::NewTab => self.tabs.add_tab(&mut self.page, data)?,
            MainToHub::NewSatellite => self.tabs.new_satellite()?,
            MainToHub::MoveTab => self.tabs.move_selection(&self.page, data)?,
            MainToHub::ShiftTab => self.tabs.shift_selected(&self.page, data)?,
            MainToHub::CloseTab => {
                let index = self.tabs.selected().clone();
                self.tabs.close(&self.page, &index, data)?;
            }
            MainToHub::RenameTab => self.tabs.rename_tab(&field("index"), field("name"))?,
            MainToHub::SourceListTypeResponse | MainToHub::SourceListClearResponse => {
                return Err(TabsError::Unmodeled(
                    "source-list prompt responses belong to the source-list view",
                ));
            }
            MainToHub::SetRobotModeVisible => {
                if let Some(c) = self
                    .tabs
                    .selected_controller_as::<LineGraphController>(TabType::LineGraph)?
                {
                    c.set_robot_mode_visible(field("showRobotMode"));
                }
            }
            MainToHub::EditAxis => {
                if let Some(c) = self
                    .tabs
                    .selected_controller_as::<LineGraphController>(TabType::LineGraph)?
                {
                    let page = &mut self.page;
                    c.edit_axis(
                        page,
                        &field("legend"),
                        &field("lockedRange"),
                        &field("unitConversion"),
                        &field("filter"),
                    )?;
                }
            }
            MainToHub::ClearAxis => {
                if let Some(c) = self
                    .tabs
                    .selected_controller_as::<LineGraphController>(TabType::LineGraph)?
                {
                    c.clear_axis(data);
                }
            }
            MainToHub::Set3dCamera | MainToHub::EditFov => {
                let value = data.as_f64().unwrap_or(f64::NAN);
                if let Some(renderer) = self.tabs.selected_renderer(TabType::Field3d)? {
                    if message.name == MainToHub::Set3dCamera {
                        renderer.set_3d_camera(&mut self.page, value);
                    } else {
                        renderer.set_fov(&mut self.page, value);
                    }
                }
            }
            MainToHub::Set2dCamera => {
                if let Some(r) = self
                    .tabs
                    .selected_renderer_as::<Field2dRenderer>(TabType::Field2d)?
                {
                    r.set_2d_camera(&self.page, data.as_f64().unwrap_or(f64::NAN));
                }
            }
            MainToHub::AddTableRange => {
                let renderer_uuid = match data.get("rendererUUID") {
                    Some(JsValue::String(s)) => s.clone(),
                    _ => {
                        return Err(TabsError::Unmodeled(
                            "add-table-range without a rendererUUID",
                        ));
                    }
                };
                let range = match data.get("range") {
                    Some(JsValue::Array(r)) => match (r.first(), r.get(1)) {
                        (Some(JsValue::Number(a)), Some(JsValue::Number(b))) => Some([*a, *b]),
                        _ => None,
                    },
                    _ => None,
                };
                let controller = field("controllerUUID");
                self.tabs
                    .add_table_range(&mut self.page, &controller, &renderer_uuid, range)?;
            }
            MainToHub::VideoData => {
                for (uuid, controller) in self
                    .tabs
                    .controllers_with_uuid::<VideoController>(TabType::Video)
                {
                    controller.process_video_data(&mut self.page, data, uuid)?;
                }
            }
            MainToHub::StartExport => self.start_export(host),
            MainToHub::CancelExport => self.set_exporting(false, host),
            MainToHub::PrepareExport => self.prepare_export(data, host)?,
            MainToHub::FinishExport => self.set_loading(None),
            MainToHub::UpdateTouchBarSlider => {
                let mut range = self.page.log.borrow().get_timestamp_range();
                if let Some(live) = self.page.selection.get_current_live_time() {
                    range[1] = live;
                }
                let value = data.as_f64().unwrap_or(f64::NAN);
                self.page
                    .selection
                    .set_selected_time(scale_value(value, [0.0, 1.0], range));
            }
        }
        Ok(())
    }

    fn call_selection_setter(&mut self, data: &JsValue) {
        let uuid = data.get("uuid").cloned().unwrap_or_default();
        if !self.tabs.is_valid_uuid(&uuid) {
            return;
        }
        let name = match data.get("name") {
            Some(JsValue::String(s)) => s.to_string_lossy(),
            _ => return,
        };
        let args = data
            .get("args")
            .and_then(JsValue::as_array)
            .map(<[JsValue]>::to_vec)
            .unwrap_or_default();
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        let number = |i: usize| arg(i).as_f64().unwrap_or(f64::NAN);
        let selection = &mut self.page.selection;
        match name.as_str() {
            "setHoveredTime" => selection.set_hovered_time(arg(0).as_f64()),
            "setSelectedTime" => selection.set_selected_time(number(0)),
            "goIdle" => selection.go_idle(),
            "play" => selection.play(),
            "pause" => selection.pause(),
            "togglePlayback" => selection.toggle_playback(),
            "lock" => selection.lock(),
            "unlock" => selection.unlock(),
            "toggleLock" => selection.toggle_lock(),
            "stepCycle" => selection.step_cycle(truthy(&arg(0))),
            "setGrabZoomRange" => {
                let range = match &arg(0) {
                    JsValue::Array(r) => match (r.first(), r.get(1)) {
                        (Some(JsValue::Number(a)), Some(JsValue::Number(b))) => Some([*a, *b]),
                        _ => None,
                    },
                    _ => None,
                };
                selection.set_grab_zoom_range(range);
            }
            "finishGrabZoom" => selection.finish_grab_zoom(),
            "applyTimelineScroll" => {
                selection.apply_timeline_scroll(number(0), number(1), number(2))
            }
            _ => {}
        }
    }

    fn open_files(&mut self, data: &JsValue, host: &mut dyn HubHost) -> Result<(), TabsError> {
        let files: Vec<JsValue> = data
            .get("files")
            .and_then(JsValue::as_array)
            .map(<[JsValue]>::to_vec)
            .unwrap_or_default();
        let merge = truthy(&data.get("merge").cloned().unwrap_or_default());
        if self.exporting {
            let title = format!("Cannot open file{}", plural(files.len()));
            host.send_main_message(error_message(
                &title,
                "Please wait for the export to finish, then try again.",
            ));
        } else if merge && (self.live_active || self.historical.is_empty()) {
            let title = format!("Cannot insert file{}", plural(files.len()));
            host.send_main_message(error_message(
                &title,
                "No log files are currently loaded. Choose \"Open Log(s)\" to load new files.",
            ));
        } else {
            let count = files.len();
            for (index, file) in files.iter().enumerate() {
                let path = match file {
                    JsValue::String(s) => s.clone(),
                    other => JsString::from(
                        asrs_js::json::stringify(other)
                            .ok()
                            .flatten()
                            .unwrap_or_default()
                            .as_str(),
                    ),
                };
                if merge {
                    self.start_historical(&path, false, true, host)?;
                } else {
                    self.start_historical(&path, index == 0, count > 1, host)?;
                }
            }
        }
        Ok(())
    }

    fn start_publish(&mut self, data: &JsValue, host: &mut dyn HubHost) -> Result<(), TabsError> {
        if self.live_active {
            host.send_main_message(error_message(
                "Cannot publish",
                "Publishing is not allowed from a live source.",
            ));
            return Ok(());
        }
        let has_nt = self
            .page
            .log
            .borrow()
            .get_field_tree(true, &JsString::new())
            .contains_key("NT");
        if !has_nt {
            host.send_main_message(error_message(
                "Cannot publish",
                "Please open a log file with NetworkTables data, then try again.",
            ));
            return Ok(());
        }
        let (interval, _) = self.start_mock(1.0, host);
        if self.historical.is_empty() {
            self.clear_mock(&interval, host);
            self.start_publisher(data, host);
            return Ok(());
        }
        let resolved = self.with_source(0, &[], host, |source, cx| source.load_all_fields(cx))?;
        self.pending.push(Pending::Publish {
            interval,
            is_sim: data.clone(),
            waiting: usize::from(!resolved),
        });
        Ok(())
    }

    fn start_export(&mut self, host: &mut dyn HubHost) {
        let log_path = self.historical.first().map(|e| e.path.clone());
        if self.exporting {
            host.send_main_message(error_message(
                "Cannot export data",
                "Please wait for the previous export to finish, then try again.",
            ));
        } else if log_path.is_none() && !self.live_connected {
            host.send_main_message(error_message(
                "Cannot export data",
                "Please open a log file or connect to a live source, then try again.",
            ));
        } else {
            self.set_exporting(true, host);
            let preferences = self.page.preferences.as_ref();
            let incomplete_warning = self.live_connected
                && preferences.is_some_and(|p| {
                    p.live_subscribe_mode.as_str() == "low-bandwidth"
                        || p.live_mode.as_str() == "phoenix"
                });
            let supports_akit = self.page.log.borrow().field_keys().any(|key| {
                AKIT_TIMESTAMP_KEYS
                    .iter()
                    .any(|k| *key == JsString::from(*k))
            });
            let mut o = JsObject::new();
            o.insert("path", log_path.map_or(JsValue::Null, JsValue::String));
            o.insert("incompleteWarning", JsValue::Bool(incomplete_warning));
            o.insert("supportsAkit", JsValue::Bool(supports_akit));
            host.send_main_message(NamedMessage::new(
                HubToMain::PromptExport,
                JsValue::Object(o),
            ));
        }
    }

    fn prepare_export(&mut self, data: &JsValue, host: &mut dyn HubHost) -> Result<(), TabsError> {
        let (interval, mock) = self.start_mock(0.25, host);
        let mut waiting = 0;
        for index in 0..self.historical.len() {
            let resolved =
                self.with_source(index, &[], host, |source, cx| source.load_all_fields(cx))?;
            if !resolved {
                waiting += 1;
            }
        }
        self.pending.push(Pending::Export {
            interval,
            mock,
            options: data.get("options").cloned().unwrap_or_default(),
            path: data.get("path").cloned().unwrap_or_default(),
            waiting,
        });
        Ok(())
    }
}

fn uncaught<T>(task: &'static str, result: Result<T, TabsError>, host: &mut dyn HubHost) {
    if let Err(e) = result {
        host.exception(task, format!("{e}"));
    }
}

/// What upstream's sidebar throws where the port's stops with `outcome`.
fn sidebar_result(outcome: EventOutcome) -> Result<(), TabsError> {
    let message = match outcome {
        EventOutcome::Ok => return Ok(()),
        EventOutcome::HighlightAborted => "fieldElement.scrollIntoViewIfNeeded is not a function",
        EventOutcome::MergedFilenameMissing => {
            "Cannot read properties of undefined (reading 'replace')"
        }
    };
    Err(TabsError::Thrown(JsError::new("TypeError", message)))
}

/// What `set-assets` reports about load failures.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssetsAlert {
    /// The `console.warn` listing the failures.
    pub warning: Option<String>,
    /// The "Failed to load assets" error for main.
    pub error: Option<NamedMessage<HubToMain>>,
    /// Whether "All assets loaded successfully" is logged.
    pub recovered: bool,
}

fn load_failures(assets: &JsValue) -> Option<Vec<JsValue>> {
    match assets {
        JsValue::Object(o) => o
            .get("loadFailures")
            .and_then(JsValue::as_array)
            .map(<[JsValue]>::to_vec),
        _ => None,
    }
}

/// The alert half of `set-assets`, from the old and new `window.assets`.
pub fn assets_alert(old: &JsValue, new: &JsValue) -> AssetsAlert {
    let old_failed = load_failures(old).is_some_and(|f| !f.is_empty());
    let new_failures = load_failures(new).unwrap_or_default();
    let new_failed = !new_failures.is_empty();
    let warning = new_failed.then(|| {
        let names: Vec<String> = new_failures
            .iter()
            .map(|f| match f {
                JsValue::String(s) => s.to_string_lossy(),
                other => asrs_js::json::stringify(other)
                    .ok()
                    .flatten()
                    .unwrap_or_default(),
            })
            .collect();
        format!(
            "Failed to load: {}. Check that all assets follow the format described in the AdvantageScope documentation.",
            names.join(", ")
        )
    });
    AssetsAlert {
        warning,
        error: (new_failed && !old_failed).then(|| {
            error_message(
                "Failed to load assets",
                "Some asset files failed to load. Check the developer console for details.",
            )
        }),
        recovered: !new_failed && old_failed,
    }
}

fn type_memory_entries(data: &JsValue) -> SourceListTypeMemory {
    let mut memory = SourceListTypeMemory::new();
    let JsValue::Object(ids) = data else {
        return memory;
    };
    for (id, entries) in ids.iter() {
        let JsValue::Object(entries) = entries else {
            continue;
        };
        let mut map = JsObject::new();
        for (key, entry) in entries.iter() {
            if let Some(entry) = SourceListTypeMemoryEntry::from_js(entry) {
                map.insert(key.clone(), entry);
            }
        }
        memory.insert(id.clone(), map);
    }
    memory
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assets(failures: &[&str]) -> JsValue {
        let mut o = JsObject::new();
        o.insert(
            "loadFailures",
            JsValue::Array(failures.iter().map(|f| JsValue::string(f)).collect()),
        );
        JsValue::Object(o)
    }

    #[test]
    fn the_assets_alert_fires_once_per_failing_run() {
        let first = assets_alert(&JsValue::Null, &assets(&["a", "b"]));
        assert!(first.error.is_some());
        assert_eq!(
            first.warning.as_deref(),
            Some(
                "Failed to load: a, b. Check that all assets follow the format described in the AdvantageScope documentation."
            )
        );
        let again = assets_alert(&assets(&["a"]), &assets(&["c"]));
        assert!(again.error.is_none() && again.warning.is_some() && !again.recovered);
        let fixed = assets_alert(&assets(&["a"]), &assets(&[]));
        assert_eq!(
            fixed,
            AssetsAlert {
                warning: None,
                error: None,
                recovered: true
            }
        );
        assert_eq!(
            assets_alert(&JsValue::Null, &assets(&[])),
            AssetsAlert::default()
        );
    }

    #[test]
    fn file_names_split_on_the_platform_separator() {
        let path = JsString::from("C:\\logs\\a.wpilog");
        assert_eq!(
            file_name(&path, &JsString::from("win32")),
            JsString::from("a.wpilog")
        );
        assert_eq!(file_name(&path, &JsString::from("darwin")), path);
        assert_eq!(
            file_name(&JsString::from("/x/y.csv"), &JsString::from("darwin")),
            JsString::from("y.csv")
        );
    }
}
