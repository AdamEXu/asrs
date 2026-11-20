//! hub.ts's field dragging: `startDrag`, `dragMove`, `dragEnd` and the `#dragItem` they move.

use asrs_js::string::JsString;

/// The `span.field-text` the sidebar puts inside `#dragItem`.
#[derive(Clone, Debug, PartialEq)]
pub struct DragItem {
    pub text: JsString,
    pub bold: bool,
}

/// The `data` passed to `startDrag` and carried by every `drag-update` event.
#[derive(Clone, Debug, PartialEq)]
pub struct DragPayload {
    pub fields: Vec<JsString>,
    pub children: Vec<JsString>,
}

/// The `detail` of a `drag-update` event.
#[derive(Clone, Debug, PartialEq)]
pub struct DragUpdate {
    pub end: bool,
    pub x: f64,
    pub y: f64,
    pub data: DragPayload,
}

#[derive(Debug, Default)]
pub struct Drag {
    active: bool,
    offset_x: f64,
    offset_y: f64,
    last_x: f64,
    last_y: f64,
    data: Option<DragPayload>,
    item: Option<DragItem>,
    item_hidden: bool,
    item_position: Option<(f64, f64)>,
    updates: Vec<DragUpdate>,
}

impl Drag {
    /// `startDrag`, after the caller has filled `#dragItem` with `item`.
    pub fn start(
        &mut self,
        x: f64,
        y: f64,
        offset_x: f64,
        offset_y: f64,
        item: DragItem,
        data: DragPayload,
    ) {
        self.active = true;
        self.offset_x = offset_x;
        self.offset_y = offset_y;
        self.last_x = x;
        self.last_y = y;
        self.data = Some(data);
        self.item = Some(item);
        self.item_hidden = false;
        self.item_position = Some((x - offset_x, y - offset_y));
    }

    /// `dragMove`, from the window's `mousemove` and `touchmove` listeners.
    pub fn move_to(&mut self, x: f64, y: f64) {
        if !self.active {
            return;
        }
        self.item_position = Some((x - self.offset_x, y - self.offset_y));
        self.last_x = x;
        self.last_y = y;
        self.push_update(false);
    }

    /// `dragEnd`, from the window's `mouseup` and `touchend` listeners.
    pub fn end(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.item_hidden = true;
        self.push_update(true);
    }

    fn push_update(&mut self, end: bool) {
        if let Some(data) = &self.data {
            self.updates.push(DragUpdate {
                end,
                x: self.last_x,
                y: self.last_y,
                data: data.clone(),
            });
        }
    }

    /// `drag-update` events dispatched since the last call.
    pub fn take_updates(&mut self) -> Vec<DragUpdate> {
        std::mem::take(&mut self.updates)
    }

    pub fn active(&self) -> bool {
        self.active
    }

    pub fn item(&self) -> Option<&DragItem> {
        self.item.as_ref()
    }

    pub fn item_hidden(&self) -> bool {
        self.item_hidden
    }

    /// `#dragItem`'s `left` and `top` in px, once a drag has started.
    pub fn item_position(&self) -> Option<(f64, f64)> {
        self.item_position
    }
}
