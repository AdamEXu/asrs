#![allow(clippy::disallowed_methods)]

//! Replays `files` vectors against `asrs_services`' persistence port. Each record's `files`,
//! `writes`, `created`, `closed`, `sent`, `result` and `throw` are produced by the port from
//! the same inputs the oracle's fake `fs`/`electron` saw, then compared byte for byte.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::collections::HashMap;

use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{NamedZone, SetClock};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_model::json_file;
use asrs_model::preferences::Preferences;
use asrs_parity::cve1::Value;
use asrs_parity::json::Json;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::{self, Record};
use asrs_services::beta::{self, BetaConfig};
use asrs_services::constants;
use asrs_services::ds_layout;
use asrs_services::fs::{Fs, MemFs};
use asrs_services::layout;
use asrs_services::recent_units;
use asrs_services::state_tracker::{self, StateTracker};
use asrs_services::type_memory;
use asrs_services::windows::{Rect, WindowSystem};

#[derive(Clone, Copy, PartialEq)]
enum WindowKind {
    Hub,
    Satellite,
    Other,
}

struct TestWindow {
    rect: Rect,
}

struct TestWindows {
    order: Vec<u64>,
    map: HashMap<u64, TestWindow>,
    displays: Vec<Rect>,
    primary: usize,
}

/// `windows` map key: SameValueZero collapses -0 and treats NaN === NaN.
fn window_key(id: f64) -> u64 {
    if id == 0.0 {
        0.0f64.to_bits()
    } else {
        id.to_bits()
    }
}

impl Default for TestWindows {
    fn default() -> TestWindows {
        TestWindows {
            order: Vec::new(),
            map: HashMap::new(),
            displays: vec![Rect {
                x: 0.0,
                y: 0.0,
                width: 1920.0,
                height: 1080.0,
            }],
            primary: 0,
        }
    }
}

impl TestWindows {
    fn insert(&mut self, id: f64, window: TestWindow) {
        let key = window_key(id);
        if !self.map.contains_key(&key) {
            self.order.push(key);
        }
        self.map.insert(key, window);
    }

    fn remove(&mut self, id: f64) -> bool {
        let key = window_key(id);
        if self.map.remove(&key).is_some() {
            self.order.retain(|k| *k != key);
            true
        } else {
            false
        }
    }
}

impl WindowSystem for TestWindows {
    fn window_bounds(&self, id: f64) -> Option<Rect> {
        self.map.get(&window_key(id)).map(|w| w.rect)
    }

    fn displays(&self) -> Vec<Rect> {
        self.displays.clone()
    }

    fn primary_display(&self) -> Rect {
        self.displays[self.primary]
    }
}

struct Env {
    fs: MemFs,
    ws: TestWindows,
    hubs: Vec<f64>,
    satellites: Vec<(String, Vec<f64>)>,
    beta_welcomes: Vec<f64>,
    ports: Vec<f64>,
    first_load: Vec<f64>,
    window_states: HashMap<u64, JsValue>,
    next_id: f64,
    created: Vec<f64>,
    closed: Vec<f64>,
    sent: Vec<(JsString, JsValue)>,
    trackers: Vec<StateTracker>,
    clock: SetClock,
    zone: NamedZone,
    packaged: bool,
    app_version: String,
    beta: Option<BetaConfig>,
    prefs_file: String,
    state_file: String,
    tm_file: String,
    recents_file: String,
    beta_file: String,
}

const USER_DATA: &str = "/fake/userData";

/// The oracle's fake fs sees POSIX `path.join` results.
fn posix(path: String) -> String {
    path.replace(std::path::MAIN_SEPARATOR, "/")
}
const TEMP: &str = "/fake/temp";

