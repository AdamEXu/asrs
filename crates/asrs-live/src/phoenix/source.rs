//! Port of `src/hub/dataSources/PhoenixDiagnosticsSource.ts`: CTRE's Phoenix 6 diagnostics,
//! polled over HTTP on port 1250. Devices and their signal lists are discovered once a second;
//! the fields the tabs selected are plotted every 50 ms. Async fetches become pending requests
//! whose `AbortSignal.timeout` tasks and settle events the caller drives.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use asrs_js::json;
use asrs_js::number;
use asrs_js::ops;
use asrs_js::string::{JsString, encode_uri_component};
use asrs_js::time::{Clock, Rng};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::log_key_utils::PHOENIX_PREFIX;
use asrs_log::log::{Log, LoggableType};

use super::enums;
use crate::Thrown;
use crate::http::{FetchEvent, FetchTransport, RequestId};
use crate::live_data_source::{
    LiveDataSourceBase, LiveDataSourceStatus, LivePreferences, OutputCallback, StatusCallback,
    TimeSupplier,
};
use crate::timers::{TimerId, TimerQueue};

const PORT: f64 = 1250.0;
const GET_DEVICES_PERIOD_MS: f64 = 1000.0;
const GET_DEVICES_TIMEOUT_MS: f64 = 3000.0;
const GET_SIGNALS_TIMEOUT_MS: f64 = 3000.0;
const PLOT_PERIOD_MS: f64 = 50.0;
const PLOT_TIMEOUT_MS: f64 = PLOT_PERIOD_MS - 10.0;
const PLOT_RESOLUTION_MS: f64 = PLOT_PERIOD_MS;

fn js(s: &str) -> JsString {
    JsString::from(s)
}

/// What a Phoenix method may touch besides the source: the hub's timers and the browser's
/// `fetch`.
pub struct PhoenixContext<'a> {
    pub timers: &'a mut TimerQueue,
    pub net: &'a mut dyn FetchTransport,
}

/// `window` as the source sees it: only the tabs' active fields and the preferences the
/// discard timer reads.
pub struct PhoenixWindow {
    /// `window.tabs.getActiveFields()`.
    pub tabs_fields: Vec<JsString>,
    /// `window.preferences`; `None` is `null`.
    pub preferences: Option<LivePreferences>,
}

/// `this.deviceSignals[deviceName].length === 0`, the check that decides whether getsignals
/// fires again; the list is whatever `json.Signals` was.
fn is_empty_list(list: &JsValue) -> Result<bool, Thrown> {
    Ok(ops::strict_equals(
        &ops::get(list, "length")?,
        &JsValue::Number(0.0),
    ))
}

/// `list.find(signal => signal.Name === signalName)`; a receiver that is not an array throws.
fn find_named<'a>(
    list: &'a JsValue,
    signal_name: &JsString,
) -> Result<Option<&'a JsValue>, Thrown> {
    let JsValue::Array(items) = list else {
        return Err(Thrown::type_error(
            "this.deviceSignals[deviceName].find is not a function",
        ));
    };
    for signal in items {
        let name = ops::get(signal, "Name")?;
        if ops::strict_equals(&name, &JsValue::String(signal_name.clone())) {
            return Ok(Some(signal));
        }
    }
    Ok(None)
}

enum PendingKind {
    Devices,
    Signals(JsString),
    Plot(JsString, Rc<Vec<JsValue>>),
}

/// A `fetch` in flight: the request, its `AbortSignal.timeout` timer and what its promise
/// continuations need.
struct Pending {
    request: RequestId,
    abort: TimerId,
    kind: PendingKind,
}

/// One `window.setTimeout` per active device, waiting to send its plot request.
struct PlotPending {
    timer: TimerId,
    device_name: JsString,
    signals: Rc<Vec<JsValue>>,
}

