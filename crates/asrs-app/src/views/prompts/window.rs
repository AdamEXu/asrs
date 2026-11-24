//! The root view of a prompt window: a [`PromptPage`]'s table of labelled controls, its exit and
//! confirm buttons and its help link, as `prompts.css` lays them out.

use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_tabs::form::NumberInput;
use asrs_tabs::prompts::{Control, ControlValue, PromptEvent, PromptPage, TableRow};
use asrs_ui::button::IconButton;
use asrs_ui::focus::{self, LINK_OUTLINE_OFFSET, TransformedOutline};
use asrs_ui::gpui::{
    AnyElement, App, AppContext, Bounds, ClickEvent, Context, Div, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement, IntoElement, KeyboardButton, ParentElement, Pixels,
    Render, StatefulInteractiveElement, Styled, Subscription, Window, canvas, div, fill, point, px,
    rgb, size,
};
use asrs_ui::input::style::device_border;
use asrs_ui::input::{
    FieldEvent, FieldKind, FieldModel, FieldStyle, PlainText, Select, SelectEvent, TextField,
};
use asrs_ui::parity::probed;
use asrs_ui::platform;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, Text, TextStyle, snap_remainder};
use asrs_ui::theme::Appearance;
use asrs_ui::tooltip::titled;

use super::layout::{
    self, BUTTON_BOTTOM, BUTTON_SIZE, CONFIRM_RIGHT, ControlHeights, EXIT_RIGHT, HELP_RIGHT,
    HELP_WIDTH, Row, RowKind, help_style, label_style, subtitle_style, title_style,
};
use crate::views::number::FormNumber;
use crate::views::select::FormSelect;

enum Widget {
    Text(Entity<TextField<PlainText>>),
    Number(Entity<TextField<FormNumber>>),
    Select(Entity<Select<FormSelect>>),
}

struct Field {
    id: &'static str,
    widget: Widget,
    disabled: bool,
}

impl Field {
    fn set_disabled(&mut self, disabled: bool, cx: &mut App) {
        if self.disabled == disabled {
            return;
        }
        self.disabled = disabled;
        match &self.widget {
            Widget::Text(f) => f.update(cx, |f, cx| f.set_disabled(disabled, cx)),
            Widget::Number(f) => f.update(cx, |f, cx| f.set_disabled(disabled, cx)),
            Widget::Select(s) => s.update(cx, |s, cx| s.set_disabled(disabled, cx)),
        }
    }
}

/// What a prompt's element with focus is called by [`PromptWindow::active`].
pub const EXIT: &str = "exit";
pub const CONFIRM: &str = "confirm";
pub const HELP: &str = "help";

pub struct PromptWindow {
    page: Box<dyn PromptPage>,
    fields: Vec<Field>,
    body: FocusHandle,
    exit: FocusHandle,
    confirm: FocusHandle,
    help: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

/// The page's replies, alerts and errors.
impl EventEmitter<PromptEvent> for PromptWindow {}

impl Focusable for PromptWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.body.clone()
    }
}

