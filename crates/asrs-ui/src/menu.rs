//! The drawn popup menu: a [`PopupMenu`] painted by GPUI, either inside the window that opens it
//! (`deferred(anchored())`, for in-window popups such as select lists) or in a `PopUp` window of
//! its own that can overflow its parent (context menus where no native backend exists). Also
//! the accelerator table that stands in for a native menu bar.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Bounds, Context, Entity, FocusHandle, Focusable, Global,
    InteractiveElement, IntoElement, KeyDownEvent, Keystroke, MouseButton, MouseDownEvent,
    OperatingSystem, ParentElement, Pixels, Point, Render, Styled, Subscription, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, anchored, canvas,
    deferred, div, point, px, rgb, size,
};

use crate::platform::{MenuPath, PopupEntry, PopupItem, PopupMenu};
use crate::text::{Text, TextStyle};
use crate::theme::Appearance;

pub const ROW_HEIGHT: f32 = 22.0;
pub const SEPARATOR_HEIGHT: f32 = 9.0;
pub const PADDING: f32 = 4.0;
pub const LABEL_INSET: f32 = 24.0;
pub const TRAILING_INSET: f32 = 24.0;
pub const MIN_WIDTH: f32 = 120.0;
const FONT: &str = "13px ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";

/// Called once when the menu closes: with the chosen entry's path, or `None` when dismissed.
pub type CloseHandler = Rc<dyn Fn(Option<MenuPath>, &mut Window, &mut App)>;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Host {
    InWindow,
    PopUpWindow,
}

fn label_style() -> TextStyle {
    TextStyle::from_css([("font", FONT), ("white-space", "pre")]).expect("menu font")
}

fn visible_items(menu: &PopupMenu) -> impl Iterator<Item = (usize, &PopupItem)> {
    menu.items
        .iter()
        .enumerate()
        .filter(|(_, item)| match item {
            PopupItem::Separator => true,
            PopupItem::Entry(entry) => entry.visible,
        })
}

fn row_height(item: &PopupItem) -> f32 {
    match item {
        PopupItem::Separator => SEPARATOR_HEIGHT,
        PopupItem::Entry(_) => ROW_HEIGHT,
    }
}

/// The size the drawn menu takes, in logical px.
pub fn menu_size(cx: &mut App, menu: &PopupMenu, scale: f32) -> (f32, f32) {
    let style = label_style();
    let mut width: f32 = 0.0;
    let mut height = 2.0 * PADDING;
    for (_, item) in visible_items(menu) {
        height += row_height(item);
        if let PopupItem::Entry(entry) = item {
            let (w, _) = crate::text::measure(cx, &style, &entry.label, scale);
            width = width.max(w as f32);
        }
    }
    (
        (width + LABEL_INSET + TRAILING_INSET).ceil().max(MIN_WIDTH),
        height,
    )
}

fn row_top(menu: &PopupMenu, index: usize) -> f32 {
    PADDING
        + visible_items(menu)
            .take_while(|(i, _)| *i < index)
            .map(|(_, item)| row_height(item))
            .sum::<f32>()
}

fn selectable(entry: &PopupEntry) -> bool {
    entry.visible && entry.enabled
}

pub struct MenuView {
    menu: PopupMenu,
    prefix: MenuPath,
    host: Host,
    focus: FocusHandle,
    highlighted: Option<usize>,
    submenu: Option<(usize, Entity<MenuView>, Option<AnyWindowHandle>)>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    on_close: CloseHandler,
    own_window: Option<AnyWindowHandle>,
    _activation: Option<Subscription>,
}

impl MenuView {
    fn new(
        menu: PopupMenu,
        prefix: MenuPath,
        host: Host,
        on_close: CloseHandler,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        MenuView {
            menu,
            prefix,
            host,
            focus: cx.focus_handle(),
            highlighted: None,
            submenu: None,
            bounds: Rc::default(),
            on_close,
            own_window: None,
            _activation: None,
        }
    }

