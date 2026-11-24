//! A drop-down `<select>` as Chromium 142 draws and drives it. Closed, it is the native theme's
//! menu list (`NativeThemeBase::PaintMenuList`: the text-field frame and a stroked chevron) with
//! the selected option's label in Blink's inner element. Open, it is the popup of
//! [`platform::select_popup`]. Keys follow the platform's `LayoutTheme` ([`SelectKeys`]).

use std::cell::Cell;
use std::ops::Range;
use std::rc::Rc;
use std::time::Instant;

use asrs_canvas::Pixmap;
use gpui_kit::{
    App, Bounds, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, Keystroke, MouseButton, MouseDownEvent, ParentElement, Pixels, Render, Rgba,
    StatefulInteractiveElement, Styled, Window, canvas, div, px,
};

use tiny_skia::{Color, Paint, Rect, Transform};

use crate::input::skia;
use crate::input::style::{
    CONTROL_FONT, DISABLED_BACKGROUND, DISABLED_BORDER, LIGHTEN_LAYER, NATIVE_BORDER,
    NATIVE_RADIUS, device_border, native_stroke,
};
use crate::platform::{self, SelectMenu, SelectMenuItem, SelectMenuItemKind};
use crate::selection;
use crate::text::{self, LineHeight, Overflow, TextStyle, WhiteSpace};

/// One of the select's list items as the popup shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectItem {
    /// `ItemText()`: an optgroup's label, or an option's label, indented by four spaces inside
    /// an optgroup.
    pub label: String,
    pub group: bool,
    /// Not disabled, by its own attribute or its optgroup's.
    pub enabled: bool,
    /// `display: none`, which leaves it out of the popup.
    pub hidden: bool,
}

/// The select's value semantics (`asrs_tabs::form::Select` in the app), indexed by list item:
/// optgroups and options in document order.
pub trait SelectModel: 'static {
    fn items(&self) -> Vec<SelectItem>;
    fn selected_item(&self) -> Option<usize>;
    /// The selected option's label, which the closed select shows.
    fn shown_text(&self) -> String;
    /// `SelectOptionByPopup`: true when the selection changed.
    fn select_by_popup(&mut self, item: usize) -> bool;
    /// `TypeAheadFind` for a printable character typed at `time` ms: true when the selection
    /// changed.
    fn type_ahead(&mut self, char_code: u16, time: f64) -> bool;
    fn type_ahead_active(&self, time: f64) -> bool;
    /// The type-ahead session ends when the select loses focus.
    fn end_type_ahead(&mut self);
    /// The option an arrow or paging key selects in a closed select: true when the selection
    /// changed.
    fn step(&mut self, step: SelectStep) -> bool;
}

/// `NextValidOption(list_index, direction, skip)`, `FirstSelectableOption` and
/// `LastSelectableOption`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectStep {
    Forward(usize),
    Back(usize),
    First,
    Last,
}

/// Which keys open a closed select's popup and which change its selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectKeys {
    /// `LayoutThemeMac`: `PopsMenuByArrowKeys`, so nothing but type-ahead changes the selection.
    Mac,
    /// `LayoutThemeDefault`: `PopsMenuByAltDownUpOrF4Key` and `PopsMenuByReturnKey`.
    Default,
}

impl SelectKeys {
    pub const HOST: SelectKeys = if cfg!(target_os = "macos") {
        SelectKeys::Mac
    } else {
        SelectKeys::Default
    };

    fn current(cx: &App) -> SelectKeys {
        #[cfg(feature = "parity")]
        if let Some(scripted) = cx.try_global::<ScriptedSelectKeys>() {
            return scripted.0;
        }
        let _ = cx;
        SelectKeys::HOST
    }
}

/// The key rules of new selects where a test replays another platform's records.
#[cfg(feature = "parity")]
pub struct ScriptedSelectKeys(pub SelectKeys);

