//! `src/hub/SelectionImpl.ts`: the hub's selection and playback state machine.
//!
//! Headless: the log is the shared `window.log` handle, the clock is injected
//! (`new Date().getTime() / 1000`), and the things upstream reads from
//! `window.tabs` / `window.platform` / button rects come in through
//! [`SelectionContext`] and method arguments. Button visibility, the
//! `--show-lock-buttons` property and the `ask-playback-options` message are
//! kept as observable state and an outbox so traces see what the DOM sees.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::math;
use asrs_js::string::JsString;
use asrs_js::time::Clock;
use asrs_js::value::{JsObject, JsValue};
use asrs_log::log::Log;
use asrs_log::log::robot_state::AKIT_TIMESTAMP_KEYS;

use crate::messages::{HubToMain, NamedMessage};
use crate::selection::{Selection, SelectionMode, TimeSupplier};
use crate::tab_type::TabType;

const STEP_SIZE: f64 = 0.02;
const TIMELINE_MIN_ZOOM_TIME: f64 = 0.001;
const TIMELINE_ZOOM_BASE: f64 = 1.001;

/// The hub's `Log`, shared the way upstream hands the same object to `window.log`.
/// Only the foreground thread touches it.
pub type SharedLog = Rc<RefCell<Log>>;

/// The play/pause/lock/unlock control buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlButton {
    Play,
    Pause,
    Lock,
    Unlock,
}

/// `event.target` buckets for the keydown guard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyTarget {
    Other,
    Input,
    TextArea,
    ContentEditable,
}

/// `getBoundingClientRect()` for the element an event landed on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ElementRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// What upstream reads from `window` outside this class: `window.platform` and
/// `window.tabs` (selected tab type plus the unlocked-video check).
#[derive(Clone, Debug)]
pub struct SelectionContext {
    /// `window.platform` ("" until `set-version` arrives).
    pub platform: String,
    /// `window.tabs.getSelectedTabType()`.
    pub selected_tab_type: TabType,
    /// `window.tabs.isUnlockedVideoSelected()`.
    pub unlocked_video_selected: bool,
}

impl Default for SelectionContext {
    fn default() -> Self {
        SelectionContext {
            platform: String::new(),
            selected_tab_type: TabType::LineGraph,
            unlocked_video_selected: false,
        }
    }
}

impl SelectionContext {
    fn uses_command(&self) -> bool {
        self.platform == "darwin"
    }
}

pub struct SelectionImpl {
    log: SharedLog,
    clock: Rc<dyn Clock>,
    context: SelectionContext,

    mode: SelectionMode,
    hovered_time: Option<f64>,
    static_time: f64,
    timeline_range: [f64; 2],
    timeline_max_zoom: bool,
    grab_zoom_range: Option<[f64; 2]>,
    playback_start_log: f64,
    playback_start_real: f64,
    playback_speed: f64,
    playback_looping: bool,
    live_connected: bool,
    live_time_supplier: Option<TimeSupplier>,

    play_hidden: bool,
    pause_hidden: bool,
    lock_hidden: bool,
    unlock_hidden: bool,
    /// The inline `--show-lock-buttons` value: unset ("") until the first mode change.
    show_lock_buttons: String,

    outbox: Vec<NamedMessage<HubToMain>>,
}

impl SelectionImpl {
    pub fn new(log: SharedLog, clock: Rc<dyn Clock>) -> Self {
        Self {
            log,
            clock,
            context: SelectionContext::default(),
            mode: SelectionMode::Idle,
            hovered_time: None,
            static_time: 0.0,
            timeline_range: [0.0, 10.0],
            timeline_max_zoom: true,
            grab_zoom_range: None,
            playback_start_log: 0.0,
            playback_start_real: 0.0,
            playback_speed: 1.0,
            playback_looping: false,
            live_connected: false,
            live_time_supplier: None,
            play_hidden: false,
            pause_hidden: true,
            lock_hidden: true,
            unlock_hidden: true,
            show_lock_buttons: String::new(),
            outbox: Vec::new(),
        }
    }

    /// `new Date().getTime() / 1000`: the Date constructor TimeClips to whole
    /// milliseconds, unlike `Date.now()` which returns the raw clock.
    fn now(&self) -> f64 {
        self.clock.now().trunc() / 1000.0
    }

    /// `window.tabs` and `window.platform` values, refreshed by the hub.
    pub fn set_context(&mut self, context: SelectionContext) {
        self.context = context;
    }

    pub fn context(&self) -> &SelectionContext {
        &self.context
    }

    /// Replaces `window.log` (hub clears the log between loads).
    pub fn set_log(&mut self, log: SharedLog) {
        self.log = log;
    }