    /// A menu drawn inside the window that opens it. The caller renders it with [`overlay`].
    pub fn in_window(
        menu: PopupMenu,
        window: &mut Window,
        cx: &mut App,
        on_close: CloseHandler,
    ) -> Entity<MenuView> {
        let view = cx.new(|cx| MenuView::new(menu, Vec::new(), Host::InWindow, on_close, cx));
        view.read(cx).focus.clone().focus(window, cx);
        view
    }

    /// A menu in a `PopUp` window whose top-left corner is at `origin` in screen coordinates.
    pub fn open_window(
        menu: PopupMenu,
        origin: Point<Pixels>,
        scale: f32,
        cx: &mut App,
        on_close: CloseHandler,
    ) -> Option<AnyWindowHandle> {
        Self::open_window_at(menu, Vec::new(), origin, scale, cx, on_close)
            .map(|(handle, _)| handle)
    }

    fn open_window_at(
        menu: PopupMenu,
        prefix: MenuPath,
        origin: Point<Pixels>,
        scale: f32,
        cx: &mut App,
        on_close: CloseHandler,
    ) -> Option<(AnyWindowHandle, Entity<MenuView>)> {
        let (width, height) = menu_size(cx, &menu, scale);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                origin,
                size(px(width), px(height)),
            ))),
            titlebar: None,
            focus: true,
            show: true,
            kind: WindowKind::PopUp,
            window_background: WindowBackgroundAppearance::Transparent,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            ..WindowOptions::default()
        };
        crate::open_window(options, cx, move |window, cx| {
            let view = cx.new(|cx| {
                let mut view = MenuView::new(menu, prefix, Host::PopUpWindow, on_close, cx);
                view.own_window = Some(window.window_handle());
                view._activation =
                    Some(cx.observe_window_activation(window, |view, window, cx| {
                        if !window.is_window_active() && view.submenu.is_none() {
                            view.dismiss(window, cx);
                        }
                    }));
                view
            });
            view.read(cx).focus.clone().focus(window, cx);
            view
        })
        .ok()
    }

    fn entry(&self, index: usize) -> Option<&PopupEntry> {
        match self.menu.items.get(index)? {
            PopupItem::Entry(entry) => Some(entry),
            PopupItem::Separator => None,
        }
    }

    fn close_submenu(&mut self, cx: &mut App) {
        if let Some((_, view, handle)) = self.submenu.take() {
            view.update(cx, |view, cx| view.close_submenu(cx));
            remove_window(handle, cx);
        }
    }

    /// `window` is the window the choice was made in, which for a submenu in a `PopUp` window
    /// is not this menu's.
    fn finish(&mut self, path: Option<MenuPath>, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.close_submenu(cx);
        (self.on_close)(path, window, cx);
        remove_window(self.own_window, cx);
    }

    fn dismiss(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.finish(None, window, cx);
    }

    fn open_submenu(
        &mut self,
        index: usize,
        focus: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.submenu.as_ref().is_some_and(|(i, _, _)| *i == index) {
            return;
        }
        self.close_submenu(cx);
        let Some(submenu) = self.entry(index).and_then(|e| e.submenu.clone()) else {
            return;
        };
        let mut prefix = self.prefix.clone();
        prefix.push(index);
        let parent = cx.entity().downgrade();
        let on_close: CloseHandler = Rc::new(move |path, window, cx| {
            if let Some(parent) = parent.upgrade() {
                parent.update(cx, |parent, cx| {
                    parent.submenu = None;
                    match path {
                        Some(path) => parent.finish(Some(path), window, cx),
                        None => {
                            if let Some(handle) = parent.own_window {
                                cx.defer(move |cx| {
                                    handle.update(cx, |_, w, _| w.activate_window()).ok();
                                });
                            }
                            cx.notify();
                        }
                    }
                });
            }
        });
        let bounds = self.bounds.get();
        let offset = point(
            bounds.size.width - px(2.0),
            px(row_top(&self.menu, index) - PADDING),
        );
        match self.host {
            Host::InWindow => {
                let view =
                    cx.new(|cx| MenuView::new(submenu, prefix, Host::InWindow, on_close, cx));
                if focus {
                    view.read(cx).focus.clone().focus(window, cx);
                }
                self.submenu = Some((index, view, None));
            }
            Host::PopUpWindow => {
                let origin = window.bounds().origin + bounds.origin + offset;
                if let Some((handle, view)) = Self::open_window_at(
                    submenu,
                    prefix,
                    origin,
                    window.scale_factor(),
                    cx,
                    on_close,
                ) {
                    self.submenu = Some((index, view, Some(handle)));
                }
            }
        }
        cx.notify();
    }

    fn activate(&mut self, index: usize, window: &mut Window, cx: &mut Context<'_, Self>) {
        let Some(entry) = self.entry(index) else {
            return;
        };
        if !selectable(entry) {
            return;
        }
        if entry.submenu.is_some() {
            self.open_submenu(index, true, window, cx);
        } else {
            let mut path = self.prefix.clone();
            path.push(index);
            self.finish(Some(path), window, cx);
        }
    }

    fn step(&mut self, forward: bool, cx: &mut Context<'_, Self>) {
        let candidates: Vec<usize> = visible_items(&self.menu)
            .filter_map(|(i, item)| match item {
                PopupItem::Entry(entry) if selectable(entry) => Some(i),
                _ => None,
            })
            .collect();
        if candidates.is_empty() {
            return;
        }
        let position = self
            .highlighted
            .and_then(|h| candidates.iter().position(|&c| c == h));
        let next = match (position, forward) {
            (None, true) => 0,
            (None, false) => candidates.len() - 1,
            (Some(p), true) => (p + 1) % candidates.len(),
            (Some(p), false) => (p + candidates.len() - 1) % candidates.len(),
        };
        self.highlighted = Some(candidates[next]);
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        match event.keystroke.key.as_str() {
            "down" => self.step(true, cx),
            "up" => self.step(false, cx),
            "enter" | "space" => {
                if let Some(index) = self.highlighted {
                    self.activate(index, window, cx);
                }
            }
            "right" => {
                if let Some(index) = self.highlighted
                    && self
                        .entry(index)
                        .is_some_and(|e| e.submenu.is_some() && e.enabled)
                {
                    self.open_submenu(index, true, window, cx);
                }
            }
            "left" if !self.prefix.is_empty() => self.finish(None, window, cx),
            "escape" => self.finish(None, window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn row(
        &self,
        index: usize,
        entry: &PopupEntry,
        appearance: Appearance,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement + use<> {
        let theme = appearance.theme();
        let highlighted = self.highlighted == Some(index) && entry.enabled;
        let (background, text) = if highlighted {
            (Some(theme.focus_ring), 0xffffff)
        } else if entry.enabled {
            (None, theme.text)
        } else {
            (None, appearance.pick(0x9a9a9a, 0x7a7a7a))
        };
        let style = label_style();
        let mut row = div()
            .id(("menu-row", index))
            .h(px(ROW_HEIGHT))
            .mx(px(PADDING))
            .rounded(px(4.0))
            .flex()
            .items_center()
            .on_mouse_move(cx.listener(move |view, _, window, cx| {
                if view.highlighted != Some(index) {
                    view.highlighted = Some(index);
                    let has_submenu = view
                        .entry(index)
                        .is_some_and(|e| e.submenu.is_some() && e.enabled);
                    if has_submenu {
                        view.open_submenu(index, false, window, cx);
                    } else {
                        view.close_submenu(cx);
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| view.activate(index, window, cx)),
            )
            .child(
                div()
                    .w(px(LABEL_INSET - PADDING))
                    .flex()
                    .justify_center()
                    .when_some(entry.checked.then_some(()), |d, _| {
                        d.child(Text::new("\u{2713}", style).color(text))
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .child(Text::new(entry.label.clone(), style).color(text)),
            )
            .child(
                div()
                    .w(px(TRAILING_INSET - PADDING))
                    .flex()
                    .justify_center()
                    .when(entry.submenu.is_some(), |d| {
                        d.child(Text::new("\u{203a}", style).color(text))
                    }),
            );
        if let Some(background) = background {
            row = row.bg(rgb(background));
        }
        row
    }
}

impl Focusable for MenuView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for MenuView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let appearance = Appearance::of(window, cx);
        let theme = appearance.theme();
        let bounds = self.bounds.clone();
        let mut body = div()
            .id("menu")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down_out(cx.listener(|view, _: &MouseDownEvent, window, cx| {
                if view.host == Host::InWindow && view.submenu.is_none() {
                    view.dismiss(window, cx);
                }
            }))
            .relative()
            .py(px(PADDING))
            .min_w(px(MIN_WIDTH))
            .bg(rgb(theme.background))
            .border_1()
            .border_color(rgb(appearance.pick(0xc8c8c8, 0x4a4a4a)))
            .rounded(px(6.0))
            .shadow_md()
            .child(
                canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            );
        let items: Vec<(usize, PopupItem)> = visible_items(&self.menu)
            .map(|(i, item)| (i, item.clone()))
            .collect();
        for (index, item) in items {
            body = match &item {
                PopupItem::Separator => body.child(
                    div()
                        .h(px(SEPARATOR_HEIGHT))
                        .flex()
                        .items_center()
                        .px(px(PADDING))
                        .child(
                            div()
                                .h(px(1.0))
                                .w_full()
                                .bg(rgb(appearance.pick(0xdddddd, 0x444444))),
                        ),
                ),
                PopupItem::Entry(entry) => body.child(self.row(index, entry, appearance, cx)),
            };
        }
        let submenu = match &self.submenu {
            Some((index, view, None)) => {
                let b = self.bounds.get();
                Some(
                    deferred(
                        anchored()
                            .position(
                                b.origin
                                    + point(
                                        b.size.width - px(2.0),
                                        px(row_top(&self.menu, *index) - PADDING),
                                    ),
                            )
                            .snap_to_window()
                            .child(view.clone()),
                    )
                    .with_priority(1),
                )
            }
            _ => None,
        };
        div().child(body).children(submenu)
    }
}

fn remove_window(handle: Option<AnyWindowHandle>, cx: &mut App) {
    if let Some(handle) = handle {
        cx.defer(move |cx| {
            handle
                .update(cx, |_, window, _| window.remove_window())
                .ok();
        });
    }
}

/// The element that shows an in-window menu with its top-left corner at `position`, in window
/// coordinates.
pub fn overlay(menu: &Entity<MenuView>, position: Point<Pixels>) -> impl IntoElement {
    deferred(
        anchored()
            .position(position)
            .snap_to_window()
            .child(menu.clone()),
    )
}

fn current_os() -> OperatingSystem {
    if cfg!(target_os = "macos") {
        OperatingSystem::Mac
    } else if cfg!(windows) {
        OperatingSystem::Windows
    } else {
        OperatingSystem::Linux
    }
}

struct Accelerators(#[allow(dead_code)] Subscription);

impl Global for Accelerators {}

/// Electron's accelerator table on platforms without a visible native menu bar: a keystroke no
/// action handled triggers the menu entry registered for it. Replaces the previous table.
pub fn install_accelerators(
    cx: &mut App,
    table: Vec<(String, MenuPath)>,
    on_trigger: impl Fn(MenuPath, &mut Window, &mut App) + 'static,
) {
    let table: Vec<(Keystroke, MenuPath)> = table
        .into_iter()
        .filter_map(|(keystroke, path)| {
            Some((
                Keystroke::parse_with_os(&keystroke, current_os()).ok()?,
                path,
            ))
        })
        .collect();
    let subscription = cx.observe_keystrokes(move |event, window, cx| {
        if event.action.is_some() {
            return;
        }
        let pressed = &event.keystroke;
        if let Some((_, path)) = table
            .iter()
            .find(|(k, _)| k.modifiers == pressed.modifiers && k.key == pressed.key)
        {
            on_trigger(path.clone(), window, cx);
        }
    });
    cx.set_global(Accelerators(subscription));
}