#[cfg(feature = "parity")]
impl gpui_kit::Global for ScriptedSelectKeys {}

/// `SkipForwards` by the page keys.
const PAGE_STEP: usize = 3;

/// The DOM events a select dispatches that upstream can listen for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectEvent {
    Input,
    Change,
    Focus,
    Blur,
}

/// What a key does to a focused, closed select.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectKey {
    OpenPopup,
    Step(SelectStep),
    TypeAhead(u16),
    Ignore,
}

/// `MenuListSelectType::DefaultEventHandler` and `HTMLSelectElement::DefaultEventHandler`: the
/// keydown checks, then the keypress of the key's character.
pub fn select_key(keys: SelectKeys, keystroke: &Keystroke, type_ahead_active: bool) -> SelectKey {
    let m = keystroke.modifiers;
    let key = keystroke.key.as_str();
    match (keys, key) {
        (SelectKeys::Mac, "up" | "down") => return SelectKey::OpenPopup,
        (SelectKeys::Default, "up" | "down") if m.alt => return SelectKey::OpenPopup,
        (_, "f4") if !m.alt && !m.control => return SelectKey::OpenPopup,
        (SelectKeys::Default, _) if !(m.shift || m.control || m.alt || m.platform) => {
            let step = match key {
                "down" | "right" => Some(SelectStep::Forward(1)),
                "up" | "left" => Some(SelectStep::Back(1)),
                "pagedown" => Some(SelectStep::Forward(PAGE_STEP)),
                "pageup" => Some(SelectStep::Back(PAGE_STEP)),
                "home" => Some(SelectStep::First),
                "end" => Some(SelectStep::Last),
                _ => None,
            };
            if let Some(step) = step {
                return SelectKey::Step(step);
            }
        }
        _ => {}
    }
    if keys == SelectKeys::Default && key == "enter" {
        return SelectKey::OpenPopup;
    }
    let Some(ch) = keystroke.key_char.as_deref() else {
        return SelectKey::Ignore;
    };
    let mut units = ch.encode_utf16();
    let (Some(code), None) = (units.next(), units.next()) else {
        return SelectKey::Ignore;
    };
    if code == u16::from(b' ') && !type_ahead_active {
        return SelectKey::OpenPopup;
    }
    if m.control || m.alt || m.platform || !is_printable(code) {
        return SelectKey::Ignore;
    }
    SelectKey::TypeAhead(code)
}

/// `u_isprint`: any general category outside `C` (controls, format characters, surrogates,
/// private use and unassigned code points).
fn is_printable(code: u16) -> bool {
    let category = icu_properties::maps::general_category().get32(u32::from(code));
    !icu_properties::GeneralCategoryGroup::Other.contains(category)
}

/// The popup `GetPopupMenuInfo` builds for `items`, and for each of its rows the list item it
/// stands for (`ToPopupMenuItemIndex`).
pub fn popup_menu(items: &[SelectItem], selected: Option<usize>) -> (SelectMenu, Vec<usize>) {
    let mut rows = Vec::new();
    let mut map = Vec::new();
    for (i, item) in items.iter().enumerate().filter(|(_, item)| !item.hidden) {
        rows.push(SelectMenuItem {
            label: item.label.clone(),
            kind: if item.group {
                SelectMenuItemKind::Group
            } else {
                SelectMenuItemKind::Option
            },
            enabled: item.enabled,
            tool_tip: String::new(),
        });
        map.push(i);
    }
    let selected = selected.and_then(|s| map.iter().position(|&i| i == s));
    let menu = SelectMenu {
        items: rows,
        font_size: f64::from(CONTROL_FONT.size).trunc(),
        selected,
        right_aligned: false,
    };
    (menu, map)
}

/// `GetWebThemeState` for the menu list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SelectState {
    Normal,
    Hovered,
    Disabled,
}

