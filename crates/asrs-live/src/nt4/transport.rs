//! What the client needs from the browser: `WebSocket` and `fetch`. Implementations own
//! `readyState` and the browser's rules for it (a send while connecting throws, a send or
//! `close()` after closing does nothing, messages are only delivered while open) and report
//! events back through the client's `on_*` methods, one event per task, in arrival order.

use crate::http::{FetchEvent, FetchTransport};
use crate::thrown::Thrown;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SocketId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReadyState {
    Connecting,
    Open,
    Closing,
    Closed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WsMessage {
    Text(String),
    Binary(Vec<u8>),
}

/// A browser event for the client, in dispatch order. A failed handshake is `WsError` then
/// `WsClose`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetEvent {
    WsOpen(SocketId, String),
    WsMessage(SocketId, WsMessage),
    WsError(SocketId),
    WsClose(SocketId),
    Fetch(FetchEvent),
}

impl From<FetchEvent> for NetEvent {
    fn from(event: FetchEvent) -> Self {
        Self::Fetch(event)
    }
}

pub trait Nt4Transport: FetchTransport {
    /// `new WebSocket(url, protocols)`.
    fn ws_open(&mut self, url: &str, protocols: &[&str]) -> Result<SocketId, Thrown>;

    fn ws_ready_state(&self, id: SocketId) -> ReadyState;

    /// `ws.send(data)`.
    fn ws_send(&mut self, id: SocketId, data: WsMessage) -> Result<(), Thrown>;

    /// `ws.close()`.
    fn ws_close(&mut self, id: SocketId);
}
