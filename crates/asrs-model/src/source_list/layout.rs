//! Where Blink puts a source list's elements (`www/hub.css`, "Source list", and the controller
//! rules that place each list's root), so a view answers [`SourceListDom`] reads with the
//! layout it draws.

use asrs_js::math;

use super::{ItemView, ListButton, Rect};
use crate::tab_type::TabType;

const TITLE_HEIGHT: f64 = 30.0;
const ITEM_HEIGHT: f64 = 30.0;
/// `item.style.height` while the preview line shows.
const PREVIEW_ITEM_HEIGHT: f64 = 50.0;
const BUTTON_SIZE: f64 = 25.0;
/// `button.type`'s `left` (`top` is the same 15 px), `55px` less the child indent.
const TYPE_BUTTON_LEFT: f64 = 15.0;
const CHILD_INDENT: f64 = 25.0;
/// `button.edit` and `button.clear`'s `right`; `button.help` sits 25 px further left.
const CORNER_BUTTON_RIGHT: f64 = 15.0;

/// A `div.source-list` root: its border box and the side borders inside it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ListRoot {
    pub rect: Rect,
    /// `border-left` and `border-right` widths (the line graph's discrete column has 1 px).
    pub border_x: f64,
}

/// Blink's `LayoutUnit` from a computed length: 1/64 px, rounded toward zero.
fn layout_unit(px: f64) -> f64 {
    math::trunc(px * 64.0) / 64.0
}

/// The roots of a controller's lists, in construction order, inside the `div.controls-content`
/// box `controls`, for the tab types whose controls hold source lists.
pub fn controller_roots(tab_type: TabType, controls: Rect) -> Vec<ListRoot> {
    let Rect {
        x,
        y,
        width,
        height,
    } = controls;
    let column = |left: f64, border_x: f64| ListRoot {
        rect: Rect {
            x: x + left,
            y,
            width: layout_unit(width / 3.0),
            height,
        },
        border_x,
    };
    let sources = |min_inset: f64| ListRoot {
        rect: Rect {
            x,
            y,
            width: math::max2(layout_unit(width * 0.6), width - min_inset),
            height,
        },
        border_x: 0.0,
    };
    match tab_type {
        TabType::LineGraph => vec![
            column(0.0, 0.0),
            column(layout_unit(width / 3.0 * 2.0), 0.0),
            column(layout_unit(width / 3.0), 1.0),
        ],
        TabType::Field2d => vec![sources(250.0)],
        TabType::Field3d | TabType::Statistics | TabType::Swerve | TabType::Points => {
            vec![sources(200.0)]
        }
        TabType::Mechanism => vec![ListRoot {
            rect: controls,
            border_x: 0.0,
        }],
        _ => Vec::new(),
    }
}

/// The boxes of one list laid out in `root`, scrolled down by `scroll_top`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListLayout {
    pub root: ListRoot,
    pub scroll_top: f64,
    /// What the list's scrollbar takes of its width.
    pub scrollbar: f64,
    /// Each item's top within the list's content, height and whether it is a child.
    items: Vec<(f64, f64, bool)>,
}

impl ListLayout {
    pub fn new<'a>(
        root: ListRoot,
        items: impl IntoIterator<Item = &'a ItemView>,
        scroll_top: f64,
    ) -> ListLayout {
        let mut top = 0.0;
        let items = items
            .into_iter()
            .map(|item| {
                let height = if item.preview_shown == Some(true) {
                    PREVIEW_ITEM_HEIGHT
                } else {
                    ITEM_HEIGHT
                };
                let placed = (top, height, item.child);
                top += height;
                placed
            })
            .collect();
        ListLayout {
            root,
            scroll_top,
            scrollbar: 0.0,
            items,
        }
    }

    fn displayed(&self) -> bool {
        self.root.rect.width != 0.0 || self.root.rect.height != 0.0
    }

    /// `div.list`: below the 30 px title, inside the side borders.
    pub fn list_rect(&self) -> Rect {
        if !self.displayed() {
            return Rect::default();
        }
        let r = self.root.rect;
        Rect {
            x: r.x + self.root.border_x,
            y: r.y + TITLE_HEIGHT,
            width: r.width - 2.0 * self.root.border_x,
            height: r.height - TITLE_HEIGHT,
        }
    }

    pub fn item_rect(&self, index: usize) -> Rect {
        let Some(&(top, height, _)) = self.items.get(index).filter(|_| self.displayed()) else {
            return Rect::default();
        };
        let list = self.list_rect();
        Rect {
            x: list.x,
            y: list.y + top - self.scroll_top,
            width: list.width - self.taken(),
            height,
        }
    }

    /// What the list's scrollbar takes of the items' width.
    pub fn taken(&self) -> f64 {
        let content = self
            .items
            .last()
            .map_or(0.0, |(top, height, _)| top + height);
        if content > self.list_rect().height {
            self.scrollbar
        } else {
            0.0
        }
    }

    /// `clientWidth`: the item's width snapped at its offset of 0 in the list.
    pub fn item_client_width(&self, index: usize) -> f64 {
        math::floor(self.item_rect(index).width + 0.5)
    }

    /// `border-top`: 1 px between items, none on the first item or a child.
    pub fn item_border_top(&self, index: usize) -> f64 {
        match self.items.get(index) {
            Some((_, _, child)) if index > 0 && !child => 1.0,
            _ => 0.0,
        }
    }

    pub fn button_rect(&self, button: ListButton) -> Rect {
        if !self.displayed() {
            return Rect::default();
        }
        let (x, y) = match button {
            ListButton::Edit | ListButton::Clear => {
                let r = self.root.rect;
                (
                    r.x + r.width - self.root.border_x - CORNER_BUTTON_RIGHT - BUTTON_SIZE / 2.0,
                    r.y + TITLE_HEIGHT / 2.0 - BUTTON_SIZE / 2.0,
                )
            }
            ListButton::Type(index) => {
                let item = self.item_rect(index);
                let child = self.items.get(index).is_some_and(|i| i.2);
                let left = TYPE_BUTTON_LEFT + if child { CHILD_INDENT } else { 0.0 };
                (
                    item.x + left - BUTTON_SIZE / 2.0,
                    item.y + self.item_border_top(index) + ITEM_HEIGHT / 2.0 - BUTTON_SIZE / 2.0,
                )
            }
        };
        Rect {
            x,
            y,
            width: BUTTON_SIZE,
            height: BUTTON_SIZE,
        }
    }
}