/// Chromium's light control colours (upstream sets no `color-scheme`), as
/// `NativeThemeBase::GetControlColor` resolves them: `0xRRGGBBAA`.
const HOVERED_BORDER: u32 = 0x4f4f4fff;
const FIELD_TEXT: u32 = 0x000000;
/// `light-dark(GrayText, #aaa)` of `select:disabled` in light mode.
const GRAY_TEXT: u32 = 0x808080;
/// `select:disabled { opacity: 0.7 }`.
const DISABLED_OPACITY: f32 = 0.7;
/// `MenuListArrowWidthInDIP()`: the native theme's scrollbar button width.
const ARROW_BOX: f32 = 15.0;

/// Where Blink lays out and paints a menu list's parts, in device px from its border box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectGeometry {
    /// The inner element's border box, which clips the text (`overflow: hidden`).
    pub clip: [f32; 4],
    /// The line box the label sits in.
    pub text: [f32; 4],
    /// The chevron's rect, from the pixel-snapped border box: `[x, y, width, height]`.
    pub arrow: [i32; 4],
}

fn lu(v: f32) -> i64 {
    (f64::from(v) * 64.0).round() as i64
}

fn px_of(v: i64) -> f32 {
    v as f32 / 64.0
}

/// The parts of a `width` x `height` select (border box in device px, `snapped` the border box
/// after pixel snapping) whose font is `line` device px tall (`FontMetrics::Height()`). The
/// inner element takes the content box less `PopupInternalPaddingStart` (4 zoomed px) and
/// `PopupInternalPaddingEnd` (1 zoomed px plus the arrow box) as margins, has 1 zoomed px of
/// padding above and below the line, and is centred by auto margins when it fits.
pub fn geometry(
    width: f32,
    height: f32,
    snapped: (i32, i32),
    line: f32,
    scale: f32,
) -> SelectGeometry {
    let border = lu(device_border(1.0, scale));
    let (w, h) = (lu(width), lu(height));
    let start = lu((4.0 * scale).trunc());
    let end = lu((scale + ARROW_BOX * scale).trunc());
    let pad = lu(scale.trunc());
    let (cw, ch) = (w - 2 * border, h - 2 * border);
    let inner_w = (cw - start - end).max(0);
    let inner_h = lu(line) + 2 * pad;
    let free = ch - inner_h;
    let top = border + if free > 0 { free / 2 } else { 0 };
    let x = border + start;

    let (sw, sh) = snapped;
    let border_px = device_border(1.0, scale).floor() as i32;
    let right = sw - border_px;
    let middle = sh / 2;
    let arrow_box = ARROW_BOX * scale;
    let size = 8.0 * scale;
    let arrow_x = (right as f32 - (arrow_box + size) / 2.0) as i32;
    let aw = size as i32;
    let ah = aw / 2;
    SelectGeometry {
        clip: [x, top, inner_w, inner_h].map(px_of),
        text: [x, top + pad, inner_w, lu(line)].map(px_of),
        arrow: [arrow_x, middle - ah / 2, aw, ah],
    }
}

fn text_style(line: f32, scale: f32) -> TextStyle {
    let mut style = TextStyle::new(CONTROL_FONT);
    style.line_height = LineHeight::Px(line / scale);
    style.white_space = WhiteSpace::Pre;
    style.overflow = Overflow::Clip;
    style
}

