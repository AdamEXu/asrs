//! The download window: `www/download.html` and `www/download.css` over
//! [`DownloadPage`], and `main.ts`'s `openDownload`.

mod line;
pub mod loading;

use std::ops::Range;
use std::time::Instant;

use asrs_js::time::{Clock, SystemClock};
use asrs_model::preferences::Preferences;
use asrs_model::scroll_sensor::{WheelScroll, assign_scroll_offset};
use asrs_services::download::downloader::WindowMessage;
use asrs_services::download::page::{self, DownloadPage, PageEvent, Row, RowIcon};
use asrs_ui::button::IconButton;
use asrs_ui::gpui::{
    AnyElement, App, AppContext, Context, Div, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, ParentElement, Render,
    ScrollEvent, StatefulInteractiveElement, Styled, Subscription, Window, div, point, px, rgb,
    rgba,
};
use asrs_ui::image_icon::{DropShadow, download_icon, image_icon};
use asrs_ui::parity::probed;
use asrs_ui::platform;
use asrs_ui::progress::progress;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, Text, TextStyle};
use asrs_ui::theme::{Appearance, FONT_FAMILY, MONOSPACE_FAMILY};

use crate::views::frame::wheel_of;
use crate::windows::{self, SingletonOpening, WindowId};
use line::InlineRun;
use loading::Loading;

const CONTROL_BAR: f64 = 46.0;
const ROW_HEIGHT: f64 = page::FILE_ITEM_HEIGHT_PX;
const ROW_MARGIN: f64 = 6.0;
const ROW_PADDING_RIGHT: f64 = 5.0;
const ROW_RADIUS: f32 = 8.0;
const LIST_MARGIN: f64 = 3.0;
const BUTTON_SIZE: f64 = 30.0;
const BUTTON_BOTTOM: f64 = 8.0;
const EXIT_RIGHT: f64 = 43.0;
const DOWNLOAD_RIGHT: f64 = 8.0;
const DETAILS_LEFT: f64 = 12.0;
const DETAILS_RIGHT: f64 = 85.0;

/// What the window's element with focus is called by [`DownloadWindow::active`].
pub const LIST: &str = "file-list";
pub const EXIT: &str = "exit";
pub const DOWNLOAD: &str = "download";

/// Where Tab goes on from while no element has focus.
#[derive(Clone, Copy, PartialEq)]
enum Start {
    Document,
    /// The list, which had focus until it stopped scrolling.
    List,
    /// Something between the list's start and the buttons.
    AfterList,
}

pub struct DownloadWindow {
    page: DownloadPage,
    list_height: f64,
    body: FocusHandle,
    list: FocusHandle,
    exit: FocusHandle,
    download: FocusHandle,
    list_from_keyboard: bool,
    start: Start,
    offset: f64,
    wheel: WheelScroll,
    scale: f32,
    frames_since: Instant,
    loading: Loading,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PageEvent> for DownloadWindow {}

impl Focusable for DownloadWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.body.clone()
    }
}

fn now_ms() -> f64 {
    SystemClock.now()
}

impl DownloadWindow {
    pub fn new(platform: &str, window: &mut Window, cx: &mut Context<'_, Self>) -> DownloadWindow {
        let subscriptions = vec![
            cx.observe_window_activation(window, |view, window, cx| {
                view.page.set_focused(window.is_window_active());
                cx.notify();
            }),
            Self::keys(window, cx),
        ];
        let mut page = DownloadPage::default();
        page.set_platform(platform);
        let body = cx.focus_handle();
        window.focus(&body, cx);
        let mut view = DownloadWindow {
            page,
            list_height: 0.0,
            body,
            list: cx.focus_handle(),
            exit: cx.focus_handle(),
            download: cx.focus_handle(),
            list_from_keyboard: false,
            start: Start::Document,
            offset: 0.0,
            wheel: WheelScroll::default(),
            scale: window.scale_factor(),
            frames_since: cx.background_executor().now(),
            loading: Loading::default(),
            _subscriptions: subscriptions,
        };
        view.measure(window);
        view
    }

    pub fn page(&self) -> &DownloadPage {
        &self.page
    }

