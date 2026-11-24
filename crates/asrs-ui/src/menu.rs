//! The drawn popup menu: a [`PopupMenu`] painted by GPUI, either inside the window that opens it
//! (`deferred(anchored())`, for in-window popups) or in windows of its own that can overflow its
//! parent (context menus, title bar menus and select lists where no native backend exists). Also
//! the accelerator table that stands in for a native menu bar.

use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::popup::{PopupAnchor, PopupConstraintAdjustment, PopupGravity, PopupOptions};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Bounds, Context, DispatchPhase, Entity, FocusHandle,
    Focusable, Global, InteractiveElement, IntoElement, KeyDownEvent, Keystroke, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, OperatingSystem, ParentElement, Pixels,
    PlatformInput, Point, Render, ScrollEvent, ScrollHandle, Size, StatefulInteractiveElement,
    Styled, Subscription, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions, anchored, canvas, deferred, div, point, px, rgb, size,
};

use crate::platform::{self, MenuPath, PopupEntry, PopupItem, PopupMenu};
use crate::text::{Text, TextStyle};
use crate::theme::Appearance;

pub const ROW_HEIGHT: f32 = 22.0;
pub const SEPARATOR_HEIGHT: f32 = 9.0;
/// From the menu's edge, its border included.
pub const PADDING: f32 = 4.0;
const BORDER: f32 = 1.0;
pub const LABEL_INSET: f32 = 24.0;
pub const TRAILING_INSET: f32 = 24.0;
const SHORTCUT_GAP: f32 = 24.0;
pub const MIN_WIDTH: f32 = 120.0;
const GROUP_FONT: &str = "bold 13px ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";
const FONT: &str = "13px ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";

/// Called once when the menu closes: with the chosen entry's path, or `None` when dismissed.
pub type CloseHandler = Rc<dyn Fn(Option<MenuPath>, &mut Window, &mut App)>;

/// How far the pointer moves over a menu before a button release chooses an entry: the release
/// of the press that opened the menu chooses nothing.
const ARM_DISTANCE: f32 = 4.0;
/// The most rows a select list shows, which is also how far its page keys go, less one.
const LIST_ROWS: usize = 20;
const TYPE_AHEAD_TIMEOUT: Duration = Duration::from_secs(1);
/// How far a submenu overlaps its parent.
const SUBMENU_OVERLAP: f32 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Host {
    InWindow,
    /// A `PopUp` window at a position on screen.
    PopUpWindow,
    /// A popup surface the compositor places against its parent's.
    AnchoredPopup,
}

/// Which side of its anchor a menu opens on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Below it, or above where the screen has no room below.
    Below,
    /// To its right, or left where the screen has no room there: a submenu.
    Beside,
}

/// Where a menu of `size` opens for `anchor`, all in screen coordinates.
pub fn place(
    anchor: Bounds<Pixels>,
    size: Size<Pixels>,
    screen: Bounds<Pixels>,
    side: Side,
) -> Bounds<Pixels> {
    let fit = |v: Pixels, low: Pixels, high: Pixels| v.min(high).max(low);
    let max_x = screen.right() - size.width;
    let max_y = screen.bottom() - size.height;
    let (x, y) = match side {
        Side::Below => {
            let above = anchor.top() - size.height;
            let y = if anchor.bottom() > max_y && above >= screen.top() {
                above
            } else {
                anchor.bottom()
            };
            (anchor.left(), y)
        }
        Side::Beside => {
            let right = anchor.right() - px(SUBMENU_OVERLAP);
            let left = anchor.left() + px(SUBMENU_OVERLAP) - size.width;
            let x = if right > max_x && left >= screen.left() {
                left
            } else {
                right
            };
            (x, anchor.top() - px(PADDING))
        }
    };
    let origin = point(fit(x, screen.left(), max_x), fit(y, screen.top(), max_y));
    Bounds::new(origin, size)
}

