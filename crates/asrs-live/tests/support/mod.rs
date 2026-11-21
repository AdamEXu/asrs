//! The client side of `oracle/src/live/loop.ts` and `net.ts`: one fake-clock loop running the
//! mock server, the session's client steps, deliveries and timers, and browser `WebSocket` and
//! `fetch` semantics over the in-memory network. Drivers stand in for upstream's sources.

#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use asrs_js::string::JsString;
use asrs_js::time::{Clock, V8MathRandom};
use asrs_live::nt4::{
    NetEvent, Nt4Client, Nt4Context, Nt4Handler, Nt4Transport, ReadyState, RequestId, SocketId,
    WsMessage,
};
use asrs_live::{ManualClock, Thrown, TimerId, TimerQueue};
use asrs_log::realm::JsRealm;
use asrs_parity::Value;
use asrs_parity::json::Json;
use asrs_parity::live::{ClientEvent, Payload, ServerEvent, ServerHost, Session, trace_line};
use asrs_parity::vectors;

#[path = "../../../asrs-cli/src/js_json.rs"]
#[allow(dead_code, clippy::all)]
pub mod js_json;

#[derive(Clone, Default)]
pub struct Trace(Rc<RefCell<Vec<String>>>);

impl Trace {
    pub fn push(&self, line: String) {
        self.0.borrow_mut().push(line);
    }

    pub fn app(&self, t: f64, ev: &str, v: Option<&Value>) {
        self.push(trace_line(t, ev, None, v));
    }

    pub fn lines(&self) -> Vec<String> {
        self.0.borrow().clone()
    }
}

/// `Date.now()`: the session epoch plus loop time.
#[derive(Clone)]
pub struct WallClock {
    pub loop_clock: ManualClock,
    pub epoch_ms: f64,
}

impl Clock for WallClock {
    fn now(&self) -> f64 {
        self.epoch_ms + self.loop_clock.now()
    }
}

#[derive(Clone, Debug)]
pub enum Delivery {
    WsAccepted(u32, String),
    WsFailed(u32),
    WsReceived(u32, WsMessage),
    WsDropped(u32),
    WsCloseDone(u32, bool),
    HttpSettled(u32, Option<f64>),
}

struct FakeSocket {
    ready: ReadyState,
    protocol: String,
}

struct FakeRequest {
    url: String,
    done: bool,
}

pub struct FakeNet {
    pub host: ServerHost,
    clock: ManualClock,
    latency_ms: f64,
    trace: Trace,
    next_id: u32,
    sockets: BTreeMap<u32, FakeSocket>,
    requests: BTreeMap<u32, FakeRequest>,
    tasks: Vec<(f64, u64, Delivery)>,
    seq: u64,
    microtasks: VecDeque<NetEvent>,
}

fn payload(data: &WsMessage) -> Payload {
    match data {
        WsMessage::Text(s) => Payload::Text(s.clone()),
        WsMessage::Binary(b) => Payload::Binary(b.clone()),
    }
}

/// `new URL(url).href`; only URLs that parse reach the server.
fn href(url: &str) -> String {
    url::Url::parse(url).expect("fetched URLs parse").into()
}

impl FakeNet {
    fn now(&self) -> f64 {
        self.clock.now()
    }

    fn send_to_server(&mut self, event: ClientEvent) {
        let now = self.now();
        self.trace.push(event.line(now));
        self.host.arrive(now, event);
    }

    pub fn deliver(&mut self, delay: f64, delivery: Delivery) {
        self.tasks.push((self.now() + delay, self.seq, delivery));
        self.seq += 1;
    }

    fn next_task_due(&self) -> Option<f64> {
        self.tasks.iter().map(|t| t.0).reduce(f64::min)
    }

    fn take_task(&mut self, now: f64) -> Option<Delivery> {
        let i = self
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.0 <= now)
            .min_by(|(_, a), (_, b)| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
            .map(|(i, _)| i)?;
        Some(self.tasks.remove(i).2)
    }

    fn server_events(&mut self, t: f64) -> Result<(), String> {
        for event in self.host.process(t)? {
            self.trace.push(event.line(t));
            let delivery = match event {
                ServerEvent::WsAccept { id, protocol } => Delivery::WsAccepted(id, protocol),
                ServerEvent::WsReject { id } => Delivery::WsFailed(id),
                ServerEvent::WsPush { id, data } => Delivery::WsReceived(
                    id,
                    match data {
                        Payload::Text(s) => WsMessage::Text(s),
                        Payload::Binary(b) => WsMessage::Binary(b),
                    },
                ),
                ServerEvent::WsDrop { id } => Delivery::WsDropped(id),
                ServerEvent::HttpReply { id, status, .. } => {
                    Delivery::HttpSettled(id, Some(status))
                }
                ServerEvent::HttpRefuse { id } => Delivery::HttpSettled(id, None),
                other => return Err(format!("unexpected server event {other:?}")),
            };
            self.deliver(self.latency_ms, delivery);
        }
        Ok(())
    }

