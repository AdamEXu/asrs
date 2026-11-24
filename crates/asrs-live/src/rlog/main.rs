//! The `live-rlog-start`/`live-rlog-stop` arms of `handleHubMessage` in
//! `src/main/electron/main.ts` (the same code `oracle/src/live/rlogMain.ts` transcribes):
//! one TCP socket per hub window, framing the stream as `[int32 BE length][payload]`, a 3 s
//! socket idle timeout reported with a `status` key (never `success`), a 3 s no-data watchdog
//! that destroys the socket, and the global 500 ms heartbeat written to every open socket.

use std::collections::{HashMap, VecDeque};

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};

use crate::thrown::Thrown;
use crate::timers::{TimerId, TimerQueue};

use super::transport::{RlogNetEvent, RlogTransport, TcpId};

pub const RLOG_CONNECT_TIMEOUT_MS: f64 = 3000.0;
pub const RLOG_DATA_TIMEOUT_MS: f64 = 3000.0;
pub const RLOG_HEARTBEAT_DELAY_MS: f64 = 500.0;
pub const RLOG_HEARTBEAT_DATA: [u8; 4] = [6, 3, 2, 8];

fn js(s: &str) -> JsString {
    JsString::from(s)
}

/// `sendMessage(window, "live-data", data)`: an object the renderer gets in a later task,
/// keys in the order upstream builds them.
pub enum MainMessage {
    /// `{uuid, success: true, raw}` — one framed payload.
    Data { uuid: String, raw: Vec<u8> },
    /// `{uuid, success: false}` — socket `error` or `close`.
    Failed { uuid: String },
    /// `{uuid, status: false}` — the connect idle timeout: the message carries `status`
    /// instead of `success`, so the renderer's `data.success` check reads `undefined`.
    IdleTimeout { uuid: String },
}

impl MainMessage {
    /// The IPC object as the renderer's `handleMainMessage` receives it.
    pub fn into_value(self) -> JsValue {
        match self {
            Self::Data { uuid, raw } => JsValue::Object(JsObject::from_iter([
                (js("uuid"), JsValue::String(JsString::from(uuid))),
                (js("success"), JsValue::Bool(true)),
                (js("raw"), JsValue::Bytes(raw)),
            ])),
            Self::Failed { uuid } => JsValue::Object(JsObject::from_iter([
                (js("uuid"), JsValue::String(JsString::from(uuid))),
                (js("success"), JsValue::Bool(false)),
            ])),
            Self::IdleTimeout { uuid } => JsValue::Object(JsObject::from_iter([
                (js("uuid"), JsValue::String(JsString::from(uuid))),
                (js("status"), JsValue::Bool(false)),
            ])),
        }
    }
}

/// A `live-rlog-*` message from the renderer.
#[derive(Clone, Debug, PartialEq)]
pub enum MainCall {
    /// `"live-rlog-start", {uuid, address, port}`.
    Start {
        uuid: String,
        address: String,
        port: f64,
    },
    /// `"live-rlog-stop"`.
    Stop,
}

impl MainCall {
    pub const START: &str = "live-rlog-start";
    pub const STOP: &str = "live-rlog-stop";

    pub fn name(&self) -> &'static str {
        match self {
            Self::Start { .. } => Self::START,
            Self::Stop => Self::STOP,
        }
    }

    /// The message's data, keys in the order `RLOGServerSource` builds them.
    pub fn into_data(self) -> JsValue {
        match self {
            Self::Start {
                uuid,
                address,
                port,
            } => JsValue::Object(JsObject::from_iter([
                (js("uuid"), JsValue::String(JsString::from(uuid))),
                (js("address"), JsValue::String(JsString::from(address))),
                (js("port"), JsValue::Number(port)),
            ])),
            Self::Stop => JsValue::Undefined,
        }
    }

    /// The call main receives as `name` with `data`.
    pub fn parse(name: &str, data: &JsValue) -> Option<Self> {
        let text = |key| {
            let value = data.get(key).and_then(JsValue::as_str);
            value.map(JsString::to_string_lossy)
        };
        match name {
            Self::START => Some(Self::Start {
                uuid: text("uuid")?,
                address: text("address")?,
                port: data.get("port").and_then(JsValue::as_f64)?,
            }),
            Self::STOP => Some(Self::Stop),
            _ => None,
        }
    }
}

/// What `handle` may touch: the network, and the main process's timer queue for the data
/// watchdog.
pub struct MainContext<'a> {
    pub timers: &'a mut TimerQueue,
    pub net: &'a mut dyn RlogTransport,
}

struct Socket {
    uuid: String,
    open: bool,
    /// `socket.setTimeout(ms, callback)`'s timer, until it fires or the socket closes. The
    /// callback is a `once` listener, so a second fire would do nothing.
    idle: Option<TimerId>,
}

