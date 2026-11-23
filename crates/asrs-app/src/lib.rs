#![allow(clippy::disallowed_methods)]

use asrs_model::app_identity::AppIdentity;

pub mod download_flow;
mod export_flow;
pub mod hub;
mod hub_dialogs;
pub mod licenses;
pub mod main_process;
pub mod menus;
pub mod satellite;
pub mod scenario;
mod video;
pub mod views;
pub mod windows;

/// The name the app presents: upstream's in `parity` builds, so titles, menus and pixels
/// compare unchanged.
pub const IDENTITY: AppIdentity = if cfg!(feature = "parity") {
    AppIdentity::UPSTREAM
} else {
    AppIdentity::RELEASE
};
