//! Source lists (`div.source-list` under `www/hub.css`): the DOM side of `src/hub/SourceList.ts`.
//! Everything a list shows comes from `asrs_model::source_list::SourceList`; its elements sit
//! where [`ListLayout`] puts them, and [`ListsGeometry`] answers the model's layout reads from
//! the same layout.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use asrs_hub::frame::client_coordinate;
use asrs_hub::hub::HubModel;
use asrs_js::math;
use asrs_model::scroll_sensor::assign_scroll_offset;
use asrs_model::source_list::layout::{ListLayout, ListRoot, controller_roots};
use asrs_model::source_list::{
    ItemView, Length, ListButton, ListInput, ListPointer, Rect, SourceList, SourceListDom,
};
use asrs_ui::button::IconButton;
use asrs_ui::focus::{self, ScrollAxis};
use asrs_ui::gpui::{
    AnyElement, App, Bounds, BoxShadow, ClickEvent, Context, Div, FocusHandle, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseUpEvent, ParentElement,
    Pixels, Point, ScrollHandle, SharedString, Stateful, StatefulInteractiveElement, Styled,
    Window, div, point, px, rgb, rgba,
};
use asrs_ui::icon::{Icon, IconFit, IconSource};
use asrs_ui::symbols::Symbol;
use asrs_ui::text::{self, Measurer, Text, TextStyle, snap_remainder};
use asrs_ui::theme::{Appearance, FONT_FAMILY, IconTint, chains};
use asrs_ui::tooltip::{titled_rtl, untitled};

use super::frame::{FrameHost, HubFrame};

const TITLE_HEIGHT: f64 = 30.0;
const BUTTON: f32 = 25.0;
const NAME_LEFT: f64 = 30.0;
const CHILD_INDENT: f64 = 25.0;
const VALUE_COLOR: u32 = 0x777777;

fn font(css: &str) -> String {
    format!("{css} {}", FONT_FAMILY.join(", "))
}

fn style(declarations: &[(&str, &str)]) -> TextStyle {
    TextStyle::from_css(declarations.iter().copied()).expect("source list text style")
}

fn title_style(align: &str) -> TextStyle {
    style(&[
        ("font", &font("bold 16px")),
        ("line-height", "30px"),
        ("text-align", align),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ])
}

/// Where `div.title`'s centred line starts in a box `available` wide: Blink halves the free
/// space in `LayoutUnit`s, so the offset is truncated to 1/64 px rather than snapped with the box.
pub(super) fn centred_offset(available: f64, text_width: f64) -> Option<f64> {
    let free = math::floor(available * 64.0) - math::ceil(text_width * 64.0);
    (free >= 0.0).then(|| math::floor(free / 2.0) / 64.0)
}

pub fn type_name_style() -> TextStyle {
    style(&[
        ("font", &font("bold 14px")),
        ("line-height", "30px"),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ])
}

fn key_style() -> TextStyle {
    style(&[
        ("font", &font("14px")),
        ("line-height", "30px"),
        ("text-align", "right"),
        ("direction", "rtl"),
        ("unicode-bidi", "plaintext"),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ])
}

fn value_style() -> TextStyle {
    style(&[
        ("font", &font("14px")),
        ("line-height", "15px"),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ])
}

/// `clientWidth` of `updateItem`'s detached `div.type-name` in a `div.source-list` on a body
/// `body_width` wide: its shrink-to-fit width, capped by `max-width: calc(100% - 85px)` of the
/// list's `width: 50%`, snapped at its whole-pixel `left: 30px`.
pub fn type_name_client_width(
    measurer: &mut Measurer,
    text: &str,
    body_width: f64,
    scale: f32,
) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let (width, _) = measurer.measure(&type_name_style(), text, scale);
    let max = math::trunc(body_width * 0.5 * 64.0) / 64.0 - 85.0;
    math::floor(math::min2(width, math::max2(max, 0.0)) + 0.5)
}

