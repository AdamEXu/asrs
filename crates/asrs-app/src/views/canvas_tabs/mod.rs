//! The Swerve, Mechanism, Points and Joysticks tabs: their renderer elements (`#renderer8` to
//! `#renderer11` in `www/hub.html`, styled by `www/renderers.css`) and their controllers' controls
//! (`www/hub.css`). Their source lists are the frame's.

mod boxes;
mod inline_svg;
mod joysticks;
mod mechanism;
mod points;
mod settings;
mod swerve;

use std::collections::{HashMap, HashSet};

use asrs_hub::hub::{HubModel, HubPage};
use asrs_hub::tabs::TabRef;
use asrs_js::math;
use asrs_model::tab_type::TabType;
use asrs_ui::gpui::{AnyElement, App, Context, Div, Styled, Window, div, px};
use asrs_ui::theme::Appearance;

use super::frame::{FrameHost, HubFrame};

pub use joysticks::JoysticksTab;
pub use settings::Control;

/// `div.swerve-canvas-container`, `div.mechanism-svg-container` and
/// `div.points-background-container` are `height: calc(100% + 10px)` from the bottom of the
/// renderer element.
const OVERHANG: f64 = 10.0;

#[derive(Default)]
pub struct CanvasTabViews {
    swerve: HashMap<usize, swerve::SwerveView>,
    joysticks: HashMap<usize, joysticks::JoysticksView>,
    mechanism: inline_svg::SvgView,
    points: inline_svg::SvgView,
    settings: settings::SettingsViews,
}

impl CanvasTabViews {
    fn retain(&mut self, live: &HashSet<usize>, cx: &mut App) {
        let mut dropped = Vec::new();
        self.swerve.retain(|key, view| {
            let keep = live.contains(key);
            if !keep {
                dropped.push(view.take_presenter());
            }
            keep
        });
        for mut presenter in dropped {
            presenter.release(cx);
        }
        self.joysticks.retain(|key, _| live.contains(key));
        self.settings.retain(live);
    }
}

/// A tab by the address of its controller, which stays put while the tab exists.
fn tab_key(tab: &TabRef<'_, HubPage>) -> usize {
    std::ptr::from_ref(tab.controller()).cast::<()>() as usize
}

/// The index and key of the displayed tab when its type is `kind`.
fn displayed(hub: &HubModel, kind: TabType) -> Option<(usize, usize)> {
    hub.tabs()
        .tabs()
        .enumerate()
        .find(|(_, tab)| !tab.content_hidden() && tab.tab_type() == Some(kind))
        .map(|(i, tab)| (i, tab_key(&tab)))
}

fn tab_index(hub: &HubModel, key: usize) -> Option<usize> {
    hub.tabs().tabs().position(|tab| tab_key(&tab) == key)
}

fn with_controller<C: 'static, R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut C) -> R,
) -> Option<R> {
    let tab = hub.tabs_mut().0.tab_mut(index)?;
    Some(f(tab.controller.downcast_mut::<C>()?))
}

fn with_renderer<T: 'static, R>(
    hub: &mut HubModel,
    index: usize,
    f: impl FnOnce(&mut T) -> R,
) -> Option<R> {
    let tab = hub.tabs_mut().0.tab_mut(index)?;
    Some(f(tab.renderer.downcast_mut::<T>()?))
}

/// A CSS length in LayoutUnits, truncated as `LayoutUnit(float)` truncates.
fn lu(v: f64) -> f64 {
    math::trunc(v * 64.0) / 64.0
}

fn local(rect: [f64; 4], origin: [f64; 4]) -> Div {
    div()
        .absolute()
        .left(px((rect[0] - origin[0]) as f32))
        .top(px((rect[1] - origin[1]) as f32))
        .w(px(rect[2] as f32))
        .h(px(rect[3] as f32))
}

fn overhung(rect: [f64; 4]) -> [f64; 4] {
    [rect[0], rect[1] - OVERHANG, rect[2], rect[3] + OVERHANG]
}

impl<H: FrameHost> HubFrame<H> {
    /// The displayed tab's renderer element over `rect` (`div.renderer-content`), if the tab is
    /// one of the four.
    pub(super) fn canvas_tab_element(
        &mut self,
        rect: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let live = self.host().hub(cx).tabs().tabs();
        let live = live.map(|tab| tab_key(&tab)).collect();
        self.canvas_tabs.retain(&live, cx);
        if let Some(element) = self.swerve_element(rect, appearance, scale, window, cx) {
            return Some(element);
        }
        if let Some(element) = self.mechanism_element(rect, appearance, scale, window, cx) {
            return Some(element);
        }
        if let Some(element) = self.points_element(rect, appearance, scale, window, cx) {
            return Some(element);
        }
        self.joysticks_element(cx)
    }

    /// The displayed tab's controls in `controls` (`div.controls-content`), if the tab is one of
    /// the four and has any beside its source list.
    pub(super) fn canvas_tab_controls(
        &mut self,
        controls: [f64; 4],
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        if let Some(element) = self.swerve_settings(controls, appearance, scale, window, cx) {
            return Some(element);
        }
        if let Some(element) = self.points_settings(controls, appearance, scale, window, cx) {
            return Some(element);
        }
        self.joystick_selector(controls, appearance, scale, window, cx)
    }
}
