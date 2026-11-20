//! hub.ts's touch bar interval: every `1000 / 60` ms on every platform, but only
//! macOS reads the selection and sends `update-touch-bar-slider`.

use asrs_log::log::Log;
use asrs_log::util::{clamp_value, scale_value};
use asrs_model::selection::Selection;

pub const PERIOD_MS: f64 = 1000.0 / 60.0;

/// One tick: the slider position in `[0, 1]`, or `None` when `window.platform`
/// is not `darwin`. `getSelectedTime` can change the selection mode, so a tick
/// is observable even when nothing is sent.
pub fn tick(platform: &str, log: &Log, selection: &mut impl Selection) -> Option<f64> {
    if platform != "darwin" {
        return None;
    }
    let mut range = log.get_timestamp_range();
    if let Some(live_time) = selection.get_current_live_time() {
        range[1] = live_time;
    }
    let selected_time = selection.get_selected_time().unwrap_or(range[0]);
    Some(clamp_value(
        scale_value(selected_time, range, [0.0, 1.0]),
        0.0,
        1.0,
    ))
}