/// What the frame last laid out: each displayed list's layout by UUID, and each list's scroll
/// position.
pub struct ListsLayout {
    lists: HashMap<String, ListLayout>,
    scroll: HashMap<String, ScrollHandle>,
    /// `div.list`'s place in the focus order.
    places: HashMap<String, FocusHandle>,
    measurer: Measurer,
    body_width: f64,
    scale: f32,
}

impl Default for ListsLayout {
    fn default() -> Self {
        ListsLayout {
            lists: HashMap::new(),
            scroll: HashMap::new(),
            places: HashMap::new(),
            measurer: Measurer::default(),
            body_width: 0.0,
            scale: 1.0,
        }
    }
}

/// Every list's layout reads; a list the frame did not lay out reads as undisplayed.
#[derive(Clone, Default)]
pub struct ListsGeometry(Rc<RefCell<ListsLayout>>);

impl ListsGeometry {
    fn layout(&self, list: &str) -> ListLayout {
        self.0.borrow().lists.get(list).cloned().unwrap_or_default()
    }

    fn list_at(&self, x: f64, y: f64) -> Option<String> {
        let layout = self.0.borrow();
        layout.lists.iter().find_map(|(uuid, l)| {
            let r = l.list_rect();
            (x >= r.left() && x < r.right() && y >= r.top() && y < r.bottom()).then(|| uuid.clone())
        })
    }
}

impl SourceListDom for ListsGeometry {
    fn root_rect(&mut self, list: &str) -> Rect {
        self.layout(list).root.rect
    }
    fn list_rect(&mut self, list: &str) -> Rect {
        self.layout(list).list_rect()
    }
    fn item_rect(&mut self, list: &str, index: usize) -> Rect {
        self.layout(list).item_rect(index)
    }
    fn item_client_width(&mut self, list: &str, index: usize) -> f64 {
        self.layout(list).item_client_width(index)
    }
    fn button_rect(&mut self, list: &str, button: ListButton) -> Rect {
        self.layout(list).button_rect(button)
    }
    fn type_name_width(&mut self, text: &str) -> f64 {
        let layout = &mut *self.0.borrow_mut();
        type_name_client_width(&mut layout.measurer, text, layout.body_width, layout.scale)
    }
}

fn displayed_lists(hub: &HubModel, controls: Rect) -> Vec<(&SourceList, ListRoot)> {
    let Some(tab) = hub.tabs().tabs().find(|t| !t.content_hidden()) else {
        return Vec::new();
    };
    let Some(tab_type) = tab.tab_type() else {
        return Vec::new();
    };
    tab.controller()
        .source_lists()
        .into_iter()
        .zip(controller_roots(tab_type, controls))
        .collect()
}

fn find_list<'a>(hub: &'a HubModel, uuid: &str) -> Option<&'a SourceList> {
    hub.tabs()
        .tabs()
        .flat_map(|t| t.controller().source_lists())
        .find(|l| l.uuid() == uuid)
}

fn rect(r: [f64; 4]) -> Rect {
    Rect {
        x: r[0],
        y: r[1],
        width: r[2],
        height: r[3],
    }
}

fn length(l: Length, of: f64) -> f32 {
    match l {
        Length::Px(v) => v as f32,
        Length::Percent(v) => (of * v / 100.0) as f32,
    }
}

/// A box `rel` CSS px right of a parent whose exact CSS position is `parent` and whose GPUI box
/// sits there snapped to the device pixel grid: the offset to author, which GPUI keeps whole,
/// and the fraction of the exact position that snapping drops.
pub(super) fn place_in(parent: f64, rel: f64, scale: f64) -> (f32, f64) {
    let snap = |v: f64| math::floor(v * scale + 0.5) / scale;
    let exact = parent + rel;
    ((snap(exact) - snap(parent)) as f32, exact - snap(exact))
}

/// One of the list's 25 px buttons whose CSS box is `layout` from its parent's exact position
/// `parent` and painted `translate` from there: `top: 15px` with `translate(±50%, -50%)`.
fn translated_button(
    button: IconButton,
    parent: f64,
    layout_x: f64,
    translate_x: f64,
    scale: f64,
) -> Div {
    let half = f64::from(BUTTON) / 2.0;
    let (x, dx) = place_in(parent, layout_x + translate_x, scale);
    let (y, dy) = place_in(0.0, 15.0 - half, scale);
    div().absolute().left(px(x)).top(px(y)).child(
        button
            .size(BUTTON, BUTTON)
            .translate(translate_x, -half)
            .subpixel(dx, dy),
    )
}

