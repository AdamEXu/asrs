//! `preferences.ts` and the table of `preferences.html`.

use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;

use super::{
    Control, ControlValue, PromptEvent, PromptPage, TableRow, events, focus_message, text_value,
};
use crate::form::{Select, SelectOption};

const LINUX_THEMES: [(&str, &str); 3] = [
    ("system", "Match System"),
    ("light", "Light"),
    ("dark", "Dark"),
];

fn select(options: &[(&str, &str)]) -> Select {
    Select::with_options(
        options
            .iter()
            .map(|(value, text)| SelectOption::new(Some(value), text))
            .collect(),
    )
}

/// `select.value = value`.
fn assign(select: &mut Select, value: &JsValue) {
    select.set_value(&ops::to_string(value));
}

/// `input.value = value`, whose `null` is the empty string.
fn assign_text(text: &mut JsString, value: &JsValue) {
    *text = if value.is_null() {
        JsString::new()
    } else {
        text_value(&ops::to_string(value))
    };
}

/// `value.toString()`.
fn method_to_string(value: &JsValue) -> Result<JsString, JsError> {
    if matches!(value, JsValue::Undefined | JsValue::Null) {
        ops::get(value, "toString")?;
    }
    Ok(ops::to_string(value))
}

/// `value` when it is one of the strings `close` knows, else `default`.
fn known(value: &JsString, default: &str, others: &[&str]) -> JsValue {
    JsValue::string(
        others
            .iter()
            .find(|k| *value == JsString::from(**k))
            .unwrap_or(&default),
    )
}

#[derive(Clone, Debug)]
pub struct Preferences {
    blurred: bool,
    linux: bool,
    theme: Select,
    coordinate_system: Select,
    timestamps: Select,
    tba_api_key: JsString,
    robot_address: JsString,
    remote_path: JsString,
    live_subscribe_mode: Select,
    live_discard: Select,
    publish_filter: JsString,
    field3d_mode_ac: Select,
    field3d_mode_battery: Select,
    field3d_antialiasing: Select,
    /// The `oldPrefs` of every message that attached the exit triggers.
    old_prefs: Vec<JsValue>,
}

impl Default for Preferences {
    fn default() -> Preferences {
        Preferences {
            blurred: false,
            linux: false,
            theme: select(&[
                ("system", "Match System"),
                ("light", "Always Light"),
                ("dark", "Always Dark"),
            ]),
            coordinate_system: select(&[
                ("automatic", "Automatic"),
                ("center-red", "Center/Red (Systemcore)"),
                ("wall-blue", "Blue Wall (FRC 2023-2026)"),
                ("wall-alliance", "Alliance Wall (FRC 2022)"),
                ("center-rotated", "Center/Rotated (FTC Traditional)"),
            ]),
            timestamps: select(&[("start-zero", "Start at Zero"), ("original", "Original")]),
            tba_api_key: JsString::new(),
            robot_address: JsString::new(),
            remote_path: JsString::new(),
            live_subscribe_mode: select(&[
                ("low-bandwidth", "Low Bandwidth"),
                ("logging", "Logging"),
            ]),
            live_discard: select(&[
                ("60", "After 1 Minute"),
                ("300", "After 5 Minutes"),
                ("1200", "After 20 Minutes"),
                ("-1", "Never"),
            ]),
            publish_filter: JsString::new(),
            field3d_mode_ac: select(&[
                ("cinematic", "Cinematic"),
                ("standard", "Standard"),
                ("low-power", "Low Power"),
            ]),
            field3d_mode_battery: select(&[
                ("", "Default"),
                ("cinematic", "Cinematic"),
                ("standard", "Standard"),
                ("low-power", "Low Power"),
            ]),
            field3d_antialiasing: select(&[("true", "On"), ("false", "Off")]),
            old_prefs: Vec::new(),
        }
    }
}

