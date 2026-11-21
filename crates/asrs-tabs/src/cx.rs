//! What the hub-side tab controllers and renderers read in place of upstream's `window.*`
//! globals. `Tabs` calls every tab with the same `Cx`; the controllers reach the log, the
//! selection, the preferences and the seeded `Math.random` stream through it.

use std::cell::Ref;

use asrs_js::time::Rng;
use asrs_log::log::{Log, TimestampDisplay};
use asrs_model::preferences::{Preferences, Timestamps};
use asrs_model::selection::Selection;

pub trait TabCx {
    /// `window.log`.
    fn tab_log(&self) -> Ref<'_, Log>;

    /// `window.selection`. Every upstream read of `getSelectedTime` can mutate, so the
    /// accessor hands out `&mut`.
    fn tab_selection(&mut self) -> &mut dyn Selection;

    /// `window.preferences`; `None` before `set-preferences` arrives.
    fn tab_preferences(&self) -> Option<&Preferences>;

    /// `Math.random`, for `createUUID` in controller and renderer constructors.
    fn tab_rng(&mut self) -> &mut dyn Rng;

    /// `window.tabs.tabListElement.getBoundingClientRect().top + controlledHeight` for the
    /// selected tab, used by the renderers' timestamp/scroll math.
    fn tab_renderer_top(&self) -> f64;
}

/// `window.preferences?.timestamps !== "original"`.
pub fn is_start_at_zero(preferences: Option<&Preferences>) -> bool {
    !matches!(preferences, Some(p) if p.timestamps == Timestamps::Original)
}

/// `window.log.getTimestampDisplayOffset()`.
pub fn display_offset<C: TabCx + ?Sized>(cx: &C) -> f64 {
    let timestamps = cx.tab_preferences().map(|p| match p.timestamps {
        Timestamps::StartZero => TimestampDisplay::StartZero,
        Timestamps::Original => TimestampDisplay::Original,
    });
    cx.tab_log().get_timestamp_display_offset(timestamps)
}