fn hsla(color: u32) -> Hsla {
    rgba(color).into()
}

/// The `type-color` SVG of a type icon: upstream sets the `<object>`'s `svg` element's `color`,
/// which its paths take through `fill="currentColor"`.
fn type_icon(symbol_path: &str, color: &str) -> Option<IconSource> {
    let name = symbol_path.strip_prefix("symbols/")?;
    let svg = std::str::from_utf8(Symbol::named(name)?.svg()).ok()?;
    Some(IconSource::Svg(svg.replace("currentColor", color).into()))
}

const NO_FILTER: IconTint = IconTint {
    normal: ["", ""],
    blurred: ["", ""],
    disabled: ["", ""],
};

const OBJECT_FIT: IconFit = IconFit::Centered {
    max: 1.0,
    scale: 0.75,
};

fn fade<E: Styled>(element: E, hidden: bool) -> E {
    if hidden {
        element.opacity(0.4)
    } else {
        element
    }
}

/// What the view draws of one list, read before the frame's listeners borrow it mutably.
struct Shown {
    uuid: String,
    title: String,
    edit_button: bool,
    /// The `blurred` classes of the list's own buttons and of its items'.
    blurred: [bool; 2],
    items: Vec<ItemView>,
    hand_icon: bool,
    highlight: asrs_model::source_list::DragHighlight,
}

impl Shown {
    fn of(list: &SourceList, hub: &HubModel) -> Shown {
        Shown {
            blurred: [hub.list_buttons_blurred(list.uuid()), !hub.is_focused()],
            uuid: list.uuid().to_owned(),
            title: list.title().to_owned(),
            edit_button: list.has_edit_button(),
            items: list.items().cloned().collect(),
            hand_icon: list.hand_icon_shown(),
            highlight: list.drag_highlight(),
        }
    }
}

type Dispatch = Rc<dyn Fn(ListPointer, &mut App)>;

fn on(
    dispatch: &Option<Dispatch>,
    event: ListPointer,
) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let dispatch = dispatch.clone();
    move |_, _, cx| {
        if let Some(d) = &dispatch {
            d(event, cx);
        }
    }
}

/// `img.hand-icon`: `width: 60px` and an auto height, so the SVG scales from its 24 px
/// intrinsic size by its aspect ratio.
fn hand_icon() -> IconSource {
    ratio_symbol("trackpadinput.svg")
}