/// `RLOGMain` for one hub window (upstream keys its maps by `windowId`; the emulation's one
/// window is id 1, so the fields are the maps' single entries).
pub struct RlogMain {
    /// `rlogSockets[windowId]`.
    socket: Option<TcpId>,
    /// Every socket that has not reported `close`; a destroyed one still owes that message the
    /// uuid its `live-rlog-start` carried.
    sockets: HashMap<TcpId, Socket>,
    /// `rlogSocketTimeouts[windowId]`: the no-data watchdog. Not cleared on a restart —
    /// upstream's map entry keeps the old timer id, so a watchdog armed under a dead socket
    /// can still destroy its replacement.
    data_timeout: Option<TimerId>,
    /// `rlogDataArrays[windowId]`.
    data_array: Vec<u8>,
    /// The one global heartbeat interval, started at construction.
    heartbeat: TimerId,
    /// `sendMessage` results, in call order; the caller delivers each to the renderer.
    pub outbox: VecDeque<MainMessage>,
}

impl RlogMain {
    /// Registers the global heartbeat interval (`setInterval`, all sockets, 500 ms).
    pub fn new(timers: &mut TimerQueue) -> Self {
        Self {
            socket: None,
            sockets: HashMap::new(),
            data_timeout: None,
            data_array: Vec::new(),
            heartbeat: timers.set_interval(RLOG_HEARTBEAT_DELAY_MS),
            outbox: VecDeque::new(),
        }
    }

    /// Whether `rlogSockets[windowId]` is a live socket. Without one, the heartbeat and the
    /// watchdog do nothing.
    pub fn has_socket(&self) -> bool {
        self.socket.is_some()
    }

    /// `sendMessage` returns whether the IPC send worked; ours always does.
    fn send(&mut self, message: MainMessage) -> bool {
        self.outbox.push_back(message);
        true
    }

    fn uuid(&self, id: TcpId) -> String {
        self.sockets
            .get(&id)
            .map(|s| s.uuid.clone())
            .unwrap_or_default()
    }

    fn clear_idle(&mut self, id: TcpId, cx: &mut MainContext<'_>) {
        if let Some(timer) = self.sockets.get_mut(&id).and_then(|s| s.idle.take()) {
            cx.timers.clear(timer);
        }
    }

    /// Connecting, reads and writes re-arm the idle timer of an open socket.
    fn touch(&mut self, id: TcpId, cx: &mut MainContext<'_>) {
        if let Some(socket) = self.sockets.get_mut(&id)
            && socket.open
            && let Some(timer) = socket.idle
        {
            cx.timers.clear(timer);
            socket.idle = Some(cx.timers.set_timeout(RLOG_CONNECT_TIMEOUT_MS));
        }
    }

    fn destroy(&mut self, id: TcpId, cx: &mut MainContext<'_>) {
        self.clear_idle(id, cx);
        cx.net.tcp_destroy(id);
    }

    /// `rlogMain.handle(name, data)`. Node throws on an invalid port after the old socket is
    /// destroyed: the hub gets no `live-data` for this start.
    pub fn handle(&mut self, call: &MainCall, cx: &mut MainContext<'_>) -> Result<(), Thrown> {
        match call {
            MainCall::Start {
                uuid,
                address,
                port,
            } => {
                if let Some(socket) = self.socket.take() {
                    self.destroy(socket, cx);
                }
                let socket = cx.net.tcp_connect(address, *port)?;
                self.sockets.insert(
                    socket,
                    Socket {
                        uuid: uuid.clone(),
                        open: false,
                        idle: Some(cx.timers.set_timeout(RLOG_CONNECT_TIMEOUT_MS)),
                    },
                );
                self.socket = Some(socket);
                self.data_array = Vec::new();
            }
            MainCall::Stop => {
                if let Some(socket) = self.socket.take() {
                    self.destroy(socket, cx);
                }
            }
        }
        Ok(())
    }

