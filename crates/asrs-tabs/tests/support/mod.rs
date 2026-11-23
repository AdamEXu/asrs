#![allow(dead_code)]

//! A `TabCx` that records what a renderer asks of it, for replays of Blink runs.

pub mod table_geometry;

use std::cell::{Ref, RefCell};

use asrs_js::time::Rng;
use asrs_js::value::JsValue;
use asrs_log::log::Log;
use asrs_model::assets::AdvantageScopeAssets;
use asrs_model::messages::{HubToMain, NamedMessage};
use asrs_model::preferences::Preferences;
use asrs_model::selection::{Selection, SelectionMode, TimeSupplier};
use asrs_parity::Value;
use asrs_tabs::cx::TabCx;

#[derive(Default)]
pub struct RecordingSelection {
    pub calls: Vec<Value>,
}

impl RecordingSelection {
    fn call(&mut self, name: &str, args: Vec<Value>) {
        let mut call = vec![Value::String(name.into())];
        call.extend(args);
        self.calls.push(Value::Array(call));
    }

    fn unexpected(&self, name: &str) -> ! {
        panic!("the renderer called selection.{name}")
    }
}

pub fn opt_number(value: Option<f64>) -> Value {
    value.map_or(Value::Null, |n| Value::Number(n.to_bits()))
}

impl Selection for RecordingSelection {
    fn get_mode(&self) -> SelectionMode {
        self.unexpected("getMode")
    }
    fn get_hovered_time(&self) -> Option<f64> {
        self.unexpected("getHoveredTime")
    }
    fn set_hovered_time(&mut self, value: Option<f64>) {
        self.call("setHoveredTime", vec![opt_number(value)]);
    }
    fn get_selected_time(&mut self) -> Option<f64> {
        self.unexpected("getSelectedTime")
    }
    fn set_selected_time(&mut self, time: f64) {
        self.call("setSelectedTime", vec![Value::Number(time.to_bits())]);
    }
    fn go_idle(&mut self) {
        self.call("goIdle", Vec::new());
    }
    fn play(&mut self) {
        self.call("play", Vec::new());
    }
    fn pause(&mut self) {
        self.call("pause", Vec::new());
    }
    fn toggle_playback(&mut self) {
        self.unexpected("togglePlayback")
    }
    fn lock(&mut self) {
        self.call("lock", Vec::new());
    }
    fn unlock(&mut self) {
        self.call("unlock", Vec::new());
    }
    fn toggle_lock(&mut self) {
        self.unexpected("toggleLock")
    }
    fn step_cycle(&mut self, _: bool) {
        self.unexpected("stepCycle")
    }
    fn set_live_connected(&mut self, _: TimeSupplier) {
        self.unexpected("setLiveConnected")
    }
    fn set_live_disconnected(&mut self) {
        self.unexpected("setLiveDisconnected")
    }
    fn get_current_live_time(&self) -> Option<f64> {
        self.unexpected("getCurrentLiveTime")
    }
    fn get_render_time(&mut self) -> Option<f64> {
        self.unexpected("getRenderTime")
    }
    fn set_playback_speed(&mut self, _: f64) {
        self.unexpected("setPlaybackSpeed")
    }
    fn set_playback_looping(&mut self, _: bool) {
        self.unexpected("setPlaybackLooping")
    }
    fn set_grab_zoom_range(&mut self, _: Option<[f64; 2]>) {
        self.unexpected("setGrabZoomRange")
    }
    fn get_grab_zoom_range(&self) -> Option<[f64; 2]> {
        self.unexpected("getGrabZoomRange")
    }
    fn finish_grab_zoom(&mut self) {
        self.unexpected("finishGrabZoom")
    }
    fn get_timeline_range(&mut self) -> [f64; 2] {
        self.unexpected("getTimelineRange")
    }
    fn get_timeline_is_max_zoom(&self) -> bool {
        self.unexpected("getTimelineIsMaxZoom")
    }
    fn set_timeline_range(&mut self, _: [f64; 2], _: bool) {
        self.unexpected("setTimelineRange")
    }
    fn apply_timeline_scroll(&mut self, _: f64, _: f64, _: f64) {
        self.unexpected("applyTimelineScroll")
    }
}

pub struct NoRandom;

impl Rng for NoRandom {
    fn next_f64(&mut self) -> f64 {
        panic!("the renderer draws no Math.random values")
    }
}

pub struct Host {
    log: RefCell<Log>,
    pub selection: RecordingSelection,
    pub preferences: Option<Preferences>,
    pub messages: Vec<NamedMessage<HubToMain>>,
    rng: NoRandom,
}

impl Default for Host {
    fn default() -> Self {
        Host {
            log: RefCell::new(Log::new(true)),
            selection: RecordingSelection::default(),
            preferences: None,
            messages: Vec::new(),
            rng: NoRandom,
        }
    }
}

impl TabCx for Host {
    fn tab_log(&self) -> Ref<'_, Log> {
        self.log.borrow()
    }
    fn tab_selection(&mut self) -> &mut dyn Selection {
        &mut self.selection
    }
    fn tab_preferences(&self) -> Option<&Preferences> {
        self.preferences.as_ref()
    }
    fn tab_preferences_mut(&mut self) -> Option<&mut Preferences> {
        self.preferences.as_mut()
    }
    fn tab_assets(&self) -> Option<&AdvantageScopeAssets> {
        None
    }
    fn tab_rng(&mut self) -> &mut dyn Rng {
        &mut self.rng
    }
    fn send_main_message(&mut self, message: NamedMessage<HubToMain>) {
        self.messages.push(message);
    }
    fn send_open_xr(&mut self, _: JsValue) {
        panic!("the renderer opens no XR")
    }
}

pub fn field<'a>(object: &'a Value, key: &str) -> &'a Value {
    match object {
        Value::Object(pairs) => pairs
            .iter()
            .find(|(k, _)| *k == key.into())
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("no {key}")),
        other => panic!("not an object: {other:?}"),
    }
}

pub fn number(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("not a number: {value:?}"))
}

pub fn opt(value: &Value) -> Option<f64> {
    match value {
        Value::Null => None,
        v => Some(number(v)),
    }
}