    /// `div.file-list`'s `scrollTop`.
    pub fn scroll_top(&self) -> f64 {
        assign_scroll_offset(self.offset, self.max_scroll(), f64::from(self.scale))
    }

    pub fn set_scroll_top(&mut self, top: f64, cx: &mut Context<'_, Self>) {
        self.wheel.stop();
        self.scroll_to(top);
        self.offset = self.scroll_top();
        cx.notify();
    }

    fn scroll_to(&mut self, offset: f64) {
        self.offset = offset.clamp(0.0, self.max_scroll());
    }

    /// The time of the frame, in ms.
    fn frame_time(&self, cx: &App) -> f64 {
        let elapsed = cx
            .background_executor()
            .now()
            .saturating_duration_since(self.frames_since);
        elapsed.as_secs_f64() * 1000.0
    }

    fn scrolled(&mut self, event: &ScrollEvent, _: &mut Window, cx: &mut Context<'_, Self>) {
        if !self.page.scrolls() {
            return;
        }
        let now = self.frame_time(cx);
        let (offset, max) = ([0.0, self.offset], [0.0, self.max_scroll()]);
        let client = [0.0, self.list_height];
        if let Some([_, to]) = self.wheel.wheel(wheel_of(event), now, offset, max, client) {
            self.scroll_to(to);
        }
        cx.notify();
    }

    fn content_height(&self) -> f64 {
        match self.page.rows().len() {
            0 => 0.0,
            rows => rows as f64 * ROW_HEIGHT + 2.0 * LIST_MARGIN,
        }
    }

    fn max_scroll(&self) -> f64 {
        if self.page.scrolls() {
            (self.content_height() - self.list_height).max(0.0)
        } else {
            0.0
        }
    }

    /// The page's `resize` event when the window's content size changed.
    fn measure(&mut self, window: &Window) {
        let height = f64::from(f32::from(window.viewport_size().height)) - CONTROL_BAR;
        if height != self.list_height {
            self.list_height = height;
            self.page.resize(height);
        }
    }

    /// A message of the download service.
    pub fn message(
        &mut self,
        message: &WindowMessage,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.message_at(message, now_ms(), window, cx);
    }

    /// [`Self::message`] with the `Date` it arrives at.
    pub fn message_at(
        &mut self,
        message: &WindowMessage,
        now_ms: f64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.measure(window);
        if let Err(e) = self.page.message(message, now_ms) {
            eprintln!("asrs: download window: {}: {}", e.name, e.message);
        }
        self.changed(window, cx);
    }

    /// The `set-preferences` message.
    pub fn set_preferences(&mut self, preferences: &Preferences, cx: &mut Context<'_, Self>) {
        let event = self.page.set_preferences(preferences);
        cx.emit(event);
    }

    /// The `set-focused` message.
    pub fn set_focused(&mut self, focused: bool, cx: &mut Context<'_, Self>) {
        self.page.set_focused(focused);
        cx.notify();
    }

    /// Keeps the scroll position in its bounds, and the focus off a list that stopped scrolling.
    fn changed(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.offset > self.max_scroll() {
            self.set_scroll_top(self.max_scroll(), cx);
        }
        if self.list.is_focused(window) && !self.list_takes_focus() {
            self.start = Start::List;
            window.focus(&self.body, cx);
        }
    }

    /// Blink lets the keyboard focus a scroller without focusable content.
    fn list_takes_focus(&self) -> bool {
        self.page.scrolls() && !self.page.rows().is_empty()
    }