/// `PhoenixDiagnosticsSource`.
pub struct PhoenixSource {
    base: LiveDataSourceBase,
    clock: Rc<dyn Clock>,
    /// Every getDevices interval id: a second `connect` leaves the first running.
    get_devices_timers: Vec<TimerId>,
    /// Every plot interval id.
    plot_timers: Vec<TimerId>,
    device_descriptions: JsObject<JsValue>,
    device_signals: JsObject<JsValue>,
    live_start_real_time: Rc<Cell<Option<f64>>>,
    live_start_log_time: Option<f64>,
    pending: Vec<Pending>,
    plot_pending: Vec<PlotPending>,
}

impl PhoenixSource {
    pub fn new(rng: &mut dyn Rng, clock: Rc<dyn Clock>) -> Self {
        Self {
            base: LiveDataSourceBase::new(rng),
            clock,
            get_devices_timers: Vec::new(),
            plot_timers: Vec::new(),
            device_descriptions: JsObject::new(),
            device_signals: JsObject::new(),
            live_start_real_time: Rc::new(Cell::new(None)),
            live_start_log_time: None,
            pending: Vec::new(),
            plot_pending: Vec::new(),
        }
    }

    pub fn connect(
        &mut self,
        address: &str,
        status_callback: StatusCallback,
        output_callback: OutputCallback,
        cx: &mut PhoenixContext<'_>,
    ) {
        self.base
            .connect(address, status_callback, output_callback, true, cx.timers);
        self.get_devices_periodic(cx);
        self.get_devices_timers
            .push(cx.timers.set_interval(GET_DEVICES_PERIOD_MS));
        self.plot_timers
            .push(cx.timers.set_interval(PLOT_PERIOD_MS));
    }

    pub fn stop(&mut self, cx: &mut PhoenixContext<'_>) {
        // `clearInterval(this.getDevicesInterval)`: only the latest interval; older ones a
        // repeated connect left behind keep running.
        if let Some(id) = self.get_devices_timers.last() {
            cx.timers.clear(*id);
        }
        if let Some(id) = self.plot_timers.last() {
            cx.timers.clear(*id);
        }
        self.base.stop(cx.timers);
    }

    /// Whether handling timer `id` reads [`PhoenixWindow`]'s active fields.
    pub fn reads_fields(&self, id: TimerId) -> bool {
        self.plot_timers.contains(&id)
    }

    pub fn base(&self) -> &LiveDataSourceBase {
        &self.base
    }

    /// Dispatches a fired timer: the getDevices and plot intervals, the per-device plot
    /// timeouts, each request's abort timer, then the base's discard interval.
    pub fn handle_timer(
        &mut self,
        id: TimerId,
        window: &PhoenixWindow,
        cx: &mut PhoenixContext<'_>,
    ) -> bool {
        if self.get_devices_timers.contains(&id) {
            self.get_devices_periodic(cx);
            return true;
        }
        if self.plot_timers.contains(&id) {
            let _ = self.plot_tick(window, cx);
            return true;
        }
        if let Some(i) = self.plot_pending.iter().position(|p| p.timer == id) {
            let pending = self.plot_pending.remove(i);
            self.plot_device(pending.device_name, pending.signals, cx);
            return true;
        }
        if let Some(i) = self.pending.iter().position(|p| p.abort == id) {
            let pending = self.pending.remove(i);
            cx.net.fetch_abort(pending.request);
            self.reject(pending.kind);
            return true;
        }
        self.base.handle_timer(id, window.preferences.as_ref())
    }

    /// A fetch settled: the response body (`response.json()`) or the network rejection.
    /// Requests that are not this source's are ignored.
    pub fn on_fetch_event(&mut self, event: &FetchEvent, cx: &mut PhoenixContext<'_>) {
        let Some(i) = self.pending.iter().position(|p| p.request == event.id()) else {
            return;
        };
        let pending = self.pending.remove(i);
        let FetchEvent::Response(_, response) = event else {
            self.reject(pending.kind);
            return;
        };
        let body = response.body.as_str();
        match pending.kind {
            PendingKind::Devices => {
                if self.devices_reply(body, cx).is_err() {
                    self.base.set_status(LiveDataSourceStatus::Connecting);
                }
            }
            PendingKind::Signals(device_name) => {
                let _ = self.signals_reply(body, &device_name);
            }
            PendingKind::Plot(device_name, signals) => {
                let _ = self.plot_reply(body, &device_name, &signals);
            }
        }
    }