impl Preferences {
    fn receive(&mut self, data: &JsValue) -> Result<(), JsError> {
        if let Some(focused) = focus_message(data)? {
            self.blurred = !focused;
            return Ok(());
        }
        let platform = ops::get(data, "platform")?;
        let old = ops::get(data, "prefs")?;
        if platform == JsValue::string("linux") {
            self.linux = true;
            let index = self.theme.selected_index();
            self.theme = select(&LINUX_THEMES);
            self.theme.set_selected_index(index);
        }
        let get = |key| ops::get(&old, key);
        assign(&mut self.theme, &get("theme")?);
        assign_text(&mut self.robot_address, &get("robotAddress")?);
        assign_text(&mut self.remote_path, &get("remotePath")?);
        assign(&mut self.live_subscribe_mode, &get("liveSubscribeMode")?);
        let discard = method_to_string(&get("liveDiscard")?)?;
        self.live_discard.set_value(&discard);
        assign_text(&mut self.publish_filter, &get("publishFilter")?);
        assign(&mut self.coordinate_system, &get("coordinateSystem")?);
        assign(&mut self.timestamps, &get("timestamps")?);
        assign(&mut self.field3d_mode_ac, &get("field3dModeAc")?);
        assign(&mut self.field3d_mode_battery, &get("field3dModeBattery")?);
        let antialiasing = method_to_string(&get("field3dAntialiasing")?)?;
        self.field3d_antialiasing.set_value(&antialiasing);
        assign_text(&mut self.tba_api_key, &get("tbaApiKey")?);
        self.old_prefs.push(old);
        Ok(())
    }

    /// The `newPrefs` literal of `close(true)`, whose key order `prefs.json` gets.
    fn new_prefs(&self, old: &JsValue) -> Result<JsValue, JsError> {
        let kept = |key| ops::get(old, key);
        let text = |value: &JsString| JsValue::String(value.clone());
        Ok(ops::object([
            (
                "theme",
                known(&self.theme.value(), "system", &["light", "dark"]),
            ),
            ("robotAddress", text(&self.robot_address)),
            ("remotePath", text(&self.remote_path)),
            ("liveMode", kept("liveMode")?),
            (
                "liveSubscribeMode",
                known(
                    &self.live_subscribe_mode.value(),
                    "low-bandwidth",
                    &["logging"],
                ),
            ),
            (
                "liveDiscard",
                JsValue::Number(ops::value_to_number(&JsValue::String(
                    self.live_discard.value(),
                ))?),
            ),
            ("publishFilter", text(&self.publish_filter)),
            ("rlogPort", kept("rlogPort")?),
            (
                "coordinateSystem",
                known(
                    &self.coordinate_system.value(),
                    "automatic",
                    &["wall-alliance", "wall-blue", "center-rotated", "center-red"],
                ),
            ),
            (
                "field3dModeAc",
                known(
                    &self.field3d_mode_ac.value(),
                    "standard",
                    &["cinematic", "low-power"],
                ),
            ),
            (
                "field3dModeBattery",
                known(
                    &self.field3d_mode_battery.value(),
                    "",
                    &["cinematic", "standard", "low-power"],
                ),
            ),
            (
                "field3dAntialiasing",
                JsValue::Bool(self.field3d_antialiasing.value() == JsString::from("true")),
            ),
            ("tbaApiKey", text(&self.tba_api_key)),
            ("userAssetsFolder", kept("userAssetsFolder")?),
            ("skipHootNonProWarning", kept("skipHootNonProWarning")?),
            (
                "skipFRCCoordinateSystemWarning",
                kept("skipFRCCoordinateSystemWarning")?,
            ),
            (
                "skipFTCCoordinateSystemWarning",
                kept("skipFTCCoordinateSystemWarning")?,
            ),
            ("ctreLicenseAccepted", kept("ctreLicenseAccepted")?),
            ("systemcoreStaticAddress", kept("systemcoreStaticAddress")?),
            ("hasScrolledLineGraph", kept("hasScrolledLineGraph")?),
            ("hasScrolledTimeline", kept("hasScrolledTimeline")?),
            ("prefersFTC", kept("prefersFTC")?),
            (
                "timestamps",
                known(&self.timestamps.value(), "start-zero", &["original"]),
            ),
        ]))
    }
}