impl PromptWindow {
    pub fn new(
        page: Box<dyn PromptPage>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> PromptWindow {
        let mut subscriptions = vec![
            cx.observe_window_activation(window, |view, window, cx| {
                let focused = JsValue::Bool(window.is_window_active());
                view.message(&asrs_js::ops::object([("isFocused", focused)]), window, cx);
            }),
            Self::keys(window, cx),
        ];
        let fields = page
            .controls()
            .into_iter()
            .map(|control| Self::field(control, &mut subscriptions, window, cx))
            .collect();
        let body = cx.focus_handle();
        window.focus(&body, cx);
        PromptWindow {
            page,
            fields,
            body,
            exit: cx.focus_handle(),
            confirm: cx.focus_handle(),
            help: cx.focus_handle().tab_stop(true),
            _subscriptions: subscriptions,
        }
    }

    /// Keys reach the page before the focused control: its `keydown` listener is on the window
    /// and takes the main Enter key whatever has focus, and Tab moves between the controls from anywhere.
    fn keys(window: &Window, cx: &mut Context<'_, Self>) -> Subscription {
        let handle = window.window_handle();
        let this = cx.entity().downgrade();
        cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != handle || platform::menu_takes_keys(window) {
                return;
            }
            let _ = this.update(cx, |view, cx| match event.keystroke.key.as_str() {
                "enter" => {
                    if !platform::numpad_enter(cx) {
                        view.act(|page| page.click_confirm(), window, cx);
                    }
                    if view.commit(window, cx) || view.press(window, cx) {
                        cx.stop_propagation();
                    }
                }
                "tab" => {
                    if let Some(back) = focus::tab_key(&event.keystroke) {
                        view.tab(back, window, cx);
                        cx.stop_propagation();
                    }
                }
                _ => {}
            });
        })
    }

    fn field(
        control: Control,
        subscriptions: &mut Vec<Subscription>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Field {
        let id = control.id;
        let style = FieldStyle::native();
        let widget = match control.value {
            ControlValue::Text(value) => {
                let model = PlainText::new(&value.to_string_lossy());
                let field = cx.new(|cx| {
                    let mut field = TextField::new(model, FieldKind::Text, style, window, cx);
                    field.set_placeholder(control.placeholder);
                    field
                });
                subscriptions.push(Self::on_field(id, &field, window, cx));
                Widget::Text(field)
            }
            ControlValue::Number(value) => {
                let mut input = NumberInput::default();
                input.set_value(value);
                let model = FormNumber(input);
                let field =
                    cx.new(|cx| TextField::new(model, FieldKind::Number, style, window, cx));
                subscriptions.push(Self::on_field(id, &field, window, cx));
                Widget::Number(field)
            }
            ControlValue::Select(select) => {
                let hidden = hidden_items(&select, control.hidden_options);
                let mut model = FormSelect::new(select);
                model.set_hidden(hidden);
                let select = cx.new(|cx| Select::new(model, window, cx));
                subscriptions.push(cx.subscribe_in(
                    &select,
                    window,
                    move |view, select, event: &SelectEvent, window, cx| {
                        if *event == SelectEvent::Change {
                            let index = select.read(cx).model().select().selected_index();
                            if let Ok(index) = usize::try_from(index) {
                                view.act(|page| page.pick(id, index), window, cx);
                            }
                        }
                    },
                ));
                Widget::Select(select)
            }
        };
        let mut field = Field {
            id,
            widget,
            disabled: false,
        };
        field.set_disabled(control.disabled, cx);
        field
    }

    fn on_field<M: FieldModel>(
        id: &'static str,
        field: &Entity<TextField<M>>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Subscription {
        cx.subscribe_in(
            field,
            window,
            move |view, field, event: &FieldEvent, window, cx| match event {
                FieldEvent::Input => {
                    let visible = JsString::from(field.read(cx).shown());
                    view.act(
                        |page| {
                            page.edit(id, &visible);
                            Vec::new()
                        },
                        window,
                        cx,
                    );
                }
                FieldEvent::Change => view.act(|page| page.change(id), window, cx),
                FieldEvent::Enter | FieldEvent::Focus | FieldEvent::Blur => {}
            },
        )
    }

    pub fn page(&self) -> &dyn PromptPage {
        &*self.page
    }

    /// `port2.postMessage(data)` from main.
    pub fn message(&mut self, data: &JsValue, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.act(|page| page.message(data), window, cx);
    }

    fn act(
        &mut self,
        f: impl FnOnce(&mut dyn PromptPage) -> Vec<PromptEvent>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let events = f(&mut *self.page);
        self.sync(window, cx);
        for event in events {
            match event {
                PromptEvent::Select(id) => self.select(id, window, cx),
                event => cx.emit(event),
            }
        }
        cx.notify();
    }

    /// Shows what the page's script assigned to its controls.
    fn sync(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        for (i, control) in self.page.controls().into_iter().enumerate() {
            let field = &self.fields[i];
            match (&field.widget, control.value) {
                (Widget::Text(f), ControlValue::Text(value)) => {
                    assign(f, &value.to_string_lossy(), window, cx);
                }
                (Widget::Number(f), ControlValue::Number(value)) => {
                    assign(f, &value.to_string_lossy(), window, cx);
                }
                (Widget::Select(s), ControlValue::Select(value)) => {
                    let hidden = hidden_items(&value, control.hidden_options);
                    let model = s.read(cx).model();
                    if *model.select() != value || model.hidden() != hidden {
                        s.update(cx, |s, cx| {
                            s.update_model(
                                |m| {
                                    *m.select_mut() = value;
                                    m.set_hidden(hidden);
                                },
                                cx,
                            );
                        });
                    }
                }
                _ => unreachable!("a control keeps its kind"),
            }
            self.fields[i].set_disabled(control.disabled, cx);
        }
    }

    /// `element.select()`, or `focus()` on a select.
    pub fn select(&mut self, id: &str, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.active(window, cx) != Some(id) {
            self.commit(window, cx);
        }
        match self.fields.iter().find(|f| f.id == id).map(|f| &f.widget) {
            Some(Widget::Text(f)) => f.update(cx, |f, cx| f.select(window, cx)),
            Some(Widget::Number(f)) => f.update(cx, |f, cx| f.select(window, cx)),
            Some(Widget::Select(s)) => s.update(cx, |s, cx| s.focus(window, cx)),
            None => {}
        }
    }

    /// The controls that take focus, in document order.
    fn tab_stops(&self) -> Vec<&'static str> {
        let fields = self.fields.iter().filter(|f| !f.disabled).map(|f| f.id);
        let help = self.page.has_help().then_some(HELP);
        fields.chain([EXIT, CONFIRM]).chain(help).collect()
    }

    fn tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.commit(window, cx);
        let stops = self.tab_stops();
        let at = self
            .active(window, cx)
            .and_then(|id| stops.iter().position(|s| *s == id));
        let next = match (at, back) {
            (None, false) => 0,
            (None, true) => stops.len() - 1,
            (Some(i), false) => (i + 1) % stops.len(),
            (Some(i), true) => (i + stops.len() - 1) % stops.len(),
        };
        match stops[next] {
            EXIT => window.focus(&self.exit, cx),
            CONFIRM => window.focus(&self.confirm, cx),
            HELP => window.focus(&self.help, cx),
            id => self.select(id, window, cx),
        }
        cx.notify();
    }

    /// Enter's `keypress` on the focused button or link, which clicks it; false when neither has
    /// focus. GPUI would click at the key's release, after a reply closed the window.
    fn press(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> bool {
        let click: fn(&mut dyn PromptPage) -> Vec<PromptEvent> = match self.active(window, cx) {
            Some(EXIT) => |page| page.click_exit(),
            Some(CONFIRM) => |page| page.click_confirm(),
            Some(HELP) => |page| page.click_help(),
            _ => return false,
        };
        self.click(click, window, cx);
        true
    }

    /// Fires the `change` event the focused text or number field owes; false when no such
    /// field has focus.
    fn commit(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> bool {
        let Some(field) = self
            .active(window, cx)
            .and_then(|id| self.fields.iter().find(|f| f.id == id))
        else {
            return false;
        };
        let id = field.id;
        let changed = match &field.widget {
            Widget::Text(f) => f.update(cx, |f, _| f.take_change()),
            Widget::Number(f) => f.update(cx, |f, _| f.take_change()),
            Widget::Select(_) => return false,
        };
        if changed {
            self.act(|page| page.change(id), window, cx);
        }
        true
    }

    /// The id of `document.activeElement`.
    pub fn active(&self, window: &Window, cx: &App) -> Option<&'static str> {
        let focused = |handle: &FocusHandle| handle.is_focused(window);
        self.fields
            .iter()
            .find(|f| match &f.widget {
                Widget::Text(w) => focused(&w.focus_handle(cx)),
                Widget::Number(w) => focused(&w.focus_handle(cx)),
                Widget::Select(w) => focused(&w.focus_handle(cx)),
            })
            .map(|f| f.id)
            .or_else(|| {
                [
                    (EXIT, &self.exit),
                    (CONFIRM, &self.confirm),
                    (HELP, &self.help),
                ]
                .into_iter()
                .find(|(_, handle)| focused(handle))
                .map(|(id, _)| id)
            })
    }

    /// The select with this id, whose native popup a test cannot use.
    pub fn select_widget(&self, id: &str) -> Option<&Entity<Select<FormSelect>>> {
        self.fields
            .iter()
            .find(|f| f.id == id)
            .and_then(|f| match &f.widget {
                Widget::Select(s) => Some(s),
                _ => None,
            })
    }

    /// Whether the control with this id takes no input.
    pub fn disabled(&self, id: &str, cx: &App) -> bool {
        self.fields
            .iter()
            .find(|f| f.id == id)
            .is_some_and(|f| match &f.widget {
                Widget::Select(s) => s.read(cx).is_disabled(),
                Widget::Text(_) | Widget::Number(_) => f.disabled,
            })
    }

    /// What the control with this id shows, as the page's script would read it.
    pub fn shown(&self, id: &str, cx: &App) -> Option<ControlValue> {
        let field = self.fields.iter().find(|f| f.id == id)?;
        Some(match &field.widget {
            Widget::Text(f) => ControlValue::Text(JsString::from(f.read(cx).value().as_str())),
            Widget::Number(f) => ControlValue::Number(JsString::from(f.read(cx).value().as_str())),
            Widget::Select(s) => ControlValue::Select(s.read(cx).model().select().clone()),
        })
    }

    /// A click, which takes the focus and so fires a pending `change` before the page sees it.
    fn click(
        &mut self,
        f: impl FnOnce(&mut dyn PromptPage) -> Vec<PromptEvent>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        for i in 0..self.fields.len() {
            let changed = match &self.fields[i].widget {
                Widget::Text(f) => f.update(cx, |f, _| f.take_change()),
                Widget::Number(f) => f.update(cx, |f, _| f.take_change()),
                Widget::Select(_) => false,
            };
            if changed {
                let id = self.fields[i].id;
                self.act(|page| page.change(id), window, cx);
            }
        }
        self.act(f, window, cx);
    }

    fn button(
        &self,
        id: &'static str,
        icon: &str,
        focus: &FocusHandle,
        right: f64,
        f: fn(&mut dyn PromptPage) -> Vec<PromptEvent>,
        cx: &mut Context<'_, Self>,
    ) -> Div {
        let button = IconButton::new(id, Symbol::named(icon).expect("prompt icon"))
            .size(BUTTON_SIZE as f32, BUTTON_SIZE as f32)
            .title(match id {
                EXIT => "Cancel",
                _ if self.page.has_help() => "Export",
                _ => "Confirm",
            })
            .blurred(self.page.blurred())
            .track_focus(focus)
            .on_click(cx.listener(move |view, _, window, cx| view.click(f, window, cx)));
        div()
            .absolute()
            .right(px(right as f32))
            .bottom(px(BUTTON_BOTTOM as f32))
            .child(probed(format!("button#{id}"), button))
    }

    fn help_link(
        &mut self,
        width: f64,
        height: f64,
        scale: f32,
        window: &Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let appearance = Appearance::of(window, cx);
        let theme = appearance.theme();
        let (w, h) = text::measure(cx, &help_style(), "Help", scale);
        let left = width - HELP_RIGHT - HELP_WIDTH;
        let top = height - BUTTON_BOTTOM - BUTTON_SIZE;
        let x = left + HELP_WIDTH / 2.0 - w / 2.0;
        let y = top + BUTTON_SIZE / 2.0 - h / 2.0;
        let mut link = div()
            .id(HELP)
            .track_focus(&self.help)
            .absolute()
            .left(px(x as f32))
            .top(px(y as f32))
            .w(px(w as f32))
            .h(px(h as f32))
            .cursor_pointer()
            .on_click(cx.listener(|view, event: &ClickEvent, window, cx| {
                // Space clicks a button and scrolls from a link.
                let space =
                    matches!(event, ClickEvent::Keyboard(e) if e.button == KeyboardButton::Space);
                if !space {
                    view.click(|page| page.click_help(), window, cx);
                }
            }))
            .child(
                Text::new("Help", help_style())
                    .color(theme.link)
                    .underline()
                    .subpixel(
                        snap_remainder(x as f32, scale),
                        snap_remainder(y as f32, scale),
                    )
                    .probe("a"),
            );
        if focus::focus_visible(&self.help, window, cx) {
            let outline = TransformedOutline {
                size: [w, h],
                offset: LINK_OUTLINE_OFFSET,
                subpixel: [x, y].map(|v| f64::from(snap_remainder(v as f32, scale))),
                translate: [-w / 2.0, -h / 2.0],
                appearance,
            };
            link = link.child(outline.element());
        }
        let boxed = div()
            .absolute()
            .left(px(left as f32))
            .top(px(top as f32))
            .w(px(HELP_WIDTH as f32))
            .h(px(BUTTON_SIZE as f32));
        div()
            .child(probed("div.help-div", boxed))
            .child(titled("Open export documentation", link))
            .into_any_element()
    }
}