impl Env {
    fn new(zone: NamedZone) -> Env {
        Env {
            fs: MemFs::new(),
            ws: TestWindows::default(),
            hubs: Vec::new(),
            satellites: Vec::new(),
            beta_welcomes: Vec::new(),
            ports: Vec::new(),
            first_load: Vec::new(),
            window_states: HashMap::new(),
            next_id: 1.0,
            created: Vec::new(),
            closed: Vec::new(),
            sent: Vec::new(),
            trackers: Vec::new(),
            clock: SetClock(0.0),
            beta: beta::beta_config(true, &zone),
            zone,
            packaged: false,
            app_version: String::new(),
            beta_file: posix(constants::beta_filename(USER_DATA, "2027")),
            prefs_file: posix(constants::prefs_filename(USER_DATA)),
            state_file: String::new(),
            tm_file: posix(constants::type_memory_filename(USER_DATA)),
            recents_file: posix(constants::recent_units_filename(USER_DATA)),
        }
    }

    /// `new StateTracker()` at module load, deferred until the `config` record names the state
    /// file (the packaged spec runs a different `appVersion`).
    fn configure(&mut self, app_version: &str, packaged: bool) {
        self.app_version = app_version.to_owned();
        self.packaged = packaged;
        self.state_file = posix(constants::state_filename(USER_DATA, app_version));
        self.trackers.insert(
            0,
            StateTracker::new(&self.fs, &self.ws, &self.state_file).expect("main tracker"),
        );
    }

    fn create_window(&mut self, kind: WindowKind, rect: Rect, state: Option<JsValue>) -> f64 {
        let id = self.next_id;
        self.next_id += 1.0;
        self.ws.insert(id, TestWindow { rect });
        self.created.push(id);
        match kind {
            WindowKind::Hub => {
                self.hubs.push(id);
                self.ports.push(id);
                self.first_load.push(id);
            }
            WindowKind::Satellite => {
                if let Some(state) = &state
                    && let Some(uuid) = ops::get(state, "uuid")
                        .ok()
                        .and_then(|u| u.as_str().cloned())
                {
                    let uuid_str = uuid.to_string_lossy();
                    match self.satellites.iter_mut().find(|(u, _)| *u == uuid_str) {
                        Some((_, ids)) => ids.push(id),
                        None => self.satellites.push((uuid_str, vec![id])),
                    }
                    let mut collection = JsValue::Object(JsObject::new());
                    for (u, ids) in &self.satellites {
                        let arr: Vec<JsValue> = ids
                            .iter()
                            .map(|id| {
                                let mut o = JsObject::new();
                                o.insert(JsString::from("id"), JsValue::Number(*id));
                                JsValue::Object(o)
                            })
                            .collect();
                        ops::set_key(
                            &mut collection,
                            &JsString::from(u.as_str()),
                            JsValue::Array(arr),
                        )
                        .expect("collection set");
                    }
                    let _ = self.trackers[0].save_satellite_ids(&collection);
                }
            }
            WindowKind::Other => {}
        }
        if let Some(state) = state {
            self.window_states.insert(window_key(id), state);
        }
        id
    }

    /// `window.close()`: removes the window and splices it out of `hubWindows` /
    /// `satelliteWindows` by `indexOf`.
    fn close_window(&mut self, id: f64) {
        let key = window_key(id);
        if self.ws.map.remove(&key).is_none() {
            return;
        }
        self.ws.order.retain(|k| *k != key);
        self.closed.push(id);
        if let Some(index) = self.hubs.iter().position(|w| *w == id) {
            self.hubs.remove(index);
        }
        for (_, ids) in self.satellites.iter_mut() {
            if let Some(index) = ids.iter().position(|w| *w == id) {
                ids.remove(index);
            }
        }
    }

    /// `dom.destroy()` semantics for conversion windows: gone, but not in `closed`.
    fn destroy_window(&mut self, id: f64) {
        let key = window_key(id);
        self.ws.map.remove(&key);
        self.ws.order.retain(|k| *k != key);
    }

