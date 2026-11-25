//! Port of `src/hub/dataSources/ftcdashboard/FTCDashboardSource.ts`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use asrs_js::json;
use asrs_js::number;
use asrs_js::ops;
use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::{Log, LoggableType};
use asrs_log::units;

use super::config;
use crate::live_data_source::{
    LiveDataSourceBase, LiveDataSourceStatus, LiveLog, LivePreferences, OutputCallback,
    StatusCallback,
};
use crate::live_data_tuner::TunedValue;
use crate::nt4::{NetEvent, Nt4Transport, ReadyState, SocketId, WsMessage};
use crate::thrown::Thrown;
use crate::timers::{TimerId, TimerQueue};

const FTC_DASHBOARD_PORT: u16 = 8000;
const FTC_DASHBOARD_PING_MS: f64 = 1000.0;

fn js(text: &str) -> JsString {
    JsString::from(text)
}

/// `BasicVarType`: the config var types `parseConfigState` registers as tunable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BasicVarType {
    String,
    Boolean,
    Int,
    Long,
    Float,
    Double,
}

impl BasicVarType {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "string" => BasicVarType::String,
            "boolean" => BasicVarType::Boolean,
            "int" => BasicVarType::Int,
            "long" => BasicVarType::Long,
            "float" => BasicVarType::Float,
            "double" => BasicVarType::Double,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            BasicVarType::String => "string",
            BasicVarType::Boolean => "boolean",
            BasicVarType::Int => "int",
            BasicVarType::Long => "long",
            BasicVarType::Float => "float",
            BasicVarType::Double => "double",
        }
    }
}

/// The environment the source's callbacks run against: the hub's timer queue, the websocket
/// transport and `Date`.
pub struct FtcContext<'a> {
    pub timers: &'a mut TimerQueue,
    pub net: &'a mut dyn Nt4Transport,
    pub clock: &'a dyn Clock,
}

pub struct FtcSource {
    base: LiveDataSourceBase,
    clock: Rc<dyn Clock>,
    url: String,
    socket: Option<SocketId>,
    ping_timers: Vec<TimerId>,
    live_zero_time: Rc<Cell<f64>>,
    has_log: Rc<Cell<bool>>,
    robot_clock_skew: f64,
    running: bool,
    config_state: JsValue,
    is_initial_state: bool,
    tunable_keys_types: HashMap<JsString, BasicVarType>,
}

impl FtcSource {
    pub fn new(rng: &mut dyn Rng, clock: Rc<dyn Clock>) -> Self {
        FtcSource {
            base: LiveDataSourceBase::new(rng),
            clock,
            url: String::new(),
            socket: None,
            ping_timers: Vec::new(),
            live_zero_time: Rc::new(Cell::new(0.0)),
            has_log: Rc::new(Cell::new(false)),
            robot_clock_skew: 0.0,
            running: false,
            config_state: config::initial_state(),
            is_initial_state: true,
            tunable_keys_types: HashMap::new(),
        }
    }

    pub fn base(&self) -> &LiveDataSourceBase {
        &self.base
    }

    pub fn connect(
        &mut self,
        address: &str,
        status_callback: StatusCallback,
        output_callback: OutputCallback,
        preferences: Option<&LivePreferences>,
        cx: &mut FtcContext<'_>,
    ) {
        self.base
            .connect(address, status_callback, output_callback, true, cx.timers);
        if preferences.is_none() {
            self.base.set_status(LiveDataSourceStatus::Error);
        } else {
            if let Some(socket) = self.socket {
                cx.net.ws_close(socket);
            }
            self.url = format!("ws://{address}:{FTC_DASHBOARD_PORT}");
            self.ping_timers
                .push(cx.timers.set_interval(FTC_DASHBOARD_PING_MS));
        }
    }

    /// The interval body upstream registers with `setInterval`; the interval is never cleared,
    /// so every reconnect leaks another one.
    fn ping(&mut self, cx: &mut FtcContext<'_>) {
        let closed = match self.socket {
            None => true,
            Some(id) => cx.net.ws_ready_state(id) == ReadyState::Closed,
        };
        if closed {
            self.base.set_status(LiveDataSourceStatus::Connecting);
            self.base
                .set_log(Some(Rc::new(RefCell::new(Log::new(true)))));
            self.has_log.set(true);
            self.live_zero_time.set(0.0);
            self.robot_clock_skew = 0.0;
            self.config_state = config::initial_state();
            self.is_initial_state = true;
            self.tunable_keys_types.clear();
            // An address the WebSocket constructor throws on leaves the old socket in place.
            if let Ok(id) = cx.net.ws_open(&self.url, &[]) {
                self.socket = Some(id);
            }
        } else if let Some(id) = self.socket
            && cx.net.ws_ready_state(id) == ReadyState::Open
        {
            let status = WsMessage::Text("{\"type\":\"GET_ROBOT_STATUS\"}".to_owned());
            let _ = cx.net.ws_send(id, status);
        }
    }