/// A symbol without its 24 px intrinsic size, so an `<img>` sized by CSS draws it across its
/// width at its aspect ratio.
pub(super) fn ratio_symbol(name: &str) -> IconSource {
    let svg = std::str::from_utf8(symbol(name).svg()).unwrap_or_default();
    IconSource::Svg(
        svg.replacen(r#" height="24px""#, "", 1)
            .replacen(r#" width="24px""#, "", 1)
            .into(),
    )
}

fn symbol(name: &str) -> Symbol {
    Symbol::named(name).unwrap_or_else(|| panic!("bundled symbol {name}"))
}

#[derive(Clone, Copy)]
struct ItemPlace {
    width: f64,
    borders: (f64, f64),
    origin_x: f64,
    scale: f32,
    blurred: bool,
}

/// Where Tab goes on from after a press on a list item: its element, its text and its preview.
struct ItemPlaces {
    item: FocusHandle,
    text: FocusHandle,
    value: FocusHandle,
}

impl ItemPlaces {
    fn new(uuid: &str, index: usize, window: &mut Window, cx: &mut App) -> ItemPlaces {
        let mut place = |part: &str| {
            let id = SharedString::from(format!("sl-{part}-{uuid}-{index}"));
            focus::place(id, window, cx)
        };
        ItemPlaces {
            item: place("item"),
            text: place("text"),
            value: place("value"),
        }
    }
}

/// An element of a list item, which the drag copy's has no place for.
trait Placed: InteractiveElement {
    fn at_place(
        self,
        places: Option<&ItemPlaces>,
        place: impl FnOnce(&ItemPlaces) -> FocusHandle,
    ) -> Self {
        match places {
            Some(places) => self.track_focus(&place(places)),
            None => self,
        }
    }
}

impl<E: InteractiveElement> Placed for E {}

/// One `div.item` at `index`, `width` wide: the list's, whose buttons `dispatch`, or the drag
/// copy's. `borders` are its top and bottom border widths.
fn item_body(
    item: &ItemView,
    index: usize,
    place: ItemPlace,
    appearance: Appearance,
    dispatch: &Option<Dispatch>,
    places: Option<&ItemPlaces>,
) -> Stateful<Div> {
    let ItemPlace {
        width,
        borders,
        origin_x,
        scale,
        blurred,
    } = place;
    let text_dx = |x: f64| snap_remainder((origin_x + x) as f32, scale);
    let theme = appearance.theme();
    let dark = appearance == Appearance::Dark;
    let height = if item.preview_shown == Some(true) {
        50.0
    } else {
        30.0
    };
    let border_color = if dark { 0x333333 } else { 0xeeeeee };
    let mut body = div()
        .id(("sl-item", index))
        .relative()
        .flex_none()
        .w(px(width as f32))
        .h(px(height as f32))
        .border_color(rgb(border_color));
    if let Some(places) = places {
        body = focus::section(body, index as isize + 1, &places.item);
    }
    if borders.0 > 0.0 {
        body = body.border_t_1();
    }
    if borders.1 > 0.0 {
        body = body.border_b_1();
    }
    if item.parent_highlight {
        body = body.shadow(vec![BoxShadow {
            color: hsla(0x00ff00bb),
            offset: point(px(-0.5), px(0.0)),
            blur_radius: px(1.5),
            spread_radius: px(2.0),
            inset: true,
        }]);
    }
    let indent = if item.child { CHILD_INDENT } else { 0.0 };
    let hidden = item.rendered.as_ref().is_some_and(|r| !r.visible);

    if let Some(icon) = item
        .rendered
        .as_ref()
        .and_then(|r| type_icon(&r.symbol_path, &r.color))
    {
        body = body.child(fade(
            translated_button(
                IconButton::new(("sl-type", index), icon)
                    .title("Edit field configuration")
                    .tint(NO_FILTER)
                    .fit(OBJECT_FIT)
                    .on_click(on(dispatch, ListPointer::TypeClick(index))),
                origin_x,
                15.0 + indent,
                -f64::from(BUTTON) / 2.0,
                f64::from(scale),
            ),
            hidden,
        ));
    }
    let type_name = item
        .rendered
        .as_ref()
        .map_or(String::new(), |r| r.type_name.clone());
    let type_width = item.type_width.unwrap_or(0.0);
    let name_click = {
        let dispatch = dispatch.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            let now = math::trunc(asrs_js::time::Clock::now(&asrs_js::time::SystemClock));
            if let Some(d) = &dispatch {
                d(ListPointer::NameClick { index, now }, cx);
            }
        }
    };
    if !type_name.is_empty() {
        body = body.child(fade(
            div()
                .id(("sl-type-name", index))
                .at_place(places, |places| places.text.clone())
                .on_click(name_click.clone())
                .absolute()
                .left(px((NAME_LEFT + indent) as f32))
                .top_0()
                .h(px(30.0))
                .max_w(px((width - 30.0 - 55.0).max(0.0) as f32))
                .overflow_hidden()
                .child(
                    Text::new(type_name, type_name_style())
                        .color(theme.text)
                        .subpixel(text_dx(NAME_LEFT + indent), 0.0),
                ),
            hidden,
        ));
    }
    if let Some(r) = &item.rendered {
        let key = r.log_key.to_string_lossy();
        let child_inset = if item.child { CHILD_INDENT } else { 0.0 };
        let max = (width - 31.0 - child_inset - type_width - 55.0).max(0.0);
        let mut key_box = div()
            .id(("sl-key", index))
            .at_place(places, |places| places.text.clone())
            .on_click(name_click)
            .absolute()
            .left(px((NAME_LEFT + indent + type_width) as f32))
            .top_0()
            .h(px(30.0))
            .max_w(px(max as f32))
            .overflow_hidden()
            .child(
                Text::new(r.key_text.to_string_lossy(), key_style())
                    .color(theme.text)
                    .subpixel(text_dx(NAME_LEFT + indent + type_width), 0.0),
            )
            .child(untitled(
                div().absolute().left_0().right_0().top(px(6.0)).h(px(18.0)),
            ));
        if !r.available {
            key_box = key_box.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .top(px(16.0))
                    .h(px(1.0))
                    .bg(rgb(theme.text)),
            );
        }
        body = body.child(titled_rtl(key, fade(key_box, hidden)));
    }
    if item.preview_shown == Some(true) {
        body = body
            .child(fade(
                div()
                    .absolute()
                    .left(px((32.0 + indent) as f32))
                    .top(px(32.0))
                    .w(px(11.0))
                    .h(px(11.0))
                    .child(
                        Icon::new(
                            ratio_symbol("subdirectoryarrowright.svg"),
                            chains::SOURCE_LIST_VALUE_SYMBOL,
                        )
                        .fit(IconFit::Natural),
                    ),
                hidden,
            ))
            .child(fade(
                div()
                    .absolute()
                    .left(px((45.0 + indent) as f32))
                    .right_0()
                    .top(px(30.0))
                    .h(px(15.0))
                    .pl(px(2.0))
                    .pr(px(4.0))
                    .overflow_hidden()
                    .at_place(places, |places| focus::last(&places.value))
                    .child(
                        Text::new(item.preview_text.clone(), value_style())
                            .color(VALUE_COLOR)
                            .subpixel(text_dx(47.0 + indent), 0.0),
                    ),
                hidden,
            ));
    }
    let visible = item.rendered.as_ref().is_none_or(|r| r.visible);
    let half = f64::from(BUTTON) / 2.0;
    body.child(translated_button(
        IconButton::new(
            ("sl-hide", index),
            symbol(if visible {
                "visibility.svg"
            } else {
                "visibilityoff.svg"
            }),
        )
        .blurred(blurred)
        .title(if visible { "Hide field" } else { "Show field" })
        .on_click(on(dispatch, ListPointer::HideClick(index))),
        origin_x,
        width - 40.0 - 2.0 * half,
        half,
        f64::from(scale),
    ))
    .child(translated_button(
        IconButton::new(("sl-remove", index), symbol("close.svg"))
            .blurred(blurred)
            .title("Remove field")
            .on_click({
                let remove = on(dispatch, ListPointer::RemoveClick(index));
                let item = places.map(|places| places.item.clone());
                move |event, window, cx| {
                    // The focused button goes with its item, and Tab goes on from there.
                    if let Some(item) = &item {
                        item.focus(window, cx);
                    }
                    remove(event, window, cx);
                }
            }),
        origin_x,
        width - 15.0 - 2.0 * half,
        half,
        f64::from(scale),
    ))
}

