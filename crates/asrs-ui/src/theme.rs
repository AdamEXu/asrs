use gpui_kit::{App, Global, Window, WindowAppearance};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Appearance {
    Light,
    Dark,
}

/// Upstream's `theme` preference, applied through `nativeTheme.themeSource`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeSource {
    #[default]
    System,
    Light,
    Dark,
}

impl Global for ThemeSource {}

impl Appearance {
    pub fn resolve(source: ThemeSource, system: WindowAppearance) -> Appearance {
        match source {
            ThemeSource::Light => Appearance::Light,
            ThemeSource::Dark => Appearance::Dark,
            ThemeSource::System => match system {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
                WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
            },
        }
    }

    pub fn of(window: &Window, cx: &App) -> Appearance {
        let source = cx.try_global::<ThemeSource>().copied().unwrap_or_default();
        Appearance::resolve(source, window.appearance())
    }

    pub fn pick<T>(self, light: T, dark: T) -> T {
        match self {
            Appearance::Light => light,
            Appearance::Dark => dark,
        }
    }

    pub fn theme(self) -> &'static Theme {
        self.pick(&LIGHT, &DARK)
    }
}

/// `body { font-family }` in `www/global.css`, resolved by `asrs-canvas::text`.
pub const FONT_FAMILY: &[&str] = &[
    "ui-sans-serif",
    "system-ui",
    "-apple-system",
    "BlinkMacSystemFont",
];
pub const MONOSPACE_FAMILY: &[&str] = &["Courier", "monospace"];
pub const FONT_SIZE: f32 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub appearance: Appearance,
    pub text: u32,
    pub background: u32,
    pub link: u32,
    pub focus_ring: u32,
    pub button_hover: u32,
    pub button_active: u32,
}

pub const FOCUS_RING_WIDTH: f32 = 2.0;
pub const FOCUS_RING_RADIUS: f32 = 3.0;
pub const BUTTON_RADIUS: f32 = 6.0;

/// The light link colour is Chromium's default `-webkit-link`; `global.css` only sets the dark one.
pub const LIGHT: Theme = Theme {
    appearance: Appearance::Light,
    text: 0x222222,
    background: 0xffffff,
    link: 0x0000ee,
    focus_ring: 0x446ce3,
    button_hover: 0xeeeeee,
    button_active: 0xdddddd,
};

pub const DARK: Theme = Theme {
    appearance: Appearance::Dark,
    text: 0xeeeeee,
    background: 0x222222,
    link: 0x58a7ff,
    focus_ring: 0x446ce3,
    button_hover: 0x333333,
    button_active: 0x444444,
};

/// Every `filter` chain upstream applies to `<img>` content in `global.css`, `hub.css` and
/// `renderers.css`, verbatim.
pub mod chains {
    pub const BUTTON_LIGHT: &str =
        "invert(32%) sepia(0%) saturate(60%) hue-rotate(224deg) brightness(100%) contrast(95%)";
    pub const BUTTON_BLURRED_LIGHT: &str =
        "invert(82%) sepia(3%) saturate(20%) hue-rotate(339deg) brightness(84%) contrast(86%)";
    pub const BUTTON_DARK: &str =
        "invert(75%) sepia(0%) saturate(0%) hue-rotate(138deg) brightness(83%) contrast(81%)";
    pub const BUTTON_BLURRED_DARK: &str =
        "invert(30%) sepia(11%) saturate(12%) hue-rotate(104deg) brightness(104%) contrast(93%)";
    pub const TUNING_ACTIVE_LIGHT: &str =
        "invert(9%) sepia(91%) saturate(7487%) hue-rotate(277deg) brightness(106%) contrast(110%)";
    pub const TUNING_ACTIVE_DARK: &str =
        "invert(35%) sepia(76%) saturate(4981%) hue-rotate(259deg) brightness(102%) contrast(101%)";
    pub const UPDATE: &str =
        "invert(60%) sepia(41%) saturate(3324%) hue-rotate(356deg) brightness(96%) contrast(112%)";
    pub const UPDATE_BLURRED_LIGHT: &str =
        "invert(85%) sepia(44%) saturate(1384%) hue-rotate(314deg) brightness(114%) contrast(101%)";
    pub const UPDATE_BLURRED_DARK: &str =
        "invert(30%) sepia(37%) saturate(3821%) hue-rotate(26deg) brightness(97%) contrast(101%)";
    pub const FEEDBACK_LIGHT: &str =
        "invert(45%) sepia(26%) saturate(2440%) hue-rotate(246deg) brightness(88%) contrast(91%)";
    pub const FEEDBACK_BLURRED_LIGHT: &str =
        "invert(89%) sepia(17%) saturate(2426%) hue-rotate(196deg) brightness(93%) contrast(95%)";
    pub const FEEDBACK_DARK: &str =
        "invert(57%) sepia(42%) saturate(2311%) hue-rotate(232deg) brightness(90%) contrast(95%)";
    pub const FEEDBACK_BLURRED_DARK: &str =
        "invert(31%) sepia(7%) saturate(5761%) hue-rotate(239deg) brightness(98%) contrast(77%)";
    pub const UNLOCK: &str = "invert(10%) sepia(100%) saturate(7058%) hue-rotate(358deg) brightness(105%) contrast(117%)";
    pub const UNLOCK_BLURRED_LIGHT: &str =
        "invert(83%) sepia(68%) saturate(3617%) hue-rotate(294deg) brightness(99%) contrast(119%)";
    pub const UNLOCK_BLURRED_DARK: &str =
        "invert(37%) sepia(16%) saturate(1793%) hue-rotate(314deg) brightness(98%) contrast(77%)";
    pub const SOURCE_LIST_VALUE_SYMBOL: &str =
        "invert(43%) sepia(0%) saturate(2564%) hue-rotate(1deg) brightness(110%) contrast(107%)";
    pub const INVERT: &str = "invert(100%)";
    pub const INVERT_ONE: &str = "invert(1)";
    pub const TUNABLE_HOVER: &str = "brightness(0.75)";
    pub const LIGHTEN_DARK: &str = "hue-rotate(-45deg) brightness(450%)";
    pub const VIDEO_SOURCE: &str = "brightness(0%) saturate(0%) invert(100%)";
    pub const CONSOLE_WARNING: &str =
        "invert(84%) sepia(28%) saturate(6635%) hue-rotate(354deg) brightness(96%) contrast(112%)";
    pub const XR_BUTTON: &str =
        "invert(56%) sepia(51%) saturate(1742%) hue-rotate(225deg) brightness(101%) contrast(101%)";