/// Where a menu's window goes.
#[derive(Clone, Copy, Debug)]
pub enum Placement {
    /// A `PopUp` window against a box on screen, inside the box of its monitor.
    Screen(Bounds<Pixels>, Option<Bounds<Pixels>>),
    /// A popup of this window against a box in it, where the compositor places windows.
    Anchored(AnyWindowHandle, Bounds<Pixels>),
}

/// A select list's differences from a menu.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListStyle {
    pub min_width: f32,
    /// The entry highlighted as the list opens.
    pub selected: Option<usize>,
    /// The `<optgroup>` labels.
    pub groups: Vec<usize>,
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
            let label = platform::native_menu_label(&entry.label);
            let (w, _) = crate::text::measure(cx, &style, &label, scale);
            let shortcut = entry.shortcut.as_ref().map_or(0.0, |shortcut| {
                SHORTCUT_GAP + crate::text::measure(cx, &style, shortcut, scale).0 as f32
            });
            width = width.max(w as f32 + shortcut);
        }
    }
    (
        (width + LABEL_INSET + TRAILING_INSET).ceil().max(MIN_WIDTH),
        height,
    )
}

fn row_bottom(menu: &PopupMenu, index: usize) -> f32 {
    PADDING
        + visible_items(menu)
            .take_while(|(i, _)| *i <= index)
            .map(|(_, item)| row_height(item))
            .sum::<f32>()
}

fn selectable(entry: &PopupEntry) -> bool {
    entry.visible && entry.enabled
}

/// The key context of an open menu.
const CONTEXT: &str = "AsrsMenu";

/// Whether a menu drawn in `window` has its focus.
pub fn focused(window: &Window) -> bool {
    window.context_stack().iter().any(|c| c.contains(CONTEXT))
}

