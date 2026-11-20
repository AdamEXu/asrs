//! Port of `NT4_Client` from `src/hub/dataSources/nt4/NT4.ts`.
//!
//! The client is sans-IO. Upstream's event listeners, timer callbacks and `fetch` continuations
//! become `on_*` and [`Nt4Client::handle_timer`] calls, made by whoever owns the transport and
//! the timer queue; upstream's constructor callbacks become an [`Nt4Handler`] passed to each
//! call that can reach them. Every call that upstream reaches through an event listener or a
//! timer returns the exception that would have escaped it, which the caller drops as a browser
//! does.

use asrs_js::json;
use asrs_js::math;
use asrs_js::number;
use asrs_js::string::JsString;
use asrs_js::time::{Clock, Rng};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::msgpack::{self, KeyCache, MultiDecoder};

use super::js;
use super::transport::{NetEvent, Nt4Transport, ReadyState, RequestId, SocketId, WsMessage};
use crate::thrown::Thrown;
use crate::timers::{TimerId, TimerQueue};

pub const NT4_PORTS_DEFAULT: &[u16] = &[5810];
pub const NT4_PORTS_DS: &[u16] = &[6767];
pub const NT4_PORTS_SYSTEMCORE: &[u16] = &[6810];

pub const PROTOCOL_V41: &str = "v4.1.networktables.first.wpi.edu";
pub const PROTOCOL_V40: &str = "networktables.first.wpi.edu";
pub const PROTOCOL_RTT: &str = "rtt.networktables.first.wpi.edu";

const RTT_PERIOD_MS_V40: f64 = 1000.0;
const RTT_PERIOD_MS_V41: f64 = 250.0;
const TIMEOUT_MS_V40: f64 = 5000.0;
const TIMEOUT_MS_V41: f64 = 1000.0;
const ALIVE_TIMEOUT_MS: f64 = 250.0;
const ALIVE_RETRY_MS: f64 = 350.0;

const TYPESTR_IDX_LOOKUP: [(&str, f64); 15] = [
    ("boolean", 0.0),
    ("double", 1.0),
    ("int", 2.0),
    ("float", 3.0),
    ("string", 4.0),
    ("json", 4.0),
    ("raw", 5.0),
    ("rpc", 5.0),
    ("msgpack", 5.0),
    ("protobuf", 5.0),
    ("boolean[]", 16.0),
    ("double[]", 17.0),
    ("int[]", 18.0),
    ("float[]", 19.0),
    ("string[]", 20.0),
];

/// `Object.prototype`'s own properties, which `type in typestrIdxLookup` also finds.
const OBJECT_PROTOTYPE_METHODS: [&str; 11] = [
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "toLocaleString",
];

/// `NT4_Topic`. Server topics hold whatever the announcement carried, so every field is a JS
/// value; published topics always have a string name and type, a numeric uid and an object of
/// properties.
#[derive(Clone, Debug, PartialEq)]
pub struct Nt4Topic {
    pub uid: JsValue,
    pub name: JsValue,
    pub ty: JsValue,
    pub properties: JsValue,
}

impl Nt4Topic {
    pub fn name_str(&self) -> Option<&JsString> {
        self.name.as_str()
    }

    pub fn type_str(&self) -> Option<&JsString> {
        self.ty.as_str()
    }