/// `#dragItem`'s copy of a dragged item: `clientWidth` wide, 30 px tall, without borders.
pub fn drag_item(item: &ItemView, width: f64, blurred: bool, appearance: Appearance) -> AnyElement {
    div()
        .w(px(width as f32))
        .h(px(30.0))
        .overflow_hidden()
        .child(item_body(
            item,
            0,
            ItemPlace {
                width,
                borders: (0.0, 0.0),
                origin_x: 0.0,
                scale: 1.0,
                blurred,
            },
            appearance,
            &None,
            None,
        ))
        .into_any_element()
}

impl<H: FrameHost> HubFrame<H> {
    /// Lays out the displayed tab's lists in `controls` (`div.controls-content`) for a
    /// `body_width` wide window at `scale`, for the reads the model makes before the next
    /// render.
    pub(super) fn layout_source_lists(
        &mut self,
        controls: [f64; 4],
        body_width: f64,
        scale: f32,
        cx: &mut Context<'_, Self>,
    ) {
        let geometry = self.host().source_list_geometry();
        let mut layout = geometry.0.borrow_mut();
        layout.body_width = body_width;
        layout.scale = scale;
        let mut lists = HashMap::new();
        for (list, root) in displayed_lists(self.host().hub(cx), rect(controls)) {
            let scroll = layout
                .scroll
                .entry(list.uuid().to_owned())
                .or_default()
                .offset();
            let scroll_top = -f64::from(f32::from(scroll.y));
            lists.insert(
                list.uuid().to_owned(),
                ListLayout::new(root, list.items(), scroll_top),
            );
        }
        layout.lists = lists;
    }