pub struct MenuView {
    menu: PopupMenu,
    prefix: MenuPath,
    host: Host,
    focus: FocusHandle,
    highlighted: Option<usize>,
    submenu: Option<(usize, Entity<MenuView>, Option<AnyWindowHandle>)>,
    on_close: CloseHandler,
    own_window: Option<AnyWindowHandle>,
    list: Option<ListStyle>,
    /// The entry keys chose in a select list, which closing it keeps.
    provisional: Option<usize>,
    typed: String,
    typed_at: Option<Instant>,
    /// Where a `PopUp` window was put, on screen.
    screen: Option<Bounds<Pixels>>,
    monitor: Option<Bounds<Pixels>>,
    armed: bool,
    first_move: Option<Point<Pixels>>,
    scroll: ScrollHandle,
    subscriptions: Vec<Subscription>,
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
            on_close,
            own_window: None,
            list: None,
            provisional: None,
            typed: String::new(),
            typed_at: None,
            screen: None,
            monitor: None,
            armed: host == Host::InWindow,
            first_move: None,
            scroll: ScrollHandle::new(),
            subscriptions: Vec::new(),
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
        let size = menu_size(cx, &menu, scale);
        let bounds = Bounds::new(origin, gpui_kit::size(px(size.0), px(size.1)));
        Self::open_at(
            menu,
            Vec::new(),
            WindowKind::PopUp,
            bounds,
            None,
            None,
            cx,
            on_close,
        )
        .map(|(handle, _)| handle)
    }

    /// A menu in a window of its own, placed by the screen's edges and no taller than the screen.
    pub fn open(
        menu: PopupMenu,
        placement: Placement,
        list: Option<ListStyle>,
        scale: f32,
        cx: &mut App,
        on_close: CloseHandler,
    ) -> Option<(AnyWindowHandle, Entity<MenuView>)> {
        Self::open_placed(
            menu,
            Vec::new(),
            placement,
            Side::Below,
            list,
            scale,
            cx,
            on_close,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn open_placed(
        menu: PopupMenu,
        prefix: MenuPath,
        placement: Placement,
        side: Side,
        list: Option<ListStyle>,
        scale: f32,
        cx: &mut App,
        on_close: CloseHandler,
    ) -> Option<(AnyWindowHandle, Entity<MenuView>)> {
        let (width, height) = menu_size(cx, &menu, scale);
        let width = width.max(list.as_ref().map_or(0.0, |list| list.min_width));
        let height = match &list {
            Some(_) => height.min(2.0 * PADDING + LIST_ROWS as f32 * ROW_HEIGHT),
            None => height,
        };
        let monitor = match placement {
            Placement::Screen(_, monitor) => monitor,
            Placement::Anchored(..) => None,
        };
        let screen = monitor.or_else(|| cx.primary_display().map(|display| display.bounds()));
        let height = screen.map_or(height, |s| height.min(f32::from(s.size.height)));
        let size = size(px(width), px(height));
        let (kind, bounds) = match placement {
            Placement::Screen(anchor, _) => {
                let bounds = match screen {
                    Some(screen) => place(anchor, size, screen, side),
                    None => Bounds::new(anchor.bottom_left(), size),
                };
                (WindowKind::PopUp, bounds)
            }
            Placement::Anchored(parent, anchor_rect) => {
                let (anchor, flip, offset) = match side {
                    Side::Below => (
                        PopupAnchor::BottomLeft,
                        PopupConstraintAdjustment::FLIP_Y,
                        point(px(0.0), px(0.0)),
                    ),
                    Side::Beside => (
                        PopupAnchor::TopRight,
                        PopupConstraintAdjustment::FLIP_X,
                        point(px(-SUBMENU_OVERLAP), px(-PADDING)),
                    ),
                };
                let options = PopupOptions {
                    parent,
                    anchor_rect,
                    anchor,
                    gravity: PopupGravity::BottomRight,
                    constraint_adjustment: flip
                        | PopupConstraintAdjustment::SLIDE_X
                        | PopupConstraintAdjustment::SLIDE_Y,
                    offset,
                    grab: true,
                };
                (
                    WindowKind::AnchoredPopup(options),
                    Bounds::new(Point::default(), size),
                )
            }
        };
        Self::open_at(menu, prefix, kind, bounds, monitor, list, cx, on_close)
    }

    #[allow(clippy::too_many_arguments)]
    fn open_at(
        menu: PopupMenu,
        prefix: MenuPath,
        kind: WindowKind,
        bounds: Bounds<Pixels>,
        monitor: Option<Bounds<Pixels>>,
        list: Option<ListStyle>,
        cx: &mut App,
        on_close: CloseHandler,
    ) -> Option<(AnyWindowHandle, Entity<MenuView>)> {
        let host = if kind == WindowKind::PopUp {
            Host::PopUpWindow
        } else {
            Host::AnchoredPopup
        };
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: None,
            focus: true,
            show: true,
            kind,
            window_background: WindowBackgroundAppearance::Transparent,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            ..WindowOptions::default()
        };
        let root = prefix.is_empty();
        crate::open_window(options, cx, move |window, cx| {
            let view = cx.new(|cx| {
                let mut view = MenuView::new(menu, prefix, host, on_close, cx);
                view.own_window = Some(window.window_handle());
                view.highlighted = list.as_ref().and_then(|list| list.selected);
                view.list = list;
                view.monitor = monitor;
                if let Some(index) = view.highlighted {
                    let hidden = px(row_bottom(&view.menu, index)) - bounds.size.height;
                    let offset = point(px(0.0), -hidden.max(px(0.0)));
                    view.scroll.set_offset(offset);
                }
                if host == Host::PopUpWindow {
                    view.screen = Some(bounds);
                }
                view.subscriptions.push(cx.observe_window_activation(
                    window,
                    |view, window, cx| {
                        if !window.is_window_active() && view.submenu.is_none() {
                            view.dismiss(window, cx);
                        }
                    },
                ));
                view
            });
            if host == Host::PopUpWindow {
                platform::popup_window_opened(window, bounds, root, cx);
            }
            view.read(cx).focus.clone().focus(window, cx);
            view
        })
        .ok()
    }

    /// Closes the menu with no choice.
    pub fn cancel(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.dismiss(window, cx);
    }

    /// Dismisses the menu when `window`, which it popped up for, loses the focus.
    pub fn dismiss_with(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.subscriptions
            .push(cx.observe_window_activation(window, |view, window, cx| {
                if !window.is_window_active() {
                    view.dismiss(window, cx);
                }
            }));
    }

    /// The window of the innermost open submenu, or the menu's own.
    pub fn innermost_window(&self, cx: &App) -> Option<AnyWindowHandle> {
        match &self.submenu {
            Some((_, view, _)) => view.read(cx).innermost_window(cx),
            None => self.own_window,
        }
    }

    /// Whether `window` is the menu's or one of its submenus'.
    pub fn owns_window(&self, window: AnyWindowHandle, cx: &App) -> bool {
        self.own_window == Some(window)
            || self
                .submenu
                .as_ref()
                .is_some_and(|(_, view, _)| view.read(cx).owns_window(window, cx))
    }

    /// The innermost menu window at a screen position.
    fn window_at(
        &self,
        position: Point<Pixels>,
        cx: &App,
    ) -> Option<(AnyWindowHandle, Bounds<Pixels>)> {
        let inner = self
            .submenu
            .as_ref()
            .and_then(|(_, view, _)| view.read(cx).window_at(position, cx));
        inner.or_else(|| {
            let bounds = self.screen.filter(|b| b.contains(&position))?;
            Some((self.own_window?, bounds))
        })
    }

    /// The pointer is grabbed by the outermost menu's window, which hands its events on to the
    /// submenu under it and closes on a press outside every menu. Returns whether `event` is
    /// not this window's own.
    fn route(
        &mut self,
        position: Point<Pixels>,
        event: impl FnOnce(Point<Pixels>) -> PlatformInput,
        press: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> bool {
        let Some(screen) = self.screen.filter(|_| self.prefix.is_empty()) else {
            return false;
        };
        let on_screen = screen.origin + position;
        match self.window_at(on_screen, cx) {
            Some((handle, _)) if Some(handle) == self.own_window => false,
            Some((handle, bounds)) => {
                let event = event(on_screen - bounds.origin);
                cx.defer(move |cx| {
                    handle
                        .update(cx, |_, window, cx| {
                            window.dispatch_event(event, cx);
                        })
                        .ok();
                });
                true
            }
            None => {
                if press {
                    self.dismiss(window, cx);
                }
                true
            }
        }
    }

    fn route_pointer(view: &Entity<MenuView>, window: &mut Window) {
        macro_rules! route {
            ($event:ident, $variant:ident, $press:expr) => {
                let view = view.downgrade();
                window.on_mouse_event(move |event: &$event, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    let routed = view.update(cx, |view, cx| {
                        let at = |position| {
                            PlatformInput::$variant($event {
                                position,
                                ..event.clone()
                            })
                        };
                        view.route(event.position, at, $press, window, cx)
                    });
                    if routed.unwrap_or(false) {
                        cx.stop_propagation();
                    }
                });
            };
        }
        route!(MouseDownEvent, MouseDown, true);
        route!(MouseUpEvent, MouseUp, false);
        route!(MouseMoveEvent, MouseMove, false);
        route!(ScrollEvent, Scroll, false);
    }

    fn arm(&mut self, position: Point<Pixels>) {
        let first = *self.first_move.get_or_insert(position);
        let moved = position - first;
        if moved.x.abs().max(moved.y.abs()) >= px(ARM_DISTANCE) {
            self.armed = true;
        }
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
        let path = self.provisional.map(|index| vec![index]);
        self.finish(path, window, cx);
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
        let placement = match self.host {
            Host::InWindow => None,
            Host::PopUpWindow => self.screen.map(|screen| {
                let row = self.row_bounds(index);
                let anchor = Bounds::new(screen.origin + row.origin, row.size);
                Placement::Screen(anchor, self.monitor)
            }),
            Host::AnchoredPopup => self
                .own_window
                .map(|own| Placement::Anchored(own, self.row_bounds(index))),
        };
        match placement {
            None => {
                let view =
                    cx.new(|cx| MenuView::new(submenu, prefix, Host::InWindow, on_close, cx));
                if focus {
                    view.read(cx).focus.clone().focus(window, cx);
                }
                self.submenu = Some((index, view, None));
            }
            Some(placement) => {
                if let Some((handle, view)) = Self::open_placed(
                    submenu,
                    prefix,
                    placement,
                    Side::Beside,
                    None,
                    window.scale_factor(),
                    cx,
                    on_close,
                ) {
                    view.update(cx, |view, _| view.armed = true);
                    self.submenu = Some((index, view, Some(handle)));
                }
            }
        }
        cx.notify();
    }

    /// The child of the menu's body that shows `index`.
    fn child(&self, index: usize) -> usize {
        visible_items(&self.menu)
            .take_while(|(i, _)| *i < index)
            .count()
    }

    fn scroll_to(&self, index: usize) {
        self.scroll.scroll_to_item(self.child(index));
    }

    /// The row's box in the menu's window, as wide as the menu.
    fn row_bounds(&self, index: usize) -> Bounds<Pixels> {
        let body = self.scroll.bounds();
        let row = self.scroll.bounds_for_item(self.child(index));
        let row = row.unwrap_or_default();
        Bounds::new(
            point(body.left(), row.top() + self.scroll.offset().y),
            size(body.size.width, row.size.height),
        )
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

    fn candidates(&self) -> Vec<usize> {
        visible_items(&self.menu)
            .filter_map(|(i, item)| match item {
                PopupItem::Entry(entry) if selectable(entry) => Some(i),
                _ => None,
            })
            .collect()
    }

    fn highlight(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        self.highlighted = Some(index);
        self.provisional = self.list.as_ref().map(|_| index);
        self.scroll_to(index);
        cx.notify();
    }

    fn step(&mut self, forward: bool, cx: &mut Context<'_, Self>) {
        let candidates = self.candidates();
        if candidates.is_empty() {
            return;
        }
        let position = self
            .highlighted
            .and_then(|h| candidates.iter().position(|&c| c == h));
        let last = candidates.len() - 1;
        let next = match (position, forward) {
            (None, true) => 0,
            (None, false) => last,
            (Some(p), true) if self.list.is_some() => (p + 1).min(last),
            (Some(p), false) if self.list.is_some() => p.saturating_sub(1),
            (Some(p), true) => (p + 1) % candidates.len(),
            (Some(p), false) => (p + last) % candidates.len(),
        };
        self.highlight(candidates[next], cx);
    }

    fn step_to_end(&mut self, last: bool, cx: &mut Context<'_, Self>) {
        let candidates = self.candidates();
        let end = if last {
            candidates.last()
        } else {
            candidates.first()
        };
        if let Some(&index) = end {
            self.highlight(index, cx);
        }
    }

    /// `ListBoxSelectType::NextSelectableOptionPageAway`: the last selectable option among the next
    /// page of rows, and past them only where the page ends on none.
    fn page(&mut self, forward: bool, cx: &mut Context<'_, Self>) {
        let candidates = self.candidates();
        let mut rows: Vec<usize> = visible_items(&self.menu).map(|(i, _)| i).collect();
        let page = rows.len().min(LIST_ROWS).saturating_sub(1);
        if !forward {
            rows.reverse();
        }
        let start = self
            .highlighted
            .and_then(|h| rows.iter().position(|&row| row == h))
            .map_or(0, |start| start + 1);
        let mut found = None;
        for (at, row) in rows.into_iter().skip(start).enumerate() {
            if candidates.contains(&row) {
                found = Some(row);
                if at + 1 >= page {
                    break;
                }
            }
        }
        if let Some(index) = found {
            self.highlight(index, cx);
        }
    }

    fn typing(&self) -> bool {
        self.typed_at
            .is_some_and(|at| at.elapsed() <= TYPE_AHEAD_TIMEOUT)
    }

    /// Typing over a select list looks for the next entry that starts with the typed characters,
    /// or cycles through those that start with one repeated character.
    fn find_typed(&mut self, typed: &str, cx: &mut Context<'_, Self>) {
        if !self.typing() {
            self.typed.clear();
        }
        self.typed_at = Some(Instant::now());
        self.typed.push_str(&typed.to_lowercase());
        let mut chars = self.typed.chars();
        let repeated = chars
            .next()
            .filter(|first| chars.all(|c| c == *first))
            .map(String::from);
        let candidates = self.candidates();
        let start = self
            .highlighted
            .and_then(|h| candidates.iter().position(|&c| c == h))
            .map_or(0, |p| p + usize::from(repeated.is_some()));
        let prefix = repeated.unwrap_or_else(|| self.typed.clone());
        let found = (0..candidates.len())
            .map(|i| candidates[(start + i) % candidates.len()])
            .find(|&index| {
                let label = self
                    .entry(index)
                    .map(|e| e.label.trim_start().to_lowercase());
                label.is_some_and(|label| label.starts_with(&prefix))
            });
        if let Some(index) = found {
            self.highlight(index, cx);
        }
    }

    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<'_, Self>) {
        let list = self.list.is_some();
        let alt = event.keystroke.modifiers.alt;
        let key = event.keystroke.key.as_str();
        match key {
            "down" | "up" if list && alt => self.dismiss(window, cx),
            "down" => self.step(true, cx),
            "up" => self.step(false, cx),
            "pageup" if list => self.page(false, cx),
            "pagedown" if list => self.page(true, cx),
            "home" | "pageup" => self.step_to_end(false, cx),
            "end" | "pagedown" => self.step_to_end(true, cx),
            "space" if list => {
                if self.typing() {
                    self.find_typed(" ", cx);
                }
            }
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
            "escape" | "tab" => self.dismiss(window, cx),
            _ => {
                if list && let Some(typed) = event.keystroke.key_char.as_deref() {
                    self.find_typed(typed, cx);
                }
            }
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
        let group = self
            .list
            .as_ref()
            .is_some_and(|list| list.groups.contains(&index));
        let text = if group { theme.text } else { text };
        let style = if group {
            TextStyle::from_css([("font", GROUP_FONT), ("white-space", "pre")]).expect("menu font")
        } else {
            label_style()
        };
        let label = platform::native_menu_label(&entry.label);
        let mut row = div()
            .id(("menu-row", index))
            .h(px(ROW_HEIGHT))
            .mx(px(PADDING))
            .rounded(px(4.0))
            .flex()
            .items_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _, _| view.armed = true),
            )
            .on_mouse_move(
                cx.listener(move |view, event: &MouseMoveEvent, window, cx| {
                    view.arm(event.position);
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
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    if view.armed {
                        view.activate(index, window, cx);
                    }
                }),
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
            .child(div().flex_1().child(Text::new(label, style).color(text)))
            .children(entry.shortcut.clone().map(|shortcut| {
                let color = if highlighted {
                    0xffffff
                } else {
                    appearance.pick(0x777777, 0xaaaaaa)
                };
                div()
                    .flex_none()
                    .child(Text::new(shortcut, label_style()).color(color))
            }))
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
        let own_window = self.host != Host::InWindow;
        let grabbed = self.screen.is_some() && self.prefix.is_empty();
        let view = cx.entity();
        let mut body = div()
            .id("menu")
            .track_focus(&self.focus)
            .key_context(CONTEXT)
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down_out(cx.listener(|view, _: &MouseDownEvent, window, cx| {
                if view.host == Host::InWindow && view.submenu.is_none() {
                    view.dismiss(window, cx);
                }
            }))
            .relative()
            .py(px(PADDING - BORDER))
            .min_w(px(MIN_WIDTH))
            .track_scroll(&self.scroll)
            .bg(rgb(theme.background))
            .border(px(BORDER))
            .border_color(rgb(appearance.pick(0xc8c8c8, 0x4a4a4a)))
            .rounded(px(6.0))
            .shadow_md();
        if own_window {
            body = body.size_full().overflow_y_scroll();
        }
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
                let row = self.row_bounds(*index);
                Some(
                    deferred(
                        anchored()
                            .position(row.top_right() - point(px(SUBMENU_OVERLAP), px(PADDING)))
                            .snap_to_window()
                            .child(view.clone()),
                    )
                    .with_priority(1),
                )
            }
            _ => None,
        };
        let root = if own_window { div().size_full() } else { div() };
        let pointer = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                if grabbed {
                    Self::route_pointer(&view, window);
                }
            },
        );
        root.child(pointer.absolute().size_full())
            .child(body)
            .children(submenu)
    }
}