    /// Keys reach the page before the focused button, whose own click comes after the window's
    /// `keydown` listener. The numpad's Enter is not `event.code === "Enter"`.
    fn keys(window: &Window, cx: &mut Context<'_, Self>) -> Subscription {
        let handle = window.window_handle();
        let this = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != handle {
                return;
            }
            let modifiers = event.keystroke.modifiers;
            let _ = this.update(cx, |view, cx| match event.keystroke.key.as_str() {
                "enter" => {
                    if !platform::numpad_enter(cx) {
                        cx.emit(view.page.save());
                    }
                    if view.press(window, cx) {
                        cx.stop_propagation();
                    }
                }
                "tab" if !modifiers.control && !modifiers.alt && !modifiers.platform => {
                    view.tab(modifiers.shift, window, cx);
                    cx.stop_propagation();
                }
                "a" if !modifiers.shift => {
                    let command = match view.page.platform() {
                        "darwin" => modifiers.platform,
                        _ => modifiers.control,
                    };
                    if command {
                        view.page.select_all();
                        cx.notify();
                        cx.stop_propagation();
                    }
                }
                _ => {}
            });
        })
    }

    /// Enter's click on the focused button; false when neither has focus.
    fn press(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> bool {
        match self.active(window) {
            Some(EXIT) => cx.emit(PageEvent::Close),
            Some(DOWNLOAD) => cx.emit(self.page.save()),
            _ => return false,
        }
        true
    }

    fn tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<'_, Self>) {
        let list = self.list_takes_focus();
        let stops = &[LIST, EXIT, DOWNLOAD][usize::from(!list)..];
        let (first, last) = (0, stops.len() - 1);
        let at = self
            .active(window)
            .and_then(|id| stops.iter().position(|stop| *stop == id));
        let next = match (at, back) {
            (Some(i), false) => (i + 1) % stops.len(),
            (Some(i), true) => (i + last) % stops.len(),
            (None, false) if self.start == Start::Document => first,
            (None, false) => last - 1,
            (None, true) if self.start == Start::AfterList && list => first,
            (None, true) => last,
        };
        let handle = match stops[next] {
            LIST => &self.list,
            EXIT => &self.exit,
            _ => &self.download,
        };
        window.focus(handle, cx);
        self.list_from_keyboard = true;
        cx.notify();
    }

    /// The id of `document.activeElement`, or the list's class.
    pub fn active(&self, window: &Window) -> Option<&'static str> {
        [
            (LIST, &self.list),
            (EXIT, &self.exit),
            (DOWNLOAD, &self.download),
        ]
        .into_iter()
        .find(|(_, handle)| handle.is_focused(window))
        .map(|(id, _)| id)
    }

    fn mouse_down(&mut self, _: &MouseDownEvent, _: &mut Window, _: &mut Context<'_, Self>) {
        self.start = Start::AfterList;
    }

    fn click_row(&mut self, index: usize, shift: bool, cx: &mut Context<'_, Self>) {
        if let Err(e) = self.page.click(index, shift) {
            eprintln!("asrs: download window: {}: {}", e.name, e.message);
        }
        cx.notify();
    }

    fn button(
        &self,
        id: &'static str,
        icon: &str,
        focus: &FocusHandle,
        right: f64,
        event: fn(&DownloadPage) -> PageEvent,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let button = IconButton::new(id, Symbol::named(icon).expect("download icon"))
            .size(BUTTON_SIZE as f32, BUTTON_SIZE as f32)
            .blurred(self.page.blurred())
            .track_focus(focus)
            .on_click(cx.listener(move |view, _, _, cx| {
                let event = event(&view.page);
                cx.emit(event);
            }));
        div()
            .absolute()
            .right(px(right as f32))
            .bottom(px(BUTTON_BOTTOM as f32))
            .child(probed(format!("button#{id}"), button))
    }

    /// The rows at least partly in the list.
    fn shown_rows(&self, cx: &App) -> Range<usize> {
        #[cfg(feature = "parity")]
        if cx.has_global::<asrs_ui::parity::Recorder>() {
            return 0..self.page.rows().len();
        }
        let _ = cx;
        let top = self.scroll_top() - LIST_MARGIN;
        let first = (top / ROW_HEIGHT).floor().max(0.0) as usize;
        let last = ((top + self.list_height) / ROW_HEIGHT).ceil().max(0.0) as usize;
        let rows = self.page.rows().len();
        first.min(rows)..last.min(rows)
    }
}

fn font_style(css: &str, families: &[&str], declarations: &[(&str, &str)]) -> TextStyle {
    let font = format!("{css} {}", families.join(", "));
    let all = std::iter::once(("font", font.as_str())).chain(declarations.iter().copied());
    TextStyle::from_css(all).expect("download text style")
}

