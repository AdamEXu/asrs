//! The sidebar: `div.side-bar` and `div.search-results` of `www/hub.html` under `hub.css`,
//! the DOM side of `src/hub/Sidebar.ts`. Rows come from `asrs_hub::sidebar::Sidebar`, which
//! keeps every row upstream builds; this view lays out only the rows a scroll position shows
//! and answers the model's layout reads ([`ViewGeometry`]) from the same arithmetic, so the
//! active fields follow what is on screen.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::ops::Range;
use std::rc::Rc;

use asrs_hub::hub::HubModel;
use asrs_hub::sidebar::{RowRect, RowRef, RowValue, SidebarGeometry, SidebarRow};
use asrs_js::string::JsString;
use asrs_ui::button::IconButton;
use asrs_ui::focus::{self, ScrollAxis};
use asrs_ui::gpui::{
    AnyElement, App, AppContext, Context, DispatchPhase, Entity, HitboxBehavior,
    InteractiveElement, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseUpEvent,
    ParentElement, Render, ScrollHandle, StatefulInteractiveElement, Styled, Subscription,
    TouchDragEvent, TouchPhase, Window, anchored, canvas, deferred, div, point, px, rgb, rgba,
};
use asrs_ui::icon::{Icon, IconFit, IconSource};
use asrs_ui::input::{FieldEvent, FieldKind, FieldStyle, PlainText, TextField};
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{Measurer, Text, TextStyle};
use asrs_ui::theme::{Appearance, FONT_FAMILY, IconTint, chains};
use asrs_ui::tooltip::{covering, titled, untitled};

use crate::hub::Hub;
use crate::views::frame::Section;
use crate::views::number::FormNumber;

const TITLE_HEIGHT: f64 = 30.0;
const SEARCH_TOP: f64 = TITLE_HEIGHT + 5.0;
const SEARCH_HEIGHT: f64 = 29.0;
const LIST_TOP: f64 = TITLE_HEIGHT + 39.0;
const ROW_HEIGHT: f64 = 30.0;
/// The box of an inline element in a row.
const INLINE_HEIGHT: f32 = 18.0;
const TUNING_INPUT_WIDTH: f64 = 56.0;
const BOOLEAN_WIDTH: f64 = 9.0;
const RESULTS_ITEM_HEIGHT: f64 = 25.0;
const RESULTS_MAX_HEIGHT: f64 = 290.0;

fn font(css: &str) -> String {
    format!("{css} {}", FONT_FAMILY.join(", "))
}

fn style(declarations: &[(&str, &str)]) -> TextStyle {
    TextStyle::from_css(declarations.iter().copied()).expect("sidebar text style")
}

fn title_style() -> TextStyle {
    style(&[
        ("font", &font("bold 12px")),
        ("line-height", "20px"),
        ("text-align", "center"),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ])
}

fn label_style(italic: bool, bold: bool) -> TextStyle {
    let css = match (italic, bold) {
        (true, true) => "italic bold 15px",
        (true, false) => "italic 15px",
        (false, true) => "bold 15px",
        (false, false) => "15px",
    };
    style(&[("font", &font(css)), ("white-space", "pre")])
}

fn value_style() -> TextStyle {
    style(&[
        ("font", "15px Courier, monospace"),
        ("line-height", "30px"),
        ("white-space", "nowrap"),
    ])
}

/// `div.field-value`'s text, `right: 12px`: its box ends there at its exact width, which GPUI
/// rounds, so the text is painted from where the exact width starts.
fn value_text(text: String, color: u32, window: &Window, cx: &mut App) -> AnyElement {
    let scale = window.scale_factor();
    let (width, _) = asrs_ui::text::measure(cx, &value_style(), &text, scale);
    let device = width * f64::from(scale);
    let dx = ((device + 0.5).floor() - device) / f64::from(scale);
    Text::new(text, value_style())
        .color(color)
        .subpixel(dx as f32, 0.0)
        .into_any_element()
}

fn results_style() -> TextStyle {
    style(&[
        ("font", &font("12px")),
        ("line-height", "25px"),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ])
}

