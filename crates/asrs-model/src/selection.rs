//! `src/shared/Selection.ts`: the selection interface tab renderers and controllers use.

use std::rc::Rc;

use asrs_js::value::{JsValue, ToJs};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum SelectionMode {
    /// Nothing is selected and playback is inactive.
    Idle = 0,
    /// A time is selected but playback is inactive.
    Static = 1,
    /// Historical playback is active.
    Playback = 2,
    /// Playback is locked to the live data.
    Locked = 3,
}

impl SelectionMode {
    pub const ALL: [SelectionMode; 4] = [
        SelectionMode::Idle,
        SelectionMode::Static,
        SelectionMode::Playback,
        SelectionMode::Locked,
    ];

    pub fn name(self) -> &'static str {
        match self {
            SelectionMode::Idle => "Idle",
            SelectionMode::Static => "Static",
            SelectionMode::Playback => "Playback",
            SelectionMode::Locked => "Locked",
        }
    }

    pub fn number(self) -> f64 {
        f64::from(self as u8)
    }
}

impl ToJs for SelectionMode {
    fn to_js(&self) -> JsValue {
        JsValue::Number(self.number())
    }
}

pub type TimeSupplier = Rc<dyn Fn() -> f64>;

pub trait Selection {
    fn get_mode(&self) -> SelectionMode;

    fn get_hovered_time(&self) -> Option<f64>;

    fn set_hovered_time(&mut self, value: Option<f64>);

    /// The selected time for the current mode. Upstream's getter mutates: playback can
    /// loop, lock or fall out to static inside this call.
    fn get_selected_time(&mut self) -> Option<f64>;

    fn set_selected_time(&mut self, time: f64);

    /// Switches to idle if possible.
    fn go_idle(&mut self);

    fn play(&mut self);

    /// Exits playback and locked modes.
    fn pause(&mut self);

    fn toggle_playback(&mut self);

    /// Switches to locked mode if possible.
    fn lock(&mut self);

    fn unlock(&mut self);

    fn toggle_lock(&mut self);

    fn step_cycle(&mut self, is_forward: bool);

    fn set_live_connected(&mut self, time_supplier: TimeSupplier);

    fn set_live_disconnected(&mut self);

    fn get_current_live_time(&self) -> Option<f64>;

    /// The time views that can only show a single sample should display.
    /// Reads the selected time, which can mutate.
    fn get_render_time(&mut self) -> Option<f64>;

    fn set_playback_speed(&mut self, speed: f64);

    fn set_playback_looping(&mut self, looping: bool);

    /// The range of an in-progress grab zoom.
    fn set_grab_zoom_range(&mut self, range: Option<[f64; 2]>);

    fn get_grab_zoom_range(&self) -> Option<[f64; 2]>;

    /// Ends an in-progress grab zoom and applies it.
    fn finish_grab_zoom(&mut self);

    /// Upstream's getter applies a scroll pass first and can un-lock.
    fn get_timeline_range(&mut self) -> [f64; 2];

    /// Whether the timeline is locked to max zoom.
    fn get_timeline_is_max_zoom(&self) -> bool;

    fn set_timeline_range(&mut self, range: [f64; 2], lock_max_zoom: bool);

    fn apply_timeline_scroll(&mut self, dx: f64, dy: f64, width_pixels: f64);
}