fn row_style(weight: &str) -> TextStyle {
    let declarations = [("line-height", "25px"), ("white-space", "pre")];
    font_style(&format!("{weight} 14px"), FONT_FAMILY, &declarations)
}

fn alert_style() -> TextStyle {
    let declarations = [("line-height", "30px"), ("white-space", "pre")];
    font_style("14px", FONT_FAMILY, &declarations)
}

fn details_style() -> TextStyle {
    let declarations = [
        ("text-align", "center"),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ];
    font_style("12px", MONOSPACE_FAMILY, &declarations)
}

/// `div.file-item img`'s box, less the paddings that only push the text on.
struct ImagePlace {
    padding_left: f64,
    size: f64,
    padding_right: f64,
    /// `position: relative; top`.
    top: f64,
}

impl ImagePlace {
    fn of(icon: &RowIcon) -> ImagePlace {
        let (padding_left, size, padding_right, top) = match icon.class {
            Some("mac") => (7.0, 18.0, 4.0, 3.0),
            _ => (8.0, 15.0, 5.0, 2.0),
        };
        ImagePlace {
            padding_left,
            size,
            padding_right,
            top,
        }
    }

    fn width(&self) -> f64 {
        self.padding_left + self.size + self.padding_right
    }
}

/// The `filter` of a row's `img`: whether it inverts, and its shadow.
fn image_filter(icon: &RowIcon, selected: bool, appearance: Appearance) -> (bool, DropShadow) {
    let shadow = |blur, color| DropShadow { blur, color };
    match (appearance, selected) {
        (Appearance::Light, _) => (false, shadow(1.0, 0xbbbbbb)),
        (Appearance::Dark, true) => (false, shadow(1.0, 0x000000)),
        (Appearance::Dark, false) => (icon.class == Some("folder"), shadow(4.0, 0x000000)),
    }
}

/// A row's background and, unless it is selected, its hover background.
fn row_colors(index: usize, selected: bool, appearance: Appearance) -> (Option<u32>, Option<u32>) {
    let even = index % 2 == 1;
    let pick = |light: (u32, u32), dark: (u32, u32)| {
        let (odd_color, even_color) = appearance.pick(light, dark);
        if even { even_color } else { odd_color }
    };
    if selected {
        return (Some(pick((0x8888ff, 0x7777ee), (0x2222aa, 0x3333bb))), None);
    }
    let background = even.then(|| appearance.pick(0xdddddd, 0x1a1a1a));
    let hover = pick((0xe8e8e8, 0xd7d7d7), (0x151515, 0x1e1e1e));
    (background, Some(hover))
}

impl DownloadWindow {
    fn row(
        &self,
        index: usize,
        row: &Row,
        width: f64,
        appearance: Appearance,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let (background, hover) = row_colors(index, row.selected, appearance);
        let mut element = div()
            .id(("file-item", index))
            .absolute()
            .left(px(ROW_MARGIN as f32))
            .top(px((LIST_MARGIN + index as f64 * ROW_HEIGHT) as f32))
            .w(px((width - 2.0 * ROW_MARGIN) as f32))
            .h(px(ROW_HEIGHT as f32))
            .rounded(px(ROW_RADIUS))
            .overflow_hidden();
        if let Some(color) = background {
            element = element.bg(rgb(color));
        }
        if let Some(color) = hover {
            element = element.hover(move |s| s.bg(rgb(color)));
        }
        if let Some(item) = &row.item {
            let scale = window.scale_factor();
            let place = ImagePlace::of(&item.icon);
            let style = row_style("400");
            let (ascent, descent) = text::font_metrics(cx, &style, scale);
            let (ascent, descent) = (f64::from(ascent), f64::from(descent));
            let device = f64::from(scale);
            let leading = ((ROW_HEIGHT * device - ascent - descent) / 2.0).floor();
            let lowered = (place.size * device - leading - ascent).max(0.0);
            let baseline = (leading + lowered + ascent) / device;
            if let Some(png) = item.icon.file.as_deref().and_then(download_icon) {
                let (invert, shadow) = image_filter(&item.icon, row.selected, appearance);
                element = element.child(
                    div()
                        .absolute()
                        .left(px(place.padding_left as f32))
                        .top(px((baseline - place.size + place.top) as f32))
                        .child(image_icon(png, place.size, invert, shadow)),
                );
            }
            let image = div()
                .absolute()
                .left(px(0.0))
                .top(px((baseline - place.size + place.top) as f32))
                .w(px(place.width() as f32))
                .h(px(place.size as f32));
            element = element.child(probed("div.file-item img", image));
            let size_color = match (appearance, row.selected) {
                (_, false) => 0x777777,
                (Appearance::Light, true) => 0x555555,
                (Appearance::Dark, true) => 0x999999,
            };
            let runs = [
                InlineRun {
                    text: item.name.clone(),
                    style,
                    color: appearance.theme().text,
                    underline: false,
                },
                InlineRun {
                    text: item.size.clone(),
                    style: row_style("300"),
                    color: size_color,
                    underline: false,
                },
            ];
            let left = place.width();
            let available = width - 2.0 * ROW_MARGIN - ROW_PADDING_RIGHT;
            let top = lowered / device;
            for child in line::line(&runs, &style, left, top, available, ROW_MARGIN, scale, cx) {
                element = element.child(child);
            }
            element = element.on_click(cx.listener(move |view, event, _, cx| {
                let event: &asrs_ui::gpui::ClickEvent = event;
                view.click_row(index, event.modifiers().shift, cx);
            }));
        }
        probed("div.file-item", element).into_any_element()
    }

