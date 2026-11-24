//! The hub's scrollers: which one a key scrolls, and their scrollbars.

use std::collections::HashMap;

use asrs_js::string::JsString;
use asrs_model::scroll_sensor::{Wheel, WheelScroll, assign_scroll_offset};
use asrs_model::selection_impl::KeyTarget;
use asrs_ui::gpui::{
    Context, FocusHandle, IntoElement, KeyDownEvent, Pixels, Point, ScrollHandle, Window, point, px,
};
use asrs_ui::scroll::{self, Extent, Request, Scrollbars};

use super::frame::{FrameHost, HubFrame, key_target};

/// A scroller, by the tab it is in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Area {
    Table(JsString),
    Console(usize),
    Statistics(usize),
    StatisticsSettings(usize),
    CanvasSettings(usize),
    /// A source list's UUID.
    List(String),
    SideBar,
    Timeline,
    TabBar,
    LineGraph(usize),
    Field2d(usize),
}

#[derive(Default)]
pub struct Scrollers {
    /// The scroller with `mouse_press_node`.
    pressed: Option<Area>,
    bars: HashMap<Area, Scrollbars>,
}

/// A wheel event that scrolls smoothly by `by`.
pub fn smooth(by: [f64; 2]) -> Wheel {
    Wheel {
        delta: by.map(|v| -v),
        precise: false,
    }
}

/// A box GPUI lays out and scrolls vertically by its handle, which Chromium's wheel and key
/// scrolling moves.
#[derive(Clone, Default)]
pub struct HandleScroller {
    pub handle: ScrollHandle,
    pub bars: Scrollbars,
    wheel: WheelScroll,
    /// The scroll offset before it is snapped to the device pixel grid, and what it is snapped to.
    offset: Option<(f64, f32)>,
}

impl HandleScroller {
    /// `clientWidth` and `clientHeight` and the largest offsets, as last laid out.
    pub fn range(&self) -> [[f64; 2]; 2] {
        let size = self.handle.bounds().size;
        let client = [size.width, size.height].map(|v| f64::from(f32::from(v)));
        let max = f64::from(f32::from(self.handle.max_offset().y));
        [client, [0.0, max]]
    }

    pub fn extent(&self) -> Extent {
        let [client, max] = self.range();
        Extent {
            content: [0.0, client[1] + max[1]],
            offset: [0.0, self.top()],
            axes: [false, true],
            dark: false,
        }
    }

    pub fn top(&self) -> f64 {
        -f64::from(f32::from(self.handle.offset().y))
    }

    fn offset(&self) -> f64 {
        let shown = self.top();
        match self.offset {
            Some((raw, snapped)) if snapped == shown as f32 => raw,
            _ => shown,
        }
    }

    fn set(&mut self, offset: f64, scale: f64) {
        let [_, [_, max]] = self.range();
        let raw = offset.clamp(0.0, max);
        let snapped = assign_scroll_offset(raw, max, scale) as f32;
        self.offset = Some((raw, snapped));
        self.handle.set_offset(point(px(0.0), px(-snapped)));
    }

    /// Scrolls to `offset` at once.
    pub fn scroll_to(&mut self, offset: f64, scale: f64) {
        self.wheel.stop();
        self.set(offset, scale);
    }

    pub fn wheel(&mut self, wheel: Wheel, now: f64, scale: f64) {
        let [client, _] = self.range();
        self.scroll(wheel, client, now, scale);
    }

    /// Whether a wheel event would move it.
    pub fn takes(&self, wheel: Wheel) -> bool {
        let [_, [_, max]] = self.range();
        let to = (self.offset() - wheel.delta[1]).clamp(0.0, max);
        to != self.offset()
    }

    pub fn smooth(&mut self, by: f64, now: f64) {
        self.scroll(smooth([0.0, by]), [f64::INFINITY; 2], now, 1.0);
    }

    fn scroll(&mut self, wheel: Wheel, client: [f64; 2], now: f64, scale: f64) {
        let [_, max] = self.range();
        let offset = [0.0, self.offset()];
        if let Some([_, to]) = self.wheel.wheel(wheel, now, offset, max, client) {
            self.set(to, scale);
        }
    }

    /// Advances a smooth scroll to the frame at `now` (ms); whether it goes on.
    pub fn animate(&mut self, now: f64, scale: f64) -> bool {
        if let Some([_, to]) = self.wheel.animate(now) {
            self.set(to, scale);
        }
        self.wheel.animating()
    }
}

impl<H: FrameHost> HubFrame<H> {
    pub(super) fn press_area(&mut self, area: &Area) {
        self.scrollers.pressed = Some(area.clone());
    }

    /// A press anywhere, before those of the scrollers.
    pub(super) fn press_outside(&mut self) {
        self.scrollers.pressed = None;
    }

    pub(super) fn on_scrollbar(&self, position: Point<Pixels>) -> bool {
        self.scrollers.bars.values().any(|b| b.contains(position))
    }