    pub const ALL: &[&str] = &[
        BUTTON_LIGHT,
        BUTTON_BLURRED_LIGHT,
        BUTTON_DARK,
        BUTTON_BLURRED_DARK,
        TUNING_ACTIVE_LIGHT,
        TUNING_ACTIVE_DARK,
        UPDATE,
        UPDATE_BLURRED_LIGHT,
        UPDATE_BLURRED_DARK,
        FEEDBACK_LIGHT,
        FEEDBACK_BLURRED_LIGHT,
        FEEDBACK_DARK,
        FEEDBACK_BLURRED_DARK,
        UNLOCK,
        UNLOCK_BLURRED_LIGHT,
        UNLOCK_BLURRED_DARK,
        SOURCE_LIST_VALUE_SYMBOL,
        INVERT,
        INVERT_ONE,
        TUNABLE_HOVER,
        LIGHTEN_DARK,
        VIDEO_SOURCE,
        CONSOLE_WARNING,
        XR_BUTTON,
    ];
}

/// The filter chain on a button's `<img>` per state, from the cascade of `global.css` and
/// `hub.css`. `hub.css` loads later, so its same-specificity rules for `update`, `feedback` and
/// `unlock` beat `button:disabled > img`: those keep their normal tint when disabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IconTint {
    pub normal: [&'static str; 2],
    pub blurred: [&'static str; 2],
    pub disabled: [&'static str; 2],
}

impl IconTint {
    pub const BUTTON: IconTint = IconTint {
        normal: [chains::BUTTON_LIGHT, chains::BUTTON_DARK],
        blurred: [chains::BUTTON_BLURRED_LIGHT, chains::BUTTON_BLURRED_DARK],
        disabled: [chains::BUTTON_BLURRED_LIGHT, chains::BUTTON_BLURRED_DARK],
    };
    pub const TUNING_ACTIVE: IconTint = IconTint {
        normal: [chains::TUNING_ACTIVE_LIGHT, chains::TUNING_ACTIVE_DARK],
        blurred: [chains::TUNING_ACTIVE_LIGHT, chains::TUNING_ACTIVE_DARK],
        disabled: [chains::TUNING_ACTIVE_LIGHT, chains::TUNING_ACTIVE_DARK],
    };
    pub const UPDATE: IconTint = IconTint {
        normal: [chains::UPDATE, chains::UPDATE],
        blurred: [chains::UPDATE_BLURRED_LIGHT, chains::UPDATE_BLURRED_DARK],
        disabled: [chains::UPDATE, chains::UPDATE],
    };
    pub const FEEDBACK: IconTint = IconTint {
        normal: [chains::FEEDBACK_LIGHT, chains::FEEDBACK_DARK],
        blurred: [
            chains::FEEDBACK_BLURRED_LIGHT,
            chains::FEEDBACK_BLURRED_DARK,
        ],
        disabled: [chains::FEEDBACK_LIGHT, chains::FEEDBACK_DARK],
    };
    pub const UNLOCK: IconTint = IconTint {
        normal: [chains::UNLOCK, chains::UNLOCK],
        blurred: [chains::UNLOCK_BLURRED_LIGHT, chains::UNLOCK_BLURRED_DARK],
        disabled: [chains::UNLOCK, chains::UNLOCK],
    };

    /// `.blurred` outranks `:disabled` wherever the two differ.
    pub fn chain(&self, appearance: Appearance, blurred: bool, disabled: bool) -> &'static str {
        let pair = if blurred {
            self.blurred
        } else if disabled {
            self.disabled
        } else {
            self.normal
        };
        appearance.pick(pair[0], pair[1])
    }
}