/// The frame, chevron and label of a closed select, composited as one layer so the disabled
/// state's opacity applies to the group, as it does in Chromium.
fn draw(
    cx: &mut App,
    (w, h): (u32, u32),
    origin: (f32, f32),
    css: (f32, f32),
    state: SelectState,
    label: &str,
    selected: Option<Rgba>,
    scale: f32,
) -> Option<Pixmap> {
    let (ascent, descent) = text::font_metrics(cx, &TextStyle::new(CONTROL_FONT), scale);
    let line = ascent + descent;
    let g = geometry(
        css.0 * scale,
        css.1 * scale,
        (w as i32, h as i32),
        line,
        scale,
    );

    let stroke = f64::from(native_stroke(scale));
    let radius = f64::from(NATIVE_RADIUS * scale);
    let (border, fill) = match state {
        SelectState::Normal => (NATIVE_BORDER, 0xffffffff),
        SelectState::Hovered => (HOVERED_BORDER, 0xffffffff),
        SelectState::Disabled => (DISABLED_BORDER, DISABLED_BACKGROUND),
    };
    let i = stroke / 2.0;
    let (bw, bh) = (f64::from(w) - stroke, f64::from(h) - stroke);
    let lighten = if state == SelectState::Disabled {
        skia::rect(
            i,
            i,
            bw,
            bh,
            radius,
            &skia::paint_attrs("fill", LIGHTEN_LAYER),
        )
    } else {
        String::new()
    };
    let mut pixmap = skia::svg(w, h, 1.0, &lighten)?;
    skia::fill_aliased_rrect(&mut pixmap, [i, i, bw, bh], radius, fill);
    let mut body = skia::rect(
        i,
        i,
        bw,
        bh,
        radius,
        &format!(
            "fill=\"none\" {} stroke-width=\"{stroke}\"",
            skia::paint_attrs("stroke", border)
        ),
    );
    let color = if state == SelectState::Disabled {
        GRAY_TEXT
    } else {
        FIELD_TEXT
    };
    let [ax, ay, aw, ah] = g.arrow;
    body += &format!(
        "<path d=\"M{ax} {ay} L{} {} L{} {ay}\" fill=\"none\" {} stroke-width=\"2\"/>",
        f64::from(ax) + f64::from(aw) / 2.0,
        ay + ah,
        ax + aw,
        skia::paint_attrs("stroke", (color << 8) | 0xff)
    );
    skia::over(&mut pixmap, &skia::svg(w, h, 1.0, &body)?);

    if !label.is_empty() {
        let [tx, ty, tw, _] = g.text;
        let masks = text::glyph_masks(
            cx,
            &text_style(line, scale),
            label,
            scale,
            tw / scale,
            (origin.0 + tx, origin.1 + ty),
            color,
        );
        let [cx0, cy0, cw, ch] = g.clip;
        let clip_x = ((origin.0 + cx0).round() as i32).max(0)
            ..((origin.0 + cx0 + cw).round() as i32).min(w as i32);
        let clip_y = ((origin.1 + cy0).round() as i32).max(0)
            ..((origin.1 + cy0 + ch).round() as i32).min(h as i32);
        if let Some(selected) = selected {
            let style = text_style(line, scale);
            let width = text::measure(cx, &style, label, scale).0 as f32 * scale;
            let space = text::space_width(cx, &style, scale);
            let edge =
                |v: f32, range: &Range<i32>| (v.round() as i32).clamp(range.start, range.end);
            let left = edge(origin.0 + tx, &clip_x);
            let right = edge(origin.0 + tx + width + space, &clip_x);
            let top = edge(origin.1 + ty, &clip_y);
            let bottom = edge(origin.1 + ty + line, &clip_y);
            let rect = Rect::from_ltrb(left as f32, top as f32, right as f32, bottom as f32);
            if let Some(rect) = rect {
                let mut paint = Paint::default();
                paint.set_color(Color::from_rgba(
                    selected.r, selected.g, selected.b, selected.a,
                )?);
                pixmap.fill_rect(rect, &paint, Transform::identity(), None);
            }
        }
        composite_masks(&mut pixmap, &masks, color, (clip_x, clip_y));
    }
    if state == SelectState::Disabled {
        for c in pixmap.data_mut() {
            *c = (f32::from(*c) * DISABLED_OPACITY).round() as u8;
        }
    }
    Some(pixmap)
}