    /// A socket event. Events for ids the transport no longer tracks are dropped before
    /// they reach here.
    pub fn handle_net_event(&mut self, event: &RlogNetEvent, cx: &mut MainContext<'_>) {
        match event {
            RlogNetEvent::Data(id, chunk) => {
                self.touch(*id, cx);
                self.data_array.extend_from_slice(chunk);
                if let Some(timer) = self.data_timeout.take() {
                    cx.timers.clear(timer);
                }
                self.data_timeout = Some(cx.timers.set_timeout(RLOG_DATA_TIMEOUT_MS));
                loop {
                    let len = self.data_array.len() as i64;
                    if len < 4 {
                        break;
                    }
                    let expected =
                        i64::from(i32::from_be_bytes(self.data_array[..4].try_into().unwrap())) + 4;
                    if len < expected {
                        break;
                    }
                    // `Uint8Array.prototype.slice`: negative indexes count from the end, an
                    // inverted range is empty.
                    let clamp = |x: i64| -> usize {
                        if x < 0 {
                            (len + x).max(0) as usize
                        } else {
                            x.min(len) as usize
                        }
                    };
                    let (begin, end) = (clamp(4), clamp(expected));
                    // The buffer would never shrink, and upstream's main process spins here for
                    // good. This loop runs on the foreground thread, so the connection is dropped.
                    if end == 0 {
                        self.data_array.clear();
                        self.destroy(*id, cx);
                        break;
                    }
                    let payload = if begin > end {
                        Vec::new()
                    } else {
                        self.data_array[begin..end].to_vec()
                    };
                    self.data_array = self.data_array[clamp(expected)..].to_vec();
                    if !payload.is_empty() {
                        let uuid = self.uuid(*id);
                        let ok = self.send(MainMessage::Data { uuid, raw: payload });
                        if !ok {
                            self.destroy(*id, cx);
                        }
                    }
                }
            }
            RlogNetEvent::Error(id) => {
                self.clear_idle(*id, cx);
                let uuid = self.uuid(*id);
                self.send(MainMessage::Failed { uuid });
            }
            RlogNetEvent::Close(id) => {
                self.clear_idle(*id, cx);
                let uuid = self.sockets.remove(id).map(|s| s.uuid).unwrap_or_default();
                if self.socket == Some(*id) {
                    self.socket = None;
                }
                self.send(MainMessage::Failed { uuid });
            }
            RlogNetEvent::Connect(id) => {
                if let Some(socket) = self.sockets.get_mut(id) {
                    socket.open = true;
                }
                self.touch(*id, cx);
            }
        }
    }

    /// The heartbeat interval, the data watchdog and the sockets' idle timers.
    pub fn handle_timer(&mut self, id: TimerId, cx: &mut MainContext<'_>) -> bool {
        if id == self.heartbeat {
            if let Some(socket) = self.socket {
                cx.net.tcp_write(socket, &RLOG_HEARTBEAT_DATA);
                self.touch(socket, cx);
            }
            true
        } else if self.data_timeout == Some(id) {
            self.data_timeout = None;
            if let Some(socket) = self.socket {
                self.destroy(socket, cx);
            }
            true
        } else if let Some(socket) = self.sockets.values_mut().find(|s| s.idle == Some(id)) {
            socket.idle = None;
            let uuid = socket.uuid.clone();
            self.send(MainMessage::IdleTimeout { uuid });
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;
    use crate::timers::ManualClock;

    struct NoNet;

    impl RlogTransport for NoNet {
        fn tcp_connect(&mut self, _host: &str, _port: f64) -> Result<TcpId, Thrown> {
            Ok(TcpId(1))
        }
        fn tcp_write(&mut self, _id: TcpId, _data: &[u8]) {}
        fn tcp_destroy(&mut self, _id: TcpId) {}
    }

    fn payloads(chunks: &[&[u8]]) -> Vec<Vec<u8>> {
        let mut timers = TimerQueue::new(Rc::new(ManualClock::new(0.0)));
        let mut net = NoNet;
        let mut main = RlogMain::new(&mut timers);
        let mut cx = MainContext {
            timers: &mut timers,
            net: &mut net,
        };
        let start = MainCall::Start {
            uuid: "u".to_owned(),
            address: "a".to_owned(),
            port: 5800.0,
        };
        main.handle(&start, &mut cx).unwrap();
        for chunk in chunks {
            main.handle_net_event(&RlogNetEvent::Data(TcpId(1), chunk.to_vec()), &mut cx);
        }
        main.outbox
            .drain(..)
            .map(|m| match m {
                MainMessage::Data { raw, .. } => raw,
                _ => panic!("not a payload"),
            })
            .collect()
    }

    #[test]
    fn framing_is_independent_of_chunk_boundaries() {
        let mut stream = Vec::new();
        for payload in [&b"\x01"[..], b"", b"abcdef", b"\x00\x00\x00\x04"] {
            stream.extend_from_slice(&(payload.len() as i32).to_be_bytes());
            stream.extend_from_slice(payload);
        }
        stream.extend_from_slice(&[0, 0, 0, 9, 1, 2]);
        let whole = payloads(&[&stream]);
        assert_eq!(
            whole,
            vec![
                b"\x01".to_vec(),
                b"abcdef".to_vec(),
                b"\x00\x00\x00\x04".to_vec()
            ]
        );
        for a in 0..=stream.len() {
            for b in a..=stream.len() {
                let split = payloads(&[&stream[..a], &stream[a..b], &stream[b..]]);
                assert_eq!(split, whole, "cut at {a} and {b}");
            }
        }
    }

    #[test]
    fn a_length_that_never_consumes_anything_drops_the_connection() {
        for length in [-4i32, -8, i32::MIN] {
            assert!(payloads(&[&length.to_be_bytes()]).is_empty(), "{length}");
        }
    }
}