/// The `title` of the control with this id, in whichever page has it.
fn control_title(id: &str) -> &'static str {
    match id {
        "max" => "Maximum value",
        "min" => "Minimum value",
        "fov" => "Set camera field of view (degrees)",
        "name" => "Enter tab name",
        "unitType" => "Select unit dimension type",
        "fromUnit" => "Select unit to convert from",
        "toUnit" => "Select unit to convert to",
        "extraFactor" => "Enter custom scale factor (multiplier)",
        "theme" => "Select the application theme",
        "coordinateSystem" => "Select the 3D coordinate system origin and orientation",
        "timestamps" => "Select how timestamps are displayed",
        "tbaApiKey" => "Enter your API key for The Blue Alliance",
        "robotAddress" => "Enter the IP address or hostname of the robot",
        "remotePath" => "Enter the path to log files on the robot",
        "liveSubscribeMode" => "Select live data subscription mode",
        "liveDiscard" => "Select how long to retain live data before discarding",
        "publishFilter" => "Enter prefixes for NetworkTables publishing, separated by commas",
        "field3dModeAc" => "Select the default 3D rendering mode",
        "field3dModeBattery" => "Select the 3D rendering mode when running on battery",
        "field3dAntialiasing" => "Toggle 3D antialiasing",
        "format" => "Select export format",
        "samplingMode" => "Select timestamp sampling mode",
        "samplingPeriod" => "Enter sampling period in milliseconds",
        "prefixes" => "Enter field prefixes to export, separated by commas (optional)",
        "includeGenerated" => "Select whether to include generated fields in the export",
        other => unreachable!("no control {other}"),
    }
}

