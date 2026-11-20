//! `src/shared/Preferences.ts` plus the prefs.json flows of `src/main/electron/main.ts`.
//!
//! [`merge_preferences`] works on raw JS values because `update-preferences` merges into the
//! file exactly as read (its key order and unknown keys survive). [`Preferences`] is the typed
//! form the app reads and the startup flow writes.

use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            /// The member whose string `=== value`.
            pub fn from_js_str(value: &JsString) -> Option<$name> {
                Self::ALL.iter().copied().find(|v| *value == JsString::from(v.as_str()))
            }

            fn is_member(value: &JsString) -> bool {
                Self::from_js_str(value).is_some()
            }
        }

        impl ToJs for $name {
            fn to_js(&self) -> JsValue {
                JsValue::string(self.as_str())
            }
        }
    };
}

string_enum!(Theme {
    Light = "light",
    Dark = "dark",
    System = "system",
});

string_enum!(LiveMode {
    Nt4 = "nt4",
    Nt4AKit = "nt4-akit",
    Nt4Systemcore = "nt4-systemcore",
    Phoenix = "phoenix",
    Rlog = "rlog",
    FtcDashboard = "ftcdashboard",
});

string_enum!(LiveSubscribeMode {
    LowBandwidth = "low-bandwidth",
    Logging = "logging",
});

string_enum!(
    /// `"automatic" | CoordinateSystem`.
    CoordinateSystemPreference {
        Automatic = "automatic",
        WallAlliance = "wall-alliance",
        WallBlue = "wall-blue",
        CenterRotated = "center-rotated",
        CenterRed = "center-red",
    }
);

string_enum!(Field3dMode {
    Cinematic = "cinematic",
    Standard = "standard",
    LowPower = "low-power",
});

string_enum!(
    /// `field3dModeBattery`, where `""` means "same as on AC".
    Field3dBatteryMode {
        SameAsAc = "",
        Cinematic = "cinematic",
        Standard = "standard",
        LowPower = "low-power",
    }
);

string_enum!(SystemcoreStaticAddress {
    Off = "",
    Usb = "usb",
    Wifi = "wifi",
});

string_enum!(Timestamps {
    StartZero = "start-zero",
    Original = "original",
});

impl LiveMode {
    /// `getLiveModeName`.
    pub fn display_name(self) -> &'static str {
        match self {
            LiveMode::Nt4 => "NetworkTables",
            LiveMode::Nt4AKit => "NetworkTables (AdvantageKit)",
            LiveMode::Nt4Systemcore => "Systemcore Diagnostics",
            LiveMode::Phoenix => "Phoenix Diagnostics",
            LiveMode::Rlog => "RLOG Server",
            LiveMode::FtcDashboard => "FTC Dashboard",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preferences {
    pub theme: Theme,
    pub robot_address: JsString,
    pub remote_path: JsString,
    pub live_mode: LiveMode,
    pub live_subscribe_mode: LiveSubscribeMode,
    pub live_discard: f64,
    pub publish_filter: JsString,
    pub rlog_port: f64,
    pub coordinate_system: CoordinateSystemPreference,
    pub field3d_mode_ac: Field3dMode,
    pub field3d_mode_battery: Field3dBatteryMode,
    pub field3d_antialiasing: bool,
    pub tba_api_key: JsString,
    pub user_assets_folder: Option<JsString>,
    pub skip_hoot_non_pro_warning: bool,
    pub skip_frc_coordinate_system_warning: bool,
    pub skip_ftc_coordinate_system_warning: bool,
    pub ctre_license_accepted: bool,
    pub systemcore_static_address: SystemcoreStaticAddress,
    pub has_scrolled_line_graph: bool,
    pub has_scrolled_timeline: bool,
    pub prefers_ftc: bool,
    pub timestamps: Timestamps,
}

/// `DEFAULT_PREFS`.
impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            theme: Theme::System,
            robot_address: JsString::from("robot.local"),
            remote_path: JsString::from("/U/logs"),
            live_mode: LiveMode::Nt4,
            live_subscribe_mode: LiveSubscribeMode::LowBandwidth,
            live_discard: 1200.0,
            publish_filter: JsString::new(),
            rlog_port: 5800.0,
            coordinate_system: CoordinateSystemPreference::Automatic,
            field3d_mode_ac: Field3dMode::Standard,
            field3d_mode_battery: Field3dBatteryMode::SameAsAc,
            field3d_antialiasing: true,
            tba_api_key: JsString::new(),
            user_assets_folder: None,
            skip_hoot_non_pro_warning: false,
            skip_frc_coordinate_system_warning: false,
            skip_ftc_coordinate_system_warning: false,
            ctre_license_accepted: false,
            systemcore_static_address: SystemcoreStaticAddress::Off,
            has_scrolled_line_graph: false,
            has_scrolled_timeline: false,
            prefers_ftc: false,
            timestamps: Timestamps::StartZero,
        }
    }
}

