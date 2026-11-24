//! Phoenix 6 diagnostics (`PhoenixDiagnosticsSource.ts`): device and signal discovery plus
//! signal plots, over HTTP on port 1250. Real use pairs it with
//! [`crate::nt4::TokioTransport::with_bodies`].

mod enums;
mod source;

pub use source::{PhoenixContext, PhoenixSource, PhoenixWindow};
