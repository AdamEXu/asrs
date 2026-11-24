//! FTC Dashboard (`src/hub/dataSources/ftcdashboard/*`): the websocket source, its config
//! reducer and the `/_Config` tuner.

pub mod config;
mod source;

pub use source::{FtcContext, FtcSource};
