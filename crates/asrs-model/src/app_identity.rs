//! The name asrs presents itself under, and the identifiers that keep it apart from the official
//! AdvantageScope install. Bytes under the parity bar (NT4 client name, export metadata, files in
//! the user-data directory, layout JSON) keep upstream's strings and never pass through here.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppIdentity {
    /// `app.name`, as presentation strings (window titles, the app menu, About, dialogs) use it.
    pub name: &'static str,
}

impl AppIdentity {
    /// Upstream's `productName`: what `parity` builds present, so titles, menu models and pixels
    /// compare against upstream unchanged.
    pub const UPSTREAM: AppIdentity = AppIdentity {
        name: "AdvantageScope",
    };

    pub const RELEASE: AppIdentity = AppIdentity { name: "asrs" };

    /// A hub window title as upstream sets it (`"<file> — AdvantageScope"`, or just the name),
    /// with the app name this identity presents.
    pub fn window_title(&self, upstream: &str) -> String {
        let upstream_name = Self::UPSTREAM.name;
        let separator = format!(" \u{2014} {upstream_name}");
        if let Some(at) = upstream.rfind(&separator) {
            return format!(
                "{} \u{2014} {}{}",
                &upstream[..at],
                self.name,
                &upstream[at + separator.len()..]
            );
        }
        match upstream.strip_prefix(upstream_name) {
            Some(rest) if rest.is_empty() || rest.starts_with(' ') => {
                format!("{}{rest}", self.name)
            }
            _ => upstream.to_owned(),
        }
    }
}

/// The directory under the platform's app-data directory (`app.getPath("appData")`) that holds
/// asrs's `prefs.json`, state, assets and the rest, under upstream's file names. It is never
/// upstream's `AdvantageScope`, so asrs does not read or write the official app's state.
pub const USER_DATA_DIR: &str = "asrs";

pub const BUNDLE_ID: &str = "net.adamxu.asrs";

/// The Windows AppUserModelID.
pub const APP_USER_MODEL_ID: &str = "AdamXu.asrs";

/// `WM_CLASS` on Linux, which upstream sets through `app.setName`.
pub const WM_CLASS: &str = "asrs";
