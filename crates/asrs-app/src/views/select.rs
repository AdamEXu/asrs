//! [`SelectModel`] over `asrs_tabs::form::Select`: the value semantics the views' selects use.

use asrs_tabs::form::{self, SelectItem as FormItem, TypeAhead};
use asrs_ui::input::{SelectItem, SelectModel};

/// A `<select>` element's state: its options, the selection and the type-ahead session.
#[derive(Clone, Debug, Default)]
pub struct FormSelect {
    select: form::Select,
    type_ahead: TypeAhead,
    hidden: Vec<bool>,
}

impl FormSelect {
    pub fn new(select: form::Select) -> FormSelect {
        FormSelect {
            select,
            type_ahead: TypeAhead::default(),
            hidden: Vec::new(),
        }
    }

    pub fn select(&self) -> &form::Select {
        &self.select
    }

    /// Script access (`value`, `selectedIndex`, adding and removing options), which fires no
    /// events.
    pub fn select_mut(&mut self) -> &mut form::Select {
        &mut self.select
    }

    pub fn hidden(&self) -> &[bool] {
        &self.hidden
    }

    /// `display: none` per list item, which hides it from the popup.
    pub fn set_hidden(&mut self, hidden: Vec<bool>) {
        self.hidden = hidden;
    }
}

impl SelectModel for FormSelect {
    fn items(&self) -> Vec<SelectItem> {
        self.select
            .items()
            .iter()
            .enumerate()
            .map(|(i, item)| SelectItem {
                label: self.select.item_text(i).to_string_lossy(),
                group: matches!(item, FormItem::Group(_)),
                enabled: !self.select.item_disabled(i),
                hidden: self.hidden.get(i).copied().unwrap_or(false),
            })
            .collect()
    }

    fn selected_item(&self) -> Option<usize> {
        self.select.selected_list_index()
    }

    fn shown_text(&self) -> String {
        self.select.shown_text().to_string_lossy()
    }

    fn select_by_popup(&mut self, item: usize) -> bool {
        self.select.select_by_popup(item)
    }

    fn type_ahead(&mut self, char_code: u16, time: f64) -> bool {
        self.select
            .type_ahead(&mut self.type_ahead, char_code, time)
    }

    fn type_ahead_active(&self, time: f64) -> bool {
        self.type_ahead.has_active_session(time)
    }

    fn end_type_ahead(&mut self) {
        self.type_ahead.reset();
    }
}
