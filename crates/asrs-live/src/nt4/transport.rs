//! What the client needs from the browser: `WebSocket` and `fetch`. Implementations own
//! `readyState` and the browser's rules for it (a send while connecting throws, a send or
//! `close()` after closing does nothing, messages are only delivered while open) and report
//! events back through the client's `on_*` methods, one event per task, in arrival order.

use crate::thrown::Thrown;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SocketId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(pub u32);

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

pub trait Nt4Transport {
    /// `new WebSocket(url, protocols)`.
    fn ws_open(&mut self, url: &str, protocols: &[&str]) -> Result<SocketId, Thrown>;

    fn ws_ready_state(&self, id: SocketId) -> ReadyState;

    /// `ws.send(data)`.
    fn ws_send(&mut self, id: SocketId, data: WsMessage) -> Result<(), Thrown>;

    /// `ws.close()`.
    fn ws_close(&mut self, id: SocketId);

    /// `fetch(url)` (a GET). Completion arrives as `on_fetch_response` or `on_fetch_failed`.
    fn fetch(&mut self, url: &str) -> RequestId;

    /// Aborts a request that has not completed; it then never completes.
    fn fetch_abort(&mut self, id: RequestId);
}