/// `DEFAULT_PREFS` key order, which every file the startup flow writes keeps.
impl ToJs for Preferences {
    fn to_js(&self) -> JsValue {
        let mut o = JsObject::new();
        o.insert("theme", self.theme.to_js());
        o.insert("robotAddress", self.robot_address.to_js());
        o.insert("remotePath", self.remote_path.to_js());
        o.insert("liveMode", self.live_mode.to_js());
        o.insert("liveSubscribeMode", self.live_subscribe_mode.to_js());
        o.insert("liveDiscard", self.live_discard.to_js());
        o.insert("publishFilter", self.publish_filter.to_js());
        o.insert("rlogPort", self.rlog_port.to_js());
        o.insert("coordinateSystem", self.coordinate_system.to_js());
        o.insert("field3dModeAc", self.field3d_mode_ac.to_js());
        o.insert("field3dModeBattery", self.field3d_mode_battery.to_js());
        o.insert("field3dAntialiasing", self.field3d_antialiasing.to_js());
        o.insert("tbaApiKey", self.tba_api_key.to_js());
        o.insert(
            "userAssetsFolder",
            match &self.user_assets_folder {
                Some(folder) => folder.to_js(),
                None => JsValue::Null,
            },
        );
        o.insert(
            "skipHootNonProWarning",
            self.skip_hoot_non_pro_warning.to_js(),
        );
        o.insert(
            "skipFRCCoordinateSystemWarning",
            self.skip_frc_coordinate_system_warning.to_js(),
        );
        o.insert(
            "skipFTCCoordinateSystemWarning",
            self.skip_ftc_coordinate_system_warning.to_js(),
        );
        o.insert("ctreLicenseAccepted", self.ctre_license_accepted.to_js());
        o.insert(
            "systemcoreStaticAddress",
            self.systemcore_static_address.to_js(),
        );
        o.insert("hasScrolledLineGraph", self.has_scrolled_line_graph.to_js());
        o.insert("hasScrolledTimeline", self.has_scrolled_timeline.to_js());
        o.insert("prefersFTC", self.prefers_ftc.to_js());
        o.insert("timestamps", self.timestamps.to_js());
        JsValue::Object(o)
    }
}

#[derive(Clone, Copy)]
enum Accept {
    Member(fn(&JsString) -> bool),
    String,
    Number,
    Boolean,
}

impl Accept {
    fn matches(self, value: &JsValue) -> bool {
        match (self, value) {
            (Accept::Member(is_member), JsValue::String(s)) => is_member(s),
            (Accept::String, JsValue::String(_)) => true,
            (Accept::Number, JsValue::Number(_)) => true,
            (Accept::Boolean, JsValue::Bool(_)) => true,
            _ => false,
        }
    }
}