    /// `getTypeIdx()`. `in` also sees `Object.prototype`, so a type named after one of its
    /// methods indexes a function, which the encoder rejects, and `__proto__` indexes the
    /// prototype itself, which encodes as an empty map.
    fn type_idx(&self) -> Result<JsValue, Thrown> {
        let Some(ty) = self.ty.as_str() else {
            return Ok(JsValue::Number(5.0));
        };
        let ty = ty.to_string_lossy();
        if let Some((_, idx)) = TYPESTR_IDX_LOOKUP.iter().find(|(name, _)| *name == ty) {
            Ok(JsValue::Number(*idx))
        } else if ty == "__proto__" {
            Ok(JsValue::Object(JsObject::new()))
        } else if OBJECT_PROTOTYPE_METHODS.contains(&ty.as_str()) {
            Err(Thrown::error(
                "Error",
                "Unrecognized object: [object Function]",
            ))
        } else {
            Ok(JsValue::Number(5.0))
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Nt4SubscriptionOptions {
    pub periodic: f64,
    pub all: bool,
    pub topics_only: bool,
    pub prefix: bool,
}

impl Default for Nt4SubscriptionOptions {
    fn default() -> Self {
        Self {
            periodic: 0.1,
            all: false,
            topics_only: false,
            prefix: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Nt4Subscription {
    pub uid: f64,
    pub topics: Vec<JsString>,
    pub options: Nt4SubscriptionOptions,
}

impl Nt4Subscription {
    fn new(uid: f64, patterns: &[JsString]) -> Self {
        let mut topics: Vec<JsString> = Vec::new();
        for pattern in patterns {
            if !topics.contains(pattern) {
                topics.push(pattern.clone());
            }
        }
        Self {
            uid,
            topics,
            options: Nt4SubscriptionOptions::default(),
        }
    }

    fn to_subscribe_obj(&self) -> JsValue {
        js::object([
            (
                "topics",
                JsValue::Array(self.topics.iter().cloned().map(JsValue::String).collect()),
            ),
            ("subuid", JsValue::Number(self.uid)),
            (
                "options",
                js::object([
                    ("periodic", JsValue::Number(self.options.periodic)),
                    ("all", JsValue::Bool(self.options.all)),
                    ("topicsonly", JsValue::Bool(self.options.topics_only)),
                    ("prefix", JsValue::Bool(self.options.prefix)),
                ]),
            ),
        ])
    }
}

/// What the client may touch besides itself. `clock` is `Date`; `rng` and `keys` are the
/// realm's `Math.random` and msgpack key cache, which the client shares with everything else
/// in the hub renderer.
pub struct Nt4Context<'a> {
    pub timers: &'a mut TimerQueue,
    pub net: &'a mut dyn Nt4Transport,
    pub clock: &'a dyn Clock,
    pub rng: &'a mut dyn Rng,
    pub keys: &'a mut KeyCache,
}

/// The constructor's callbacks. An error is an exception escaping the callback, which
/// propagates through the client exactly as upstream's would.
pub trait Nt4Handler {
    fn on_topic_announce(
        &mut self,
        _topic: &Nt4Topic,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        Ok(())
    }

    fn on_topic_unannounce(
        &mut self,
        _topic: &Nt4Topic,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        Ok(())
    }

    fn on_topic_properties_update(
        &mut self,
        _topic: &Nt4Topic,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        Ok(())
    }

    fn on_new_topic_data(
        &mut self,
        _topic: &Nt4Topic,
        _timestamp_us: f64,
        _value: JsValue,
        _cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        Ok(())
    }

    fn on_connect(&mut self, _cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        Ok(())
    }

    fn on_disconnect(&mut self, _cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        Ok(())
    }
}

/// A handler that ignores everything, as `NT4Publisher` passes for the topic callbacks.
pub struct NoopHandler;

impl Nt4Handler for NoopHandler {}

/// One `connectOnAlive` round: `Promise.any` over a request per port.
struct AliveCheck {
    id: u32,
    start: f64,
    outstanding: usize,
    settled: bool,
}

struct PendingRequest {
    id: RequestId,
    check: u32,
}

pub struct Nt4Client {
    server_addr: String,
    server_ports: Vec<u16>,
    app_name: String,
    active_server_port: Option<f64>,
    ws: Option<SocketId>,
    rtt_ws: Option<SocketId>,
    sockets: Vec<(SocketId, bool)>,
    timestamp_interval: TimerId,
    rtt_ws_timestamp_interval: TimerId,
    disconnect_timeout: Option<TimerId>,
    server_connection_active: bool,
    server_connection_requested: bool,
    server_time_offset_us: Option<f64>,
    network_latency_us: f64,

    subscriptions: Vec<Nt4Subscription>,
    published_topics: Vec<Nt4Topic>,
    server_topics: Vec<(JsValue, Nt4Topic)>,

    alive_checks: Vec<AliveCheck>,
    next_alive_check: u32,
    requests: Vec<PendingRequest>,
    abort_timers: Vec<(TimerId, RequestId)>,
    retry_timers: Vec<TimerId>,
}

fn same_name(topic: &Nt4Topic, name: &JsString) -> bool {
    topic.name.as_str() == Some(name)
}

impl Nt4Client {
    /// `new NT4_Client(...)`: creates both time sync intervals, which run until `disconnect`.
    pub fn new(
        server_addr: &str,
        server_ports: &[u16],
        app_name: &str,
        timers: &mut TimerQueue,
    ) -> Self {
        let timestamp_interval = timers.set_interval(RTT_PERIOD_MS_V40);
        let rtt_ws_timestamp_interval = timers.set_interval(RTT_PERIOD_MS_V41);
        Self {
            server_addr: server_addr.to_owned(),
            server_ports: server_ports.to_vec(),
            app_name: app_name.to_owned(),
            active_server_port: None,
            ws: None,
            rtt_ws: None,
            sockets: Vec::new(),
            timestamp_interval,
            rtt_ws_timestamp_interval,
            disconnect_timeout: None,
            server_connection_active: false,
            server_connection_requested: false,
            server_time_offset_us: None,
            network_latency_us: 0.0,
            subscriptions: Vec::new(),
            published_topics: Vec::new(),
            server_topics: Vec::new(),
            alive_checks: Vec::new(),
            next_alive_check: 0,
            requests: Vec::new(),
            abort_timers: Vec::new(),
            retry_timers: Vec::new(),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.server_connection_active
    }

    pub fn server_topics(&self) -> impl Iterator<Item = &Nt4Topic> {
        self.server_topics.iter().map(|(_, t)| t)
    }

    pub fn published_topic(&self, name: &JsString) -> Option<&Nt4Topic> {
        self.published_topics.iter().find(|t| same_name(t, name))
    }

    pub fn subscriptions(&self) -> &[Nt4Subscription] {
        &self.subscriptions
    }

    // Upstream's `connectOnAlive` is async and nothing awaits it, so whatever it throws is an
    // unhandled rejection: it never reaches a caller.
    fn connect_on_alive(&mut self, cx: &mut Nt4Context<'_>) {
        if !self.server_connection_requested {
            return;
        }
        let start = cx.clock.now();
        let check = self.next_alive_check;
        self.next_alive_check += 1;
        for port in self.server_ports.clone() {
            let timer = cx.timers.set_timeout(ALIVE_TIMEOUT_MS);
            let url = format!("http://{}:{port}", self.server_addr);
            let request = cx.net.fetch(&url);
            self.abort_timers.push((timer, request));
            self.requests.push(PendingRequest { id: request, check });
        }
        self.alive_checks.push(AliveCheck {
            id: check,
            start,
            outstanding: self.server_ports.len(),
            settled: false,
        });
        if self.server_ports.is_empty() {
            self.alive_check_failed(check, cx);
        }
    }

    fn alive_check_failed(&mut self, check: u32, cx: &mut Nt4Context<'_>) {
        let Some(c) = self.alive_checks.iter_mut().find(|c| c.id == check) else {
            return;
        };
        c.settled = true;
        let request_length = cx.clock.now() - c.start;
        self.retry_timers
            .push(cx.timers.set_timeout(ALIVE_RETRY_MS - request_length));
        self.prune_alive_checks();
    }

    fn prune_alive_checks(&mut self) {
        self.alive_checks
            .retain(|c| !(c.settled && c.outstanding == 0));
    }

    /// Takes a request off the pending list; `None` if it already settled or was aborted.
    fn settle_request(&mut self, id: RequestId) -> Option<u32> {
        let i = self.requests.iter().position(|r| r.id == id)?;
        let check = self.requests.remove(i).check;
        if let Some(c) = self.alive_checks.iter_mut().find(|c| c.id == check) {
            c.outstanding -= 1;
        }
        Some(check)
    }

    fn request_rejected(&mut self, id: RequestId, cx: &mut Nt4Context<'_>) {
        let Some(check) = self.settle_request(id) else {
            return;
        };
        if let Some(c) = self.alive_checks.iter().find(|c| c.id == check)
            && !c.settled
            && c.outstanding == 0
        {
            self.alive_check_failed(check, cx);
        }
        self.prune_alive_checks();
    }

    /// The `fetch` promise fulfilled with a response. `url` is `response.url`.
    pub fn on_fetch_response(
        &mut self,
        id: RequestId,
        status: f64,
        url: &str,
        cx: &mut Nt4Context<'_>,
    ) {
        let Some(check) = self.settle_request(id) else {
            return;
        };
        let Some(c) = self.alive_checks.iter_mut().find(|c| c.id == check) else {
            return;
        };
        if c.settled {
            self.prune_alive_checks();
            return;
        }
        c.settled = true;
        if !(200.0..=299.0).contains(&status) {
            self.alive_check_failed(check, cx);
            return;
        }
        self.prune_alive_checks();
        let Some(port) = parse_port(url) else {
            return;
        };
        self.active_server_port = Some(port);
        let _ = self.ws_connect(false, cx);
    }

    /// The `fetch` promise rejected (network error).
    pub fn on_fetch_failed(&mut self, id: RequestId, cx: &mut Nt4Context<'_>) {
        self.request_rejected(id, cx);
    }

    /// `connect()`: starts the connection; the client reconnects on its own until
    /// `disconnect()`.
    pub fn connect(&mut self, cx: &mut Nt4Context<'_>) {
        if !self.server_connection_requested {
            self.server_connection_requested = true;
            self.connect_on_alive(cx);
        }
    }

    /// `disconnect()`. The time sync intervals are not restarted by a later `connect()`.
    pub fn disconnect(
        &mut self,
        handler: &mut dyn Nt4Handler,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        if self.server_connection_requested {
            self.server_connection_requested = false;
            if self.server_connection_active
                && let Some(ws) = self.ws
            {
                self.ws_on_close(ws, handler, cx)?;
            }
            cx.timers.clear(self.timestamp_interval);
            cx.timers.clear(self.rtt_ws_timestamp_interval);
            if let Some(timeout) = self.disconnect_timeout {
                cx.timers.clear(timeout);
            }
        }
        Ok(())
    }

    fn new_uid(cx: &mut Nt4Context<'_>) -> f64 {
        math::floor(cx.rng.next_f64() * 99999999.0)
    }

    fn insert_subscription(&mut self, sub: Nt4Subscription) {
        match self.subscriptions.iter_mut().find(|s| s.uid == sub.uid) {
            Some(existing) => *existing = sub,
            None => self.subscriptions.push(sub),
        }
    }

    /// `subscribe(topicPatterns, prefixMode, sendAll, periodic)`; returns the subscription id.
    pub fn subscribe(
        &mut self,
        topic_patterns: &[JsString],
        prefix_mode: bool,
        send_all: bool,
        periodic: f64,
        cx: &mut Nt4Context<'_>,
    ) -> Result<f64, Thrown> {
        let mut sub = Nt4Subscription::new(Self::new_uid(cx), topic_patterns);
        sub.options.prefix = prefix_mode;
        sub.options.all = send_all;
        sub.options.periodic = periodic;
        let uid = sub.uid;
        let obj = sub.to_subscribe_obj();
        self.insert_subscription(sub);
        if self.server_connection_active {
            self.ws_send_json("subscribe", obj, cx)?;
        }
        Ok(uid)
    }

    /// `subscribeTopicsOnly(topicPatterns, prefixMode)`.
    pub fn subscribe_topics_only(
        &mut self,
        topic_patterns: &[JsString],
        prefix_mode: bool,
        cx: &mut Nt4Context<'_>,
    ) -> Result<f64, Thrown> {
        let mut sub = Nt4Subscription::new(Self::new_uid(cx), topic_patterns);
        sub.options.prefix = prefix_mode;
        sub.options.topics_only = true;
        let uid = sub.uid;
        let obj = sub.to_subscribe_obj();
        self.insert_subscription(sub);
        if self.server_connection_active {
            self.ws_send_json("subscribe", obj, cx)?;
        }
        Ok(uid)
    }

    /// `unsubscribe(subscriptionId)`; throws a string for an unknown id.
    pub fn unsubscribe(
        &mut self,
        subscription_id: f64,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let Some(i) = self
            .subscriptions
            .iter()
            .position(|s| s.uid == subscription_id || (s.uid.is_nan() && subscription_id.is_nan()))
        else {
            return Err(Thrown::new(format!(
                "Unknown subscription ID \"{}\"",
                number::to_string(subscription_id)
            )));
        };
        let sub = self.subscriptions.remove(i);
        if self.server_connection_active {
            self.ws_send_json(
                "unsubscribe",
                js::object([("subuid", JsValue::Number(sub.uid))]),
                cx,
            )?;
        }
        Ok(())
    }

    pub fn clear_all_subscriptions(&mut self, cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        let ids: Vec<f64> = self.subscriptions.iter().map(|s| s.uid).collect();
        for id in ids {
            self.unsubscribe(id, cx)?;
        }
        Ok(())
    }

    /// `setProperties(topic, properties)`: a `null` value deletes the property.
    pub fn set_properties(
        &mut self,
        topic: &JsString,
        properties: &JsObject,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let update = |target: &mut JsValue| -> Result<(), Thrown> {
            for (key, value) in properties.iter() {
                if value.is_null() {
                    js::delete_key(target, key)?;
                } else {
                    js::set_key(target, key, value.clone())?;
                }
            }
            Ok(())
        };
        if let Some(published) = self
            .published_topics
            .iter_mut()
            .find(|t| same_name(t, topic))
        {
            update(&mut published.properties)?;
        }
        let key = JsValue::String(topic.clone());
        if let Some((_, server)) = self
            .server_topics
            .iter_mut()
            .find(|(k, _)| js::same_value_zero(k, &key))
        {
            update(&mut server.properties)?;
        }
        if self.server_connection_active {
            self.ws_send_json(
                "setproperties",
                js::object([
                    ("name", JsValue::String(topic.clone())),
                    ("update", JsValue::Object(properties.clone())),
                ]),
                cx,
            )?;
        }
        Ok(())
    }

    pub fn set_persistent(
        &mut self,
        topic: &JsString,
        is_persistent: bool,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let mut properties = JsObject::new();
        properties.insert("persistent", JsValue::Bool(is_persistent));
        self.set_properties(topic, &properties, cx)
    }

    pub fn set_retained(
        &mut self,
        topic: &JsString,
        is_retained: bool,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let mut properties = JsObject::new();
        properties.insert("retained", JsValue::Bool(is_retained));
        self.set_properties(topic, &properties, cx)
    }

    /// `publishTopic(topic, type)`; does nothing if the topic is already published.
    pub fn publish_topic(
        &mut self,
        topic: &JsString,
        ty: &JsString,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        if self.published_topic(topic).is_some() {
            return Ok(());
        }
        let new_topic = Nt4Topic {
            uid: JsValue::Number(Self::new_uid(cx)),
            name: JsValue::String(topic.clone()),
            ty: JsValue::String(ty.clone()),
            properties: JsValue::Object(JsObject::new()),
        };
        let obj = publish_obj(&new_topic);
        self.published_topics.push(new_topic);
        if self.server_connection_active {
            self.ws_send_json("publish", obj, cx)?;
        }
        Ok(())
    }

    /// `unpublishTopic(topic)`; throws a string for a topic that is not published.
    pub fn unpublish_topic(
        &mut self,
        topic: &JsString,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let Some(i) = self
            .published_topics
            .iter()
            .position(|t| same_name(t, topic))
        else {
            return Err(not_found(topic));
        };
        let removed = self.published_topics.remove(i);
        if self.server_connection_active {
            self.ws_send_json("unpublish", js::object([("pubuid", removed.uid)]), cx)?;
        }
        Ok(())
    }

    /// `addSample(topic, value)`, stamped with the server time (0 before the first sync).
    pub fn add_sample(
        &mut self,
        topic: &JsString,
        value: JsValue,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let timestamp = self.server_time_us(cx.clock, None).unwrap_or(0.0);
        self.add_timestamped_sample(topic, timestamp, value, cx)
    }

    /// `addTimestampedSample(topic, timestamp, value)`. Safe integers, the timestamp included,
    /// go out as the smallest msgpack int; other numbers as float64.
    pub fn add_timestamped_sample(
        &mut self,
        topic: &JsString,
        timestamp: f64,
        value: JsValue,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let Some(topic_obj) = self.published_topic(topic) else {
            return Err(not_found(topic));
        };
        let tx_data = msgpack::encode(&JsValue::Array(vec![
            topic_obj.uid.clone(),
            JsValue::Number(timestamp),
            topic_obj.type_idx()?,
            value,
        ]))?;
        self.ws_send_binary(tx_data, false, cx)
    }

    /// `getClientTime_us()`: `Date` milliseconds times 1000.
    pub fn client_time_us(clock: &dyn Clock) -> f64 {
        clock.now() * 1000.0
    }

    /// `getServerTime_us(clientTime?)`; `None` until the first time sync reply.
    pub fn server_time_us(&self, clock: &dyn Clock, client_time: Option<f64>) -> Option<f64> {
        let offset = self.server_time_offset_us?;
        Some(client_time.unwrap_or_else(|| Self::client_time_us(clock)) + offset)
    }

    /// `getNetworkLatency_us()`.
    pub fn network_latency_us(&self) -> f64 {
        self.network_latency_us
    }

    fn ws_send_timestamp(&mut self, cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        let time_to_send = Self::client_time_us(cx.clock);
        let tx_data = msgpack::encode(&JsValue::Array(vec![
            JsValue::Number(-1.0),
            JsValue::Number(0.0),
            JsValue::Number(2.0),
            JsValue::Number(time_to_send),
        ]))?;
        self.ws_send_binary(tx_data, self.rtt_ws.is_some(), cx)
    }

    fn ws_handle_receive_timestamp(
        &mut self,
        server_timestamp: f64,
        client_timestamp: &JsValue,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let rx_time = Self::client_time_us(cx.clock);
        let rtt = rx_time - js::value_to_number(client_timestamp)?;
        self.network_latency_us = rtt / 2.0;
        let server_time_at_rx = server_timestamp + self.network_latency_us;
        self.server_time_offset_us = Some(server_time_at_rx - rx_time);
        Ok(())
    }

    fn ws_send_json(
        &mut self,
        method: &str,
        params: JsValue,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        if let Some(ws) = self.ws
            && cx.net.ws_ready_state(ws) == ReadyState::Open
        {
            let message = JsValue::Array(vec![js::object([
                ("method", JsValue::string(method)),
                ("params", params),
            ])]);
            let text = json::stringify(&message)?.unwrap_or_default();
            cx.net.ws_send(ws, WsMessage::Text(text))?;
        }
        Ok(())
    }

    fn ws_send_binary(
        &mut self,
        data: Vec<u8>,
        rtt_ws: bool,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let ws = if rtt_ws { self.rtt_ws } else { self.ws };
        if let Some(ws) = ws
            && cx.net.ws_ready_state(ws) == ReadyState::Open
        {
            cx.net.ws_send(ws, WsMessage::Binary(data))?;
        }
        Ok(())
    }

    fn is_rtt_socket(&self, id: SocketId) -> Option<bool> {
        self.sockets
            .iter()
            .find(|(s, _)| *s == id)
            .map(|(_, rtt)| *rtt)
    }

    /// The socket's `open` event; `protocol` is `ws.protocol`.
    pub fn on_ws_open(
        &mut self,
        id: SocketId,
        protocol: &str,
        handler: &mut dyn Nt4Handler,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        if self.is_rtt_socket(id).is_none() {
            return Ok(());
        }
        self.server_connection_active = true;
        if protocol == PROTOCOL_V41 {
            self.ws_connect(true, cx)?;
        } else {
            self.ws_send_timestamp(cx)?;
        }
        if protocol != PROTOCOL_RTT {
            for i in 0..self.published_topics.len() {
                let obj = publish_obj(&self.published_topics[i]);
                self.ws_send_json("publish", obj, cx)?;
            }
            for i in 0..self.subscriptions.len() {
                let obj = self.subscriptions[i].to_subscribe_obj();
                self.ws_send_json("subscribe", obj, cx)?;
            }
            handler.on_connect(cx)?;
        }
        Ok(())
    }

    /// The socket's `message` event.
    pub fn on_ws_message(
        &mut self,
        id: SocketId,
        data: &WsMessage,
        handler: &mut dyn Nt4Handler,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        if self.is_rtt_socket(id).is_none() {
            return Ok(());
        }
        self.ws_reset_timeout(cx);
        match data {
            WsMessage::Text(text) => {
                let JsValue::Array(messages) = json::parse_str(text)? else {
                    return Ok(());
                };
                for message in messages {
                    self.handle_text_message(message, handler, cx)?;
                }
                Ok(())
            }
            WsMessage::Binary(bytes) => self.handle_binary(bytes, handler, cx),
        }
    }

    fn handle_text_message(
        &mut self,
        msg: JsValue,
        handler: &mut dyn Nt4Handler,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        if msg.type_of() != "object" {
            return Ok(());
        }
        if !js::has(&msg, "method")? || !js::has(&msg, "params")? {
            return Ok(());
        }
        let method = js::get(&msg, "method")?;
        let params = js::get(&msg, "params")?;
        let Some(method) = method.as_str() else {
            return Ok(());
        };
        if params.type_of() != "object" {
            return Ok(());
        }
        match method.to_string_lossy().as_str() {
            "announce" => {
                let uid = js::get(&params, "id")?;
                let name = js::get(&params, "name")?;
                let ty = js::get(&params, "type")?;
                let properties = js::get(&params, "properties")?;
                let topic = Nt4Topic {
                    uid,
                    name: name.clone(),
                    ty,
                    properties,
                };
                let i = match self
                    .server_topics
                    .iter()
                    .position(|(k, _)| js::same_value_zero(k, &name))
                {
                    Some(i) => {
                        self.server_topics[i].1 = topic;
                        i
                    }
                    None => {
                        self.server_topics.push((name, topic));
                        self.server_topics.len() - 1
                    }
                };
                handler.on_topic_announce(&self.server_topics[i].1, cx)
            }
            "unannounce" => {
                let name = js::get(&params, "name")?;
                let Some(i) = self
                    .server_topics
                    .iter()
                    .position(|(k, _)| js::same_value_zero(k, &name))
                else {
                    return Ok(());
                };
                let (_, removed) = self.server_topics.remove(i);
                handler.on_topic_unannounce(&removed, cx)
            }
            "properties" => {
                let name = js::get(&params, "name")?;
                let Some(i) = self
                    .server_topics
                    .iter()
                    .position(|(k, _)| js::same_value_zero(k, &name))
                else {
                    return Ok(());
                };
                let update = js::get(&params, "update")?;
                for key in js::object_keys(&update)? {
                    let value = js::get_key(&update, &key)?;
                    let properties = &mut self.server_topics[i].1.properties;
                    if value.is_null() {
                        js::delete_key(properties, &key)?;
                    } else {
                        js::set_key(properties, &key, value)?;
                    }
                }
                handler.on_topic_properties_update(&self.server_topics[i].1, cx)
            }
            _ => Ok(()),
        }
    }

    fn handle_binary(
        &mut self,
        bytes: &[u8],
        handler: &mut dyn Nt4Handler,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let mut multi = MultiDecoder::new(bytes);
        while let Some(unpacked) = multi.next(cx.keys, cx.rng) {
            let unpacked = unpacked?;
            let topic_id = js::index(&unpacked, 0)?;
            let timestamp_us = js::index(&unpacked, 1)?;
            let type_idx = js::index(&unpacked, 2)?;
            let value = js::index(&unpacked, 3)?;
            let (Some(topic_id), Some(timestamp_us), Some(_)) =
                (topic_id.as_f64(), timestamp_us.as_f64(), type_idx.as_f64())
            else {
                return Ok(());
            };
            if topic_id >= 0.0 {
                let id = JsValue::Number(topic_id);
                let Some((_, topic)) = self
                    .server_topics
                    .iter()
                    .find(|(_, t)| js::strict_equals(&t.uid, &id))
                else {
                    return Ok(());
                };
                handler.on_new_topic_data(topic, timestamp_us, value, cx)?;
            } else if topic_id == -1.0 {
                self.ws_handle_receive_timestamp(timestamp_us, &value, cx)?;
            }
        }
        Ok(())
    }

    /// The socket's `error` event. The RTT socket has no listener.
    pub fn on_ws_error(&mut self, id: SocketId, cx: &mut Nt4Context<'_>) {
        if self.is_rtt_socket(id) != Some(false) || self.ws != Some(id) {
            return;
        }
        self.close_sockets(cx);
    }

    /// The socket's `close` event. The RTT socket has no listener.
    pub fn on_ws_close(
        &mut self,
        id: SocketId,
        handler: &mut dyn Nt4Handler,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        let rtt = self.is_rtt_socket(id);
        self.sockets.retain(|(s, _)| *s != id);
        if rtt != Some(false) {
            return Ok(());
        }
        self.ws_on_close(id, handler, cx)
    }

    fn close_sockets(&mut self, cx: &mut Nt4Context<'_>) {
        if let Some(ws) = self.ws {
            cx.net.ws_close(ws);
        }
        if let Some(rtt) = self.rtt_ws {
            cx.net.ws_close(rtt);
        }
    }

    fn ws_on_close(
        &mut self,
        source: SocketId,
        handler: &mut dyn Nt4Handler,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        if self.ws != Some(source) {
            return Ok(());
        }
        self.close_sockets(cx);
        self.ws = None;
        self.rtt_ws = None;
        if let Some(timeout) = self.disconnect_timeout {
            cx.timers.clear(timeout);
        }
        if self.server_connection_active {
            handler.on_disconnect(cx)?;
            self.server_connection_active = false;
        }
        self.server_topics.clear();
        if self.server_connection_requested {
            self.connect_on_alive(cx);
        }
        Ok(())
    }

    fn ws_reset_timeout(&mut self, cx: &mut Nt4Context<'_>) {
        if let Some(timeout) = self.disconnect_timeout {
            cx.timers.clear(timeout);
        }
        let timeout = if self.rtt_ws.is_none() {
            TIMEOUT_MS_V40
        } else {
            TIMEOUT_MS_V41
        };
        self.disconnect_timeout = Some(cx.timers.set_timeout(timeout));
    }

    fn ws_connect(&mut self, rtt_ws: bool, cx: &mut Nt4Context<'_>) -> Result<(), Thrown> {
        let Some(port) = self.active_server_port else {
            return Ok(());
        };
        let url = format!(
            "ws://{}:{}/nt/{}",
            self.server_addr,
            number::to_string(port),
            self.app_name
        );
        let protocols: &[&str] = if rtt_ws {
            &[PROTOCOL_RTT]
        } else {
            &[PROTOCOL_V41, PROTOCOL_V40]
        };
        let ws = cx.net.ws_open(&url, protocols)?;
        if rtt_ws {
            self.rtt_ws = Some(ws);
        } else {
            self.ws = Some(ws);
        }
        self.sockets.push((ws, rtt_ws));
        Ok(())
    }

    /// Hands a browser event to the listener upstream registers for it. An exception escaping
    /// the listener comes back as the error; a browser reports it and carries on.
    pub fn handle_net_event(
        &mut self,
        event: &NetEvent,
        handler: &mut dyn Nt4Handler,
        cx: &mut Nt4Context<'_>,
    ) -> Result<(), Thrown> {
        match event {
            NetEvent::WsOpen(id, protocol) => self.on_ws_open(*id, protocol, handler, cx),
            NetEvent::WsMessage(id, data) => self.on_ws_message(*id, data, handler, cx),
            NetEvent::WsError(id) => {
                self.on_ws_error(*id, cx);
                Ok(())
            }
            NetEvent::WsClose(id) => self.on_ws_close(*id, handler, cx),
            NetEvent::FetchResponse(id, status, url) => {
                self.on_fetch_response(*id, f64::from(*status), url, cx);
                Ok(())
            }
            NetEvent::FetchFailed(id) => {
                self.on_fetch_failed(*id, cx);
                Ok(())
            }
        }
    }

    /// Runs a fired timer if it is one of the client's. Returns `None` for any other id.
    pub fn handle_timer(
        &mut self,
        id: TimerId,
        handler: &mut dyn Nt4Handler,
        cx: &mut Nt4Context<'_>,
    ) -> Option<Result<(), Thrown>> {
        if id == self.timestamp_interval {
            return Some(if self.rtt_ws.is_none() {
                self.ws_send_timestamp(cx)
            } else {
                Ok(())
            });
        }
        if id == self.rtt_ws_timestamp_interval {
            return Some(if self.rtt_ws.is_some() {
                self.ws_send_timestamp(cx)
            } else {
                Ok(())
            });
        }
        if self.disconnect_timeout == Some(id) {
            return Some(match self.ws {
                Some(ws) => self.ws_on_close(ws, handler, cx),
                None => Ok(()),
            });
        }
        if let Some(i) = self.abort_timers.iter().position(|(t, _)| *t == id) {
            let (_, request) = self.abort_timers.remove(i);
            if self.requests.iter().any(|r| r.id == request) {
                cx.net.fetch_abort(request);
                self.request_rejected(request, cx);
            }
            return Some(Ok(()));
        }
        if let Some(i) = self.retry_timers.iter().position(|t| *t == id) {
            self.retry_timers.remove(i);
            self.connect_on_alive(cx);
            return Some(Ok(()));
        }
        None
    }
}

fn publish_obj(topic: &Nt4Topic) -> JsValue {
    js::object([
        ("name", topic.name.clone()),
        ("type", topic.ty.clone()),
        ("pubuid", topic.uid.clone()),
        ("properties", topic.properties.clone()),
    ])
}

fn not_found(topic: &JsString) -> Thrown {
    let mut text = JsString::from("Topic \"");
    text.push_str(topic);
    text.push_str(&JsString::from("\" not found"));
    Thrown(text)
}

/// `parseInt(url.split(":")[2].split("/")[0])`; `None` where upstream throws a TypeError.
fn parse_port(url: &str) -> Option<f64> {
    let url = JsString::from(url);
    let parts = url.split(Some(&JsString::from(":")), None);
    let host_port = parts.get(2)?;
    let port = host_port.split(Some(&JsString::from("/")), None);
    Some(number::parse_int(port[0].units(), 0.0))
}