/// [`FormSelect::set_hidden`]'s list for a select whose items are all options.
fn hidden_items(select: &asrs_tabs::form::Select, options: &[usize]) -> Vec<bool> {
    (0..select.length()).map(|i| options.contains(&i)).collect()
}

fn assign<M: FieldModel>(
    field: &Entity<TextField<M>>,
    value: &str,
    window: &mut Window,
    cx: &mut Context<'_, PromptWindow>,
) {
    if field.read(cx).value() != value {
        field.update(cx, |field, cx| field.set_value(value, window, cx));
    }
}

fn positioned([x, y, w, h]: [f64; 4]) -> Div {
    div()
        .absolute()
        .left(px(x as f32))
        .top(px(y as f32))
        .w(px(w as f32))
        .h(px(h as f32))
}

/// An `hr`'s `1px inset gray` border, in the colours Blink paints it and with its mitred corners,
/// whose diagonal pixels are half of each side's.
fn rule_border() -> impl IntoElement {
    const DARK: u32 = 0x9a9a9a;
    const CORNER: u32 = 0xc4c4c4;
    const LIGHT: u32 = 0xeeeeee;
    canvas(
        |_, _, _| {},
        |bounds, _, window, _| {
            let scale = window.scale_factor();
            let device = |v: Pixels| (f32::from(v) * scale).round();
            let (x, y) = (device(bounds.origin.x), device(bounds.origin.y));
            let width = device(bounds.right()) - x;
            let border = device_border(1.0, scale);
            let rows = (2.0 * border) as usize;
            for row in 0..rows {
                let at = row as f32;
                let corner = if at < border {
                    width - 1.0 - at
                } else {
                    2.0 * border - 1.0 - at
                };
                for (from, to, color) in [
                    (0.0, corner, DARK),
                    (corner, corner + 1.0, CORNER),
                    (corner + 1.0, width, LIGHT),
                ] {
                    if to > from {
                        let origin = point(px((x + from) / scale), px((y + at) / scale));
                        let extent = size(px((to - from) / scale), px(1.0 / scale));
                        window.paint_quad(fill(Bounds::new(origin, extent), rgb(color)));
                    }
                }
            }
        },
    )
    .size_full()
}