/// `(key read from newPrefs, key assigned on basePrefs, accepted values)`, in the order
/// `mergePreferences` tests them. Later rules win, so `robotAddress` beats the legacy
/// `rioAddress` (v4) and `address` (v1) whatever order the file lists them in.
const RULES: &[(&str, &str, Accept)] = &[
    ("theme", "theme", Accept::Member(Theme::is_member)),
    ("address", "robotAddress", Accept::String),
    ("rioAddress", "robotAddress", Accept::String),
    ("robotAddress", "robotAddress", Accept::String),
    ("rioPath", "remotePath", Accept::String),
    ("remotePath", "remotePath", Accept::String),
    ("liveMode", "liveMode", Accept::Member(LiveMode::is_member)),
    (
        "liveSubscribeMode",
        "liveSubscribeMode",
        Accept::Member(LiveSubscribeMode::is_member),
    ),
    ("liveDiscard", "liveDiscard", Accept::Number),
    ("publishFilter", "publishFilter", Accept::String),
    ("rlogPort", "rlogPort", Accept::Number),
    (
        "coordinateSystem",
        "coordinateSystem",
        Accept::Member(CoordinateSystemPreference::is_member),
    ),
    (
        "threeDimensionModeAc",
        "field3dModeAc",
        Accept::Member(Field3dMode::is_member),
    ),
    (
        "threeDimensionModeBattery",
        "field3dModeBattery",
        Accept::Member(Field3dBatteryMode::is_member),
    ),
    (
        "field3dModeAc",
        "field3dModeAc",
        Accept::Member(Field3dMode::is_member),
    ),
    (
        "field3dModeBattery",
        "field3dModeBattery",
        Accept::Member(Field3dBatteryMode::is_member),
    ),
    (
        "field3dAntialiasing",
        "field3dAntialiasing",
        Accept::Boolean,
    ),
    ("tbaApiKey", "tbaApiKey", Accept::String),
    ("userAssetsFolder", "userAssetsFolder", Accept::String),
    (
        "skipHootNonProWarning",
        "skipHootNonProWarning",
        Accept::Boolean,
    ),
    (
        "skipFRCCoordinateSystemWarning",
        "skipFRCCoordinateSystemWarning",
        Accept::Boolean,
    ),
    (
        "skipFTCCoordinateSystemWarning",
        "skipFTCCoordinateSystemWarning",
        Accept::Boolean,
    ),
    (
        "ctreLicenseAccepted",
        "ctreLicenseAccepted",
        Accept::Boolean,
    ),
    (
        "systemcoreStaticAddress",
        "systemcoreStaticAddress",
        Accept::Member(SystemcoreStaticAddress::is_member),
    ),
    (
        "hasScrolledLineGraph",
        "hasScrolledLineGraph",
        Accept::Boolean,
    ),
    (
        "hasScrolledTimeline",
        "hasScrolledTimeline",
        Accept::Boolean,
    ),
    ("prefersFTC", "prefersFTC", Accept::Boolean),
    (
        "timestamps",
        "timestamps",
        Accept::Member(Timestamps::is_member),
    ),
];

/// What V8 prints for a primitive in an `in` or property-assignment TypeError.
fn describe_primitive(value: &JsValue) -> String {
    match value {
        JsValue::Undefined => "undefined".to_owned(),
        JsValue::Null => "null".to_owned(),
        JsValue::Bool(b) => if *b { "true" } else { "false" }.to_owned(),
        JsValue::Number(x) => number::to_string(*x),
        JsValue::String(s) => s.to_string_lossy(),
        JsValue::BigInt(b) => format!("{b}"),
        _ => "[object Object]".to_owned(),
    }
}

/// The assignments `mergePreferences(base, newPrefs)` makes, in order, as
/// `(basePrefs key, value)`. Throws as the `in` operator does when `newPrefs` is not an object.
fn accepted_assignments(new_prefs: &JsValue) -> Result<Vec<(&'static str, &JsValue)>, JsError> {
    let object = match new_prefs {
        JsValue::Object(object) => Some(object),
        JsValue::Array(_) | JsValue::Bytes(_) | JsValue::Date(_) | JsValue::ExtData(..) => None,
        primitive => {
            return Err(JsError::type_error(format!(
                "Cannot use 'in' operator to search for '{}' in {}",
                RULES[0].0,
                describe_primitive(primitive)
            )));
        }
    };
    let Some(object) = object else {
        return Ok(Vec::new());
    };
    Ok(RULES
        .iter()
        .filter_map(|(source, target, accept)| {
            object
                .get(source)
                .filter(|value| accept.matches(value))
                .map(|value| (*target, value))
        })
        .collect())
}

/// `mergePreferences(basePrefs, newPrefs)` on raw values, mutating `base` in place: assigned keys
/// keep their position, new ones are appended, unknown keys stay.
///
/// `main.ts` is bundled as sloppy-mode CommonJS, so an assignment to a property of a number,
/// string or boolean base is silently dropped, while a `null` or `undefined` base throws.
pub fn merge_preferences(base: &mut JsValue, new_prefs: &JsValue) -> Result<(), JsError> {
    for (key, value) in accepted_assignments(new_prefs)? {
        match base {
            JsValue::Object(object) => {
                object.insert(key, value.clone());
            }
            JsValue::Null | JsValue::Undefined => {
                return Err(JsError::type_error(format!(
                    "Cannot set properties of {} (setting '{key}')",
                    describe_primitive(base)
                )));
            }
            _ => {}
        }
    }
    Ok(())
}

impl Preferences {
    /// `mergePreferences(this, newPrefs)`.
    pub fn merge(&mut self, new_prefs: &JsValue) -> Result<(), JsError> {
        for (key, value) in accepted_assignments(new_prefs)? {
            self.assign(key, value);
        }
        Ok(())
    }

