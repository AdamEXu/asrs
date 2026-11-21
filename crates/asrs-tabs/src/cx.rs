//! What tab controllers and renderers read in place of upstream's `window.*` globals. Every
//! `asrs-tabs` half implements `TabController<Cx>` or `TabRenderer<Cx>` for any
//! `Cx: TabCx`; the hub's context type implements `TabCx` alongside `asrs_hub::tabs::TabsHost`.

use std::cell::Ref;

use asrs_js::time::Rng;
use asrs_log::log::{Log, TimestampDisplay};
use asrs_model::assets::AdvantageScopeAssets;
use asrs_model::messages::{HubToMain, NamedMessage};
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

    /// `window.preferences` for the handlers that assign into it before telling main
    /// (`prefersFTC` on a field select); main's next `set-preferences` replaces it.
    fn tab_preferences_mut(&mut self) -> Option<&mut Preferences>;

    /// `window.assets`; `None` before `set-assets` arrives.
    fn tab_assets(&self) -> Option<&AdvantageScopeAssets>;

    /// `Math.random`, for `createUUID` in controller and renderer constructors.
    fn tab_rng(&mut self) -> &mut dyn Rng;

    /// `window.sendMainMessage(name, data)`.
    fn send_main_message(&mut self, message: NamedMessage<HubToMain>);
}

/// `window.log.getTimestampDisplayOffset()`.
pub fn display_offset<C: TabCx + ?Sized>(cx: &C) -> f64 {
    let timestamps = cx.tab_preferences().map(|p| match p.timestamps {
        Timestamps::StartZero => TimestampDisplay::StartZero,
        Timestamps::Original => TimestampDisplay::Original,
    });
    cx.tab_log().get_timestamp_display_offset(timestamps)
}