    fn rect_from(&self, entry: &JsValue) -> Rect {
        fn num(v: &JsValue) -> f64 {
            match v {
                JsValue::Undefined | JsValue::Null => 0.0,
                other => ops::value_to_number(other).unwrap_or(f64::NAN),
            }
        }
        Rect {
            x: num(&ops::get(entry, "x").unwrap_or_default()),
            y: num(&ops::get(entry, "y").unwrap_or_default()),
            width: num(&ops::get(entry, "width").unwrap_or_default()),
            height: num(&ops::get(entry, "height").unwrap_or_default()),
        }
    }

    /// `app.whenReady`: prefs startup write, state load, window creation.
    fn ready(&mut self) -> Result<(), JsError> {
        let prefs = if !self.fs.exists(&self.prefs_file) {
            Preferences::default()
        } else {
            let file = json_file::parse(&self.fs.read(&self.prefs_file)?)?;
            Preferences::startup("darwin", Some(&file), |folder| {
                self.fs.exists(&folder.to_string_lossy())
            })?
        };
        self.fs
            .write(&self.prefs_file, &json_file::stringify(&prefs.to_js())?)?;
        let application_state =
            state_tracker::saved_application_state(&self.fs, &self.ws, &self.state_file, true)?;
        if application_state.is_null() || array_len(&application_state, "hubs")? == 0 {
            self.create_window(
                WindowKind::Hub,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    width: constants::HUB_DEFAULT_WIDTH,
                    height: constants::HUB_DEFAULT_HEIGHT,
                },
                None,
            );
        } else {
            for hub in array_items(&application_state, "hubs")? {
                let rect = self.rect_from(&hub);
                self.create_window(WindowKind::Hub, rect, Some(hub));
            }
            for satellite in array_items(&application_state, "satellites")? {
                let rect = self.rect_from(&satellite);
                self.create_window(WindowKind::Satellite, rect, Some(satellite));
            }
        }
        Ok(())
    }

    /// `webContents.emit("dom-ready")`: hubs get the restore sends plus the beta block; a
    /// beta welcome window's dom-ready creates its port (any later message completes the
    /// welcome).
    fn dom_ready(&mut self, id: f64) -> Result<(), JsError> {
        if self.beta_welcomes.contains(&id) {
            self.ports.push(id);
            return Ok(());
        }
        if !self.hubs.contains(&id) {
            return Ok(());
        }
        if self.fs.exists(&self.tm_file) {
            let memory = json_file::parse(&self.fs.read(&self.tm_file)?)?;
            self.sent
                .push((JsString::from("restore-type-memory"), memory));
        }
        let first = self.first_load.contains(&id);
        let created_state = self.window_states.get(&window_key(id)).cloned();
        let restore = match (first, &created_state) {
            (true, Some(state)) => Some(ops::get(state, "state")?),
            _ => match self.trackers[0].get_renderer_state(id) {
                JsValue::Undefined => None,
                cached => Some(cached),
            },
        };
        if let Some(state) = restore {
            self.sent.push((JsString::from("restore-state"), state));
        }
        self.first_load.retain(|w| *w != id);
        if self.beta.is_some() {
            if beta::is_beta_expired(self.beta.as_ref(), &self.clock) {
                // Expired builds only show a dialog; the response quits the app.
            } else if !ops::to_boolean(&beta::is_beta_welcome_complete(
                &self.fs,
                Some(&self.beta_file),
            )?) {
                let welcome = self.create_window(
                    WindowKind::Other,
                    Rect {
                        x: 0.0,
                        y: 0.0,
                        width: 0.0,
                        height: 0.0,
                    },
                    None,
                );
                self.beta_welcomes.push(welcome);
            } else if beta::should_prompt_beta_survey(
                &self.fs,
                Some(&self.beta_file),
                self.beta.as_ref(),
                &self.clock,
            )? {
                beta::open_beta_survey(
                    &self.fs,
                    Some(&self.beta_file),
                    self.beta.as_ref(),
                    |_| (),
                )?;
            }
        }
        Ok(())
    }

    /// `handleHubMessage`: the emitted name's file effects. The upstream handler is async, so
    /// a throw is an unhandled rejection: swallowed, with prior effects kept.
    fn emit(&mut self, id: f64, name: &JsString, data: &JsValue) -> Result<(), JsError> {
        if !self.ws.map.contains_key(&window_key(id)) || !self.ports.contains(&id) {
            return Ok(());
        }
        if self.beta_welcomes.contains(&id) {
            self.destroy_window(id);
            beta::save_beta_welcome_complete(&self.fs, Some(&self.beta_file))?;
            let _ = beta::should_prompt_beta_survey(
                &self.fs,
                Some(&self.beta_file),
                self.beta.as_ref(),
                &self.clock,
            )?;
            return Ok(());
        }
        if name.to_string_lossy() == "save-state" {
            self.trackers[0].save_renderer_state(id, data.clone());
        } else if name.to_string_lossy() == "save-type-memory" {
            let _ = type_memory::save(&self.fs, &self.tm_file, data);
        }
        Ok(())
    }

    /// An "Edit Conversion..." click that the conversion window answers with `preset`. A
    /// `null` answer cancels: `if (newPreset === null) return;`.
    fn click_edit_conversion(&mut self, preset: &JsValue) -> Result<(), JsError> {
        let id = self.create_window(
            WindowKind::Other,
            Rect {
                x: 0.0,
                y: 0.0,
                width: 0.0,
                height: 0.0,
            },
            None,
        );
        self.destroy_window(id);
        if preset.is_null() {
            return Ok(());
        }
        recent_units::update(&self.fs, &self.recents_file, preset)
    }

    /// A "Recent Presets" submenu click: `recentUnits[index]`, read fresh as upstream's menu did.
    fn click_recent(&mut self, index: f64) -> Result<(), JsError> {
        let recents = if self.fs.exists(&self.recents_file) {
            json_file::parse(&self.fs.read(&self.recents_file)?)?
        } else {
            JsValue::Array(Vec::new())
        };
        let Some(items) = recents.as_array() else {
            return Ok(());
        };
        let Some(preset) = items.get(index as usize) else {
            return Ok(());
        };
        recent_units::update(&self.fs, &self.recents_file, preset)
    }

    /// The "Export Layout..." click: `writeFile(path, {...state, version}, {spaces: 2})`.
    fn export_layout(&mut self, path: &str) -> Result<(), JsError> {
        let mut state = self.trackers[0].current_application_state(&self.ws);
        let bytes = layout::export_bytes(&mut state, &self.app_version)?;
        self.fs.write(path, &bytes)
    }

    /// The "Import Layout..." click: merge, version gate, close windows, recreate them.
    fn import_layout(&mut self, paths: &[String], answer: f64) -> Result<(), JsError> {
        let Ok(outcome) = layout::import(&self.fs, paths, self.packaged, &self.app_version) else {
            return Ok(());
        };
        let layout::Import::Windows {
            state,
            version_mismatch,
        } = outcome
        else {
            return Ok(());
        };
        if version_mismatch && answer != 0.0 {
            return Ok(());
        }
        let hubs_len = self.hubs.len();
        for i in 0..hubs_len {
            if let Some(&id) = self.hubs.get(i) {
                self.close_window(id);
            }
        }
        for index in 0..self.satellites.len() {
            let len = self.satellites[index].1.len();
            for i in 0..len {
                if let Some(&id) = self.satellites[index].1.get(i) {
                    self.close_window(id);
                }
            }
        }
        for hub in array_items(&state, "hubs")? {
            let rect = self.rect_from(&hub);
            self.create_window(WindowKind::Hub, rect, Some(hub));
        }
        for satellite in array_items(&state, "satellites")? {
            let rect = self.rect_from(&satellite);
            self.create_window(WindowKind::Satellite, rect, Some(satellite));
        }
        Ok(())
    }

    /// "Reset to DS Layout"; `answer` 0 is the message box's Reset: `sendMessage(window, "restore-state",
    /// DEFAULT_DS_LAYOUT)`.
    fn ds_layout(&mut self, id: f64, answer: f64) -> Result<(), JsError> {
        if !self.hubs.contains(&id) || answer != 0.0 {
            return Ok(());
        }
        self.sent.push((
            JsString::from("restore-state"),
            ds_layout::default_ds_layout(true).to_js(),
        ));
        Ok(())
    }
}