/// A heading's text in the middle of `bounds`, whose lines start at its top.
fn centred(
    bounds: [f64; 4],
    text: &'static str,
    style: TextStyle,
    window: &Window,
    cx: &mut App,
) -> Div {
    let [x, y, w, _] = bounds;
    let scale = window.scale_factor();
    let x = x + (w - text::measure(cx, &style, text, scale).0) / 2.0;
    let theme = Appearance::of(window, cx).theme();
    div().absolute().left(px(x as f32)).top(px(y as f32)).child(
        Text::new(text, style)
            .color(theme.text)
            .subpixel(snap_remainder(x as f32, scale), 0.0),
    )
}

impl PromptWindow {
    fn control(
        &self,
        i: usize,
        bounds: [f64; 4],
        scale: f32,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let [x, y, w, h] = bounds;
        let size = (w as f32, h as f32);
        let subpixel = (
            snap_remainder(x as f32, scale),
            snap_remainder(y as f32, scale),
        );
        let (id, control) = match &self.fields[i].widget {
            Widget::Text(f) => ("input", sized(f, size, subpixel, cx)),
            Widget::Number(f) => ("input", sized(f, size, subpixel, cx)),
            Widget::Select(s) => {
                s.update(cx, |s, _| {
                    s.set_size(size.0, size.1);
                    s.set_subpixel(subpixel.0, subpixel.1);
                });
                ("select", s.clone().into_any_element())
            }
        };
        let title = control_title(self.fields[i].id);
        probed(id, titled(title, positioned(bounds).child(control))).into_any_element()
    }
}