    fn file_list(
        &mut self,
        width: f64,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let now = self.frame_time(cx);
        if let Some([_, to]) = self.wheel.animate(now) {
            self.scroll_to(to);
        }
        let rows = self.shown_rows(cx);
        let mut content = div()
            .absolute()
            .left_0()
            .top(px(-self.scroll_top() as f32))
            .w_full()
            .h(px(self.content_height() as f32));
        for index in rows {
            let row = self.page.rows()[index].clone();
            content = content.child(self.row(index, &row, width, appearance, window, cx));
        }
        let mut list = div()
            .id(LIST)
            .relative()
            .size_full()
            .overflow_hidden()
            .on_scroll(cx.listener(Self::scrolled))
            .child(content);
        if self.page.loading() {
            list = list.child(self.loading.bars(width, self.list_height, now));
        } else {
            self.loading.stop();
        }
        if self.page.loading() || self.wheel.animating() {
            window.request_animation_frame();
        }
        let shadow = inset_shadow(appearance);
        let mut boxed = div()
            .relative()
            .w_full()
            .h(px(self.list_height as f32))
            .bg(rgb(appearance.pick(0xeeeeee, 0x111111)))
            .child(shadow)
            .child(list);
        let focused = self.list.is_focused(window);
        self.list_from_keyboard = focused && self.list_from_keyboard;
        if self.list_from_keyboard && window.is_window_active() {
            boxed = boxed.child(
                div()
                    .absolute()
                    .top(px(-2.0))
                    .left(px(-2.0))
                    .right(px(-2.0))
                    .bottom(px(-2.0))
                    .border(px(2.0))
                    .border_color(rgb(appearance.theme().focus_ring))
                    .rounded(px(5.0)),
            );
        }
        probed("div.file-list", boxed).into_any_element()
    }

    fn control_bar(
        &self,
        width: f64,
        height: f64,
        appearance: Appearance,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let scale = window.scale_factor();
        let color = appearance.theme().text;
        let details_width = width - DETAILS_LEFT - DETAILS_RIGHT;
        let place = |bottom: f64, h: f64| {
            div()
                .absolute()
                .left(px(DETAILS_LEFT as f32))
                .top(px((height - bottom - h) as f32))
                .w(px(details_width as f32))
                .h(px(h as f32))
        };
        let mut out = Vec::new();
        if let Some(html) = self.page.alert() {
            let runs: Vec<InlineRun> = page::alert_runs(html)
                .into_iter()
                .map(|run| InlineRun {
                    text: run.text,
                    style: alert_style(),
                    color,
                    underline: run.underline,
                })
                .collect();
            let style = alert_style();
            let alert = place(BUTTON_BOTTOM, BUTTON_SIZE)
                .overflow_hidden()
                .children(line::line(
                    &runs,
                    &style,
                    0.0,
                    0.0,
                    details_width,
                    DETAILS_LEFT,
                    scale,
                    cx,
                ));
            out.push(probed("div.alert-text", alert).into_any_element());
        }
        if let Some((value, details)) = self.page.progress() {
            let bar = place(23.0, 18.0).child(progress(value));
            out.push(probed("progress", bar).into_any_element());
            let text = Text::new(details.to_owned(), details_style())
                .color(color)
                .content_width(details_width);
            let details = place(5.0, 18.0).overflow_hidden().child(text);
            out.push(probed("div.progress-details", details).into_any_element());
        }
        out
    }
}