    /// The request rejected: only getDevices has a `.catch`, which requeues with
    /// `Connecting`; the other failures are unhandled rejections.
    fn reject(&mut self, kind: PendingKind) {
        if matches!(kind, PendingKind::Devices) {
            self.base.set_status(LiveDataSourceStatus::Connecting);
        }
    }

    fn get_devices_periodic(&mut self, cx: &mut PhoenixContext<'_>) {
        let url = format!("http://{}:{PORT}/?action=getdevices", self.address());
        self.start_fetch(&url, GET_DEVICES_TIMEOUT_MS, PendingKind::Devices, cx);
    }

    /// The async function's synchronous prefix: the URL, then `AbortSignal.timeout` as the
    /// `fetch` options are evaluated, then `fetch`.
    fn start_fetch(
        &mut self,
        url: &str,
        timeout: f64,
        kind: PendingKind,
        cx: &mut PhoenixContext<'_>,
    ) {
        let abort = cx.timers.post_delayed_task(timeout);
        let request = cx.net.fetch(url);
        self.pending.push(Pending {
            request,
            abort,
            kind,
        });
    }

    /// `getDevices().then(devices => ...).catch(() => setStatus(Connecting))`, minus the
    /// catch the caller applies.
    fn devices_reply(&mut self, body: &str, cx: &mut PhoenixContext<'_>) -> Result<(), Thrown> {
        let json = json::parse_str(body).map_err(Thrown::from)?;
        check_error(&json)?;
        let devices = ops::get(&json, "DeviceArray")?;
        let initial_connection = self.base.status() != LiveDataSourceStatus::Active;
        if initial_connection {
            self.base
                .set_log(Some(Rc::new(RefCell::new(Log::new(true)))));
            self.device_descriptions = JsObject::new();
            self.device_signals = JsObject::new();
            self.live_start_real_time
                .set(Some(self.clock.now() / 1000.0));
            self.live_start_log_time = None;
        }
        self.base.set_status(LiveDataSourceStatus::Active);
        let JsValue::Array(items) = &devices else {
            return Err(Thrown::type_error("devices.forEach is not a function"));
        };
        for device in items {
            // Phoenix 5 devices use a different protocol that does not advertise signals.
            if !truthy(&ops::get(device, "IsPROApplication")?) {
                continue;
            }
            let device_name = self.device_name(device)?;
            if self.device_descriptions.get_key(&device_name).is_none() {
                self.device_descriptions
                    .insert(device_name.clone(), device.clone());
            }
            if self.device_signals.get_key(&device_name).is_none() {
                self.device_signals
                    .insert(device_name.clone(), JsValue::Array(Vec::new()));
            }
            if let Some(list) = self.device_signals.get_key(&device_name)
                && is_empty_list(list)?
            {
                // A rejected getSignals is an unhandled rejection: nothing happens.
                if let Ok(url) = self.signals_url(device) {
                    self.start_fetch(
                        &url,
                        GET_SIGNALS_TIMEOUT_MS,
                        PendingKind::Signals(device_name),
                        cx,
                    );
                }
            }
        }
        if initial_connection {
            self.new_output();
        }
        Ok(())
    }

    /// `getSignals(device).then(signals => ...)`; a rejection is unhandled, so errors end
    /// the continuation silently.
    fn signals_reply(&mut self, body: &str, device_name: &JsString) -> Result<(), Thrown> {
        let json = json::parse_str(body).map_err(Thrown::from)?;
        check_error(&json)?;
        let signals = ops::get(&json, "Signals")?;
        self.device_signals
            .insert(device_name.clone(), signals.clone());
        let JsValue::Array(items) = &signals else {
            return Err(Thrown::type_error("signals.forEach is not a function"));
        };
        for signal in items {
            let name = ops::get(signal, "Name")?;
            let name = ops::to_string(&name);
            let key = field_key(device_name, &name);
            let is_enum = enums::has(&name);
            if let Some(log) = self.base.log() {
                log.borrow_mut().create_blank_field(
                    &key,
                    if is_enum {
                        LoggableType::String
                    } else {
                        LoggableType::Number
                    },
                );
            }
            let units = ops::get(signal, "Units")?;
            if js_length(&units)? > 0.0 {
                let metadata = ops::object([
                    ("units", units),
                    ("description", ops::get(signal, "Summary")?),
                ]);
                if let Some(log) = self.base.log() {
                    log.borrow_mut().set_metadata_string(
                        &key,
                        JsString::from(
                            json::stringify(&metadata)
                                .map_err(Thrown::from)?
                                .unwrap_or_default(),
                        ),
                    );
                }
            }
        }
        self.new_output();
        Ok(())
    }