/// What the view last laid out, which [`ViewGeometry`] answers from, and the scroll writes and
/// blurs the model asked for since, which the view applies on its next render.
pub struct SidebarLayout {
    /// `div.side-bar`'s box in client px.
    side_bar: [f64; 4],
    scroll_top: f64,
    displayed: usize,
    tuning_shown: bool,
    search_focused: bool,
    scale: f32,
    measurer: Measurer,
    scroll_to: Option<f64>,
    blur: bool,
    results_scroll: Option<ResultsScroll>,
    menu_buttons: Vec<[f64; 4]>,
    touched: Option<usize>,
}

#[derive(Clone, Copy)]
enum ResultsScroll {
    Top,
    Item(usize),
}

impl Default for SidebarLayout {
    fn default() -> Self {
        SidebarLayout {
            side_bar: [0.0; 4],
            scroll_top: 0.0,
            displayed: 0,
            tuning_shown: false,
            search_focused: false,
            scale: 1.0,
            measurer: Measurer::default(),
            scroll_to: None,
            blur: false,
            results_scroll: None,
            menu_buttons: Vec::new(),
            touched: None,
        }
    }
}

impl SidebarLayout {
    fn max_scroll(&self) -> f64 {
        (LIST_TOP + self.displayed as f64 * ROW_HEIGHT - self.side_bar[3]).max(0.0)
    }

    fn scroll(&mut self, top: f64) {
        let top = top.clamp(0.0, self.max_scroll()).round();
        self.scroll_top = top;
        self.scroll_to = Some(top);
    }

    fn row_top(&self, index: usize) -> f64 {
        self.side_bar[1] + LIST_TOP + index as f64 * ROW_HEIGHT - self.scroll_top
    }
}

/// The sidebar's layout reads, answered from [`SidebarLayout`].
#[derive(Clone, Default)]
pub struct ViewGeometry(Rc<RefCell<SidebarLayout>>);

impl ViewGeometry {
    pub fn new() -> Self {
        ViewGeometry::default()
    }
}

impl SidebarGeometry for ViewGeometry {
    fn field_item_rect(&mut self, row: RowRef<'_>) -> RowRect {
        let layout = self.0.borrow();
        let Some(index) = row.display_index() else {
            return RowRect::default();
        };
        RowRect {
            x: layout.side_bar[0],
            y: layout.row_top(index),
            width: (layout.side_bar[2] - row.value_width()).max(0.0),
            height: ROW_HEIGHT,
        }
    }

    fn value_client_width(&mut self, row: RowRef<'_>, value: &RowValue, hidden: bool) -> f64 {
        if hidden || row.display_index().is_none() {
            return 0.0;
        }
        let mut layout = self.0.borrow_mut();
        let scale = layout.scale;
        let mut text_width = |text: &JsString| {
            if text.is_empty() {
                0.0
            } else {
                let (w, _) =
                    layout
                        .measurer
                        .measure(&value_style(), &text.to_string_lossy(), scale);
                w.round()
            }
        };
        match value {
            RowValue::None => 0.0,
            RowValue::Bool { .. } => BOOLEAN_WIDTH,
            RowValue::Number {
                input_hidden: false,
                ..
            } => TUNING_INPUT_WIDTH,
            RowValue::Number { span_text, .. } => text_width(span_text),
            RowValue::Text { text } => text_width(text),
        }
    }

    fn label_rect(&mut self, row: RowRef<'_>) -> RowRect {
        let rect = self.field_item_rect(row);
        if rect.height == 0.0 {
            return rect;
        }
        let indent = row.indent();
        RowRect {
            x: rect.x + 35.0 + indent,
            y: rect.y + 5.0,
            width: (rect.width - 45.0 - indent).max(0.0),
            height: 20.0,
        }
    }

    fn scroll_into_view_if_needed(&mut self, row: RowRef<'_>) {
        let mut layout = self.0.borrow_mut();
        let Some(index) = row.display_index() else {
            return;
        };
        let top = LIST_TOP + index as f64 * ROW_HEIGHT;
        let height = layout.side_bar[3];
        if top < layout.scroll_top || top + ROW_HEIGHT > layout.scroll_top + height {
            let centered = top + ROW_HEIGHT / 2.0 - height / 2.0;
            layout.scroll(centered);
        }
    }

    fn search_input_rect(&mut self) -> RowRect {
        let layout = self.0.borrow();
        let [x, y, width, _] = layout.side_bar;
        let tuning = if layout.tuning_shown { 26.0 } else { 0.0 };
        RowRect {
            x: x + 16.0,
            y: y + SEARCH_TOP - layout.scroll_top,
            width: (width - 32.0 - tuning).max(0.0),
            height: SEARCH_HEIGHT,
        }
    }

