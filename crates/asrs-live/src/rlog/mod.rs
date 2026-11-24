//! RLOG live: `RLOGServerSource.ts` (renderer) plus the TCP socket, framing, heartbeat and
//! timeouts from `main.ts` (emulated main process), on the shared live runtime.

pub mod main;
pub mod source;
pub mod tokio_transport;
pub mod transport;

pub use main::{MainCall, MainContext, MainMessage, RlogMain};
pub use source::{RlogContext, RlogPreferences, RlogSource};
pub use tokio_transport::TokioRlogTransport;
pub use transport::{RlogNetEvent, RlogTransport, TcpId, socket_port};

/// A `sendMainMessage`/`sendMessage` hop between the renderer and the emulated main process,
/// delivered as a task on the session loop.
#[derive(Clone)]
pub enum Ipc {
    /// `"live-rlog-start"`/`"live-rlog-stop"` reaching [`RlogMain::handle`].
    ToMain(MainCall),
    /// `"live-data"` reaching `RlogSource::handle_main_message`.
    ToSource(asrs_js::value::JsValue),
}

impl std::fmt::Debug for Ipc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToMain(call) => f.debug_tuple("ToMain").field(call).finish(),
            Self::ToSource(_) => f.debug_tuple("ToSource").field(&"..").finish(),
        }
    }
}
