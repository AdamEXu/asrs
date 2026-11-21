//! What `PhoenixDiagnosticsSource` needs from the browser: `fetch(url)` GETs plus aborts.
//! Request ids share the browser-wide space NT4 uses, since one `FakeNet` hands out both.

use crate::nt4::RequestId;
use crate::timers::TimerQueue;

/// A settled `fetch`.
pub enum HttpEvent {
    /// The response arrived: status and body, as `response.json()` would see them.
    Response(RequestId, u16, String),
    /// The request failed at the network layer.
    Failed(RequestId),
}

pub trait PhoenixTransport {
    /// `fetch(url)` (a GET). Settling reports back as an [`HttpEvent`].
    fn fetch(&mut self, url: &str) -> RequestId;
    /// `AbortSignal` fired: an unsettled request never completes.
    fn fetch_abort(&mut self, id: RequestId);
}

/// What a Phoenix method may touch besides the source: the hub's timers and the browser's
/// fetch.
pub struct PhoenixContext<'a> {
    pub timers: &'a mut TimerQueue,
    pub net: &'a mut dyn PhoenixTransport,
}