    /// The defaults merged with a prefs object, which is how the app reads prefs.json. Upstream
    /// passes the raw file around instead, so a field that holds an invalid value there reads as
    /// its default here; every writer upstream stores valid values.
    pub fn from_js(value: &JsValue) -> Preferences {
        let mut prefs = Preferences::default();
        let _ = prefs.merge(value);
        prefs
    }

    /// Applies one assignment `accepted_assignments` produced, so `value` already has the type
    /// the key needs.
    fn assign(&mut self, key: &str, value: &JsValue) {
        fn set<T>(field: &mut T, parse: fn(&JsString) -> Option<T>, value: &JsValue) {
            if let Some(v) = value.as_str().and_then(parse) {
                *field = v;
            }
        }
        let string = || value.as_str().cloned().unwrap_or_default();
        let number = value.as_f64().unwrap_or(f64::NAN);
        let boolean = value.as_bool().unwrap_or(false);
        match key {
            "theme" => set(&mut self.theme, Theme::from_js_str, value),
            "robotAddress" => self.robot_address = string(),
            "remotePath" => self.remote_path = string(),
            "liveMode" => set(&mut self.live_mode, LiveMode::from_js_str, value),
            "liveSubscribeMode" => set(
                &mut self.live_subscribe_mode,
                LiveSubscribeMode::from_js_str,
                value,
            ),
            "liveDiscard" => self.live_discard = number,
            "publishFilter" => self.publish_filter = string(),
            "rlogPort" => self.rlog_port = number,
            "coordinateSystem" => set(
                &mut self.coordinate_system,
                CoordinateSystemPreference::from_js_str,
                value,
            ),
            "field3dModeAc" => set(&mut self.field3d_mode_ac, Field3dMode::from_js_str, value),
            "field3dModeBattery" => set(
                &mut self.field3d_mode_battery,
                Field3dBatteryMode::from_js_str,
                value,
            ),
            "field3dAntialiasing" => self.field3d_antialiasing = boolean,
            "tbaApiKey" => self.tba_api_key = string(),
            "userAssetsFolder" => self.user_assets_folder = Some(string()),
            "skipHootNonProWarning" => self.skip_hoot_non_pro_warning = boolean,
            "skipFRCCoordinateSystemWarning" => self.skip_frc_coordinate_system_warning = boolean,
            "skipFTCCoordinateSystemWarning" => self.skip_ftc_coordinate_system_warning = boolean,
            "ctreLicenseAccepted" => self.ctre_license_accepted = boolean,
            "systemcoreStaticAddress" => set(
                &mut self.systemcore_static_address,
                SystemcoreStaticAddress::from_js_str,
                value,
            ),
            "hasScrolledLineGraph" => self.has_scrolled_line_graph = boolean,
            "hasScrolledTimeline" => self.has_scrolled_timeline = boolean,
            "prefersFTC" => self.prefers_ftc = boolean,
            "timestamps" => set(&mut self.timestamps, Timestamps::from_js_str, value),
            _ => unreachable!("mergePreferences assigns no key {key:?}"),
        }
    }

    /// `getRobotAddress(preferences, platform)`, `platform` being a `process.platform` value.
    pub fn robot_address_for(&self, platform: &str) -> JsString {
        match self.systemcore_static_address {
            SystemcoreStaticAddress::Usb if platform == "win32" => JsString::from("172.26.0.1"),
            SystemcoreStaticAddress::Usb => JsString::from("172.27.0.1"),
            SystemcoreStaticAddress::Wifi => JsString::from("172.30.0.1"),
            SystemcoreStaticAddress::Off => self.robot_address.clone(),
        }
    }

    /// The preferences `app.whenReady` settles on and writes back: the defaults (theme "light"
    /// on Linux), merged with the existing file when there is one, with a `userAssetsFolder`
    /// that `folder_exists` rejects reset to `null`. `file` is the parsed prefs.json, `None` when
    /// it does not exist. An error means upstream's startup threw before creating any window.
    pub fn startup(
        platform: &str,
        file: Option<&JsValue>,
        folder_exists: impl FnOnce(&JsString) -> bool,
    ) -> Result<Preferences, JsError> {
        let mut prefs = Preferences::default();
        if platform == "linux" {
            prefs.theme = Theme::Light;
        }
        if let Some(file) = file {
            prefs.merge(file)?;
            if prefs
                .user_assets_folder
                .as_ref()
                .is_some_and(|folder| !folder_exists(folder))
            {
                prefs.user_assets_folder = None;
            }
        }
        Ok(prefs)
    }
}