    fn scroll_search_item_into_view(&mut self, index: usize) {
        self.0.borrow_mut().results_scroll = Some(ResultsScroll::Item(index));
    }

    fn reset_search_results_scroll(&mut self) {
        self.0.borrow_mut().results_scroll = Some(ResultsScroll::Top);
    }

    fn scroll_top(&mut self) -> f64 {
        self.0.borrow().scroll_top
    }

    fn set_scroll_top(&mut self, value: f64) {
        self.0.borrow_mut().scroll(value);
    }

    fn menu_button_rect(&mut self, index: usize) -> RowRect {
        match self.0.borrow().menu_buttons.get(index) {
            Some(&[x, y, width, height]) => RowRect {
                x,
                y,
                width,
                height,
            },
            None => RowRect::default(),
        }
    }

    fn blur_search_input(&mut self) -> bool {
        let mut layout = self.0.borrow_mut();
        let focused = layout.search_focused;
        if focused {
            layout.search_focused = false;
            layout.blur = true;
        }
        focused
    }
}

struct TuningField {
    field: Entity<TextField<FormNumber>>,
    /// The field's row, which moves as a live log gains fields.
    index: Rc<Cell<usize>>,
    _subscription: Subscription,
}

pub struct SidebarView {
    hub: Entity<Hub>,
    layout: Rc<RefCell<SidebarLayout>>,
    scroll: ScrollHandle,
    results_scroll: ScrollHandle,
    search: Option<(Entity<TextField<PlainText>>, Subscription)>,
    tuning: HashMap<JsString, TuningField>,
    rendered: Range<usize>,
}

/// The display indices of the rows upstream's `onScreen` test passes at this scroll position
/// (`top >= -height && top <= innerHeight`): the only rows the view lays out. Those whose box
/// lies above `div.side-bar` are laid out under its clip, as in Chromium.
fn shown_rows(side_bar_top: f64, scroll_top: f64, inner_height: f64, rows: usize) -> Range<usize> {
    let origin = side_bar_top + LIST_TOP - scroll_top;
    let first = ((-ROW_HEIGHT - origin) / ROW_HEIGHT).ceil().max(0.0) as usize;
    let last = (((inner_height - origin) / ROW_HEIGHT).floor() + 1.0).max(0.0) as usize;
    first.min(rows)..last.min(rows)
}

/// A field label's `touchstart` listener, over its box.
fn touch_start(layout: Rc<RefCell<SidebarLayout>>, flat: usize) -> impl IntoElement {
    canvas(
        |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
        move |_, hitbox, window, _| {
            window.on_mouse_event(move |event: &TouchDragEvent, phase, window, _| {
                if phase == DispatchPhase::Bubble
                    && event.phase == TouchPhase::Started
                    && hitbox.is_hovered(window)
                {
                    window.prevent_default();
                    layout.borrow_mut().touched = Some(flat);
                }
            });
        },
    )
    .absolute()
    .inset_0()
}

fn now() -> f64 {
    asrs_js::math::trunc(asrs_js::time::Clock::now(&asrs_js::time::SystemClock))
}

impl SidebarView {
    pub fn new(hub: Entity<Hub>, geometry: &ViewGeometry) -> Self {
        SidebarView {
            hub,
            layout: geometry.0.clone(),
            scroll: ScrollHandle::new(),
            results_scroll: ScrollHandle::new(),
            search: None,
            tuning: HashMap::new(),
            rendered: 0..0,
        }
    }

    /// The display indices of the rows the last render laid out.
    pub fn rendered_rows(&self) -> Range<usize> {
        self.rendered.clone()
    }