impl Render for PromptWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        let theme = Appearance::of(window, cx).theme();
        let viewport = window.viewport_size();
        let (width, height) = (
            f64::from(f32::from(viewport.width)),
            f64::from(f32::from(viewport.height)),
        );
        let heights = ControlHeights::measure(cx, scale);
        let controls = self.page.controls();
        let table = self.page.rows();
        let label_widths: Vec<f64> = controls
            .iter()
            .map(|c| text::measure(cx, &label_style(), c.label, scale).0)
            .collect();
        let mut unplaced = controls.iter();
        let kinds: Vec<RowKind> = table
            .iter()
            .map(|row| match row {
                TableRow::Control => {
                    let control = unplaced.next().expect("a control per control row");
                    RowKind::Control(match control.value {
                        ControlValue::Select(_) => heights.select,
                        ControlValue::Text(_) | ControlValue::Number(_) => heights.input,
                    })
                }
                TableRow::Title(_) => RowKind::Title,
                TableRow::Subtitle(_) => RowKind::Subtitle,
            })
            .collect();

        let mut root = div()
            .id("prompt")
            .track_focus(&self.body)
            .relative()
            .size_full()
            .bg(rgb(theme.background));
        let mut i = 0;
        for (row, content) in layout::rows(width, &label_widths, &kinds)
            .into_iter()
            .zip(table)
        {
            match (row, content) {
                (
                    Row::Control {
                        label,
                        input,
                        control,
                    },
                    TableRow::Control,
                ) => {
                    let [lx, ly, lw, _] = label;
                    let x = lx + lw - label_widths[i];
                    let text = Text::new(controls[i].label, label_style())
                        .color(theme.text)
                        .subpixel(snap_remainder(x as f32, scale), 0.0);
                    root = root
                        .child(probed("td.label", positioned(label)))
                        .child(probed("td.input", positioned(input)))
                        .child(
                            div()
                                .absolute()
                                .left(px(x as f32))
                                .top(px(ly as f32))
                                .child(text),
                        )
                        .child(self.control(i, control, scale, cx));
                    i += 1;
                }
                (Row::Title { cell, rule, line }, TableRow::Title(text)) => {
                    root = root
                        .child(probed("td.title", positioned(cell)))
                        .child(probed("hr", positioned(rule).child(rule_border())))
                        .child(centred(line, text, title_style(), window, cx));
                }
                (Row::Subtitle { cell }, TableRow::Subtitle(text)) => {
                    root = root
                        .child(probed("td.subtitle", positioned(cell)))
                        .child(centred(cell, text, subtitle_style(), window, cx));
                }
                (row, content) => unreachable!("{row:?} for {content:?}"),
            }
        }
        let (exit, confirm) = (self.exit.clone(), self.confirm.clone());
        let icon = self.page.confirm_icon();
        root = root
            .child(self.button(EXIT, "close.svg", &exit, EXIT_RIGHT, |p| p.click_exit(), cx))
            .child(self.button(
                CONFIRM,
                icon,
                &confirm,
                CONFIRM_RIGHT,
                |p| p.click_confirm(),
                cx,
            ));
        if self.page.has_help() {
            root = root.child(self.help_link(width, height, scale, window, cx));
        }
        root
    }
}

fn sized<M: FieldModel>(
    field: &Entity<TextField<M>>,
    size: (f32, f32),
    subpixel: (f32, f32),
    cx: &mut Context<'_, PromptWindow>,
) -> AnyElement {
    field.update(cx, |field, _| {
        field.set_size(size.0, size.1);
        field.set_subpixel(subpixel.0, subpixel.1);
    });
    field.clone().into_any_element()
}