    /// The scrollbars of `area`, to place over its content.
    pub(super) fn area_bars(
        &mut self,
        area: Area,
        extent: Extent,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<H> {
        let bars = self.scrollers.bars.entry(area.clone()).or_default();
        let frame = cx.entity().downgrade();
        scroll::scrollbars(bars, extent, move |request, window, cx| {
            let _ = frame.update(cx, |frame, cx| {
                frame.area_request(&area, request, window, cx);
            });
        })
    }

    /// Forgets the scrollbars that did not show in the last frame.
    pub(super) fn scrollers_frame(&mut self) {
        self.scrollers.bars.retain(|_, bars| bars.take_laid_out());
    }

    fn area_request(
        &mut self,
        area: &Area,
        request: Request,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) {
        match request {
            Request::Press => self.press_area(area),
            Request::To(to) => self.area_scroll_to(area, to, f64::from(window.scale_factor()), cx),
            Request::By(by) => self.area_smooth(area, by, cx),
        }
    }

    fn area_scroll_to(
        &mut self,
        area: &Area,
        to: [f64; 2],
        scale: f64,
        cx: &mut Context<'_, Self>,
    ) {
        match area {
            Area::Table(_) => self.table_scroll_to(to, cx),
            Area::Console(key) => self.console_scroll_to(*key, to[1], cx),
            Area::Statistics(key) => self.statistics_scroll_to(*key, to, cx),
            Area::StatisticsSettings(key) => self.statistics_settings_scroll_to(*key, to[1], cx),
            Area::CanvasSettings(key) => self.canvas_settings_scroll_to(*key, to[1], scale),
            Area::List(uuid) => self.source_list_scroll_to(uuid, to[1], scale),
            Area::SideBar
            | Area::Timeline
            | Area::TabBar
            | Area::LineGraph(_)
            | Area::Field2d(_) => {}
        }
        cx.notify();
    }

    fn area_smooth(&mut self, area: &Area, by: [f64; 2], cx: &mut Context<'_, Self>) {
        match area {
            Area::Table(_) => self.table_smooth(by, cx),
            Area::Console(key) => self.console_smooth(*key, by[1], cx),
            Area::Statistics(key) => self.statistics_smooth(*key, by, cx),
            Area::StatisticsSettings(key) => self.statistics_settings_smooth(*key, by[1], cx),
            Area::CanvasSettings(key) => self.canvas_settings_smooth(*key, by[1], cx),
            Area::List(uuid) => self.source_list_smooth(uuid, by[1], cx),
            Area::SideBar => self.host_mut().side_bar_smooth(by[1], cx),
            Area::Timeline => self.timeline_smooth(by, cx),
            Area::TabBar => self.tab_bar_smooth(by, cx),
            Area::LineGraph(key) => self.line_graph_smooth(*key, by, cx),
            Area::Field2d(key) => self.field_2d_smooth(*key, by, cx),
        }
        cx.notify();
    }

    /// `clientWidth` and `clientHeight` and the largest offsets of `area`, if it shows.
    fn area_range(&self, area: &Area, cx: &Context<'_, Self>) -> Option<[[f64; 2]; 2]> {
        match area {
            Area::Table(key) => self.table_range(key, cx),
            Area::Console(key) => self.console_range(*key),
            Area::Statistics(key) => self.statistics_range(*key),
            Area::StatisticsSettings(key) => self.statistics_settings_range(*key),
            Area::CanvasSettings(key) if self.scrollers.bars.contains_key(area) => {
                self.canvas_settings_range(*key)
            }
            Area::CanvasSettings(_) => None,
            Area::List(uuid) => self.source_list_range(uuid),
            Area::SideBar => self.host().side_bar_range(cx),
            Area::Timeline => self.timeline_range(),
            Area::TabBar => self.tab_bar_range(),
            Area::LineGraph(key) => self.line_graph_range(*key),
            Area::Field2d(key) => self.field_2d_range(*key),
        }
    }

    /// The scroller that holds the focused element.
    fn focused_area(
        &self,
        focused: &FocusHandle,
        window: &Window,
        cx: &Context<'_, Self>,
    ) -> Option<Area> {
        if self.places.side_bar.contains(focused, window) {
            return Some(Area::SideBar);
        }
        let list = self.focused_source_list(focused, window).map(Area::List);
        let settings = || self.canvas_tabs.holding(focused, window);
        list.or_else(|| settings().map(Area::CanvasSettings))
            .or_else(|| self.focused_statistics(focused, cx))
            .or_else(|| self.focused_table(focused).map(Area::Table))
    }

    /// A key that neither the page nor the focused element took.
    pub(super) fn scroll_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if window.default_prevented() {
            cx.stop_propagation();
            return;
        }
        if key_target(window) != Some(KeyTarget::Other) {
            return;
        }
        let Some(step) = scroll::step(&event.keystroke) else {
            return;
        };
        let focused = window.focused(cx).filter(|f| *f != self.body && f.tab_stop);
        let area = match &focused {
            Some(focused) => self.focused_area(focused, window, cx),
            None => self.scrollers.pressed.clone(),
        };
        let Some(area) = area else {
            return;
        };
        // A button takes Space, and a scroller that has the focus does not.
        let scroller = matches!(area, Area::Statistics(_));
        if event.keystroke.key == "space" && focused.is_some() && !scroller {
            return;
        }
        if let Some([client, max]) = self.area_range(&area, cx) {
            let by = step.delta(client, max, f64::from(window.scale_factor()));
            self.area_smooth(&area, by, cx);
            cx.stop_propagation();
        }
    }
}