fn array_len(value: &JsValue, key: &str) -> Result<usize, JsError> {
    let v = ops::get(value, key)?;
    match v.as_array() {
        Some(items) => Ok(items.len()),
        None => Err(JsError::type_error(format!(
            "Cannot read properties of {} (reading 'length')",
            if v.is_null() { "null" } else { "undefined" }
        ))),
    }
}

fn array_items(value: &JsValue, key: &str) -> Result<Vec<JsValue>, JsError> {
    let v = ops::get(value, key)?;
    match v.as_array() {
        Some(items) => Ok(items.to_vec()),
        None => Err(JsError::type_error(format!(
            "Cannot read properties of {} (reading 'forEach')",
            if v.is_null() { "null" } else { "undefined" }
        ))),
    }
}

fn js_string(value: &JsValue) -> Result<String, JsError> {
    Ok(ops::to_string(value).to_string_lossy())
}

fn js_number(value: &JsValue) -> Result<f64, JsError> {
    ops::value_to_number(value)
}

fn js_bool(value: &JsValue) -> bool {
    ops::to_boolean(value)
}

fn rects_of(value: &JsValue) -> Result<Vec<Rect>, JsError> {
    let v = ops::get_key(value, &JsString::from("displays"))?;
    let Some(items) = v.as_array() else {
        return Ok(Vec::new());
    };
    let mut rects = Vec::with_capacity(items.len());
    for item in items {
        rects.push(Rect {
            x: js_number(&ops::get(item, "x")?)?,
            y: js_number(&ops::get(item, "y")?)?,
            width: js_number(&ops::get(item, "width")?)?,
            height: js_number(&ops::get(item, "height")?)?,
        });
    }
    Ok(rects)
}