/// `box-shadow: inset 0px 0px 2px`, whose blur Skia draws with half the radius as its standard
/// deviation.
fn inset_shadow(appearance: Appearance) -> impl IntoElement {
    let color = appearance.pick(0xcccccc, 0x000000);
    asrs_ui::gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let scale = window.scale_factor();
            let depth = (3.0 * scale).ceil() as i32;
            for i in 0..depth {
                let coverage = gaussian_tail((i as f32 + 0.5) / scale);
                let alpha = (coverage * 255.0).round() as u32;
                let (from, thickness) = (px(i as f32 / scale), px(1.0 / scale));
                let (left, top, right, bottom) = (
                    bounds.origin.x,
                    bounds.origin.y,
                    bounds.right(),
                    bounds.bottom(),
                );
                for (x, y, w, h) in [
                    (left + from, top, thickness, bounds.size.height),
                    (right - from - thickness, top, thickness, bounds.size.height),
                    (left, top + from, bounds.size.width, thickness),
                    (
                        left,
                        bottom - from - thickness,
                        bounds.size.width,
                        thickness,
                    ),
                ] {
                    let rect = asrs_ui::gpui::Bounds::new(point(x, y), asrs_ui::gpui::size(w, h));
                    window.paint_quad(asrs_ui::gpui::fill(rect, rgba(color << 8 | alpha)));
                }
            }
        },
    )
    .absolute()
    .size_full()
}

/// The part of a standard normal distribution above `x`.
fn gaussian_tail(x: f32) -> f32 {
    let t = 1.0 / (1.0 + 0.231_641_9 * x);
    let poly = t
        * (0.319_381_53
            + t * (-0.356_563_78 + t * (1.781_477_9 + t * (-1.821_255_9 + t * 1.330_274_5))));
    0.398_942_3 * (-x * x / 2.0).exp() * poly
}

impl Render for DownloadWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.scale = window.scale_factor();
        self.measure(window);
        self.changed(window, cx);
        let appearance = Appearance::of(window, cx);
        let viewport = window.viewport_size();
        let (width, height) = (
            f64::from(f32::from(viewport.width)),
            f64::from(f32::from(viewport.height)),
        );
        let (exit, download) = (self.exit.clone(), self.download.clone());
        div()
            .id("download")
            .track_focus(&self.body)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
            .relative()
            .size_full()
            .bg(rgb(appearance.theme().background))
            .children(self.control_bar(width, height, appearance, window, cx))
            .child(self.button(
                EXIT,
                "close.svg",
                &exit,
                EXIT_RIGHT,
                |_| PageEvent::Close,
                cx,
            ))
            .child(self.button(
                DOWNLOAD,
                "download.svg",
                &download,
                DOWNLOAD_RIGHT,
                DownloadPage::save,
                cx,
            ))
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .w_full()
                    .child(self.file_list(width, appearance, window, cx)),
            )
    }
}

/// The window of `openDownload`, or `None` when it was open and now has the focus.
pub fn open(cx: &mut App, parent: WindowId, platform: &str) -> SingletonOpening<DownloadWindow> {
    windows::open_download(cx, parent, |window, cx| {
        window.set_window_title(&format!("Download Logs \u{2014} {}", crate::IDENTITY.name));
        cx.new(|cx| DownloadWindow::new(platform, window, cx))
    })
}