    /// `HTMLElement.hidden` on each control button.
    pub fn button_hidden(&self, button: ControlButton) -> bool {
        match button {
            ControlButton::Play => self.play_hidden,
            ControlButton::Pause => self.pause_hidden,
            ControlButton::Lock => self.lock_hidden,
            ControlButton::Unlock => self.unlock_hidden,
        }
    }

    /// The inline `--show-lock-buttons` custom property ("" before the first mode change).
    pub fn show_lock_buttons(&self) -> &str {
        &self.show_lock_buttons
    }

    /// Hub-to-main messages this class sends; the hub drains and forwards them.
    pub fn take_messages(&mut self) -> Vec<NamedMessage<HubToMain>> {
        std::mem::take(&mut self.outbox)
    }

    /// The button `click` listeners registered in the constructor.
    pub fn click(&mut self, button: ControlButton) {
        match button {
            ControlButton::Play => self.play(),
            ControlButton::Pause => self.pause(),
            ControlButton::Lock => self.lock(),
            ControlButton::Unlock => self.unlock(),
        }
    }

    /// The play/pause `contextmenu` listener: `ask-playback-options` with the button's rect.
    pub fn context_menu(&mut self, _button: ControlButton, rect: ElementRect) {
        let mut data = JsObject::new();
        data.insert("speed", JsValue::Number(self.playback_speed));
        data.insert("looping", JsValue::Bool(self.playback_looping));
        let mut r = JsObject::new();
        r.insert("x", JsValue::Number(rect.x));
        r.insert("y", JsValue::Number(rect.y));
        r.insert("width", JsValue::Number(rect.width));
        r.insert("height", JsValue::Number(rect.height));
        data.insert("rect", JsValue::Object(r));
        self.outbox
            .push(NamedMessage::new(HubToMain::AskPlaybackOptions, JsValue::Object(data)));
    }

    /// The window `keydown` listener. Returns whether `preventDefault` was called.
    pub fn key_down(&mut self, code: &str, meta_key: bool, ctrl_key: bool, target: KeyTarget) -> bool {
        match target {
            KeyTarget::Input | KeyTarget::TextArea | KeyTarget::ContentEditable => return false,
            KeyTarget::Other => {}
        }
        let command = if self.context.uses_command() { meta_key } else { ctrl_key };
        match code {
            "Space" => {
                self.toggle_playback();
                true
            }
            "KeyL" => {
                if self.context.selected_tab_type == TabType::Field3d && !command {
                    return false;
                }
                self.toggle_lock();
                true
            }
            "ArrowLeft" | "ArrowRight" => {
                // Unlocked video uses arrow keys to navigate by frame
                if self.context.unlocked_video_selected || self.mode != SelectionMode::Static || command {
                    return false;
                }
                self.step_cycle(code == "ArrowRight");
                true
            }
            _ => false,
        }
    }

    fn set_mode(&mut self, new_mode: SelectionMode) {
        self.mode = new_mode;
        self.show_lock_buttons = if self.live_connected { "1" } else { "0" }.to_owned();
        self.play_hidden = self.mode == SelectionMode::Playback || self.mode == SelectionMode::Locked;
        self.pause_hidden = self.mode == SelectionMode::Idle || self.mode == SelectionMode::Static;
        self.lock_hidden = !self.live_connected || self.mode == SelectionMode::Locked;
        self.unlock_hidden = !self.live_connected || self.mode != SelectionMode::Locked;
    }
}

impl Selection for SelectionImpl {
    fn get_mode(&self) -> SelectionMode {
        self.mode
    }

    fn get_hovered_time(&self) -> Option<f64> {
        self.hovered_time
    }

    fn set_hovered_time(&mut self, value: Option<f64>) {
        self.hovered_time = value;
    }

    fn get_selected_time(&mut self) -> Option<f64> {
        let log = self.log.clone();
        let log = log.borrow();
        match self.mode {
            SelectionMode::Idle => None,
            SelectionMode::Static => Some(math::max2(self.static_time, log.get_timestamp_range()[0])),
            SelectionMode::Playback => {
                let mut time = (self.now() - self.playback_start_real) * self.playback_speed
                    + self.playback_start_log;
                if self.playback_looping && time > self.timeline_range[1] {
                    time = self.timeline_range[0];
                    self.set_selected_time(time);
                }
                let mut max_time = log.get_timestamp_range()[1];
                if let Some(supplier) = &self.live_time_supplier {
                    max_time = math::max2(max_time, supplier());
                }
                if time > max_time {
                    if self.live_connected {
                        self.set_mode(SelectionMode::Locked);
                    } else {
                        self.set_mode(SelectionMode::Static);
                        self.static_time = max_time;
                    }
                    Some(max_time)
                } else {
                    Some(math::max2(time, log.get_timestamp_range()[0]))
                }
            }
            SelectionMode::Locked => match &self.live_time_supplier {
                None => Some(0.0),
                Some(supplier) => Some(supplier()),
            },
        }
    }