    /// Runs `f` on the sidebar as one page task.
    fn input<R>(
        &self,
        cx: &mut App,
        f: impl FnOnce(&mut asrs_hub::sidebar::Sidebar, &mut asrs_hub::sidebar::SidebarEnv<'_>) -> R,
    ) -> R {
        self.hub.update(cx, |hub, cx| {
            hub.update(cx, |model: &mut HubModel, _| model.with_sidebar(f))
        })
    }

    fn search_field(
        &mut self,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<TextField<PlainText>> {
        if let Some((field, _)) = &self.search {
            field.update(cx, |field, _| {
                field.set_style(FieldStyle::sidebar_search(appearance))
            });
            return field.clone();
        }
        let field = cx.new(|cx| {
            let mut field = TextField::new(
                PlainText::default(),
                FieldKind::Search,
                FieldStyle::sidebar_search(appearance),
                window,
                cx,
            );
            field.set_placeholder("Search");
            field
        });
        let subscription = cx.subscribe(&field, |view, field, event: &FieldEvent, cx| {
            match event {
                FieldEvent::Input => {
                    let value = JsString::from(field.read(cx).value().as_str());
                    view.input(cx, |sidebar, env| sidebar.search_input(env, value));
                }
                FieldEvent::Enter => {
                    view.input(cx, |sidebar, env| {
                        sidebar.search_key_down(env, "Enter", now())
                    });
                }
                FieldEvent::Focus => {
                    view.layout.borrow_mut().search_focused = true;
                    view.input(cx, |sidebar, _| sidebar.search_focus());
                }
                FieldEvent::Blur => {
                    view.layout.borrow_mut().search_focused = false;
                    view.input(cx, |sidebar, _| sidebar.search_blur());
                }
                FieldEvent::Change => {}
            }
            cx.notify();
        });
        self.search = Some((field.clone(), subscription));
        field
    }

    fn tuning_field(
        &mut self,
        row: &SidebarRow,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<Entity<TextField<FormNumber>>> {
        let key = row.full_key.clone()?;
        let RowValue::Number {
            input_value,
            placeholder,
            ..
        } = &row.value
        else {
            return None;
        };
        let entry = self.tuning.entry(key).or_insert_with(|| {
            let field = cx.new(|cx| {
                TextField::new(
                    FormNumber::default(),
                    FieldKind::Number,
                    FieldStyle::field_value(),
                    window,
                    cx,
                )
            });
            let index = Rc::new(Cell::new(row.index));
            let current = index.clone();
            let subscription = cx.subscribe(&field, move |view, field, event: &FieldEvent, cx| {
                let index = current.get();
                let value = JsString::from(field.read(cx).value().as_str());
                view.input(cx, |sidebar, _| match event {
                    FieldEvent::Input => sidebar.number_input_value(index, value),
                    FieldEvent::Focus => sidebar.number_input_focus(index, true),
                    FieldEvent::Blur => sidebar.number_input_focus(index, false),
                    FieldEvent::Enter => sidebar.number_input_key_down(index, "Enter"),
                    FieldEvent::Change => {}
                });
            });
            TuningField {
                field,
                index,
                _subscription: subscription,
            }
        });
        entry.index.set(row.index);
        let field = entry.field.clone();
        let wanted = input_value.to_string_lossy();
        let placeholder = placeholder
            .as_ref()
            .map(|p| p.to_string_lossy())
            .unwrap_or_default();
        field.update(cx, |field, cx| {
            if field.value() != wanted {
                field.set_value(&wanted, window, cx);
            }
            field.set_placeholder(placeholder);
            field.set_size(50.0, 18.0);
        });
        Some(field)
    }

    /// Applies the scroll writes and blurs the model made since the last render, and reports a
    /// scroll the user made: either way upstream gets a `scroll` event.
    fn sync_scroll(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let (scroll_to, blur, results) = {
            let mut layout = self.layout.borrow_mut();
            (
                layout.scroll_to.take(),
                std::mem::take(&mut layout.blur),
                layout.results_scroll.take(),
            )
        };
        let shown = -f64::from(f32::from(self.scroll.offset().y));
        let scrolled = match scroll_to {
            Some(top) => {
                self.scroll.set_offset(point(px(0.0), px(-(top as f32))));
                top != shown
            }
            None => {
                let mut layout = self.layout.borrow_mut();
                let moved = shown != layout.scroll_top;
                layout.scroll_top = shown;
                moved
            }
        };
        if scrolled {
            let layout = self.layout.clone();
            cx.on_next_frame(window, move |view, _, cx| {
                let mut geometry = ViewGeometry(layout);
                view.input(cx, |sidebar, _| sidebar.sidebar_scroll(&mut geometry));
            });
        }
        if blur && let Some((field, _)) = &self.search {
            field.update(cx, |field, cx| field.blur(window, cx));
        }
        match results {
            Some(ResultsScroll::Top) => self.results_scroll.set_offset(point(px(0.0), px(0.0))),
            Some(ResultsScroll::Item(index)) => self.results_scroll.scroll_to_item(index),
            None => {}
        }
    }

    fn row(
        &mut self,
        row: &SidebarRow,
        index: usize,
        width: f64,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = appearance.theme();
        let dark = appearance == Appearance::Dark;
        let flat = row.index;
        let indent = row.indent;
        let item_width = (width - row.value_width).max(0.0);
        let place = focus::place(("field-item-place", flat), window, cx);
        let mut container = div()
            .track_focus(&place)
            .id(("field-item-container", flat))
            .absolute()
            .left_0()
            .top(px((LIST_TOP + index as f64 * ROW_HEIGHT) as f32))
            .w(px(width as f32))
            .h(px(ROW_HEIGHT as f32));
        if row.highlighted {
            container = container.bg(rgb(if dark { 0xce9421 } else { 0xffff00 }));
        }

        let slot = if !row.has_children {
            2
        } else if row.expanded {
            1
        } else {
            0
        };
        let fill = if dark { "#eee" } else { "#222" };
        let shape = match slot {
            0 => format!(r#"<polygon points="1,0 1,12 11,6" fill="{fill}"/>"#),
            1 => format!(r#"<polygon points="12,1 0,1 6,11" fill="{fill}"/>"#),
            _ => format!(r#"<rect x="1" y="5" width="10" height="2" fill="{fill}"/>"#),
        };
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="12" height="12">{shape}</svg>"#
        );
        let hub = self.hub.clone();
        let icon = div()
            .id(("field-item-icon", flat))
            .absolute()
            .left(px((17.0 + indent) as f32))
            .top(px(9.0))
            .w(px(12.0))
            .h(px(12.0))
            .child(Icon::new(IconSource::Svg(svg.into()), "").fit(IconFit::Natural))
            .on_click(move |_, _, cx| {
                hub.update(cx, |hub, cx| {
                    hub.update(cx, |model, _| {
                        model.with_sidebar(|s, env| s.icon_click(env, flat, slot, now()))
                    })
                });
            });

        let mut label = div()
            .id(("field-item-label", flat))
            .absolute()
            .left(px((35.0 + indent) as f32))
            .top(px(5.0))
            .h(px(20.0))
            .max_w(px((item_width - 45.0 - indent).max(0.0) as f32))
            .overflow_hidden()
            .flex()
            .flex_row();
        if row.label_loading {
            label = label.opacity(0.5);
        }
        let italic = row.full_key.is_some();
        let bold = row.label_bold == Some(true);
        let title = row.title.to_string_lossy();
        let mut title_el = div()
            .relative()
            .child(Text::new(title.clone(), label_style(italic, bold)).color(theme.text));
        if row.known_label {
            let (w, _) = asrs_ui::text::measure(cx, &label_style(italic, bold), &title, 1.0);
            title_el = title_el.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(16.0))
                    .w(px(w as f32))
                    .h(px(1.0))
                    .bg(rgb(theme.text)),
            );
        }
        label = label.child(untitled(title_el));
        if row.full_key.as_ref().is_some_and(|k| !k.is_empty()) && !row.warning_hidden {
            label = label.child(titled(
                row.warning_title.to_string_lossy(),
                div()
                    .id(("field-item-warning", flat))
                    .h(px(INLINE_HEIGHT))
                    .cursor(asrs_ui::gpui::CursorStyle::PointingHand)
                    .child(Text::new(" \u{26A0}", label_style(italic, bold)).color(theme.text))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.input(cx, |s, _| s.warning_click(flat));
                    })),
            ));
        }
        if let Some(type_label) = &row.type_label {
            let opacity = if row.generated { 0.3 / 0.6 } else { 0.3 };
            label = label.child(
                div().opacity(opacity).child(
                    Text::new(type_label.to_string_lossy(), label_style(false, false))
                        .color(theme.text),
                ),
            );
        }
        if row.full_key.is_some() {
            label = label
                .cursor(asrs_ui::gpui::CursorStyle::OpenHand)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, event: &MouseDownEvent, _, cx| {
                        let (x, y) = client(event.position);
                        let top = view.layout.borrow().row_top(index);
                        let left = view.layout.borrow().side_bar[0] + 35.0 + indent;
                        view.input(cx, |s, _| {
                            s.label_mouse_down(flat, x, y, x - left, y - top - 5.0)
                        });
                    }),
                )
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(move |view, event: &MouseUpEvent, _, cx| {
                        let (x, y) = client(event.position);
                        let modifier = event.modifiers.control || event.modifiers.platform;
                        view.input(cx, |s, _| s.label_mouse_up(flat, x, y, modifier));
                    }),
                )
                .child(touch_start(self.layout.clone(), flat));
        }
        label = label.on_mouse_down(
            MouseButton::Right,
            cx.listener(move |view, event: &MouseDownEvent, _, cx| {
                let (x, y) = client(event.position);
                view.input(cx, |s, _| s.label_context_menu(flat, x, y));
            }),
        );

        let mut item = div()
            .absolute()
            .left_0()
            .top_0()
            .w(px(item_width as f32))
            .h(px(ROW_HEIGHT as f32))
            .overflow_hidden()
            .child(icon)
            .child(match &row.label_title {
                Some(title) => titled(title.to_string_lossy(), label).into_any_element(),
                None => label.into_any_element(),
            });
        if row.generated {
            item = item.opacity(0.6);
        }
        container = container.child(item);

        if !row.value_hidden
            && let Some(value) = self.value(row, appearance, window, cx)
        {
            container = container.child(
                div()
                    .absolute()
                    .right(px(12.0))
                    .top_0()
                    .h(px(ROW_HEIGHT as f32))
                    .flex()
                    .items_center()
                    .child(value),
            );
        }
        container.into_any_element()
    }

    fn value(
        &mut self,
        row: &SidebarRow,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let theme = appearance.theme();
        let flat = row.index;
        match &row.value {
            RowValue::None => None,
            RowValue::Text { text } if !text.is_empty() => {
                Some(value_text(text.to_string_lossy(), theme.text, window, cx))
            }
            RowValue::Text { .. } => None,
            RowValue::Number {
                input_hidden: false,
                ..
            } => {
                let field = self.tuning_field(row, window, cx)?;
                Some(
                    titled("Set value", div().w(px(50.0)).h(px(18.0)).child(field))
                        .into_any_element(),
                )
            }
            RowValue::Number { span_text, .. } if !span_text.is_empty() => Some(value_text(
                span_text.to_string_lossy(),
                theme.text,
                window,
                cx,
            )),
            RowValue::Number { .. } => None,
            RowValue::Bool {
                fill,
                line_hidden,
                tunable,
            } => {
                let fill = fill.as_ref()?.to_string_lossy();
                let line = if *line_hidden == Some(true) {
                    String::new()
                } else {
                    r#"<line x1="1.32" y1="1.32" x2="7.68" y2="7.68" stroke-width="2" stroke="white"/>"#
                        .to_owned()
                };
                let svg = format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="9" height="9"><circle cx="4.5" cy="4.5" r="4.5" fill="{fill}"/>{line}</svg>"#
                );
                let size = if *tunable { 9.0 * 1.75 } else { 9.0 };
                let offset = (size - 9.0) / 2.0;
                let mut el = div()
                    .id(("field-value-svg", flat))
                    .relative()
                    .w(px(9.0))
                    .h(px(9.0))
                    .child(
                        div()
                            .absolute()
                            .left(px(-offset))
                            .top(px(-offset))
                            .w(px(size))
                            .h(px(size))
                            .child(Icon::new(IconSource::Svg(svg.into()), "")),
                    );
                if *tunable {
                    el = el.on_click(cx.listener(move |view, _, _, cx| {
                        view.input(cx, |s, env| s.svg_click(env, flat));
                    }));
                }
                Some(el.into_any_element())
            }
        }
    }

    fn search_results(
        &self,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<AnyElement> {
        let hub = self.hub.read(cx).model();
        let sidebar = hub.sidebar();
        if sidebar.search_results_hidden() {
            return None;
        }
        let (top, min_width) = sidebar.search_results_position();
        let items = sidebar.search_items();
        let dark = appearance == Appearance::Dark;
        let theme = appearance.theme();
        let style = results_style();
        let content = items.len() as f64 * RESULTS_ITEM_HEIGHT + 3.0;
        let height = content.min(RESULTS_MAX_HEIGHT);
        // Tab reaches a scroller that holds nothing to focus.
        let place = focus::place("search-results-place", window, cx);
        let mut list = div()
            .id("search-results")
            .track_focus(&place.tab_stop(content > height))
            .w_full()
            .h(px(height as f32))
            .overflow_y_scroll()
            .track_scroll(&self.results_scroll);
        for (index, (key, hovered)) in items.into_iter().enumerate() {
            let background = if hovered {
                Some(if dark { 0x555555 } else { 0xcccccc })
            } else if index % 2 == 1 {
                Some(if dark { 0x272727 } else { 0xf5f5f5 })
            } else {
                None
            };
            let mut item = div()
                .id(("search-results-item", index))
                .h(px(RESULTS_ITEM_HEIGHT as f32))
                .pl(px(8.0))
                .pr(px(20.0))
                .child(Text::new(key.to_string_lossy(), style).color(theme.text))
                .on_mouse_move(cx.listener(move |view, _, _, cx| {
                    view.input(cx, |s, _| s.search_item_mouse_move(index));
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, _, window, cx| {
                        window.prevent_default();
                        view.input(cx, |s, env| s.search_item_activate(env, index, now()));
                    }),
                );
            if let Some(bg) = background {
                item = item.bg(rgb(bg));
            }
            list = list.child(item);
        }
        // Drawn outside the side bar's own section.
        let mut popup = div()
            .tab_group()
            .tab_index(Section::SearchResults as isize)
            .min_w(px(min_width.unwrap_or(0.0) as f32))
            .max_w(px(700.0))
            .rounded(px(6.0))
            .overflow_hidden()
            .bg(rgb(if dark { 0x222222 } else { 0xffffff }))
            .child(list);
        popup = if dark {
            popup.border(px(0.5)).border_color(rgba(0xffffff4d))
        } else {
            popup.shadow(vec![asrs_ui::gpui::BoxShadow {
                color: rgba(0x00000020).into(),
                offset: point(px(0.0), px(0.0)),
                blur_radius: px(1.5),
                spread_radius: px(2.0),
                inset: false,
            }])
        };
        let left = 16.0;
        Some(
            deferred(
                anchored()
                    .position(point(px(left), px(top.unwrap_or(0.0) as f32)))
                    .child(popup),
            )
            .with_priority(1)
            .into_any_element(),
        )
    }
}

fn client(position: asrs_ui::gpui::Point<asrs_ui::gpui::Pixels>) -> (f64, f64) {
    (
        asrs_hub::frame::client_coordinate(f64::from(f32::from(position.x))),
        asrs_hub::frame::client_coordinate(f64::from(f32::from(position.y))),
    )
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let appearance = Appearance::of(window, cx);
        let theme = appearance.theme();
        self.sync_scroll(window, cx);
        let search = self.search_field(appearance, window, cx);

        let hub = self.hub.read(cx).model();
        let sidebar = hub.sidebar();
        let rows = sidebar.displayed_rows();
        let title = sidebar.title_text().to_string_lossy();
        let tuning_shown = !sidebar.tuning_button_hidden();
        let tuning_active = sidebar.tuning_button_active();
        let side_bar = {
            let mut layout = self.layout.borrow_mut();
            layout.displayed = rows.len();
            layout.tuning_shown = tuning_shown;
            layout.scale = window.scale_factor();
            layout.side_bar
        };
        let [_, _, width, _] = side_bar;
        let scroll_top = self.layout.borrow().scroll_top;
        let inner_height = f64::from(f32::from(window.viewport_size().height));
        let range = shown_rows(side_bar[1], scroll_top, inner_height, rows.len());
        self.rendered = range.clone();

        let content_height = LIST_TOP + rows.len() as f64 * ROW_HEIGHT;
        let mut content = div()
            .relative()
            .w(px(width as f32))
            .h(px(content_height as f32))
            .child(
                div()
                    .absolute()
                    .left(px(18.0))
                    .top(px(5.0))
                    .w(px((width - 36.0).max(0.0) as f32))
                    .h(px(20.0))
                    .opacity(0.5)
                    .child(
                        Text::new(title, title_style())
                            .color(theme.text)
                            .probe("side-bar-title")
                            .w_full(),
                    ),
            );

        let search_width = (width - 32.0 - if tuning_shown { 26.0 } else { 0.0 }).max(0.0);
        search.update(cx, |field, _| {
            field.set_size(search_width as f32, SEARCH_HEIGHT as f32)
        });
        let search_icon = Symbol::named("search.svg").expect("search symbol");
        let icon_chain = if appearance == Appearance::Dark {
            chains::INVERT
        } else {
            ""
        };
        content = content
            .child(titled(
                "Search fields",
                div()
                    .id("side-bar-search")
                    .absolute()
                    .left(px(16.0))
                    .top(px(SEARCH_TOP as f32))
                    .w(px(search_width as f32))
                    .h(px(SEARCH_HEIGHT as f32))
                    .capture_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                        let code = match event.keystroke.key.as_str() {
                            "down" => "ArrowDown",
                            "up" => "ArrowUp",
                            _ => return,
                        };
                        view.input(cx, |s, env| s.search_key_down(env, code, now()));
                        cx.stop_propagation();
                    }))
                    .child(search),
            ))
            .child(covering(
                div()
                    .absolute()
                    .left(px(21.0))
                    .top(px((TITLE_HEIGHT + 9.5) as f32))
                    .w(px(20.0))
                    .h(px(20.0))
                    .child(Icon::new(search_icon, icon_chain)),
            ));
        if tuning_shown {
            let tune = Symbol::named("tune.svg").expect("tune symbol");
            let blurred = !self.hub.read(cx).model().is_focused();
            let tint = if tuning_active {
                IconTint::TUNING_ACTIVE
            } else {
                IconTint::BUTTON
            };
            content = content.child(
                div()
                    .absolute()
                    .left(px((width - 37.0) as f32))
                    .top(px(SEARCH_TOP as f32))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|view, event: &MouseDownEvent, _, cx| {
                            let (x, y) = client(event.position);
                            view.input(cx, |s, _| s.tuning_button_context_menu(x, y));
                        }),
                    )
                    .child(
                        IconButton::new("side-bar-tuning-button", tune)
                            .title("Toggle tuning mode")
                            .size(29.0, 29.0)
                            .tint(tint)
                            .blurred(blurred)
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.input(cx, |s, env| s.tuning_button_click(env));
                            })),
                    ),
            );
        }

        // Tab also reaches the inputs that are scrolled out of view.
        let tunable: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| {
                let tuned = |value: &RowValue| {
                    matches!(
                        value,
                        RowValue::Number {
                            input_hidden: false,
                            ..
                        }
                    )
                };
                !row.value_hidden && tuned(&row.value)
            })
            .map(|(index, _)| index)
            .collect();
        let (before, after) = (
            tunable.partition_point(|&index| index < range.start),
            tunable.partition_point(|&index| index < range.end),
        );
        let nearest = [
            (before > 0).then_some(0),
            before.checked_sub(1),
            (after < tunable.len()).then_some(after),
            (after < tunable.len()).then(|| tunable.len() - 1),
        ];
        let nearest = nearest.into_iter().flatten().map(|i| tunable[i]);
        let shown: BTreeSet<usize> = range.chain(nearest).collect();
        for (index, row) in shown.into_iter().map(|index| (index, &rows[index])) {
            let element = self.row(row, index, width, appearance, window, cx);
            content = content.child(element);
        }
        let shown: std::collections::HashSet<JsString> =
            rows.iter().filter_map(|r| r.full_key.clone()).collect();
        self.tuning.retain(|key, _| shown.contains(key));

        let mut root = div()
            .id("side-bar")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(content);
        if let Some(results) = self.search_results(appearance, window, cx) {
            root = root.child(results);
        }
        root
    }
}

/// `div.side-bar`'s box as the frame laid it out, for [`ViewGeometry`].
pub fn set_side_bar(geometry: &ViewGeometry, rect: [f64; 4]) {
    geometry.0.borrow_mut().side_bar = rect;
}

/// Scrolls `div.side-bar` to the focused element at this client y and height.
pub fn scroll_to_focused(geometry: &ViewGeometry, [y, height]: [f64; 2]) {
    let mut layout = geometry.0.borrow_mut();
    let [_, top, _, visible] = layout.side_bar;
    let by = focus::reveal(ScrollAxis::Vertical, visible, [y - top, height]);
    if by != 0.0 {
        let to = layout.scroll_top + by;
        layout.scroll(to);
    }
}

pub fn take_touched_field(geometry: &ViewGeometry) -> Option<usize> {
    geometry.0.borrow_mut().touched.take()
}

pub fn set_menu_buttons(geometry: &ViewGeometry, rects: Vec<[f64; 4]>) {
    geometry.0.borrow_mut().menu_buttons = rects;
}