    /// The plot interval: build the per-device signal sets the tabs ask for, then schedule
    /// one `window.setTimeout` per device at `(PLOT_PERIOD / count) * index`.
    fn plot_tick(
        &mut self,
        window: &PhoenixWindow,
        cx: &mut PhoenixContext<'_>,
    ) -> Result<(), Thrown> {
        let mut active: JsObject<Vec<JsValue>> = JsObject::new();
        for field in &window.tabs_fields {
            if !field.starts_with(&js(PHOENIX_PREFIX), None) {
                continue;
            }
            let parts = field.split(Some(&js("/")), None);
            let (device_name, signal_name) = match parts.len() {
                3 => (parts[1].clone(), parts[2].clone()),
                4 => {
                    let mut device_name = parts[1].clone();
                    device_name.push_str(&js("/"));
                    device_name.push_str(&parts[2]);
                    (device_name, parts[3].clone())
                }
                _ => continue,
            };
            if enums::is_prototype_key(&device_name) {
                // Both `in` checks below see the inherited member, which has no `find`.
                return Err(Thrown::type_error(
                    "this.deviceSignals[deviceName].find is not a function",
                ));
            }
            if active.get_key(&device_name).is_none() {
                active.insert(device_name.clone(), Vec::new());
            }
            let Some(list) = self.device_signals.get_key(&device_name) else {
                continue;
            };
            let Some(signal) = find_named(list, &signal_name)?.cloned() else {
                continue;
            };
            let id = ops::get(&signal, "Id")?;
            if let Some((_, entry)) = active.iter_mut().find(|(key, _)| **key == device_name) {
                let mut duplicate = false;
                for previous in entry.iter() {
                    if ops::strict_equals(&ops::get(previous, "Id")?, &id) {
                        duplicate = true;
                        break;
                    }
                }
                if !duplicate {
                    entry.push(signal);
                }
            }
        }
        let device_count = active.len() as f64;
        for (index, (device_name, signals)) in active.iter().enumerate() {
            let timer = cx
                .timers
                .set_timeout(PLOT_PERIOD_MS / device_count * index as f64);
            self.plot_pending.push(PlotPending {
                timer,
                device_name: device_name.clone(),
                signals: Rc::new(signals.clone()),
            });
        }
        Ok(())
    }

    /// The `window.setTimeout` callback for one device: `getPlotData(device, ids).then(...)`.
    /// Errors leave a rejected promise nobody handles, so they simply drop the request.
    fn plot_device(
        &mut self,
        device_name: JsString,
        signals: Rc<Vec<JsValue>>,
        cx: &mut PhoenixContext<'_>,
    ) {
        let Some(device) = self.device_descriptions.get_key(&device_name) else {
            return;
        };
        let mut ids = Vec::with_capacity(signals.len());
        for signal in signals.iter() {
            // `signals.map(signal => signal.Id)`: a throwing property access escapes the
            // setTimeout callback, so the request never goes out.
            let Ok(id) = ops::get(signal, "Id") else {
                return;
            };
            ids.push(id);
        }
        let Ok(url) = self.plot_url(device, &ids) else {
            return;
        };
        self.start_fetch(
            &url,
            PLOT_TIMEOUT_MS,
            PendingKind::Plot(device_name, signals),
            cx,
        );
    }