fn js_bytes(value: &JsValue) -> Result<Vec<u8>, JsError> {
    match value {
        JsValue::Bytes(bytes) => Ok(bytes.clone()),
        _ => Err(JsError::type_error("expected bytes")),
    }
}

fn dispatch(env: &mut Env, op: &str, args: &JsValue) -> Result<JsValue, JsError> {
    let get = |key: &str| -> Result<JsValue, JsError> { ops::get(args, key) };
    match op {
        "config" => Ok(ops::object([
            ("userData", JsValue::string(USER_DATA)),
            ("temp", JsValue::string(TEMP)),
            ("packaged", JsValue::Bool(env.packaged)),
            ("appVersion", JsValue::string(env.app_version.as_str())),
            ("prefsFile", JsValue::string(env.prefs_file.as_str())),
            ("stateFile", JsValue::string(env.state_file.as_str())),
            ("typeMemoryFile", JsValue::string(env.tm_file.as_str())),
            (
                "recentUnitsFile",
                JsValue::string(env.recents_file.as_str()),
            ),
            ("betaFile", JsValue::string(env.beta_file.as_str())),
        ])),
        "putFile" => {
            let name = js_string(&get("name")?)?;
            let data = js_bytes(&get("data")?)?;
            env.fs.put(&name, &data);
            Ok(JsValue::Null)
        }
        "removeFile" => {
            env.fs.remove(&js_string(&get("name")?)?);
            Ok(JsValue::Null)
        }
        "setDisplays" => {
            env.ws.displays = rects_of(args)?;
            env.ws.primary = js_number(&get("primary")?)? as usize;
            Ok(JsValue::Null)
        }
        "setWindow" => {
            let id = js_number(&get("id")?)?;
            let bounds = get("bounds")?;
            if bounds.is_null() {
                env.ws.remove(id);
            } else {
                env.ws.insert(
                    id,
                    TestWindow {
                        rect: env.rect_from(&bounds),
                    },
                );
            }
            Ok(JsValue::Null)
        }
        "setNow" => {
            env.clock = SetClock(js_number(&get("ms")?)?);
            Ok(JsValue::Null)
        }
        "newTracker" => {
            env.trackers
                .push(StateTracker::new(&env.fs, &env.ws, &env.state_file)?);
            Ok(JsValue::Number((env.trackers.len() - 1) as f64))
        }
        "getSaved" => {
            let tracker = js_number(&get("tracker")?)? as usize;
            env.trackers[tracker].get_saved_application_state(
                &env.fs,
                &env.ws,
                js_bool(&get("allowReset")?),
            )
        }
        "getCurrent" => {
            let tracker = js_number(&get("tracker")?)? as usize;
            Ok(env.trackers[tracker].current_application_state(&env.ws))
        }
        "saveRendererState" => {
            let tracker = js_number(&get("tracker")?)? as usize;
            env.trackers[tracker].save_renderer_state(js_number(&get("id")?)?, get("state")?);
            Ok(JsValue::Null)
        }
        "getRendererState" => {
            let tracker = js_number(&get("tracker")?)? as usize;
            Ok(env.trackers[tracker].get_renderer_state(js_number(&get("id")?)?))
        }
        "saveSatelliteIds" => {
            let tracker = js_number(&get("tracker")?)? as usize;
            env.trackers[tracker].save_satellite_ids(&get("windows")?)?;
            Ok(JsValue::Null)
        }
        "tickTracker" => {
            let tracker = js_number(&get("tracker")?)? as usize;
            env.trackers[tracker].tick(&env.fs, &env.ws)?;
            Ok(JsValue::Null)
        }
        "ready" => {
            let _ = env.ready();
            Ok(JsValue::Null)
        }
        "domReady" => {
            env.dom_ready(js_number(&get("window")?)?)?;
            Ok(JsValue::Null)
        }
        "emit" => {
            let name = get("name")?
                .as_str()
                .cloned()
                .unwrap_or_else(|| JsString::from(""));
            env.emit(js_number(&get("window")?)?, &name, &get("data")?)?;
            Ok(JsValue::Null)
        }
        "clickRecent" => {
            match get("index")? {
                JsValue::Number(index) => env.click_recent(index)?,
                _ => env.click_edit_conversion(&get("preset")?)?,
            }
            Ok(JsValue::Null)
        }
        "exportLayout" => {
            let _ = env.export_layout(&js_string(&get("path")?)?);
            Ok(JsValue::Null)
        }
        "importLayout" => {
            let mut paths = Vec::new();
            let paths_value = get("paths")?;
            if let Some(items) = paths_value.as_array() {
                for item in items {
                    paths.push(js_string(item)?);
                }
            }
            let _ = env.import_layout(&paths, js_number(&get("answer")?)?);
            Ok(JsValue::Null)
        }
        "dsLayout" => {
            env.ds_layout(js_number(&get("window")?)?, js_number(&get("answer")?)?)?;
            Ok(JsValue::Null)
        }
        "isBetaWelcomeComplete" => beta::is_beta_welcome_complete(&env.fs, Some(&env.beta_file)),
        "isBetaExpired" => Ok(JsValue::Bool(beta::is_beta_expired(
            env.beta.as_ref(),
            &env.clock,
        ))),
        "shouldPromptBetaSurvey" => Ok(JsValue::Bool(beta::should_prompt_beta_survey(
            &env.fs,
            Some(&env.beta_file),
            env.beta.as_ref(),
            &env.clock,
        )?)),
        "delayBetaSurvey" => {
            beta::delay_beta_survey(&env.fs, Some(&env.beta_file), &env.clock)?;
            Ok(JsValue::Null)
        }
        "openBetaSurvey" => {
            beta::open_beta_survey(&env.fs, Some(&env.beta_file), env.beta.as_ref(), |_| ())?;
            Ok(JsValue::Null)
        }
        "saveBetaWelcomeComplete" => {
            beta::save_beta_welcome_complete(&env.fs, Some(&env.beta_file))?;
            Ok(JsValue::Null)
        }
        "isBeta" => Ok(JsValue::Bool(env.beta.is_some())),
        "isAlpha" => Ok(JsValue::Bool(env.beta.as_ref().is_some_and(|c| c.is_alpha))),
        "exportName" => Ok(JsValue::string(&constants::layout_default_path(
            env.clock.0,
            &env.zone,
        ))),
        "defaultDsLayout" => Ok(ds_layout::default_ds_layout(true).to_js()),
        other => Err(JsError::new("Error", format!("unknown op {other}"))),
    }
}

