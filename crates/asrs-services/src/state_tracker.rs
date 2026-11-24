//! `src/main/electron/StateTracker.ts`. The 250 ms `setInterval` cadence is the caller's timer;
//! [`StateTracker::tick`] is the callback it runs.

use asrs_js::json::{self};
use asrs_js::number;
use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_model::json_file;

use crate::constants;
use crate::fs::Fs;
use crate::windows::WindowSystem;

pub struct StateTracker {
    state_file: String,
    last_state: JsValue,
    renderer_states: JsObject<JsValue>,
    satellite_uuids: JsObject<JsValue>,
}

impl StateTracker {
    /// `new StateTracker()`: reads the last saved state (without reset) and starts the save
    /// interval; [`tick`](Self::tick) is that interval's callback.
    pub fn new(
        fs: &dyn Fs,
        ws: &dyn WindowSystem,
        state_file: &str,
    ) -> Result<StateTracker, JsError> {
        Ok(StateTracker {
            state_file: state_file.to_owned(),
            last_state: saved_application_state(fs, ws, state_file, false)?,
            renderer_states: JsObject::new(),
            satellite_uuids: JsObject::new(),
        })
    }

    /// The interval callback: writes the current state when there is a hub and the serialized
    /// state changed.
    pub fn tick(&mut self, fs: &dyn Fs, ws: &dyn WindowSystem) -> Result<(), JsError> {
        let state = self.current_application_state(ws);
        let hubs_len = state
            .get("hubs")
            .and_then(JsValue::as_array)
            .map_or(0, <[JsValue]>::len);
        if hubs_len > 0 && stringify(&state)? != stringify(&self.last_state)? {
            fs.write(&self.state_file, &json_file::stringify(&state)?)?;
            self.last_state = state;
        }
        Ok(())
    }

    /// `getSavedApplicationState(allowReset)`.
    pub fn get_saved_application_state(
        &self,
        fs: &dyn Fs,
        ws: &dyn WindowSystem,
        allow_reset: bool,
    ) -> Result<JsValue, JsError> {
        saved_application_state(fs, ws, &self.state_file, allow_reset)
    }

    /// `getCurrentApplicationState()`.
    pub fn current_application_state(&self, ws: &dyn WindowSystem) -> JsValue {
        let mut hubs = Vec::new();
        let mut satellites = Vec::new();
        for key in self.renderer_states.keys() {
            let id = number::to_number(key.units());
            let Some(bounds) = ws.window_bounds(id) else {
                continue;
            };
            let window_state = self
                .renderer_states
                .get_key(&JsString::from(number::to_string(id)))
                .cloned()
                .unwrap_or_default();
            let mut window = JsObject::new();
            window.insert(JsString::from("x"), JsValue::Number(bounds.x));
            window.insert(JsString::from("y"), JsValue::Number(bounds.y));
            window.insert(JsString::from("width"), JsValue::Number(bounds.width));
            window.insert(JsString::from("height"), JsValue::Number(bounds.height));
            window.insert(JsString::from("state"), window_state);
            match self
                .satellite_uuids
                .get_key(&JsString::from(number::to_string(id)))
            {
                Some(JsValue::String(uuid)) => {
                    window.insert(JsString::from("uuid"), JsValue::String(uuid.clone()));
                    satellites.push(JsValue::Object(window));
                }
                Some(other) => {
                    window.insert(JsString::from("uuid"), other.clone());
                    satellites.push(JsValue::Object(window));
                }
                None => hubs.push(JsValue::Object(window)),
            }
        }
        ops::object([
            ("hubs", JsValue::Array(hubs)),
            ("satellites", JsValue::Array(satellites)),
        ])
    }

    /// `saveRendererState(window, rendererState)`; the window argument is its numeric `id`.
    pub fn save_renderer_state(&mut self, window_id: f64, renderer_state: JsValue) {
        self.renderer_states
            .insert(JsString::from(number::to_string(window_id)), renderer_state);
    }

    /// `getRendererState(window)`; `undefined` for unknown ids.
    pub fn get_renderer_state(&self, window_id: f64) -> JsValue {
        self.renderer_states
            .get_key(&JsString::from(number::to_string(window_id)))
            .cloned()
            .unwrap_or_default()
    }

    /// `saveSatelliteIds(windows)`: replaces the uuid map from `{uuid: [{id}, ...]}`.
    pub fn save_satellite_ids(&mut self, windows: &JsValue) -> Result<(), JsError> {
        self.satellite_uuids = JsObject::new();
        for uuid in ops::object_keys(windows)? {
            let value = ops::get_key(windows, &uuid)?;
            match &value {
                JsValue::Array(list) => {
                    for window in list {
                        let id = ops::get(window, "id")?;
                        self.satellite_uuids
                            .insert(ops::to_string(&id), JsValue::String(uuid.clone()));
                    }
                }
                JsValue::Bytes(bytes) => {
                    if !bytes.is_empty() {
                        self.satellite_uuids
                            .insert(JsString::from("undefined"), JsValue::String(uuid.clone()));
                    }
                }
                JsValue::Null | JsValue::Undefined => {
                    return Err(JsError::type_error(format!(
                        "Cannot read properties of {} (reading 'forEach')",
                        if value.is_null() { "null" } else { "undefined" }
                    )));
                }
                _ => {
                    return Err(JsError::type_error("value.forEach is not a function"));
                }
            }
        }
        Ok(())
    }
}