    /// Applies a delivery to the browser state and returns the events it dispatches.
    pub fn apply(&mut self, delivery: Delivery) -> Vec<NetEvent> {
        match delivery {
            Delivery::WsAccepted(id, protocol) => match self.sockets.get_mut(&id) {
                Some(s) if s.ready == ReadyState::Connecting => {
                    s.ready = ReadyState::Open;
                    s.protocol = protocol.clone();
                    vec![NetEvent::WsOpen(SocketId(id), protocol)]
                }
                _ => Vec::new(),
            },
            Delivery::WsFailed(id) => match self.sockets.get_mut(&id) {
                Some(s) if s.ready != ReadyState::Closed => {
                    s.ready = ReadyState::Closed;
                    vec![
                        NetEvent::WsError(SocketId(id)),
                        NetEvent::WsClose(SocketId(id)),
                    ]
                }
                _ => Vec::new(),
            },
            Delivery::WsReceived(id, data) => match self.sockets.get(&id) {
                Some(s) if s.ready == ReadyState::Open => {
                    vec![NetEvent::WsMessage(SocketId(id), data)]
                }
                _ => Vec::new(),
            },
            Delivery::WsDropped(id) => match self.sockets.get_mut(&id) {
                Some(s) if s.ready != ReadyState::Closed => {
                    s.ready = ReadyState::Closed;
                    vec![NetEvent::WsClose(SocketId(id))]
                }
                _ => Vec::new(),
            },
            Delivery::WsCloseDone(id, was_connecting) => match self.sockets.get_mut(&id) {
                Some(s) if s.ready != ReadyState::Closed => {
                    s.ready = ReadyState::Closed;
                    if was_connecting {
                        vec![
                            NetEvent::WsError(SocketId(id)),
                            NetEvent::WsClose(SocketId(id)),
                        ]
                    } else {
                        vec![NetEvent::WsClose(SocketId(id))]
                    }
                }
                _ => Vec::new(),
            },
            Delivery::HttpSettled(id, status) => match self.requests.get_mut(&id) {
                Some(r) if !r.done => {
                    r.done = true;
                    match status {
                        Some(status) => {
                            vec![NetEvent::FetchResponse(
                                RequestId(id),
                                status as u16,
                                href(&r.url),
                            )]
                        }
                        None => vec![NetEvent::FetchFailed(RequestId(id))],
                    }
                }
                _ => Vec::new(),
            },
        }
    }
}

impl Nt4Transport for FakeNet {
    fn ws_open(&mut self, url: &str, protocols: &[&str]) -> Result<SocketId, Thrown> {
        let id = self.next_id;
        self.next_id += 1;
        self.sockets.insert(
            id,
            FakeSocket {
                ready: ReadyState::Connecting,
                protocol: String::new(),
            },
        );
        self.send_to_server(ClientEvent::WsOpen {
            id,
            url: url.to_owned(),
            protocols: protocols.iter().map(|p| (*p).to_owned()).collect(),
        });
        Ok(SocketId(id))
    }

    fn ws_ready_state(&self, id: SocketId) -> ReadyState {
        self.sockets
            .get(&id.0)
            .map_or(ReadyState::Closed, |s| s.ready)
    }