/// Source-over of each coverage mask in `color` onto the premultiplied pixmap, inside `clip`.
fn composite_masks(
    pixmap: &mut Pixmap,
    masks: &[text::GlyphMask],
    color: u32,
    (clip_x, clip_y): (Range<i32>, Range<i32>),
) {
    let width = pixmap.width() as i32;
    let rgb = [(color >> 16) as u8, (color >> 8) as u8, color as u8];
    let data = pixmap.data_mut();
    for mask in masks {
        for row in 0..mask.height as i32 {
            let y = mask.y + row;
            if !clip_y.contains(&y) {
                continue;
            }
            for col in 0..mask.width as i32 {
                let x = mask.x + col;
                let a = u32::from(mask.alpha[(row * mask.width as i32 + col) as usize]);
                if a == 0 || !clip_x.contains(&x) {
                    continue;
                }
                let p = ((y * width + x) * 4) as usize;
                let inverse = 255 - a;
                for (c, &v) in rgb.iter().enumerate() {
                    let src = (u32::from(v) * a + 127) / 255;
                    data[p + c] = (src + (u32::from(data[p + c]) * inverse + 127) / 255) as u8;
                }
                data[p + 3] = (a + (u32::from(data[p + 3]) * inverse + 127) / 255) as u8;
            }
        }
    }
}

/// Paints a closed select of `css` size at `bounds`, whose unsnapped origin is `subpixel` CSS
/// px from `bounds.origin`.
pub fn paint_select(
    bounds: Bounds<Pixels>,
    css: (f32, f32),
    subpixel: (f32, f32),
    state: SelectState,
    label: &str,
    window: &mut Window,
    cx: &mut App,
) {
    let scale = window.scale_factor();
    let x0 = f32::from(bounds.origin.x) + subpixel.0;
    let y0 = f32::from(bounds.origin.y) + subpixel.1;
    let left = (x0 * scale).round();
    let top = (y0 * scale).round();
    let w = ((x0 + css.0) * scale).round() - left;
    let h = ((y0 + css.1) * scale).round() - top;
    let (w, h) = (w.max(0.0) as u32, h.max(0.0) as u32);
    let origin = (x0 * scale - left, y0 * scale - top);
    let snapped = Bounds {
        origin: gpui_kit::point(px(left / scale), px(top / scale)),
        size: bounds.size,
    };
    let selected = selection::all::color(window, cx);
    let key = (
        ("select", selected.map(u32::from)),
        w,
        h,
        state,
        label.to_owned(),
        origin.0.to_bits(),
        origin.1.to_bits(),
        css.0.to_bits(),
        css.1.to_bits(),
    );
    let label = label.to_owned();
    skia::paint_with_app(
        key,
        snapped,
        |cx| draw(cx, (w, h), origin, css, state, &label, selected, scale),
        window,
        cx,
    );
}

/// A drop-down `<select>` over a [`SelectModel`].
pub struct Select<M: SelectModel> {
    model: M,
    size: (f32, f32),
    subpixel: (f32, f32),
    disabled: bool,
    hovered: bool,
    popup_open: bool,
    keys: SelectKeys,
    focus: FocusHandle,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    epoch: Instant,
}

impl<M: SelectModel> EventEmitter<SelectEvent> for Select<M> {}

impl<M: SelectModel> Focusable for Select<M> {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl<M: SelectModel> Select<M> {
    pub fn new(model: M, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let focus = cx.focus_handle();
        cx.on_focus(&focus, window, |_, _, cx| cx.emit(SelectEvent::Focus))
            .detach();
        cx.on_blur(&focus, window, |this, window, cx| {
            this.model.end_type_ahead();
            // A window that is only inactive, as under a Wayland popup's grab, keeps its focus.
            if this.popup_open && !this.focus.is_focused(window) {
                platform::close_popup_menu(window);
            }
            cx.emit(SelectEvent::Blur);
        })
        .detach();
        Select {
            model,
            size: (0.0, 0.0),
            subpixel: (0.0, 0.0),
            disabled: false,
            hovered: false,
            popup_open: false,
            keys: SelectKeys::current(cx),
            focus,
            bounds: Rc::default(),
            epoch: Instant::now(),
        }
    }