    fn list_input(&mut self, uuid: &str, event: ListPointer, cx: &mut Context<'_, Self>) {
        let (out, errors) = self.host_mut().input(cx, |hub| {
            hub.source_list_input(ListInput::Pointer { uuid, event })
        });
        for e in errors {
            eprintln!("asrs: {e:?}");
        }
        if let Some(start) = out.drag {
            let item = find_list(self.host().hub(cx), &start.payload.uuid)
                .and_then(|l| l.items().nth(start.payload.index).cloned())
                .unwrap_or_default();
            self.shell_mut().start_list_drag(start, item);
        }
        cx.notify();
    }

    /// The `mousemove` listener of the `div.list` under the pointer, which runs before the
    /// window's listeners.
    pub(super) fn source_list_mouse_move(&mut self, x: f64, y: f64, cx: &mut Context<'_, Self>) {
        if let Some(uuid) = self.host().source_list_geometry().list_at(x, y) {
            self.list_input(&uuid, ListPointer::Move { x, y }, cx);
        }
    }

    /// Scrolls the list that holds the focused element, at `bounds`, to it.
    pub(super) fn scroll_source_list(
        &mut self,
        focused: &FocusHandle,
        bounds: Bounds<Pixels>,
        window: &Window,
    ) {
        let geometry = self.host().source_list_geometry();
        let layout = geometry.0.borrow();
        let holds = |(_, place): &(&String, &FocusHandle)| place.contains(focused, window);
        let Some((uuid, _)) = layout.places.iter().find(holds) else {
            return;
        };
        if let (Some(list), Some(scroll)) = (layout.lists.get(uuid), layout.scroll.get(uuid)) {
            let rect = list.list_rect();
            scroll_to(scroll, [rect.y, rect.height], bounds, window);
        }
    }