    /// `getPlotData(device, ids).then(points => ...)`; a rejection is unhandled.
    fn plot_reply(
        &mut self,
        body: &str,
        device_name: &JsString,
        signals: &[JsValue],
    ) -> Result<(), Thrown> {
        let json = json::parse_str(body).map_err(Thrown::from)?;
        check_error(&json)?;
        let points = ops::get(&json, "Points")?;

        if self.live_start_log_time.is_none()
            && self.live_start_real_time.get().is_some()
            && js_length(&points)? > 0.0
        {
            let last = ops::index(&points, (js_length(&points)? - 1.0) as usize)?;
            let timestamp = ops::value_to_number(&ops::get(&last, "Timestamp")?)?;
            self.live_start_log_time = Some(
                timestamp - (self.clock.now() / 1000.0 - self.live_start_real_time.get().unwrap()),
            );
        }

        let JsValue::Array(items) = &points else {
            return Err(Thrown::type_error("points.forEach is not a function"));
        };
        for point in items {
            let point_signals = ops::get(point, "Signals")?;
            for (id_str, value) in entries(&point_signals)? {
                let signal_id = number::to_number(id_str.units());
                let mut found = None;
                for signal in signals {
                    if ops::strict_equals(&ops::get(signal, "Id")?, &JsValue::Number(signal_id)) {
                        found = Some(signal);
                        break;
                    }
                }
                let Some(signal) = found else {
                    continue;
                };
                let name = ops::to_string(&ops::get(signal, "Name")?);
                let key = field_key(device_name, &name);
                let timestamp = ops::value_to_number(&ops::get(point, "Timestamp")?)?
                    - self.live_start_log_time.unwrap_or(0.0);
                if enums::has(&name) {
                    let value_str = enums::lookup(&name, &ops::to_string(&value)).unwrap_or("");
                    if let Some(log) = self.base.log() {
                        log.borrow_mut()
                            .put_string(&key, timestamp, JsString::from(value_str));
                    }
                } else if let Some(log) = self.base.log() {
                    match value {
                        JsValue::Number(number) => {
                            log.borrow_mut().put_number(&key, timestamp, number)
                        }
                        JsValue::Null => {
                            log.borrow_mut()
                                .put_null(&key, timestamp, LoggableType::Number)
                        }
                        // Upstream stores any other JSON value in the Number field as is, which
                        // the `Log` cannot hold (#205).
                        _ => {}
                    }
                }
            }
        }
        self.new_output();
        Ok(())
    }

    /// `newOutput()`: the log goes through the output callback with the live supplier.
    fn new_output(&mut self) {
        if !self.base.has_output_callback()
            || self.base.log().is_none()
            || self.base.status() == LiveDataSourceStatus::Stopped
        {
            return;
        }
        if let Some(log) = self.base.log() {
            log.borrow_mut().clear_before_time(0.0, true);
        }
        let clock = Rc::clone(&self.clock);
        let live_start = Rc::clone(&self.live_start_real_time);
        let supplier: TimeSupplier = Rc::new(move || match live_start.get() {
            Some(start) => clock.now() / 1000.0 - start,
            None => 0.0,
        });
        self.base.output(supplier);
    }

    /// `getDeviceName(device)`.
    fn device_name(&self, device: &JsValue) -> Result<JsString, Thrown> {
        let model = ops::get(device, "Model")?;
        let JsValue::String(model) = &model else {
            return Err(Thrown::type_error(
                "device.Model.replaceAll is not a function",
            ));
        };
        let mut name = model.replace_all(&js(" "), &JsString::new())?;
        let name_lower = name.to_lower_case();
        for (prefix, replacement) in [
            ("cancoder", "CANcoder"),
            ("talonfxs", "TalonFXS"),
            ("talonfx", "TalonFX"),
            ("pigeon2", "Pigeon2"),
            // Upstream's check is "canrage": CANrange models never match.
            ("canrage", "CANrange"),
            ("candi", "CANdi"),
        ] {
            if name_lower.starts_with(&js(prefix), None) {
                name = js(replacement);
                break;
            }
        }
        let device_name = ops::get(device, "Name")?;
        let JsValue::String(device_name) = &device_name else {
            return Err(Thrown::type_error(
                "device.Name.startsWith is not a function",
            ));
        };
        name.push_str(&js("-"));
        if device_name.starts_with(model, None) {
            name.push_str(&to_string_method(&ops::get(device, "ID")?)?);
        } else {
            name.push_str(device_name);
        }
        let canivore = ops::get(device, "CANivoreDevName")?;
        if js_length(&canivore)? > 0.0 {
            let mut prefixed = js("CANivore-");
            prefixed.push_str(&ops::to_string(&canivore));
            prefixed.push_str(&js("/"));
            prefixed.push_str(&name);
            name = prefixed;
        }
        Ok(name)
    }

