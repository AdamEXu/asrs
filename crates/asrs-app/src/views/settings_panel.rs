//! The scroller of `div.field-2d-settings`, `div.field-3d-settings`, `div.swerve-settings` and
//! `div.points-settings` (`www/hub.css`), which are alike: a 1 px left border and `overflow-y:
//! auto`.

use std::collections::{HashMap, HashSet};

use asrs_hub::hub::HubPage;
use asrs_hub::tabs::TabRef;
use asrs_ui::focus;
use asrs_ui::gpui::{
    AnyElement, Bounds, Context, Div, FocusHandle, InteractiveElement, IntoElement, ParentElement,
    Pixels, ScrollEvent, StatefulInteractiveElement, Styled, Window, div, px, rgb,
};
use asrs_ui::parity::probed;
use asrs_ui::theme::Appearance;

use super::frame::{FrameHost, HubFrame, SETTINGS_SECTION, wheel_of};
use super::scrollers::{Area, HandleScroller};
use super::source_list::scroll_to;

/// A tab by the address of its controller, which stays put while the tab exists.
pub(super) fn tab_key(tab: &TabRef<'_, HubPage>) -> usize {
    std::ptr::from_ref(tab.controller()).cast::<()>() as usize
}

#[derive(Default)]
pub struct SettingsPanels {
    scrollers: HashMap<usize, HandleScroller>,
    /// The panel drawn last: its tab, its place in the focus order and its box.
    panel: Option<(usize, FocusHandle, [f64; 4])>,
}

impl SettingsPanels {
    /// Scrolls the panel that holds the focused element, at `bounds`, to it.
    pub fn scroll_to(&mut self, focused: &FocusHandle, bounds: Bounds<Pixels>, window: &Window) {
        let Some(key) = self.holding(focused, window) else {
            return;
        };
        if let (Some((_, _, [_, y, _, height])), Some(scroller)) =
            (&self.panel, self.scrollers.get_mut(&key))
        {
            scroll_to(scroller, [*y, *height], bounds, window);
        }
    }

    /// The tab whose panel holds the focused element.
    pub fn holding(&self, focused: &FocusHandle, window: &Window) -> Option<usize> {
        let (key, place, _) = self.panel.as_ref()?;
        place.contains(focused, window).then_some(*key)
    }

    pub fn retain(&mut self, live: &HashSet<usize>) {
        self.scrollers.retain(|key, _| live.contains(key));
    }
}

/// A panel's box, in client coordinates as `controls` (`div.controls-content`) is, and what it
/// shows.
pub(super) struct SettingsPanel {
    pub key: usize,
    pub probe: &'static str,
    pub controls: [f64; 4],
    /// The border box; its first pixel column is the `border-left`.
    pub rect: [f64; 4],
    pub content: Div,
    pub content_height: f64,
}

impl<H: FrameHost> HubFrame<H> {
    pub(super) fn settings_scroller(
        &mut self,
        panel: SettingsPanel,
        appearance: Appearance,
        scale: f32,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let SettingsPanel {
            key,
            probe,
            controls,
            rect,
            content,
            content_height,
        } = panel;
        let border = match appearance {
            Appearance::Dark => 0x000000,
            Appearance::Light => 0xdddddd,
        };
        let [x, y, width, height] = rect;
        let place = focus::place(("settings-panel", key), window, cx);
        let now = self.host().frame_time(cx);
        let views = &mut self.settings_panels;
        views.panel = Some((key, place.clone(), rect));
        let scroller = views.scrollers.entry(key).or_default();
        if scroller.animate(now, f64::from(scale)) {
            window.request_animation_frame();
        }
        let (handle, extent) = (scroller.handle.clone(), scroller.extent());
        let bars = self.area_bars(Area::Settings(key), extent, cx);
        let element = focus::section(div(), SETTINGS_SECTION, &place)
            .absolute()
            .left(px((x - controls[0]) as f32))
            .top(px((y - controls[1]) as f32))
            .w(px(width as f32))
            .h(px(height as f32))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(1.0))
                    .h(px(height as f32))
                    .bg(rgb(border)),
            )
            .child(
                div()
                    .id((probe, key))
                    .absolute()
                    .left(px(1.0))
                    .top_0()
                    .w(px((width - 1.0) as f32))
                    .h(px(height as f32))
                    .overflow_hidden()
                    .track_scroll(&handle)
                    .capture_any_mouse_down(
                        cx.listener(move |frame, _, _, _| frame.press_area(&Area::Settings(key))),
                    )
                    .on_scroll(cx.listener(move |frame, event: &ScrollEvent, window, cx| {
                        frame.settings_wheel(key, event, window, cx)
                    }))
                    .child(content.relative().w_full().h(px(content_height as f32))),
            )
            .child(
                div()
                    .absolute()
                    .left(px(1.0))
                    .top_0()
                    .w(px((width - 1.0) as f32))
                    .h(px(height as f32))
                    .child(bars),
            );
        div()
            .absolute()
            .left_0()
            .top_0()
            .size_full()
            .child(probed(probe, element))
            .into_any_element()
    }

    fn settings_wheel(
        &mut self,
        key: usize,
        event: &ScrollEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let now = self.host().frame_time(cx);
        let scale = f64::from(window.scale_factor());
        if let Some(scroller) = self.settings_panels.scrollers.get_mut(&key) {
            scroller.wheel(wheel_of(event), now, scale);
        }
        cx.notify();
    }

    pub(super) fn settings_smooth(&mut self, key: usize, by: f64, cx: &mut Context<'_, Self>) {
        let now = self.host().frame_time(cx);
        if let Some(scroller) = self.settings_panels.scrollers.get_mut(&key) {
            scroller.smooth(by, now);
        }
    }

    pub(super) fn settings_scroll_to(&mut self, key: usize, to: f64, scale: f64) {
        if let Some(scroller) = self.settings_panels.scrollers.get_mut(&key) {
            scroller.scroll_to(to, scale);
        }
    }

    pub(super) fn settings_range(&self, key: usize) -> Option<[[f64; 2]; 2]> {
        let scroller = self.settings_panels.scrollers.get(&key)?;
        Some(scroller.range())
    }
}
