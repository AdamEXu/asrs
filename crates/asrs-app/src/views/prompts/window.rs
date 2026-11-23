//! The root view of a prompt window: a [`PromptPage`]'s table of labelled controls, its exit and
//! confirm buttons and its help link, as `prompts.css` lays them out.

use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_tabs::form::NumberInput;
use asrs_tabs::prompts::{Control, ControlValue, PromptEvent, PromptPage};
use asrs_ui::button::IconButton;
use asrs_ui::gpui::{
    AnyElement, App, AppContext, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement, Styled,
    Subscription, Window, div, px, rgb,
};
use asrs_ui::input::{
    FieldEvent, FieldKind, FieldModel, FieldStyle, PlainText, Select, SelectEvent, TextField,
};
use asrs_ui::parity::probed;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, Text, snap_remainder};
use asrs_ui::theme::Appearance;

use super::layout::{
    self, BUTTON_BOTTOM, BUTTON_SIZE, CONFIRM_RIGHT, ControlHeights, EXIT_RIGHT, HELP_RIGHT,
    HELP_WIDTH, help_style, label_style,
};
use crate::views::select::FormSelect;

/// A number input's value, through `asrs_tabs::form`.
pub struct NumberField(NumberInput);

impl FieldModel for NumberField {
    fn value(&self) -> String {
        self.0.value().to_string_lossy()
    }

    fn set_value(&mut self, value: &str) {
        self.0.set_value(JsString::from(value));
    }

    fn edit(&mut self, visible: &str) {
        self.0.set_visible_value(&JsString::from(visible));
    }

    fn accepts(&self, c: char) -> bool {
        asrs_ui::input::number_accepts(c)
    }
}

enum Widget {
    Text(Entity<TextField<PlainText>>),
    Number(Entity<TextField<NumberField>>),
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
    help_from_keyboard: bool,
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
            help_from_keyboard: false,
            _subscriptions: subscriptions,
        }
    }

    /// Keys reach the page before the focused control: its `keydown` listener is on the window
    /// and takes Enter whatever has focus, and Tab moves between the controls from anywhere.
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
                    view.act(|page| page.key_down("Enter"), window, cx);
                    if view.commit(window, cx) {
                        cx.stop_propagation();
                    }
                }
                "tab" if !modifiers.control && !modifiers.alt && !modifiers.platform => {
                    view.tab(modifiers.shift, window, cx);
                    cx.stop_propagation();
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
                let field = cx.new(|cx| TextField::new(model, FieldKind::Text, style, window, cx));
                subscriptions.push(Self::on_field(id, &field, window, cx));
                Widget::Text(field)
            }
            ControlValue::Number(value) => {
                let mut input = NumberInput::default();
                input.set_value(value);
                let model = NumberField(input);
                let field =
                    cx.new(|cx| TextField::new(model, FieldKind::Number, style, window, cx));
                subscriptions.push(Self::on_field(id, &field, window, cx));
                Widget::Number(field)
            }
            ControlValue::Select(select) => {
                let select = cx.new(|cx| Select::new(FormSelect::new(select), window, cx));
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
                    if *s.read(cx).model().select() != value {
                        s.update(cx, |s, cx| {
                            s.update_model(|m| *m.select_mut() = value, cx);
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
        let theme = Appearance::of(window, cx).theme();
        let (w, h) = text::measure(cx, &help_style(), "Help", scale);
        let left = width - HELP_RIGHT - HELP_WIDTH;
        let top = height - BUTTON_BOTTOM - BUTTON_SIZE;
        let x = left + HELP_WIDTH / 2.0 - w / 2.0;
        let y = top + BUTTON_SIZE / 2.0 - h / 2.0;
        let focused = self.help.is_focused(window);
        self.help_from_keyboard =
            focused && (self.help_from_keyboard || window.last_input_was_keyboard());
        let mut link = div()
            .id(HELP)
            .track_focus(&self.help)
            .absolute()
            .left(px(x as f32))
            .top(px(y as f32))
            .w(px(w as f32))
            .h(px(h as f32))
            .cursor_pointer()
            .on_click(cx.listener(|view, _, window, cx| {
                view.click(|page| page.click_help(), window, cx);
            }))
            .child(
                Text::new("Help", help_style())
                    .color(theme.link)
                    .subpixel(
                        snap_remainder(x as f32, scale),
                        snap_remainder(y as f32, scale),
                    )
                    .probe("a"),
            );
        if self.help_from_keyboard {
            link = link.child(
                div()
                    .absolute()
                    .top(px(-2.0))
                    .left(px(-2.0))
                    .right(px(-2.0))
                    .bottom(px(-2.0))
                    .border(px(2.0))
                    .border_color(rgb(theme.focus_ring))
                    .rounded(px(5.0)),
            );
        }
        let boxed = div()
            .absolute()
            .left(px(left as f32))
            .top(px(top as f32))
            .w(px(HELP_WIDTH as f32))
            .h(px(BUTTON_SIZE as f32));
        div()
            .child(probed("div.help-div", boxed))
            .child(link)
            .into_any_element()
    }
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
        let label_widths: Vec<f64> = controls
            .iter()
            .map(|c| text::measure(cx, &label_style(), c.label, scale).0)
            .collect();
        let control_heights: Vec<f64> = controls
            .iter()
            .map(|c| match c.value {
                ControlValue::Select(_) => heights.select,
                ControlValue::Text(_) | ControlValue::Number(_) => heights.input,
            })
            .collect();
        let rows = layout::rows(width, &label_widths, &control_heights);

        let mut root = div()
            .id("prompt")
            .track_focus(&self.body)
            .relative()
            .size_full()
            .bg(rgb(theme.background));
        for (i, row) in rows.iter().enumerate() {
            let [lx, ly, lw, _] = row.label;
            let x = lx + lw - label_widths[i];
            let label = Text::new(controls[i].label, label_style())
                .color(theme.text)
                .subpixel(snap_remainder(x as f32, scale), 0.0);
            root = root
                .child(probed("td.label", positioned(row.label)))
                .child(probed("td.input", positioned(row.input)))
                .child(
                    div()
                        .absolute()
                        .left(px(x as f32))
                        .top(px(ly as f32))
                        .child(label),
                );
            let [cx0, cy0, cw, ch] = row.control;
            let size = (cw as f32, ch as f32);
            let subpixel = (
                snap_remainder(cx0 as f32, scale),
                snap_remainder(cy0 as f32, scale),
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
            root = root.child(probed(id, positioned(row.control).child(control)));
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