/// A window's contents, in which a press closes the menu open for the window and does nothing
/// else, as under Chromium's menus. A Wayland popup's grab leaves the events of the client's other
/// surfaces to them.
#[cfg(target_os = "linux")]
pub(crate) struct MenuGuard<V> {
    view: Entity<V>,
}

#[cfg(target_os = "linux")]
impl<V> MenuGuard<V> {
    pub(crate) fn new(view: Entity<V>) -> Self {
        MenuGuard { view }
    }
}

#[cfg(target_os = "linux")]
impl<V: Render> Render for MenuGuard<V> {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let presses = canvas(
            |_, _, _| (),
            |_, _, window, _| {
                window.on_mouse_event(|_: &MouseDownEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && platform::dismiss_popup(window) {
                        cx.stop_propagation();
                    }
                });
            },
        );
        div()
            .size_full()
            .child(presses.absolute())
            .child(self.view.clone())
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

/// The keystroke GPUI reports on Linux for `keystroke` typed on a US layout: a shifted symbol is
/// that symbol without the shift.
fn shifted_symbol(mut keystroke: Keystroke) -> Keystroke {
    const PLAIN: &str = "`1234567890-=[]\\;',./";
    const SHIFTED: &str = "~!@#$%^&*()_+{}|:\"<>?";
    if keystroke.modifiers.shift
        && let Some(i) = PLAIN.find(&keystroke.key)
        && keystroke.key.len() == 1
    {
        keystroke.modifiers.shift = false;
        keystroke.key = SHIFTED[i..=i].to_owned();
    }
    keystroke
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
            let keystroke = Keystroke::parse_with_os(&keystroke, current_os()).ok()?;
            let keystroke = if cfg!(target_os = "macos") {
                keystroke
            } else {
                shifted_symbol(keystroke)
            };
            Some((keystroke, path))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shifted_symbols_lose_the_shift() {
        for (accelerator, typed) in [
            ("ctrl-shift-=", "ctrl-+"),
            ("ctrl-shift-/", "ctrl-?"),
            ("ctrl-shift-o", "ctrl-shift-o"),
            ("ctrl-shift-left", "ctrl-shift-left"),
            ("ctrl-=", "ctrl-="),
        ] {
            let parse = |k| Keystroke::parse_with_os(k, OperatingSystem::Linux).expect("keystroke");
            assert_eq!(shifted_symbol(parse(accelerator)), parse(typed));
        }
    }

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    #[test]
    fn menus_stay_on_the_screen() {
        let screen = bounds(0.0, 0.0, 1000.0, 800.0);
        let menu = size(px(200.0), px(300.0));
        let at = |anchor, side| place(anchor, menu, screen, side);
        let point = |x, y| bounds(x, y, 0.0, 0.0);
        assert_eq!(
            at(point(50.0, 60.0), Side::Below),
            bounds(50.0, 60.0, 200.0, 300.0)
        );
        assert_eq!(
            at(point(900.0, 60.0), Side::Below),
            bounds(800.0, 60.0, 200.0, 300.0)
        );
        assert_eq!(
            at(point(50.0, 700.0), Side::Below),
            bounds(50.0, 400.0, 200.0, 300.0)
        );
        let row = bounds(50.0, 100.0, 120.0, ROW_HEIGHT);
        assert_eq!(
            at(row, Side::Beside),
            bounds(170.0 - SUBMENU_OVERLAP, 100.0 - PADDING, 200.0, 300.0)
        );
        let row = bounds(850.0, 100.0, 120.0, ROW_HEIGHT);
        assert_eq!(
            at(row, Side::Beside),
            bounds(650.0 + SUBMENU_OVERLAP, 100.0 - PADDING, 200.0, 300.0)
        );
    }
}