    /// The `getSignals` URL: `encodeURIComponent` on `Model` and `CANbus`, `toString` on `ID`.
    fn signals_url(&self, device: &JsValue) -> Result<String, Thrown> {
        Ok(format!(
            "http://{}:{PORT}/?action=getsignals&model={}&id={}&canbus={}",
            self.address(),
            encode(&ops::get(device, "Model")?)?,
            to_string_method(&ops::get(device, "ID")?)?.to_string_lossy(),
            encode(&ops::get(device, "CANbus")?)?,
        ))
    }

    /// The `getPlotData` URL: the same device parts plus the ids' `toString` joined on ","
    /// and the resolution.
    fn plot_url(&self, device: &JsValue, signal_ids: &[JsValue]) -> Result<String, Thrown> {
        let signals = signal_ids
            .iter()
            .map(|id| to_string_method(id).map(|s| s.to_string_lossy()))
            .collect::<Result<Vec<_>, _>>()?
            .join(",");
        Ok(format!(
            "http://{}:{PORT}/?action=plotpro&model={}&id={}&canbus={}&signals={signals}&resolution={PLOT_RESOLUTION_MS}",
            self.address(),
            encode(&ops::get(device, "Model")?)?,
            to_string_method(&ops::get(device, "ID")?)?.to_string_lossy(),
            encode(&ops::get(device, "CANbus")?)?,
        ))
    }

    fn address(&self) -> &str {
        self.base.address().unwrap_or_default()
    }
}

/// `PHOENIX_PREFIX + "/" + deviceName + "/" + signal.Name`.
fn field_key(device_name: &JsString, signal_name: &JsString) -> JsString {
    let mut key = js(PHOENIX_PREFIX);
    key.push_str(&js("/"));
    key.push_str(device_name);
    key.push_str(&js("/"));
    key.push_str(signal_name);
    key
}

/// `json.GeneralReturn.Error !== 0` throws.
fn check_error(json: &JsValue) -> Result<(), Thrown> {
    let error = ops::get(&ops::get(json, "GeneralReturn")?, "Error")?;
    if ops::strict_equals(&error, &JsValue::Number(0.0)) {
        Ok(())
    } else {
        Err(Thrown::new("Non-zero error code"))
    }
}

/// `Object.entries(value)`.
fn entries(value: &JsValue) -> Result<Vec<(JsString, JsValue)>, Thrown> {
    ops::object_keys(value)?
        .into_iter()
        .map(|key| Ok((key.clone(), ops::get_key(value, &key)?)))
        .collect()
}

/// `value.length` in a `> 0` comparison, with `undefined` comparing as `NaN`.
fn js_length(value: &JsValue) -> Result<f64, Thrown> {
    Ok(ops::value_to_number(&ops::get(value, "length")?)?)
}

/// `value.toString()`, which null and undefined lack.
fn to_string_method(value: &JsValue) -> Result<JsString, Thrown> {
    match value {
        JsValue::Null => Err(Thrown::type_error(
            "Cannot read properties of null (reading 'toString')",
        )),
        JsValue::Undefined => Err(Thrown::type_error(
            "Cannot read properties of undefined (reading 'toString')",
        )),
        other => Ok(ops::to_string(other)),
    }
}

/// `encodeURIComponent(ToString(value))`.
fn encode(value: &JsValue) -> Result<String, Thrown> {
    let encoded = encode_uri_component(&ops::to_string(value)).map_err(|e| e.into_error())?;
    Ok(encoded.to_string_lossy())
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