impl PromptPage for Preferences {
    fn title(&self) -> &'static str {
        "Preferences"
    }

    fn controls(&self) -> Vec<Control> {
        let select = |id, label, select: &Select| {
            Control::new(id, label, ControlValue::Select(select.clone()))
        };
        let text = |id, label, value: &JsString| {
            Control::new(id, label, ControlValue::Text(value.clone()))
        };
        let hidden: &[usize] = if self.linux { &[0] } else { &[] };
        vec![
            select("theme", "Application Theme", &self.theme).hidden_options(hidden),
            select(
                "coordinateSystem",
                "Coordinate System",
                &self.coordinate_system,
            ),
            select("timestamps", "Timestamps", &self.timestamps),
            text("tbaApiKey", "TBA API Key", &self.tba_api_key),
            text("robotAddress", "Robot Address", &self.robot_address).placeholder("robot.local"),
            text("remotePath", "Robot Log Path", &self.remote_path).placeholder("/U/logs"),
            select("liveSubscribeMode", "Live Mode", &self.live_subscribe_mode),
            select("liveDiscard", "Discard Live Data", &self.live_discard),
            text("publishFilter", "NT Publish Prefixes", &self.publish_filter),
            select("field3dModeAc", "3D Mode (Default)", &self.field3d_mode_ac),
            select(
                "field3dModeBattery",
                "3D Mode (Battery)",
                &self.field3d_mode_battery,
            ),
            select(
                "field3dAntialiasing",
                "3D Antialiasing",
                &self.field3d_antialiasing,
            ),
        ]
    }

    fn rows(&self) -> Vec<TableRow> {
        let controls = |n| std::iter::repeat_n(TableRow::Control, n);
        controls(4)
            .chain([
                TableRow::Title("Networking Options"),
                TableRow::Subtitle("The live source can be changed in the \"File\" menu."),
            ])
            .chain(controls(5))
            .chain([TableRow::Title("3D Rendering Options")])
            .chain(controls(3))
            .collect()
    }

    fn blurred(&self) -> bool {
        self.blurred
    }

    fn message(&mut self, data: &JsValue) -> Vec<PromptEvent> {
        events(self.receive(data).map(|()| Vec::new()))
    }

    fn edit(&mut self, id: &str, visible: &JsString) {
        let text = match id {
            "tbaApiKey" => &mut self.tba_api_key,
            "robotAddress" => &mut self.robot_address,
            "remotePath" => &mut self.remote_path,
            "publishFilter" => &mut self.publish_filter,
            _ => return,
        };
        *text = text_value(visible);
    }

    fn pick(&mut self, id: &str, index: usize) -> Vec<PromptEvent> {
        let select = match id {
            "theme" => &mut self.theme,
            "coordinateSystem" => &mut self.coordinate_system,
            "timestamps" => &mut self.timestamps,
            "liveSubscribeMode" => &mut self.live_subscribe_mode,
            "liveDiscard" => &mut self.live_discard,
            "field3dModeAc" => &mut self.field3d_mode_ac,
            "field3dModeBattery" => &mut self.field3d_mode_battery,
            "field3dAntialiasing" => &mut self.field3d_antialiasing,
            _ => return Vec::new(),
        };
        select.set_selected_index(index as i32);
        Vec::new()
    }

    fn click_exit(&mut self) -> Vec<PromptEvent> {
        self.old_prefs
            .iter()
            .map(|old| PromptEvent::Reply(old.clone()))
            .collect()
    }

    fn click_confirm(&mut self) -> Vec<PromptEvent> {
        self.old_prefs
            .iter()
            .map(|old| {
                self.new_prefs(old)
                    .map_or_else(PromptEvent::Error, PromptEvent::Reply)
            })
            .collect()
    }
}