    /// The key rules of another platform, for replaying its records.
    pub fn set_keys(&mut self, keys: SelectKeys) {
        self.keys = keys;
    }

    pub fn model(&self) -> &M {
        &self.model
    }

    /// Changes the model as script would (`value`, `selectedIndex`, options), without events.
    pub fn update_model<R>(
        &mut self,
        f: impl FnOnce(&mut M) -> R,
        cx: &mut Context<'_, Self>,
    ) -> R {
        let r = f(&mut self.model);
        cx.notify();
        r
    }

    /// The border box in CSS px, which the view lays out.
    pub fn set_size(&mut self, width: f32, height: f32) {
        self.size = (width, height);
    }

    /// What GPUI's pixel snapping dropped from the box's position.
    pub fn set_subpixel(&mut self, dx: f32, dy: f32) {
        self.subpixel = (dx, dy);
    }

    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<'_, Self>) {
        self.disabled = disabled;
        cx.notify();
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    pub fn state(&self) -> SelectState {
        if self.disabled {
            SelectState::Disabled
        } else if self.hovered {
            SelectState::Hovered
        } else {
            SelectState::Normal
        }
    }

    fn changed(&mut self, cx: &mut Context<'_, Self>) {
        cx.emit(SelectEvent::Input);
        cx.emit(SelectEvent::Change);
        cx.notify();
    }

    /// A key typed at `time` ms while the select has focus. Returns whether the select handled
    /// it.
    pub fn key(
        &mut self,
        keystroke: &Keystroke,
        time: f64,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        if self.disabled {
            return false;
        }
        match select_key(self.keys, keystroke, self.model.type_ahead_active(time)) {
            SelectKey::Step(step) => {
                if self.model.step(step) {
                    self.changed(cx);
                }
                true
            }
            SelectKey::OpenPopup => {
                self.open_popup(window, cx);
                // The page's `keydown` listeners get Enter, which opens the popup at `keypress`.
                keystroke.key != "enter"
            }
            SelectKey::TypeAhead(code) => {
                if self.model.type_ahead(code, time) {
                    self.changed(cx);
                }
                true
            }
            SelectKey::Ignore => false,
        }
    }

    /// The popup's rows and the list item each stands for, as it would open now.
    pub fn popup_menu(&self) -> (SelectMenu, Vec<usize>) {
        popup_menu(&self.model.items(), self.model.selected_item())
    }

    /// `ExternalPopupMenu::ShowInternal`: the select's border box, enclosed in device pixels
    /// and scaled back to whole CSS px.
    fn popup_bounds(&self, scale: f32) -> [f64; 4] {
        let Some(b) = self.bounds.get() else {
            return [0.0; 4];
        };
        let s = f64::from(scale);
        let x = (f64::from(f32::from(b.origin.x) + self.subpixel.0) * s).floor();
        let y = (f64::from(f32::from(b.origin.y) + self.subpixel.1) * s).floor();
        let r = ((f64::from(f32::from(b.origin.x) + self.subpixel.0 + self.size.0)) * s).ceil();
        let bottom =
            ((f64::from(f32::from(b.origin.y) + self.subpixel.1 + self.size.1)) * s).ceil();
        let (x, y, r, bottom) = (
            (x / s).round(),
            (y / s).round(),
            (r / s).round(),
            (bottom / s).round(),
        );
        [x, y, r - x, bottom - y]
    }

    pub fn open_popup(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.popup_open || self.disabled {
            return;
        }
        let (menu, map) = self.popup_menu();
        if menu.items.is_empty() {
            return;
        }
        self.popup_open = true;
        let bounds = self.popup_bounds(window.scale_factor());
        let this = cx.entity().downgrade();
        platform::select_popup(window, cx, menu, bounds, move |row, cx| {
            let item = row.and_then(|r| map.get(r).copied());
            let _ = this.update(cx, |select, cx| select.popup_closed(item, cx));
        });
    }

