//! Phoenix 6 diagnostics (`PhoenixDiagnosticsSource.ts`): device and signal discovery plus
//! signal plots, over HTTP on port 1250.

mod enums;
mod source;
mod tokio;
mod transport;

pub use source::{PhoenixSource, PhoenixWindow};
pub use tokio::TokioPhoenixTransport;
pub use transport::{HttpEvent, PhoenixContext, PhoenixTransport};