    fn set_selected_time(&mut self, time: f64) {
        match self.mode {
            SelectionMode::Idle => {
                self.set_mode(SelectionMode::Static);
                self.static_time = time;
            }
            SelectionMode::Static => {
                self.static_time = time;
            }
            SelectionMode::Playback => {
                self.playback_start_log = time;
                self.playback_start_real = self.now();
            }
            SelectionMode::Locked => {}
        }
    }

    fn go_idle(&mut self) {
        if self.mode == SelectionMode::Static {
            self.set_mode(SelectionMode::Idle);
        }
    }

    fn play(&mut self) {
        match self.mode {
            SelectionMode::Idle => {
                self.set_mode(SelectionMode::Playback);
                self.playback_start_log = self.log.borrow().get_timestamp_range()[0];
                self.playback_start_real = self.now();
            }
            SelectionMode::Static => {
                let log = self.log.clone();
                let log = log.borrow();
                let mut max_time = log.get_timestamp_range()[1];
                if let Some(supplier) = &self.live_time_supplier {
                    max_time = math::max2(max_time, supplier());
                }
                if self.static_time < max_time {
                    let start_log = math::max2(self.static_time, log.get_timestamp_range()[0]);
                    self.set_mode(SelectionMode::Playback);
                    self.playback_start_log = start_log;
                    self.playback_start_real = self.now();
                }
            }
            SelectionMode::Playback | SelectionMode::Locked => {}
        }
    }

    fn pause(&mut self) {
        match self.mode {
            SelectionMode::Idle | SelectionMode::Static => {}
            SelectionMode::Playback | SelectionMode::Locked => {
                let selected_time = self.get_selected_time();
                self.set_mode(SelectionMode::Static);
                self.static_time = selected_time.unwrap_or(0.0);
            }
        }
    }

    fn toggle_playback(&mut self) {
        if self.mode == SelectionMode::Playback || self.mode == SelectionMode::Locked {
            self.pause();
        } else {
            self.play();
        }
    }

    fn lock(&mut self) {
        if self.live_connected {
            self.set_mode(SelectionMode::Locked);
        }
    }

    fn unlock(&mut self) {
        if self.mode == SelectionMode::Locked {
            let selected_time = self.get_selected_time();
            self.apply_timeline_scroll(0.0, 0.0, 0.0);
            self.set_mode(SelectionMode::Static);
            self.static_time = selected_time.unwrap_or(0.0);
        }
    }

    fn toggle_lock(&mut self) {
        if self.mode == SelectionMode::Locked {
            self.unlock();
        } else {
            self.lock();
        }
    }

    fn step_cycle(&mut self, is_forward: bool) {
        let log = self.log.clone();
        let log = log.borrow();
        let akit_key = log
            .get_field_keys()
            .into_iter()
            .find(|key| AKIT_TIMESTAMP_KEYS.iter().any(|k| *key == JsString::from(*k)));
        match akit_key {
            Some(key) => {
                let data = log.get_number(&key, f64::NEG_INFINITY, f64::INFINITY, None, None);
                let Some(data) = data else { return };
                if is_forward {
                    if let Some(next) = data.timestamps.iter().find(|&&v| v > self.static_time) {
                        self.static_time = *next;
                    }
                } else if let Some(prev) = data.timestamps.iter().rev().find(|&&v| v < self.static_time) {
                    self.static_time = *prev;
                }
            }
            None => {
                self.static_time += if is_forward { STEP_SIZE } else { -STEP_SIZE };
            }
        }
    }

    fn set_live_connected(&mut self, time_supplier: TimeSupplier) {
        let new_connection = !self.live_connected;
        self.live_connected = true;
        self.live_time_supplier = Some(time_supplier);
        if new_connection {
            self.set_mode(SelectionMode::Locked);
        } else {
            self.set_mode(self.mode);
        }
    }

    fn set_live_disconnected(&mut self) {
        self.unlock();
        self.live_connected = false;
        self.live_time_supplier = None;
        self.set_mode(self.mode);
    }

    fn get_current_live_time(&self) -> Option<f64> {
        self.live_time_supplier.as_ref().map(|supplier| supplier())
    }

    fn get_render_time(&mut self) -> Option<f64> {
        let selected_time = self.get_selected_time();
        if self.mode == SelectionMode::Playback || self.mode == SelectionMode::Locked {
            selected_time
        } else if self.hovered_time.is_some() {
            self.hovered_time
        } else {
            selected_time
        }
    }

    fn set_playback_speed(&mut self, speed: f64) {
        if self.mode == SelectionMode::Playback {
            let selected_time = self.get_selected_time();
            self.playback_start_log = selected_time.unwrap_or(0.0);
            self.playback_start_real = self.now();
        }
        self.playback_speed = speed;
    }

