//! `src/shared/buildConstants.ts`, with the values upstream's `rolldown.config.mjs` templates in.

use asrs_js::value::{JsValue, ToJs};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Distribution {
    Frc6328 = 0,
    WpiLib = 1,
    Lite = 2,
    LiteDs = 3,
}

impl Distribution {
    pub const ALL: [Distribution; 4] = [
        Distribution::Frc6328,
        Distribution::WpiLib,
        Distribution::Lite,
        Distribution::LiteDs,
    ];

    /// The TypeScript enum member name.
    pub fn name(self) -> &'static str {
        match self {
            Distribution::Frc6328 => "FRC6328",
            Distribution::WpiLib => "WPILib",
            Distribution::Lite => "Lite",
            Distribution::LiteDs => "LiteDS",
        }
    }

    pub fn number(self) -> f64 {
        f64::from(self as u8)
    }
}

impl ToJs for Distribution {
    fn to_js(&self) -> JsValue {
        JsValue::Number(self.number())
    }
}

pub const DISTRIBUTION: Distribution = if cfg!(feature = "wpilib") {
    Distribution::WpiLib
} else {
    Distribution::Frc6328
};

pub const IS_LITE: bool = matches!(DISTRIBUTION, Distribution::Lite | Distribution::LiteDs);

/// `package.json` `version`, which is also `app.getVersion()` in a packaged build.
pub const VERSION: &str = "27.0.0-alpha-6";

pub const LITE_VERSION: &str = VERSION;

/// Upstream stamps the build time (`new Date().toISOString()`). A release build passes it in
/// `ASRS_BUILD_DATE`; other builds use the oracle's pinned value so output stays reproducible.
pub const BUILD_DATE: &str = match option_env!("ASRS_BUILD_DATE") {
    Some(date) => date,
    None => "1970-01-01T00:00:00.000Z",
};

/// `package.json` `build.copyright`.
pub const COPYRIGHT: &str = "Copyright \u{a9} 2021-2026 Littleton Robotics";
