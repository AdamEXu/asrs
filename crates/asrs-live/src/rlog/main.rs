//! The `live-rlog-start`/`live-rlog-stop` arms of `handleHubMessage` in
//! `src/main/electron/main.ts` (the same code `oracle/src/live/rlogMain.ts` transcribes):
//! one TCP socket per hub window, framing the stream as `[int32 BE length][payload]`, a 3 s
//! socket idle timeout reported with a `status` key (never `success`), a 3 s no-data watchdog
//! that destroys the socket, and the global 500 ms heartbeat written to every open socket.

use std::collections::{HashMap, VecDeque};

use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};

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
                (js("raw"), JsValue::Bytes(raw.into())),
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
    Start { uuid: String, address: String, port: f64 },
    /// `"live-rlog-stop"`.
    Stop,
}

/// What `handle` may touch: the network, and the main process's timer queue for the data
/// watchdog.
pub struct MainContext<'a> {
    pub timers: &'a mut TimerQueue,
    pub net: &'a mut dyn RlogTransport,
}

/// `RLOGMain` for one hub window (upstream keys its maps by `windowId`; the emulation's one
/// window is id 1, so the fields are the maps' single entries).
pub struct RlogMain {
    /// `rlogSockets[windowId]`.
    socket: Option<TcpId>,
    /// Every socket's uuid; a destroyed socket still owes its `close` message the uuid its
    /// `live-rlog-start` carried.
    uuids: HashMap<TcpId, String>,
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
            uuids: HashMap::new(),
            data_timeout: None,
            data_array: Vec::new(),
            heartbeat: timers.set_interval(RLOG_HEARTBEAT_DELAY_MS),
            outbox: VecDeque::new(),
        }
    }

    /// `sendMessage` returns whether the IPC send worked; ours always does.
    fn send(&mut self, message: MainMessage) -> bool {
        self.outbox.push_back(message);
        true
    }

    /// `rlogMain.handle(name, data)`; false for messages it does not handle.
    pub fn handle(&mut self, call: &MainCall, cx: &mut MainContext<'_>) -> bool {
        match call {
            MainCall::Start {
                uuid,
                address,
                port,
            } => {
                if let Some(socket) = self.socket.take() {
                    cx.net.tcp_destroy(socket);
                }
                let socket = cx.net.tcp_connect(address, *port);
                self.uuids.insert(socket, uuid.clone());
                cx.net.tcp_set_timeout(socket, RLOG_CONNECT_TIMEOUT_MS);
                self.socket = Some(socket);
                self.data_array = Vec::new();
            }
            MainCall::Stop => {
                if let Some(socket) = self.socket.take() {
                    cx.net.tcp_destroy(socket);
                }
            }
        }
        true
    }

    /// A socket event. Events for ids the transport no longer tracks are dropped before
    /// they reach here.
    pub fn handle_net_event(&mut self, event: &RlogNetEvent, cx: &mut MainContext<'_>) {
        match event {
            RlogNetEvent::Data(id, chunk) => {
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
                    let expected = i64::from(i32::from_be_bytes(
                        self.data_array[..4].try_into().unwrap(),
                    )) + 4;
                    if len < expected {
                        break;
                    }
                    // `Uint8Array.prototype.slice`: negative indexes count from the end, an
                    // inverted range is empty. expected <= 0 makes this spin, as upstream's
                    // loop does.
                    let clamp = |x: i64| -> usize {
                        if x < 0 {
                            (len + x).max(0) as usize
                        } else {
                            x.min(len) as usize
                        }
                    };
                    let (begin, end) = (clamp(4), clamp(expected));
                    let payload = if begin > end {
                        Vec::new()
                    } else {
                        self.data_array[begin..end].to_vec()
                    };
                    self.data_array = self.data_array[clamp(expected)..].to_vec();
                    if !payload.is_empty() {
                        let uuid = self.uuids.get(id).cloned().unwrap_or_default();
                        let ok = self.send(MainMessage::Data { uuid, raw: payload });
                        if !ok {
                            cx.net.tcp_destroy(*id);
                        }
                    }
                }
            }
            RlogNetEvent::Error(id) => {
                let uuid = self.uuids.get(id).cloned().unwrap_or_default();
                self.send(MainMessage::Failed { uuid });
            }
            RlogNetEvent::Close(id) => {
                let uuid = self.uuids.remove(id).unwrap_or_default();
                if self.socket == Some(*id) {
                    self.socket = None;
                }
                self.send(MainMessage::Failed { uuid });
            }
            RlogNetEvent::Timeout(id) => {
                let uuid = self.uuids.get(id).cloned().unwrap_or_default();
                self.send(MainMessage::IdleTimeout { uuid });
            }
            RlogNetEvent::Connect(_) => {}
        }
    }

    /// The heartbeat interval and the data watchdog share the timer queue with the renderer.
    pub fn handle_timer(&mut self, id: TimerId, cx: &mut MainContext<'_>) -> bool {
        if id == self.heartbeat {
            if let Some(socket) = self.socket {
                cx.net.tcp_write(socket, &RLOG_HEARTBEAT_DATA);
            }
            true
        } else if self.data_timeout == Some(id) {
            self.data_timeout = None;
            if let Some(socket) = self.socket {
                cx.net.tcp_destroy(socket);
            }
            true
        } else {
            false
        }
    }
}