    pub fn handle_timer(
        &mut self,
        id: TimerId,
        preferences: Option<&LivePreferences>,
        cx: &mut FtcContext<'_>,
    ) -> bool {
        if self.ping_timers.contains(&id) {
            self.ping(cx);
            return true;
        }
        self.base.handle_timer(id, preferences)
    }

    pub fn handle_net_event(&mut self, event: &NetEvent, cx: &mut FtcContext<'_>) {
        match *event {
            NetEvent::WsOpen(id, _) if Some(id) == self.socket => {
                self.base.set_status(LiveDataSourceStatus::Active);
            }
            NetEvent::WsMessage(id, ref data) if Some(id) == self.socket => {
                let _ = self.decode_data(data, cx);
            }
            NetEvent::WsClose(id) if Some(id) == self.socket => {
                self.base.set_status(LiveDataSourceStatus::Connecting);
            }
            _ => {}
        }
    }

    pub fn stop(&mut self, cx: &mut FtcContext<'_>) {
        self.base.stop(cx.timers);
    }

    fn decode_data(&mut self, data: &WsMessage, cx: &mut FtcContext<'_>) -> Result<(), Thrown> {
        let Some(log_rc) = self.base.log().cloned() else {
            return Ok(());
        };
        if self.base.status() == LiveDataSourceStatus::Stopped {
            return Ok(());
        }
        if self.live_zero_time.get() == 0.0 {
            self.live_zero_time.set(cx.clock.now() / 1000.0);
        }
        self.base.set_status(LiveDataSourceStatus::Active);

        let decoded = match data {
            WsMessage::Text(text) => json::parse_str(text).ok(),
            WsMessage::Binary(_) => None,
        };
        let decoded = decoded.filter(|value| !value.is_null());

        if let Some(decoded) = decoded {
            let mut timestamp = cx.clock.now() / 1000.0 - self.live_zero_time.get();
            if ops::has_own(&decoded, &js("configRoot"))? {
                let (state, changed) = config::config_reducer(&self.config_state, &decoded)?;
                self.config_state = state;
                self.is_initial_state &= !changed;
                let root = ops::get(&self.config_state, "configRoot")?;
                self.parse_config_state(&root, timestamp, &js("/_Config"), &log_rc)?;
            }
            if ops::has_own(&decoded, &js("telemetry"))? {
                let packets = ops::get(&decoded, "telemetry")?;
                for packet in for_of(&packets)? {
                    if ops::has_own(&packet, &js("timestamp"))? {
                        let mut packet_timestamp =
                            ops::value_to_number(&ops::get(&packet, "timestamp")?)? / 1000.0
                                - self.live_zero_time.get();
                        if self.robot_clock_skew == 0.0 {
                            self.robot_clock_skew = timestamp - packet_timestamp;
                        }
                        packet_timestamp += self.robot_clock_skew;
                        timestamp = packet_timestamp;
                    }
                    if ops::has_own(&packet, &js("data"))? {
                        if !self.running {
                            self.running = true;
                            log_rc
                                .borrow_mut()
                                .put_boolean(&js("RUNNING"), timestamp, true)?;
                        }
                        self.log_object(
                            &ops::get(&packet, "data")?,
                            timestamp,
                            &JsString::new(),
                            &log_rc,
                        )?;
                    }
                }
            }
            if ops::has_own(&decoded, &js("status"))? {
                let status = ops::get(&decoded, "status")?;
                self.log_object(&status, timestamp, &js("/_Status/"), &log_rc)?;
                if ops::has_own(&status, &js("activeOpMode"))?
                    && ops::has_own(&status, &js("activeOpModeStatus"))?
                {
                    let op_mode = ops::get(&status, "activeOpMode")?;
                    let op_mode_status = ops::get(&status, "activeOpModeStatus")?;
                    self.running = !ops::loose_equals(&op_mode, &JsValue::string("$Stop$Robot$"))
                        && ops::loose_equals(&op_mode_status, &JsValue::string("RUNNING"));
                    log_rc
                        .borrow_mut()
                        .put_boolean(&js("RUNNING"), timestamp, self.running)?;
                }
            }
        }

        let zero = self.live_zero_time.clone();
        let has_log = self.has_log.clone();
        let clock = self.clock.clone();
        self.base.output(Rc::new(move || {
            if has_log.get() {
                clock.now() / 1000.0 - zero.get()
            } else {
                0.0
            }
        }));
        Ok(())
    }