    fn ws_send(&mut self, id: SocketId, data: WsMessage) -> Result<(), Thrown> {
        match self.ws_ready_state(id) {
            ReadyState::Connecting => Err(Thrown::error(
                "InvalidStateError",
                "Still in CONNECTING state.",
            )),
            ReadyState::Open => {
                self.send_to_server(ClientEvent::WsSend {
                    id: id.0,
                    data: payload(&data),
                });
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn ws_close(&mut self, id: SocketId) {
        let Some(socket) = self.sockets.get_mut(&id.0) else {
            return;
        };
        if matches!(socket.ready, ReadyState::Closing | ReadyState::Closed) {
            return;
        }
        let was_connecting = socket.ready == ReadyState::Connecting;
        socket.ready = ReadyState::Closing;
        self.send_to_server(ClientEvent::WsClose { id: id.0 });
        self.deliver(
            2.0 * self.latency_ms,
            Delivery::WsCloseDone(id.0, was_connecting),
        );
    }

    fn fetch(&mut self, url: &str) -> RequestId {
        let id = self.next_id;
        self.next_id += 1;
        if url::Url::parse(url).is_err() {
            self.microtasks
                .push_back(NetEvent::FetchFailed(RequestId(id)));
            return RequestId(id);
        }
        self.requests.insert(
            id,
            FakeRequest {
                url: url.to_owned(),
                done: false,
            },
        );
        self.send_to_server(ClientEvent::HttpGet {
            id,
            url: url.to_owned(),
        });
        RequestId(id)
    }

    fn fetch_abort(&mut self, id: RequestId) {
        let Some(request) = self.requests.get_mut(&id.0) else {
            return;
        };
        if request.done {
            return;
        }
        request.done = true;
        self.send_to_server(ClientEvent::HttpAbort { id: id.0 });
    }
}

/// Everything a driver can reach: the renderer's timers, the browser network, `Date`, the
/// realm (`Math.random`, the msgpack key cache and the proto realm) and the trace.
pub struct Env {
    pub timers: TimerQueue,
    pub net: FakeNet,
    pub wall: WallClock,
    pub realm: JsRealm,
    pub trace: Trace,
}

impl Env {
    pub fn now(&self) -> f64 {
        self.wall.loop_clock.now()
    }

    pub fn cx(&mut self) -> Nt4Context<'_> {
        Nt4Context {
            timers: &mut self.timers,
            net: &mut self.net,
            clock: &self.wall,
            realm: &mut self.realm,
        }
    }

    pub fn app(&self, ev: &str, v: Option<&Value>) {
        self.trace.app(self.now(), ev, v);
    }
}

pub trait Driver {
    fn op(&mut self, op: &[Json], env: &mut Env) -> Result<(), Thrown>;
    fn notify(&mut self, event: NetEvent, env: &mut Env);
    fn timer(&mut self, id: TimerId, env: &mut Env);
    fn finish(&mut self, _env: &mut Env) {}
}

fn drain_microtasks(driver: &mut dyn Driver, env: &mut Env) {
    while let Some(event) = env.net.microtasks.pop_front() {
        driver.notify(event, env);
    }
}

/// Runs a session the way `LiveLoop.run` does and returns the trace.
pub fn run(
    session: &Session,
    make: impl FnOnce(&mut Env) -> Box<dyn Driver>,
) -> Result<Vec<String>, String> {
    let clock = ManualClock::new(0.0);
    let trace = Trace::default();
    let mut env = Env {
        timers: TimerQueue::new(Rc::new(clock.clone())),
        net: FakeNet {
            host: ServerHost::new(session)?,
            clock: clock.clone(),
            latency_ms: session.latency_ms,
            trace: trace.clone(),
            next_id: 1,
            sockets: BTreeMap::new(),
            requests: BTreeMap::new(),
            tasks: Vec::new(),
            seq: 0,
            microtasks: VecDeque::new(),
        },
        wall: WallClock {
            loop_clock: clock.clone(),
            epoch_ms: session.epoch_ms,
        },
        realm: JsRealm::new(Box::new(V8MathRandom::from_seed(session.seed))),
        trace: trace.clone(),
    };
    let mut driver = make(&mut env);
    let steps = &session.client_steps;
    let mut next_step = 0;
    loop {
        let candidates = [
            env.net.host.next_time(),
            steps.get(next_step).map(|s| s.t),
            env.net.next_task_due(),
            env.timers.next_due(),
        ];
        let Some(t) = candidates.into_iter().flatten().reduce(f64::min) else {
            break;
        };
        if t > session.end_ms {
            break;
        }
        if t > clock.now() {
            clock.set(t);
        }
        let now = clock.now();
        if env.net.host.next_time() == Some(now) {
            env.net.server_events(now)?;
        }
        while let Some(step) = steps.get(next_step).filter(|s| s.t <= now) {
            next_step += 1;
            if let Err(e) = driver.op(&step.op, &mut env) {
                env.app(
                    "throw",
                    Some(&Value::String(asrs_parity::Utf16String::from_units(
                        e.0.units().to_vec(),
                    ))),
                );
            }
            drain_microtasks(driver.as_mut(), &mut env);
        }
        loop {
            if let Some(delivery) = env.net.take_task(now) {
                for event in env.net.apply(delivery) {
                    driver.notify(event, &mut env);
                }
            } else if let Some(id) = env.timers.fire_next() {
                driver.timer(id, &mut env);
            } else {
                break;
            }
            drain_microtasks(driver.as_mut(), &mut env);
        }
    }
    if session.end_ms > clock.now() {
        clock.set(session.end_ms);
    }
    driver.finish(&mut env);
    Ok(trace.lines())
}

/// Loads the session's committed vector and checks `produced` against it, line for line.
pub fn check_trace(id: &str, produced: Vec<String>) -> Result<usize, String> {
    let expected: Vec<String> = vectors::load(id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|r| r.text)
        .collect();
    if let Some(i) =
        (0..expected.len().max(produced.len())).find(|&i| expected.get(i) != produced.get(i))
    {
        let context = |lines: &[String]| {
            lines[i.saturating_sub(3)..(i + 1).min(lines.len())]
                .iter()
                .map(|l| format!("    {}", &l[..l.len().min(400)]))
                .collect::<Vec<_>>()
                .join("\n")
        };
        return Err(format!(
            "{id}: line {i} differs\n  oracle:\n{}\n  port:\n{}",
            context(&expected),
            context(&produced)
        ));
    }
    Ok(produced.len())
}

pub fn js_arg(json: &Json) -> Result<asrs_js::value::JsValue, Thrown> {
    js_json::to_js(&Value::from_json_lenient(json)).map_err(Thrown::new)
}

pub fn js_value(value: &asrs_js::value::JsValue) -> Value {
    js_json::from_js(value)
}

/// `(window.log)[op[1]](...op.slice(2))`: the session's `log` op, a `Log` method call whose
/// first argument is the field key.
pub fn log_op(
    log: &mut asrs_log::log::Log,
    realm: &mut JsRealm,
    op: &[Json],
) -> Result<(), Thrown> {
    use asrs_log::log::LoggableType;
    use asrs_log::log::log_value_sets::RawValue;
    let method = op[1].as_str().expect("method").to_string_lossy();
    let key = JsString::from_units(op[2].as_str().expect("key").units().to_vec());
    let args: Vec<asrs_js::value::JsValue> =
        op[3..].iter().map(js_arg).collect::<Result<_, _>>()?;
    let num = |i: usize| args[i].as_f64().expect("number");
    match method.as_str() {
        "putNumber" => log.put_number(&key, num(0), num(1)),
        "putBoolean" => log.put_boolean(&key, num(0), args[1].as_bool().expect("bool")),
        "putString" => log.put_string(&key, num(0), args[1].as_str().expect("string").clone()),
        "putJSON" => log.put_json(&key, num(0), args[1].as_str().expect("string").clone()),
        "putRaw" => match &args[1] {
            asrs_js::value::JsValue::Bytes(b) => log.put_raw(&key, num(0), b.clone())?,
            other => panic!("putRaw {other:?}"),
        },
        "putMsgpack" => match &args[1] {
            asrs_js::value::JsValue::Bytes(b) => {
                log.put_msgpack(realm, &key, num(0), RawValue::from(b.clone()))?
            }
            other => panic!("putMsgpack {other:?}"),
        },
        "putStruct" => match &args[1] {
            asrs_js::value::JsValue::Bytes(b) => log.put_struct(
                &key,
                num(0),
                RawValue::from(b.clone()),
                args[2].as_str().expect("schema type"),
                args[3].as_bool().expect("is array"),
            )?,
            other => panic!("putStruct {other:?}"),
        },
        "putPhotonStruct" => match &args[1] {
            asrs_js::value::JsValue::Bytes(b) => log.put_photon_struct(
                &key,
                num(0),
                RawValue::from(b.clone()),
                args[2].as_str().expect("schema type"),
            )?,
            other => panic!("putPhotonStruct {other:?}"),
        },
        "putProto" => match &args[1] {
            asrs_js::value::JsValue::Bytes(b) => log.put_proto(
                realm,
                &key,
                num(0),
                RawValue::from(b.clone()),
                args[2].as_str().expect("schema type"),
            )?,
            other => panic!("putProto {other:?}"),
        },
        "putNumberArray" => log.put_number_array(
            &key,
            num(0),
            args[1]
                .as_array()
                .expect("array")
                .iter()
                .map(|v| v.as_f64().expect("number"))
                .collect(),
        ),
        "putBooleanArray" => log.put_boolean_array(
            &key,
            num(0),
            args[1]
                .as_array()
                .expect("array")
                .iter()
                .map(|v| v.as_bool().expect("bool"))
                .collect(),
        ),
        "putStringArray" => log.put_string_array(
            &key,
            num(0),
            args[1]
                .as_array()
                .expect("array")
                .iter()
                .map(|v| v.as_str().expect("string").clone())
                .collect(),
        ),
        "createBlankField" => {
            let ty = match num(0) as u8 {
                0 => LoggableType::Raw,
                1 => LoggableType::Boolean,
                2 => LoggableType::Number,
                3 => LoggableType::String,
                4 => LoggableType::BooleanArray,
                5 => LoggableType::NumberArray,
                6 => LoggableType::StringArray,
                _ => LoggableType::Empty,
            };
            log.create_blank_field(&key, ty);
        }
        "setWpilibType" => {
            log.set_wpilib_type(&key, args[0].as_str().expect("type").clone());
        }
        "setStructuredType" => log.set_structured_type(&key, args[0].as_str().cloned()),
        "setMetadataString" => {
            log.set_metadata_string(&key, args[0].as_str().expect("metadata").clone());
        }
        "setGeneratedParent" => log.set_generated_parent(&key),
        "clearBeforeTime" => log.clear_before_time(num(0), true),
        other => panic!("the driver does not model Log.{other}"),
    }
    Ok(())
}

/// `dump(log)` from `oracle/src/logRun.ts`: `toSerialized()` spelled out as the trace's
/// dump payload.
pub fn dump(log: &asrs_log::log::Log) -> Value {
    let serialized = log.to_serialized();
    Value::object([
        ("keys", strings(serialized.fields.keys())),
        (
            "fields",
            Value::Array(
                serialized
                    .fields
                    .values()
                    .map(serialized_field_value)
                    .collect(),
            ),
        ),
        (
            "generatedParents",
            strings(serialized.generated_parents.iter()),
        ),
        (
            "timestampRange",
            serialized
                .timestamp_range
                .map_or(Value::Null, |r| numbers(&r)),
        ),
        (
            "timestampZero",
            serialized.timestamp_zero.map_or(Value::Null, Value::number),
        ),
    ])
}

fn string_value(s: &JsString) -> Value {
    Value::String(asrs_parity::Utf16String::from_units(s.units().to_vec()))
}

fn optional_string(s: &Option<JsString>) -> Value {
    s.as_ref().map_or(Value::Null, string_value)
}

fn numbers(xs: &[f64]) -> Value {
    Value::Array(xs.iter().map(|&x| Value::number(x)).collect())
}

fn strings<'a>(items: impl Iterator<Item = &'a JsString>) -> Value {
    Value::Array(items.map(string_value).collect())
}

fn column<T, F>(values: &asrs_log::log::LogValues, item: F) -> Value
where
    T: asrs_log::log::LogValue,
    F: Fn(&T) -> Value,
{
    Value::Array(
        values
            .as_slice::<T>()
            .unwrap_or_default()
            .iter()
            .map(item)
            .collect(),
    )
}

fn serialized_field_value(s: &asrs_log::log::SerializedLogField) -> Value {
    use asrs_log::log::LogValues;
    let values = match &s.values {
        LogValues::Raw(_) => {
            column::<std::sync::Arc<[u8]>, _>(&s.values, |v| Value::Bytes(v.to_vec()))
        }
        LogValues::Boolean(_) => column::<bool, _>(&s.values, |v| Value::Bool(*v)),
        LogValues::Number(_) => column::<f64, _>(&s.values, |v| Value::number(*v)),
        LogValues::String(_) => column::<JsString, _>(&s.values, string_value),
        LogValues::BooleanArray(_) => column::<Vec<bool>, _>(&s.values, |v| {
            Value::Array(v.iter().map(|b| Value::Bool(*b)).collect())
        }),
        LogValues::NumberArray(_) => column::<Vec<f64>, _>(&s.values, |v| numbers(v)),
        LogValues::StringArray(_) => column::<Vec<JsString>, _>(&s.values, |v| {
            Value::Array(v.iter().map(string_value).collect())
        }),
        LogValues::Empty(_) => column::<(), _>(&s.values, |_| Value::Undefined),
    };
    Value::object([
        ("type", Value::number(f64::from(s.get_type() as u8))),
        ("timestamps", numbers(&s.timestamps)),
        ("values", values),
        ("structuredType", optional_string(&s.structured_type)),
        ("wpilibType", optional_string(&s.wpilib_type)),
        ("metadataString", string_value(&s.metadata_string)),
        ("stripingReference", Value::Bool(s.striping_reference)),
        ("typeWarning", Value::Bool(s.type_warning)),
    ])
}

/// Hands a browser event to the client's listener; exceptions escaping it are dropped.
pub fn dispatch(
    client: &mut Nt4Client,
    event: &NetEvent,
    h: &mut dyn Nt4Handler,
    cx: &mut Nt4Context<'_>,
) {
    let _ = client.handle_net_event(event, h, cx);
}
