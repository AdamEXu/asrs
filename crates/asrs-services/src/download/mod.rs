//! Log downloading over SFTP.
//!
//! - [`downloader`]: the download section of `src/main/electron/main.ts`, sans-IO.
//! - [`window`]: the parts of `src/download.ts` that decide what is connected to and which text an
//!   error shows.
//! - [`sftp`], [`handshake`], [`client`]: what upstream gets from npm `ssh2` 1.14.0. The SFTP
//!   session, `fastGet` and the order and text of every client event are ported; the transport
//!   itself is `russh`.
//! - [`service`]: all of it on one thread, behind commands and events. This is what the download
//!   window drives.

pub mod client;
mod connection;
pub mod downloader;
pub mod handshake;
pub mod service;
pub mod sftp;
pub mod window;

pub use service::{DownloadConfig, DownloadEvent, DownloadEvents, DownloadService};