    fn parse_config_state(
        &mut self,
        state: &JsValue,
        timestamp: f64,
        path: &JsString,
        log_rc: &LiveLog,
    ) -> Result<(), Thrown> {
        let Some(type_name) = ops::get(state, "__type")?.into_string() else {
            return Ok(());
        };
        let type_name = type_name.to_string_lossy();
        if type_name == "custom" {
            let value = ops::get(state, "__value")?;
            if !value.is_null() {
                for key in ops::object_keys(&value)? {
                    let mut child = path.clone();
                    child.push_str(&js("/"));
                    child.push_str(&key);
                    self.parse_config_state(
                        &ops::get_key(&value, &key)?,
                        timestamp,
                        &child,
                        log_rc,
                    )?;
                }
            }
            return Ok(());
        }
        let Some(var_type) = BasicVarType::parse(&type_name) else {
            return Ok(());
        };
        let value = ops::get(state, "__value")?;
        {
            let mut log = log_rc.borrow_mut();
            match var_type {
                BasicVarType::Boolean => {
                    log.put_untyped(path, timestamp, LoggableType::Boolean, value)?;
                }
                BasicVarType::String => {
                    log.put_untyped(path, timestamp, LoggableType::String, value)?;
                }
                _ => log.put_number(path, timestamp, to_number(&value))?,
            }
        }
        self.tunable_keys_types.insert(path.clone(), var_type);
        Ok(())
    }

    fn log_object(
        &self,
        object: &JsValue,
        timestamp: f64,
        path: &JsString,
        log_rc: &LiveLog,
    ) -> Result<(), Thrown> {
        let mut log = log_rc.borrow_mut();
        let mut found_poses: Vec<(JsString, ([f64; 2], f64))> = Vec::new();
        for name in ops::own_property_names(object)? {
            let data = ops::get_key(object, &name)?;
            let mut key = path.clone();
            key.push_str(&name);

            let label = pose_label(&name);
            let mut set_pose = |make: &mut dyn FnMut(Option<[f64; 2]>, f64) -> ([f64; 2], f64)| {
                if let Some(entry) = found_poses.iter_mut().find(|(l, _)| *l == label) {
                    entry.1 = make(Some(entry.1.0), entry.1.1);
                } else {
                    found_poses.push((label.clone(), make(None, 0.0)));
                }
            };

            // Upstream uses the units table on first sight of " x" but divides by an inline
            // 39.37008 on repeats; the two factors are not exactly equal.
            if name.ends_with(&js(" x"), None) || name == js("x") {
                let inches = to_number(&data);
                set_pose(&mut |cur, rotation| match cur {
                    Some(translation) => ([inches / 39.37008, translation[1]], rotation),
                    None => (
                        [
                            units::convert(inches, "inches", "meters").unwrap_or(f64::NAN),
                            0.0,
                        ],
                        0.0,
                    ),
                });
                continue;
            }
            if name.ends_with(&js(" y"), None) || name == js("y") {
                let meters =
                    units::convert(to_number(&data), "inches", "meters").unwrap_or(f64::NAN);
                set_pose(&mut |cur, rotation| match cur {
                    Some(translation) => ([translation[0], meters], rotation),
                    None => ([0.0, meters], 0.0),
                });
                continue;
            }
            if name.ends_with(&js(" heading"), None) || name == js("heading") {
                let heading = to_number(&data);
                set_pose(&mut |cur, _| (cur.unwrap_or([0.0, 0.0]), heading));
                continue;
            }
            if name.ends_with(&js(" heading (deg)"), None) || name == js("heading (deg)") {
                let radians =
                    units::convert(to_number(&data), "degrees", "radians").unwrap_or(f64::NAN);
                set_pose(&mut |cur, _| (cur.unwrap_or([0.0, 0.0]), radians));
                continue;
            }

            match &data {
                JsValue::String(text) => {
                    let num = number::to_number(text.units());
                    if !num.is_nan() && !number::parse_float(text.units()).is_nan() {
                        log.put_number(&key, timestamp, num)?;
                    } else {
                        log.put_string(&key, timestamp, text.clone())?;
                    }
                }
                JsValue::Bool(value) => log.put_boolean(&key, timestamp, *value)?,
                JsValue::Number(value) => log.put_number(&key, timestamp, *value)?,
                JsValue::BigInt(value) => log.put_number(&key, timestamp, value.to_f64())?,
                _ => {}
            }
        }
        for (label, (translation, rotation)) in found_poses {
            log.put_pose(&label, timestamp, translation, rotation)?;
        }
        Ok(())
    }