/// Class instances replay as their own-property object: `Object.keys`, spread, and
/// `JSON.stringify` all act on own enumerable properties, which is all these files see.
fn demote(value: &Value) -> Value {
    match value {
        Value::Instance(_, pairs) => {
            Value::Object(pairs.iter().map(|(k, v)| (k.clone(), demote(v))).collect())
        }
        Value::Object(pairs) => {
            Value::Object(pairs.iter().map(|(k, v)| (k.clone(), demote(v))).collect())
        }
        Value::Array(items) => Value::Array(items.iter().map(demote).collect()),
        Value::Map(pairs) => {
            Value::Map(pairs.iter().map(|(k, v)| (demote(k), demote(v))).collect())
        }
        Value::Set(items) => Value::Set(items.iter().map(demote).collect()),
        Value::TypedArray(name, items) => {
            Value::TypedArray(name.clone(), items.iter().map(demote).collect())
        }
        other => other.clone(),
    }
}

fn expected_value(record: &Record, key: &str) -> Option<Value> {
    record.field(key).map(Value::from_json_lenient)
}

fn json_f64_array(record: &Record, key: &str) -> Vec<f64> {
    record
        .field(key)
        .and_then(Json::as_array)
        .map(|items| items.iter().filter_map(Json::as_f64).collect())
        .unwrap_or_default()
}