    pub(super) fn source_lists(
        &mut self,
        controls: [f64; 4],
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Vec<AnyElement> {
        let geometry = self.host().source_list_geometry();
        let shown: Vec<(Shown, ListRoot)> = displayed_lists(self.host().hub(cx), rect(controls))
            .into_iter()
            .map(|(list, root)| (Shown::of(list, self.host().hub(cx)), root))
            .collect();
        let mut out = Vec::new();
        for (at, (list, root)) in shown.into_iter().enumerate() {
            let uuid = list.uuid.clone();
            let (layout, scroll) = {
                let g = geometry.0.borrow();
                (
                    g.lists.get(&uuid).cloned().unwrap_or_default(),
                    g.scroll.get(&uuid).cloned().unwrap_or_default(),
                )
            };
            if root.border_x > 0.0 {
                let scale = f64::from(geometry.0.borrow().scale);
                let color = rgb(if appearance == Appearance::Dark {
                    0x000000
                } else {
                    0xdddddd
                });
                for edge in border_edges(root.rect, scale) {
                    out.push(
                        div()
                            .absolute()
                            .left(px((edge - controls[0]) as f32))
                            .top(px((root.rect.y - controls[1]) as f32))
                            .w(px(1.0))
                            .h(px(root.rect.height as f32))
                            .bg(color)
                            .into_any_element(),
                    );
                }
            }
            let scale = f64::from(geometry.0.borrow().scale);
            out.push(self.list_element(
                at, &list, &layout, scroll, root, controls, scale, appearance, window, cx,
            ));
        }
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn list_element(
        &self,
        at: usize,
        list: &Shown,
        layout: &ListLayout,
        scroll: ScrollHandle,
        root: ListRoot,
        controls: [f64; 4],
        scale: f64,
        appearance: Appearance,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> AnyElement {
        let theme = appearance.theme();
        let dark = appearance == Appearance::Dark;
        let uuid: Rc<str> = list.uuid.as_str().into();
        let r = root.rect;
        let inner = r.width - 2.0 * root.border_x;
        let left = r.x + root.border_x;
        let snap = |v: f64| math::floor(v * scale + 0.5) / scale;
        let place = focus::place(SharedString::from(format!("sl-root-{uuid}")), window, cx);
        let mut element = focus::section(div(), at as isize + 1, &place)
            .absolute()
            .left(px((r.x + root.border_x - controls[0]) as f32))
            .top(px((r.y - controls[1]) as f32))
            .w(px(inner as f32))
            .h(px(r.height as f32));

        let inset = if list.edit_button { 30.0 } else { 55.0 };
        let available = (inner - 2.0 * inset).max(0.0);
        let (title_width, _) = text::measure(cx, &title_style("left"), &list.title, scale as f32);
        let (title_left, title_box, title_style) = match centred_offset(available, title_width) {
            Some(offset) => (inset + offset, title_width, title_style("left")),
            None => (inset, available, title_style("center")),
        };
        let (title_x, title_dx) = place_in(left, title_left, scale);
        element = element.child(
            div()
                .absolute()
                .top_0()
                .left(px(title_x))
                .w(px(title_box as f32))
                .h(px(TITLE_HEIGHT as f32))
                .overflow_hidden()
                .child(
                    Text::new(list.title.clone(), title_style)
                        .color(theme.text)
                        .subpixel(title_dx as f32, 0.0),
                ),
        );
        let dispatch: Option<Dispatch> = {
            let frame = cx.entity().downgrade();
            let uuid = uuid.clone();
            Some(Rc::new(move |event, cx: &mut App| {
                let _ = frame.update(cx, |frame, cx| frame.list_input(&uuid, event, cx));
            }))
        };
        let half = f64::from(BUTTON) / 2.0;
        let corner =
            |right: f64, id: &'static str, title: &'static str, icon: Symbol, fit, event| {
                translated_button(
                    IconButton::new((id, uuid_hash(&uuid)), icon)
                        .fit(fit)
                        .blurred(list.blurred[0])
                        .title(title)
                        .on_click(on(&dispatch, event)),
                    left,
                    inner - right - 2.0 * half,
                    half,
                    scale,
                )
            };
        if list.edit_button {
            element = element.child(corner(
                15.0,
                "sl-edit",
                "Edit settings",
                symbol("morehoriz.svg"),
                IconFit::BUTTON,
                ListPointer::EditClick,
            ));
        } else {
            let clear_fit = IconFit::Centered {
                max: 1.0,
                scale: 0.85,
            };
            element = element
                .child(corner(
                    15.0,
                    "sl-clear",
                    "Clear all fields",
                    symbol("delete.svg"),
                    clear_fit,
                    ListPointer::ClearClick,
                ))
                .child(corner(
                    40.0,
                    "sl-help",
                    "Help",
                    symbol("help.svg"),
                    IconFit::BUTTON,
                    ListPointer::HelpClick,
                ));
        }

        let count = list.items.len();
        let place = focus::place(SharedString::from(format!("sl-list-{uuid}")), window, cx);
        let geometry = self.host().source_list_geometry();
        let places = &mut geometry.0.borrow_mut().places;
        places.insert(uuid.to_string(), place.clone());
        let mut items = div()
            .id(("sl-list", uuid_hash(&uuid)))
            .track_focus(&place)
            .absolute()
            .left_0()
            .top(px(TITLE_HEIGHT as f32))
            .w(px(inner as f32))
            .h(px((r.height - TITLE_HEIGHT).max(0.0) as f32))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .flex()
            .flex_col()
            .on_any_mouse_down({
                let uuid = uuid.clone();
                cx.listener(move |frame, e: &MouseDownEvent, _, cx| {
                    let (x, y) = client_point(e.position);
                    frame.list_input(&uuid, ListPointer::Down { x, y }, cx)
                })
            })
            .on_mouse_up(MouseButton::Left, {
                let uuid = uuid.clone();
                cx.listener(move |frame, _: &MouseUpEvent, _, cx| {
                    frame.list_input(&uuid, ListPointer::Up, cx)
                })
            });
        for (index, item) in list.items.iter().enumerate() {
            let borders = (
                layout.item_border_top(index),
                if index + 1 == count { 1.0 } else { 0.0 },
            );
            let place = ItemPlace {
                width: inner,
                borders,
                origin_x: left,
                scale: scale as f32,
                blurred: list.blurred[1],
            };
            let places = ItemPlaces::new(&uuid, index, window, cx);
            let row = item_body(item, index, place, appearance, &dispatch, Some(&places))
                .on_mouse_down(MouseButton::Right, {
                    let uuid = uuid.clone();
                    cx.listener(move |frame, e: &MouseDownEvent, _, cx| {
                        let (x, y) = client_point(e.position);
                        frame.list_input(&uuid, ListPointer::ContextMenu { index, x, y }, cx)
                    })
                });
            items = items.child(row);
        }
        // The last item's texts start below its border, over its bottom by as much.
        let last = list.items.last().filter(|i| i.preview_shown != Some(true));
        if last.is_some() && layout.item_border_top(count - 1) > 0.0 {
            items = items.child(div().flex_none().h(px(1.0)));
        }
        element = element.child(items);

        if list.hand_icon {
            element = element.child(
                div()
                    .absolute()
                    .left(px((snap(left + inner / 2.0 - 30.0) - left) as f32))
                    .top(px((snap(r.y + r.height / 2.0 + 15.0 - 30.0) - r.y) as f32))
                    .w(px(60.0))
                    .h(px(60.0))
                    .opacity(0.15)
                    .child(
                        Icon::new(hand_icon(), if dark { chains::INVERT } else { "" })
                            .fit(IconFit::Natural),
                    ),
            );
        }
        let highlight = list.highlight;
        if !highlight.hidden
            && let Some((top, height)) = highlight.placement
        {
            element = element.child(
                div()
                    .absolute()
                    .left_0()
                    .w(px(inner as f32))
                    .top(px(length(top, r.height)))
                    .h(px(length(height, r.height)))
                    .bg(rgb(0x90ee90))
                    .opacity(0.25),
            );
        }
        element.into_any_element()
    }
}

/// Scrolls `scroll`, whose view is `[y, height]`, to the focused element at `bounds`.
pub(super) fn scroll_to(
    scroll: &ScrollHandle,
    [y, height]: [f64; 2],
    bounds: Bounds<Pixels>,
    window: &Window,
) {
    let top = f64::from(f32::from(bounds.origin.y)) - y;
    let item = [top, f64::from(f32::from(bounds.size.height))];
    let by = focus::reveal(ScrollAxis::Vertical, height, item);
    if by != 0.0 {
        let offset = scroll.offset();
        let max = f64::from(f32::from(scroll.max_offset().y));
        let scale = f64::from(window.scale_factor());
        let top = assign_scroll_offset(by - f64::from(f32::from(offset.y)), max, scale);
        scroll.set_offset(point(offset.x, px(-top as f32)));
    }
}

/// Where Blink paints a 1 px left and right border of the box `r`: it snaps the box's edges to
/// device pixels, where GPUI would snap its origin and width apart.
fn border_edges(r: Rect, scale: f64) -> [f64; 2] {
    let snap = |v: f64| math::floor(v * scale + 0.5) / scale;
    [snap(r.x), snap(r.x + r.width) - 1.0]
}

fn client_point(position: Point<Pixels>) -> (f64, f64) {
    (
        client_coordinate(f64::from(f32::from(position.x))),
        client_coordinate(f64::from(f32::from(position.y))),
    )
}

fn uuid_hash(uuid: &str) -> usize {
    uuid.bytes().fold(0usize, |h, b| {
        h.wrapping_mul(31).wrapping_add(usize::from(b))
    })
}