    /// `DidAcceptIndices` (`item` chosen) or `DidCancel` (`None`).
    pub fn popup_closed(&mut self, item: Option<usize>, cx: &mut Context<'_, Self>) {
        self.popup_open = false;
        if let Some(item) = item
            && self.model.select_by_popup(item)
        {
            self.changed(cx);
        }
        cx.notify();
    }

    pub fn popup_is_open(&self) -> bool {
        self.popup_open
    }

    /// `select.focus()`, which a disabled select ignores.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if !self.disabled {
            window.focus(&self.focus, cx);
        }
    }

    fn mouse_down(&mut self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        if self.disabled {
            return;
        }
        self.focus(window, cx);
        self.open_popup(window, cx);
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        if window.default_prevented() {
            return;
        }
        let time = self.epoch.elapsed().as_secs_f64() * 1000.0;
        if self.key(&event.keystroke, time, window, cx) {
            cx.stop_propagation();
        }
    }
}

impl<M: SelectModel> Render for Select<M> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (width, height) = self.size;
        let state = self.state();
        let label = self.model.shown_text();
        let subpixel = self.subpixel;
        let size = self.size;
        let bounds = self.bounds.clone();
        let mut root = div()
            .id("select")
            .relative()
            .w(px(width))
            .h(px(height))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.hovered = *hovered;
                cx.notify();
            }))
            .child(
                canvas(
                    move |b, _, _| bounds.set(Some(b)),
                    move |b, _, window, cx| {
                        paint_select(b, size, subpixel, state, &label, window, cx);
                    },
                )
                .absolute()
                .size_full(),
            );
        if !self.disabled {
            root = root
                .track_focus(&self.focus)
                .tab_index(0)
                .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                .on_key_down(cx.listener(Self::key_down));
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::Modifiers;

    use super::*;

    fn key(key: &str, key_char: Option<&str>, m: Modifiers) -> Keystroke {
        Keystroke {
            modifiers: m,
            key: key.to_owned(),
            key_char: key_char.map(str::to_owned),
        }
    }

    fn with(f: impl FnOnce(&mut Modifiers)) -> Modifiers {
        let mut m = Modifiers::default();
        f(&mut m);
        m
    }

    #[test]
    fn mac_keys() {
        let none = Modifiers::default();
        let alt = with(|m| m.alt = true);
        let ctrl = with(|m| m.control = true);
        let cmd = with(|m| m.platform = true);
        let shift = with(|m| m.shift = true);
        let open = SelectKey::OpenPopup;
        let ignore = SelectKey::Ignore;
        for (stroke, active, want) in [
            (key("down", None, none), false, open),
            (key("up", None, none), true, open),
            (key("down", None, alt), false, open),
            (key("up", None, cmd), false, open),
            (key("f4", None, none), false, open),
            (key("f4", None, alt), false, ignore),
            (key("f4", None, ctrl), false, ignore),
            (key("space", Some(" "), none), false, open),
            (key("space", Some(" "), ctrl), false, open),
            (
                key("space", Some(" "), none),
                true,
                SelectKey::TypeAhead(0x20),
            ),
            (key("a", Some("a"), none), false, SelectKey::TypeAhead(0x61)),
            (key("a", Some("A"), shift), true, SelectKey::TypeAhead(0x41)),
            (key("a", Some("a"), ctrl), false, ignore),
            (key("v", Some("\u{221a}"), alt), false, ignore),
            (key("a", None, cmd), false, ignore),
            (key("enter", Some("\r"), none), false, ignore),
            (key("tab", Some("\t"), none), false, ignore),
            (key("escape", None, none), false, ignore),
            (key("home", None, none), false, ignore),
            (key("left", None, none), false, ignore),
            (key("pagedown", None, none), false, ignore),
            (
                key("e", Some("\u{e9}"), none),
                false,
                SelectKey::TypeAhead(0xe9),
            ),
            (key("x", Some("\u{1f600}"), none), false, ignore),
            (key("x", Some("\u{200b}"), none), false, ignore),
            (key("x", Some("\u{f710}"), none), false, ignore),
            (key("x", Some("\u{378}"), none), false, ignore),
            (
                key("x", Some("\u{a0}"), none),
                false,
                SelectKey::TypeAhead(0xa0),
            ),
        ] {
            assert_eq!(
                select_key(SelectKeys::Mac, &stroke, active),
                want,
                "{stroke:?} active {active}"
            );
        }
    }

    #[test]
    fn default_keys() {
        let none = Modifiers::default();
        let alt = with(|m| m.alt = true);
        let ctrl = with(|m| m.control = true);
        let open = SelectKey::OpenPopup;
        for (stroke, want) in [
            (
                key("down", None, none),
                SelectKey::Step(SelectStep::Forward(1)),
            ),
            (
                key("right", None, none),
                SelectKey::Step(SelectStep::Forward(1)),
            ),
            (key("up", None, none), SelectKey::Step(SelectStep::Back(1))),
            (
                key("left", None, none),
                SelectKey::Step(SelectStep::Back(1)),
            ),
            (
                key("pagedown", None, none),
                SelectKey::Step(SelectStep::Forward(3)),
            ),
            (
                key("pageup", None, none),
                SelectKey::Step(SelectStep::Back(3)),
            ),
            (key("home", None, none), SelectKey::Step(SelectStep::First)),
            (key("end", None, none), SelectKey::Step(SelectStep::Last)),
            (key("down", None, alt), open),
            (key("up", None, alt), open),
            (key("f4", None, none), open),
            (key("f4", None, alt), SelectKey::Ignore),
            (key("f4", None, ctrl), SelectKey::Ignore),
            (key("enter", Some("\r"), none), open),
            (key("enter", Some("\r"), ctrl), open),
            (key("down", None, ctrl), SelectKey::Ignore),
            (key("end", None, alt), SelectKey::Ignore),
            (key("right", None, alt), SelectKey::Ignore),
            (key("space", Some(" "), none), open),
            (key("a", Some("a"), none), SelectKey::TypeAhead(0x61)),
            (key("escape", None, none), SelectKey::Ignore),
        ] {
            assert_eq!(
                select_key(SelectKeys::Default, &stroke, false),
                want,
                "{stroke:?}"
            );
        }
    }

    fn item(label: &str, group: bool, enabled: bool, hidden: bool) -> SelectItem {
        SelectItem {
            label: label.to_owned(),
            group,
            enabled,
            hidden,
        }
    }

    #[test]
    fn popup_rows_skip_hidden_items() {
        let items = [
            item("Visible Range", false, true, false),
            item("Live: 30 Seconds", false, true, true),
            item("FRC Fields", true, true, false),
            item("    2025 Field", false, false, false),
            item("    2024 Field", false, true, false),
        ];
        let (menu, map) = popup_menu(&items, Some(4));
        assert_eq!(map, vec![0, 2, 3, 4]);
        assert_eq!(menu.selected, Some(3));
        assert_eq!(menu.font_size, 13.0);
        let rows: Vec<(&str, SelectMenuItemKind, bool)> = menu
            .items
            .iter()
            .map(|r| (r.label.as_str(), r.kind, r.enabled))
            .collect();
        assert_eq!(
            rows,
            vec![
                ("Visible Range", SelectMenuItemKind::Option, true),
                ("FRC Fields", SelectMenuItemKind::Group, true),
                ("    2025 Field", SelectMenuItemKind::Option, false),
                ("    2024 Field", SelectMenuItemKind::Option, true),
            ]
        );
        assert_eq!(popup_menu(&items, Some(1)).0.selected, None);
    }
}