    fn set_playback_looping(&mut self, looping: bool) {
        self.playback_looping = looping;
    }

    fn set_grab_zoom_range(&mut self, range: Option<[f64; 2]>) {
        self.grab_zoom_range = range.map(|mut r| {
            if r[1] < r[0] {
                r.swap(0, 1);
            }
            r
        });
    }

    fn get_grab_zoom_range(&self) -> Option<[f64; 2]> {
        self.grab_zoom_range
    }

    fn finish_grab_zoom(&mut self) {
        if let Some(grab) = self.grab_zoom_range {
            self.timeline_max_zoom = false;
            self.timeline_range = [grab[0], grab[1]];
            if self.timeline_range[1] - self.timeline_range[0] < TIMELINE_MIN_ZOOM_TIME {
                self.timeline_range[1] = self.timeline_range[0] + TIMELINE_MIN_ZOOM_TIME;
            }
        }
        self.grab_zoom_range = None;
    }

    fn get_timeline_range(&mut self) -> [f64; 2] {
        self.apply_timeline_scroll(0.0, 0.0, 0.0);
        let mut safe = self.timeline_range;
        safe[1] = math::max2(safe[1], safe[0] + 1e-3);
        safe
    }

    fn get_timeline_is_max_zoom(&self) -> bool {
        self.timeline_max_zoom
    }

    fn set_timeline_range(&mut self, range: [f64; 2], lock_max_zoom: bool) {
        self.timeline_range = range;
        self.timeline_max_zoom = lock_max_zoom;
        self.apply_timeline_scroll(0.0, 0.0, 0.0);
    }

    fn apply_timeline_scroll(&mut self, dx: f64, dy: f64, width_pixels: f64) {
        let log = self.log.clone();
        let log = log.borrow();
        let mut available_range = log.get_timestamp_range();
        if let Some(live_time) = self.get_current_live_time() {
            available_range[1] = live_time;
        }
        if available_range[1] - available_range[0] < TIMELINE_MIN_ZOOM_TIME {
            available_range[1] = available_range[0] + TIMELINE_ZOOM_BASE;
        }

        // Apply horizontal scroll
        if self.mode == SelectionMode::Locked {
            let zoom = self.timeline_range[1] - self.timeline_range[0];
            self.timeline_range[0] = available_range[1] - zoom;
            self.timeline_range[1] = available_range[1];
            if dx < 0.0 {
                self.unlock();
            }
        } else if dx != 0.0 {
            let secs_per_pixel = (self.timeline_range[1] - self.timeline_range[0]) / width_pixels;
            self.timeline_range[0] += dx * secs_per_pixel;
            self.timeline_range[1] += dx * secs_per_pixel;
        }

        // Apply vertical scroll
        if dy != 0.0 && (!self.timeline_max_zoom || dy < 0.0) {
            // If max zoom, ignore positive scroll (no effect, just apply the max zoom)
            let zoom_percent = math::pow(TIMELINE_ZOOM_BASE, dy);
            let mut new_zoom = (self.timeline_range[1] - self.timeline_range[0]) * zoom_percent;
            if new_zoom < TIMELINE_MIN_ZOOM_TIME {
                new_zoom = TIMELINE_MIN_ZOOM_TIME;
            }
            if new_zoom > available_range[1] - available_range[0] {
                new_zoom = available_range[1] - available_range[0];
            }

            let hovered_time = self
                .hovered_time
                .unwrap_or((self.timeline_range[0] + self.timeline_range[1]) / 2.0);
            let hovered_percent =
                (hovered_time - self.timeline_range[0]) / (self.timeline_range[1] - self.timeline_range[0]);
            self.timeline_range[0] = hovered_time - new_zoom * hovered_percent;
            self.timeline_range[1] = hovered_time + new_zoom * (1.0 - hovered_percent);
        } else if self.timeline_max_zoom {
            self.timeline_range = available_range;
        }

        // Enforce max range
        if self.timeline_range[1] - self.timeline_range[0] > available_range[1] - available_range[0] {
            self.timeline_range = available_range;
        }
        self.timeline_max_zoom =
            self.timeline_range[1] - self.timeline_range[0] == available_range[1] - available_range[0];

        // Enforce left limit
        if self.timeline_range[0] < available_range[0] {
            let shift = available_range[0] - self.timeline_range[0];
            self.timeline_range[0] += shift;
            self.timeline_range[1] += shift;
        }

        // Enforce right limit
        if self.timeline_range[1] > available_range[1] {
            let shift = available_range[1] - self.timeline_range[1];
            self.timeline_range[0] += shift;
            self.timeline_range[1] += shift;
            if dx > 0.0 {
                self.lock();
            }
        }
    }
}