fn json_str_array(record: &Record, key: &str) -> Vec<String> {
    record
        .field(key)
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|j| j.as_str().map(|s| s.to_string_lossy()))
                .collect()
        })
        .unwrap_or_default()
}

fn files_value(env: &Env) -> Value {
    Value::Object(
        env.fs
            .dump()
            .into_iter()
            .map(|(name, bytes)| (Utf16String::from(name), Value::Bytes(bytes)))
            .collect(),
    )
}

fn sent_values(env: &Env) -> Vec<(String, Value)> {
    env.sent
        .iter()
        .map(|(name, data)| (name.to_string_lossy(), js_json::from_js(data)))
        .collect()
}

fn check_record(env: &mut Env, record: &Record) -> Result<(), String> {
    let op = record.string("op").unwrap_or_default();
    if op == "config"
        && let Ok(result) = record.value("result")
    {
        let packaged = result
            .get("packaged")
            .is_some_and(|v| *v == Value::Bool(true));
        let app_version = result
            .get("appVersion")
            .and_then(|v| {
                if let Value::String(s) = v {
                    Some(s.to_string_lossy())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        env.configure(&app_version, packaged);
    }
    let args = match record.value("args") {
        Ok(value) => js_json::to_js(&demote(&value)).map_err(|e| format!("args: {e}"))?,
        Err(_) => JsValue::Null,
    };
    let outcome = dispatch(env, &op, &args);
    let mut problems = Vec::new();
    match (&outcome, expected_value(record, "result")) {
        (Ok(actual), Some(expected)) => {
            if js_json::from_js(actual) != expected {
                problems.push(format!(
                    "result {} != expected {}",
                    describe(&js_json::from_js(actual)),
                    describe(&expected)
                ));
            }
        }
        (Ok(_), None) => {}
        (Err(e), _) => {
            let expected = record.string("throw").unwrap_or_default();
            if e.name != expected {
                problems.push(format!("threw {} expected {}", e.name, expected));
            }
        }
    }
    if let (Ok(_), Some(expected)) = (&outcome, record.string("throw")) {
        problems.push(format!("returned but expected throw {expected}"));
    }
    compare(
        &mut problems,
        "created",
        &json_f64_array(record, "created"),
        &env.created,
    );
    compare(
        &mut problems,
        "closed",
        &json_f64_array(record, "closed"),
        &env.closed,
    );
    let expected_sent: Vec<(String, Value)> = record
        .field("sent")
        .and_then(Json::as_array)
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item.get("name")
                            .and_then(|n| n.as_str().map(|s| s.to_string_lossy()))
                            .unwrap_or_default(),
                        item.get("data")
                            .map(Value::from_json_lenient)
                            .unwrap_or(Value::Null),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let actual_sent = sent_values(env);
    if actual_sent != expected_sent {
        problems.push(format!(
            "sent {actual_sent:?} != expected {expected_sent:?}"
        ));
    }
    let expected_writes = json_str_array(record, "writes");
    let actual_writes = env.fs.take_writes();
    if actual_writes != expected_writes {
        problems.push(format!(
            "writes {actual_writes:?} != expected {expected_writes:?}"
        ));
    }
    let expected_files = expected_value(record, "files").unwrap_or(Value::Object(Vec::new()));
    let actual_files = files_value(env);
    if actual_files != expected_files {
        problems.push(format!(
            "files {} != expected {}",
            describe(&actual_files),
            describe(&expected_files)
        ));
    }
    env.created.clear();
    env.closed.clear();
    env.sent.clear();
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("{op}: {}", problems.join("; ")))
    }
}

fn compare(problems: &mut Vec<String>, field: &str, expected: &[f64], actual: &[f64]) {
    if expected != actual {
        problems.push(format!("{field} {actual:?} != expected {expected:?}"));
    }
}

fn describe(value: &Value) -> String {
    let text = format!("{value:?}");
    if text.len() > 400 {
        format!("{}...", &text[..400])
    } else {
        text
    }
}

fn check(id: &str, zone_name: &str) {
    let zone = NamedZone::get(zone_name).expect("zone resolves");
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut env = Env::new(zone);
    let mut failures = Vec::new();
    for record in &records {
        if let Err(e) = check_record(&mut env, record) {
            failures.push(format!(
                "line {} (case {:?} step {:?}): {}",
                record.line,
                record.string("case"),
                record.string("step"),
                e
            ));
        }
    }
    if !failures.is_empty() {
        panic!(
            "vectors {id}: {} of {} records differ\n\n{}",
            failures.len(),
            records.len(),
            failures
                .iter()
                .take(5)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("\n\n")
        );
    }
}

#[test]
fn files_state() {
    check("files/state", "UTC");
}

#[test]
fn files_flows() {
    check("files/flows", "UTC");
}

#[test]
fn files_flows_saved() {
    check("files/flows-saved", "UTC");
}

#[test]
fn files_flows_corrupt() {
    check("files/flows-corrupt", "UTC");
}

#[test]
fn files_flows_packaged() {
    check("files/flows-packaged", "UTC");
}

#[test]
fn files_beta_utc() {
    check("files/beta/main/UTC", "UTC");
}

#[test]
fn files_beta_los_angeles() {
    check("files/beta/main/America/Los_Angeles", "America/Los_Angeles");
}

#[test]
fn files_beta_kolkata() {
    check("files/beta/main/Asia/Kolkata", "Asia/Kolkata");
}

#[test]
fn files_beta_auckland() {
    check("files/beta/main/Pacific/Auckland", "Pacific/Auckland");
}
