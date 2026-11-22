#![allow(clippy::disallowed_methods)]

use asrs_model::app_identity::AppIdentity;

pub mod hub;
pub mod main_process;
pub mod menus;
pub mod scenario;
pub mod views;
pub mod windows;

/// The name the app presents: upstream's in `parity` builds, so titles, menus and pixels
/// compare unchanged.
pub const IDENTITY: AppIdentity = if cfg!(feature = "parity") {
    AppIdentity::UPSTREAM
} else {
    AppIdentity::RELEASE
};
