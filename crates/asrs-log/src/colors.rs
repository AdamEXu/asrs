//! `shared/Colors.ts`: the source-list color tables and `ensureThemeContrast`.

use asrs_js::color;

/// `shared/SourceListConfig.ts SourceListOptionValueConfig` (ported in full
/// with `SourceListConfig`; the color tables need only this shape).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceListOptionValueConfig {
    pub key: &'static str,
    pub display: &'static str,
}

pub const GRAPH_COLORS: &[SourceListOptionValueConfig] = &[
    SourceListOptionValueConfig {
        key: "#2b66a2",
        display: "Blue",
    },
    SourceListOptionValueConfig {
        key: "#e5b31b",
        display: "Gold",
    },
    SourceListOptionValueConfig {
        key: "#af2437",
        display: "Red",
    },
    SourceListOptionValueConfig {
        key: "#80588e",
        display: "Purple",
    },
    SourceListOptionValueConfig {
        key: "#e48b32",
        display: "Orange",
    },
    SourceListOptionValueConfig {
        key: "#c0b487",
        display: "Tan",
    },
    SourceListOptionValueConfig {
        key: "#858584",
        display: "Gray",
    },
    SourceListOptionValueConfig {
        key: "#3b875a",
        display: "Green",
    },
    SourceListOptionValueConfig {
        key: "#d993aa",
        display: "Pink",
    },
    SourceListOptionValueConfig {
        key: "#5f4528",
        display: "Brown",
    },
];

pub const NEON_COLORS: &[SourceListOptionValueConfig] = &[
    SourceListOptionValueConfig {
        key: "#00ff00",
        display: "Green",
    },
    SourceListOptionValueConfig {
        key: "#ff0000",
        display: "Red",
    },
    SourceListOptionValueConfig {
        key: "#0000ff",
        display: "Blue",
    },
    SourceListOptionValueConfig {
        key: "#ff8c00",
        display: "Orange",
    },
    SourceListOptionValueConfig {
        key: "#00ffff",
        display: "Cyan",
    },
    SourceListOptionValueConfig {
        key: "#ffff00",
        display: "Yellow",
    },
    SourceListOptionValueConfig {
        key: "#ff00ff",
        display: "Magenta",
    },
];

pub const NEON_COLORS_RED_START: &[SourceListOptionValueConfig] = &[
    SourceListOptionValueConfig {
        key: "#ff0000",
        display: "Red",
    },
    SourceListOptionValueConfig {
        key: "#0000ff",
        display: "Blue",
    },
    SourceListOptionValueConfig {
        key: "#00ff00",
        display: "Green",
    },
    SourceListOptionValueConfig {
        key: "#ff8c00",
        display: "Orange",
    },
    SourceListOptionValueConfig {
        key: "#00ffff",
        display: "Cyan",
    },
    SourceListOptionValueConfig {
        key: "#ffff00",
        display: "Yellow",
    },
    SourceListOptionValueConfig {
        key: "#ff00ff",
        display: "Magenta",
    },
];

/// `ensureThemeContrast` with `darkMode` resolved by the caller (upstream's
/// default is `window.matchMedia("(prefers-color-scheme: dark)")`).
pub fn ensure_theme_contrast(color: &str, dark_mode: bool) -> String {
    color::ensure_theme_contrast(color, dark_mode)
}