/// `getSavedApplicationState(allowReset)` as a free function, for the constructor.
pub fn saved_application_state(
    fs: &dyn Fs,
    ws: &dyn WindowSystem,
    state_file: &str,
    allow_reset: bool,
) -> Result<JsValue, JsError> {
    if !fs.exists(state_file) {
        return Ok(JsValue::Null);
    }
    match load_state(fs, ws, state_file, allow_reset) {
        Ok(state) => Ok(state),
        Err(_e) => {
            // console.error("Unable to load state. Reverting to default settings.", e);
            fs.copy(state_file, &corrupted_name(state_file))?;
            Ok(JsValue::Null)
        }
    }
}

/// `STATE_FILENAME.slice(0, -5) + "-corrupted.json"`: slice works on UTF-16 units.
fn corrupted_name(state_file: &str) -> String {
    let mut units: Vec<u16> = state_file.encode_utf16().collect();
    let keep = units.len().saturating_sub(5);
    units.truncate(keep);
    format!("{}-corrupted.json", String::from_utf16_lossy(&units))
}

fn load_state(
    fs: &dyn Fs,
    ws: &dyn WindowSystem,
    state_file: &str,
    allow_reset: bool,
) -> Result<JsValue, JsError> {
    let mut state = json_file::parse(&fs.read(state_file)?)?;
    let hubs = ops::get(&state, "hubs")?;
    let satellites = ops::get(&state, "satellites")?;
    let mut windows = ops::spread_iter(&hubs)?;
    let hubs_count = windows.len();
    windows.extend(ops::spread_iter(&satellites)?);
    for (index, window) in windows.iter_mut().enumerate() {
        if !allow_reset {
            break;
        }
        let displays = ws.displays();
        let x = ops::get(window, "x")?;
        let y = ops::get(window, "y")?;
        let width = ops::get(window, "width")?;
        let height = ops::get(window, "height")?;
        let contained = displays.iter().any(|d| {
            ge(&x, d.x)
                && ge(&y, d.y)
                && le(&js_add(&x, &width), d.x + d.width)
                && le(&js_add(&y, &height), d.y + d.height)
        });
        if !contained {
            let primary = ws.primary_display();
            let hubs_length = ops::get(&hubs, "length")?;
            let is_hub = ops::lt(&JsValue::Number(index as f64), &hubs_length).unwrap_or(false);
            let (default_width, default_height) = if is_hub {
                (constants::HUB_DEFAULT_WIDTH, constants::HUB_DEFAULT_HEIGHT)
            } else {
                (
                    constants::SATELLITE_DEFAULT_WIDTH,
                    constants::SATELLITE_DEFAULT_HEIGHT,
                )
            };
            strict_set(
                window,
                "x",
                primary.x + primary.width / 2.0 - default_width / 2.0,
            )?;
            strict_set(
                window,
                "y",
                primary.y + primary.height / 2.0 - default_height / 2.0,
            )?;
            strict_set(window, "width", default_width)?;
            strict_set(window, "height", default_height)?;
        }
    }
    write_back(&mut state, &windows, hubs_count);
    Ok(state)
}

/// The reset mutates elements of the spread copy; the originals live in `state`'s arrays when
/// `hubs`/`satellites` were arrays.
fn write_back(state: &mut JsValue, windows: &[JsValue], hubs_count: usize) {
    let Some(object) = state.as_object_mut() else {
        return;
    };
    if let Some(JsValue::Array(items)) = object.get_mut("hubs") {
        for (i, w) in windows[..hubs_count.min(items.len())].iter().enumerate() {
            items[i] = w.clone();
        }
    }
    if let Some(JsValue::Array(items)) = object.get_mut("satellites") {
        for (i, w) in windows[hubs_count..].iter().enumerate().take(items.len()) {
            items[i] = w.clone();
        }
    }
}

/// `a >= b` where `b` is a display bound number.
fn ge(a: &JsValue, b: f64) -> bool {
    ops::ge(a, &JsValue::Number(b)).unwrap_or(false)
}

/// `a <= b` where `b` is a display bound number.
fn le(a: &JsValue, b: f64) -> bool {
    ops::le(&JsValue::Number(b), a).unwrap_or(false)
}

/// `a + b` on state-file JSON, which cannot hold a BigInt for `ops::add` to reject.
fn js_add(a: &JsValue, b: &JsValue) -> JsValue {
    ops::add(a, b).unwrap_or(JsValue::Number(f64::NAN))
}

/// `window.x = v`: class bodies are strict code even in the app's sloppy CJS bundle, so
/// assigning a property on a primitive throws TypeError.
fn strict_set(target: &mut JsValue, key: &str, value: f64) -> Result<(), JsError> {
    ops::set_key(target, &JsString::from(key), JsValue::Number(value))
}

fn stringify(value: &JsValue) -> Result<Option<String>, JsError> {
    json::stringify(value).map_err(|e| JsError::new(e.name, e.message.to_string_lossy()))
}