    pub fn has_tunable_fields(&self) -> bool {
        !self.is_initial_state
    }

    pub fn is_tunable(&self, key: &JsString) -> bool {
        self.tunable_keys_types.contains_key(key)
    }

    pub fn publish(
        &mut self,
        key: &JsString,
        value: TunedValue,
        cx: &mut FtcContext<'_>,
    ) -> Result<(), Thrown> {
        let Some(&tunable_type) = self.tunable_keys_types.get(key) else {
            return Ok(());
        };
        let parts = key.split(Some(&js("/")), None);
        let in_config = parts.first().is_some_and(|part| part.is_empty())
            && parts.get(1).is_some_and(|part| *part == js("_Config"));
        if !in_config {
            return Ok(());
        }
        let diff = config_diff(&parts[2..], value, tunable_type);
        let action = JsValue::Object(JsObject::from_iter([
            (JsString::from("type"), JsValue::string("SAVE_CONFIG")),
            (JsString::from("configDiff"), diff),
        ]));
        let message = json::stringify(&action)
            .map_err(|e| Thrown::error("SyntaxError", e))?
            .unwrap_or_default();
        match self.socket {
            Some(socket) => cx.net.ws_send(socket, WsMessage::Text(message)),
            None => Err(Thrown::type_error(
                "Cannot read properties of null (reading 'send')",
            )),
        }
    }

    pub fn unpublish(&mut self, _key: &JsString) {}
}

fn config_diff(path: &[JsString], value: TunedValue, tunable_type: BasicVarType) -> JsValue {
    if let Some((first, rest)) = path.split_first() {
        let mut inner = JsObject::new();
        inner.insert(first.clone(), config_diff(rest, value, tunable_type));
        JsValue::Object(JsObject::from_iter([
            (JsString::from("__type"), JsValue::string("custom")),
            (JsString::from("__value"), JsValue::Object(inner)),
        ]))
    } else {
        JsValue::Object(JsObject::from_iter([
            (
                JsString::from("__type"),
                JsValue::string(tunable_type.name()),
            ),
            (
                JsString::from("__value"),
                match value {
                    TunedValue::Number(n) => JsValue::Number(n),
                    TunedValue::Boolean(b) => JsValue::Bool(b),
                },
            ),
        ]))
    }
}

/// Upstream drops the last word of the field name (`"... x"` -> `"..."`), or the last two for
/// `" heading (deg)"` names, then appends `" Pose"`.
fn pose_label(name: &JsString) -> JsString {
    let deg = name.ends_with(&js(" heading (deg)"), None) || *name == js("heading (deg)");
    let parts = name.split(Some(&js(" ")), None);
    let keep = parts.len().saturating_sub(if deg { 2 } else { 1 });
    let mut label = JsString::new();
    for (index, part) in parts.iter().take(keep).enumerate() {
        if index > 0 {
            label.push_str(&js(" "));
        }
        label.push_str(part);
    }
    label.push_str(&js(" Pose"));
    label
}

/// `for (const item of value)`: arrays and strings are iterable upstream; anything else throws.
fn for_of(value: &JsValue) -> Result<Vec<JsValue>, Thrown> {
    match value {
        JsValue::Array(items) => Ok(items.clone()),
        JsValue::String(text) => Ok(code_points(text.units())
            .map(JsString::from_units)
            .map(JsValue::String)
            .collect()),
        JsValue::Bytes(bytes) => Ok(bytes
            .iter()
            .map(|b| JsValue::Number(f64::from(*b)))
            .collect()),
        other => Err(Thrown::type_error(format!(
            "{} is not iterable",
            ops::to_string(other).to_string_lossy()
        ))),
    }
}

/// `Number(value)` as upstream calls it: bigints reach `to_f64`, anything else goes through
/// ToNumber and can only fail on unrepresentable symbols.
fn to_number(value: &JsValue) -> f64 {
    match value {
        JsValue::BigInt(b) => b.to_f64(),
        other => ops::value_to_number(other).unwrap_or(f64::NAN),
    }
}

fn code_points(units: &[u16]) -> impl Iterator<Item = Vec<u16>> + '_ {
    char::decode_utf16(units.iter().copied()).map(|item| match item {
        Ok(c) => c.encode_utf16(&mut [0; 2]).to_vec(),
        Err(e) => vec![e.unpaired_surrogate()],
    })
}
