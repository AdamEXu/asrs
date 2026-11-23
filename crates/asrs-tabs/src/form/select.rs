use asrs_js::collation;
use asrs_js::string::JsString;

use super::HTML_SPACES;

/// An `<option>`: its `value` attribute, if any, its text content and its `disabled` attribute.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectOption {
    value: Option<JsString>,
    text_content: JsString,
    disabled: bool,
}

impl SelectOption {
    /// `<option value="...">text</option>` from markup.
    pub fn new(value: Option<&str>, text: &str) -> SelectOption {
        SelectOption {
            value: value.map(JsString::from),
            text_content: JsString::from(text),
            disabled: false,
        }
    }

    /// `document.createElement("option")` with `innerText = text`: each line break (CR LF, CR
    /// or LF) becomes a `<br>`, which adds nothing to the text content.
    pub fn from_inner_text(text: &JsString) -> SelectOption {
        SelectOption {
            value: None,
            text_content: JsString::from_units(
                text.units()
                    .iter()
                    .copied()
                    .filter(|&u| u != 0x0A && u != 0x0D)
                    .collect(),
            ),
            disabled: false,
        }
    }

    /// `option.value = value`, which sets the attribute.
    pub fn with_value(mut self, value: &JsString) -> SelectOption {
        self.value = Some(value.clone());
        self
    }

    /// The `disabled` attribute.
    pub fn disabled(mut self, disabled: bool) -> SelectOption {
        self.disabled = disabled;
        self
    }

    /// `value`: the attribute, else the text.
    pub fn value(&self) -> JsString {
        self.value.clone().unwrap_or_else(|| self.text())
    }

    /// `text` (and `label`, as no option has a `label` attribute): the text content with HTML
    /// whitespace stripped and each run of it collapsed to one space.
    pub fn text(&self) -> JsString {
        let mut out: Vec<u16> = Vec::new();
        let mut pending_space = false;
        for &u in self.text_content.units() {
            if HTML_SPACES.contains(&u) {
                pending_space = !out.is_empty();
            } else {
                if std::mem::take(&mut pending_space) {
                    out.push(0x20);
                }
                out.push(u);
            }
        }
        JsString::from_units(out)
    }

    /// `textContent`, which is also what `innerText` reads while the option is not rendered.
    pub fn text_content(&self) -> &JsString {
        &self.text_content
    }

    /// The `disabled` IDL attribute: the option's own attribute only.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// An `<optgroup label="...">`.
#[derive(Clone, Debug, PartialEq)]
pub struct OptGroup {
    label: JsString,
    disabled: bool,
}

impl OptGroup {
    pub fn new(label: &str) -> OptGroup {
        OptGroup {
            label: JsString::from(label),
            disabled: false,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> OptGroup {
        self.disabled = disabled;
        self
    }

    /// `GroupLabelText()`: the `label` attribute.
    pub fn label(&self) -> &JsString {
        &self.label
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// One entry of `HTMLSelectElement::GetListItems()`: an optgroup, or an option that is a child
/// of the select or of the optgroup before it.
#[derive(Clone, Debug, PartialEq)]
pub enum SelectItem {
    Group(OptGroup),
    Option { option: SelectOption, grouped: bool },
}

/// A drop-down `<select>`: its list items and the selected option. Inserting or removing
/// options runs the selectedness algorithm, which selects the first option that is not disabled
/// when none is selected; assigning `value` or `selectedIndex` does not, so a value no option
/// has leaves nothing selected. A disabled option stays selectable by script.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Select {
    items: Vec<SelectItem>,
    selected: Option<usize>,
}

impl Select {
    /// An empty `<select>`: `selectedIndex === -1`.
    pub fn new() -> Select {
        Select::default()
    }

    /// Options in markup order.
    pub fn with_options(options: Vec<SelectOption>) -> Select {
        Select::with_items(
            options
                .into_iter()
                .map(|option| SelectItem::Option {
                    option,
                    grouped: false,
                })
                .collect(),
        )
    }

    /// `<option value="v">` per value, all with the same text as their value.
    pub fn with_values(values: &[&str]) -> Select {
        Select::with_options(
            values
                .iter()
                .map(|v| SelectOption::new(Some(v), v))
                .collect(),
        )
    }

    /// List items in markup order, as the parser inserts them.
    pub fn with_items(items: Vec<SelectItem>) -> Select {
        let mut select = Select::new();
        for item in items {
            select.items.push(item);
            select.inserted(select.items.len() - 1);
        }
        select
    }

    pub fn items(&self) -> &[SelectItem] {
        &self.items
    }

    fn option_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, SelectItem::Option { .. }))
            .map(|(i, _)| i)
    }

    /// The options, in `select.options` order.
    pub fn options(&self) -> impl Iterator<Item = &SelectOption> {
        self.items.iter().filter_map(|item| match item {
            SelectItem::Option { option, .. } => Some(option),
            SelectItem::Group(_) => None,
        })
    }

    /// `select.options[index]`.
    pub fn option(&self, index: usize) -> Option<&SelectOption> {
        self.options().nth(index)
    }

    /// `select.length`.
    pub fn length(&self) -> usize {
        self.options().count()
    }

    fn group_of(&self, item: usize) -> Option<&OptGroup> {
        if !matches!(
            self.items.get(item),
            Some(SelectItem::Option { grouped: true, .. })
        ) {
            return None;
        }
        self.items[..item].iter().rev().find_map(|i| match i {
            SelectItem::Group(group) => Some(group),
            SelectItem::Option { .. } => None,
        })
    }

    /// `IsDisabledFormControl()` of the list item: its own attribute or its optgroup's.
    pub fn item_disabled(&self, item: usize) -> bool {
        match self.items.get(item) {
            Some(SelectItem::Option { option, .. }) => {
                option.disabled || self.group_of(item).is_some_and(|g| g.disabled)
            }
            Some(SelectItem::Group(group)) => group.disabled,
            None => false,
        }
    }

    /// `ItemText()`: an optgroup's label, or an option's label with four spaces before it
    /// inside an optgroup (`TextIndentedToRespectGroupLabel`).
    pub fn item_text(&self, item: usize) -> JsString {
        match self.items.get(item) {
            Some(SelectItem::Group(group)) => group.label.clone(),
            Some(SelectItem::Option { option, grouped }) => {
                let label = option.text();
                if *grouped {
                    let mut units: Vec<u16> = "    ".encode_utf16().collect();
                    units.extend_from_slice(label.units());
                    JsString::from_units(units)
                } else {
                    label
                }
            }
            None => JsString::default(),
        }
    }

    /// The text a closed select shows: the selected option's label.
    pub fn shown_text(&self) -> JsString {
        match self.selected.and_then(|i| self.items.get(i)) {
            Some(SelectItem::Option { option, .. }) => option.text(),
            _ => JsString::default(),
        }
    }

    /// `value`: the selected option's, or `""` when nothing is selected.
    pub fn value(&self) -> JsString {
        match self.selected.and_then(|i| self.items.get(i)) {
            Some(SelectItem::Option { option, .. }) => option.value(),
            _ => JsString::default(),
        }
    }

    /// `selectedIndex`.
    pub fn selected_index(&self) -> i32 {
        self.selected.map_or(-1, |s| {
            self.option_indices().take_while(|&i| i < s).count() as i32
        })
    }

    /// `SelectedListIndex()`: the selected option's index among the list items.
    pub fn selected_list_index(&self) -> Option<usize> {
        self.selected
    }

    /// `select.value = value`: the first option with that value, else nothing.
    pub fn set_value(&mut self, value: &JsString) {
        let found = self.option_indices().find(|&i| match &self.items[i] {
            SelectItem::Option { option, .. } => option.value() == *value,
            SelectItem::Group(_) => false,
        });
        self.selected = found;
    }

    /// `select.selectedIndex = index`; out of range selects nothing.
    pub fn set_selected_index(&mut self, index: i32) {
        let found = usize::try_from(index)
            .ok()
            .and_then(|i| self.option_indices().nth(i));
        self.selected = found;
    }

    /// `option.disabled = disabled` for `select.options[index]`, which leaves the selection.
    pub fn set_option_disabled(&mut self, index: usize, disabled: bool) {
        let item = self.option_indices().nth(index);
        if let Some(i) = item
            && let SelectItem::Option { option, .. } = &mut self.items[i]
        {
            option.disabled = disabled;
        }
    }

    fn group_item(&self, group: usize) -> Option<usize> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, item)| matches!(item, SelectItem::Group(_)))
            .map(|(i, _)| i)
            .nth(group)
    }

    /// `optgroup.disabled = disabled` for the `group`th optgroup, which leaves the selection.
    pub fn set_group_disabled(&mut self, group: usize, disabled: bool) {
        if let Some(i) = self.group_item(group)
            && let SelectItem::Group(g) = &mut self.items[i]
        {
            g.disabled = disabled;
        }
    }

    /// `select.appendChild(option)`.
    pub fn append(&mut self, option: SelectOption) {
        self.items.push(SelectItem::Option {
            option,
            grouped: false,
        });
        self.inserted(self.items.len() - 1);
    }

    /// `select.appendChild(optgroup)` for an empty optgroup.
    pub fn append_group(&mut self, group: OptGroup) {
        self.items.push(SelectItem::Group(group));
    }

    fn group_end(&self, group_item: usize) -> usize {
        group_item
            + 1
            + self.items[group_item + 1..]
                .iter()
                .take_while(|item| matches!(item, SelectItem::Option { grouped: true, .. }))
                .count()
    }

    /// `optgroup.appendChild(option)` on the `group`th optgroup; nothing without one.
    pub fn append_to_group(&mut self, group: usize, option: SelectOption) {
        let Some(g) = self.group_item(group) else {
            return;
        };
        let at = self.group_end(g);
        self.items.insert(
            at,
            SelectItem::Option {
                option,
                grouped: true,
            },
        );
        if let Some(s) = self.selected.as_mut()
            && *s >= at
        {
            *s += 1;
        }
        self.inserted(at);
    }

    /// `while (optgroup.firstChild) optgroup.removeChild(optgroup.firstChild)` on the
    /// `group`th optgroup.
    pub fn clear_group(&mut self, group: usize) {
        while let Some(g) = self.group_item(group) {
            if self.group_end(g) == g + 1 {
                break;
            }
            self.remove_item(g + 1);
        }
    }

    /// `select.removeChild(select.children[index])`: an option, or an optgroup with its
    /// options; out of range removes nothing.
    pub fn remove(&mut self, index: usize) {
        let Some(item) = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| !matches!(item, SelectItem::Option { grouped: true, .. }))
            .map(|(i, _)| i)
            .nth(index)
        else {
            return;
        };
        if matches!(self.items[item], SelectItem::Group(_)) {
            let end = self.group_end(item);
            let selected_removed = self.selected.is_some_and(|s| (item..end).contains(&s));
            self.items.drain(item..end);
            self.selected = match self.selected {
                Some(_) if selected_removed => None,
                Some(s) if s >= end => Some(s - (end - item)),
                other => other,
            };
            if end > item + 1 {
                self.removed(selected_removed);
            }
        } else {
            self.remove_item(item);
        }
    }

    fn remove_item(&mut self, item: usize) {
        self.items.remove(item);
        let was_selected = self.selected == Some(item);
        self.selected = match self.selected {
            Some(s) if s == item => None,
            Some(s) if s > item => Some(s - 1),
            other => other,
        };
        self.removed(was_selected);
    }

    /// Removing every option, then appending each of `options`.
    pub fn replace_options(&mut self, options: Vec<SelectOption>) {
        while !self.items.is_empty() {
            self.remove(self.items.len() - 1);
        }
        for option in options {
            self.append(option);
        }
    }

    fn first_enabled(&self) -> Option<usize> {
        self.option_indices().find(|&i| !self.item_disabled(i))
    }

    /// `OptionInserted` for an option that is not selected.
    fn inserted(&mut self, item: usize) {
        if matches!(self.items[item], SelectItem::Option { .. })
            && self.selected.is_none()
            && !self.item_disabled(item)
        {
            self.selected = self.first_enabled();
        }
    }

    /// `OptionRemoved`: the selected option's removal, or any removal while nothing is
    /// selected, resets to the first enabled option.
    fn removed(&mut self, was_selected: bool) {
        if was_selected || self.selected.is_none() {
            self.selected = self.first_enabled();
        }
    }

    /// `SelectOptionByPopup(list_index)`: true when the selection changed, which dispatches
    /// `input` and `change`.
    pub fn select_by_popup(&mut self, list_index: usize) -> bool {
        let option = match self.items.get(list_index) {
            Some(SelectItem::Option { .. }) => Some(list_index),
            _ => None,
        };
        if option == self.selected {
            return false;
        }
        self.selected = option;
        true
    }

    /// An enabled option that `shown` does not leave out (`display: none`).
    fn selectable(&self, item: usize, shown: &impl Fn(usize) -> bool) -> bool {
        matches!(self.items.get(item), Some(SelectItem::Option { .. }))
            && shown(item)
            && !self.item_disabled(item)
    }

    /// `NextValidOption()` from the selected option: the last selectable option among the next
    /// `skip` list items, and past them only to reach one.
    pub fn valid_option(
        &self,
        forward: bool,
        skip: usize,
        shown: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        let mut index = self.selected.map_or(-1, |i| i as i64);
        let mut skip = skip as i64;
        let mut found = None;
        loop {
            index += if forward { 1 } else { -1 };
            if index < 0 || index >= self.items.len() as i64 {
                return found;
            }
            skip -= 1;
            if !self.selectable(index as usize, &shown) {
                continue;
            }
            found = Some(index as usize);
            if skip <= 0 {
                return found;
            }
        }
    }

    /// `FirstSelectableOption()` or `LastSelectableOption()`.
    pub fn end_option(&self, last: bool, shown: impl Fn(usize) -> bool) -> Option<usize> {
        let mut items = 0..self.items.len();
        if last {
            items.rev().find(|&i| self.selectable(i, &shown))
        } else {
            items.find(|&i| self.selectable(i, &shown))
        }
    }

    /// `OptionAtIndex()` for type-ahead: an enabled option's label, else nothing to match.
    fn type_ahead_text(&self, item: usize) -> Option<JsString> {
        match &self.items[item] {
            SelectItem::Option { option, .. } if !self.item_disabled(item) => Some(option.text()),
            _ => None,
        }
    }

    /// `TypeAheadFind()` for a printable `keypress` at `time` ms: true when it selected another
    /// option, which dispatches `input` and `change`.
    pub fn type_ahead(&mut self, state: &mut TypeAhead, char_code: u16, time: f64) -> bool {
        let Some(index) = state.handle(self, char_code, time) else {
            return false;
        };
        if self.selected == Some(index) {
            return false;
        }
        self.selected = Some(index);
        true
    }
}

/// Blink's `TypeAhead` with `kMatchPrefix | kCycleFirstChar`, the matcher of a select's
/// keyboard search.
#[derive(Clone, Debug, Default)]
pub struct TypeAhead {
    buffer: Vec<u16>,
    last_type_time: Option<f64>,
    repeating_char: u16,
}

const TYPE_AHEAD_TIMEOUT_MS: f64 = 1000.0;

impl TypeAhead {
    /// `HasActiveSession()`: a key at `time` ms continues the current search.
    pub fn has_active_session(&self, time: f64) -> bool {
        self.last_type_time
            .is_some_and(|last| time - last <= TYPE_AHEAD_TIMEOUT_MS)
    }

    /// `ResetSession()`, which the select runs on blur.
    pub fn reset(&mut self) {
        self.last_type_time = None;
        self.buffer.clear();
    }

    fn handle(&mut self, select: &Select, char_code: u16, time: f64) -> Option<usize> {
        if let Some(last) = self.last_type_time {
            if time < last {
                return None;
            }
            if time - last > TYPE_AHEAD_TIMEOUT_MS {
                self.buffer.clear();
            }
        }
        self.last_type_time = Some(time);
        self.buffer.push(char_code);

        let count = select.items.len();
        if count < 1 {
            return None;
        }
        let mut start_offset = 1;
        let prefix: Vec<u16> = if char_code == self.repeating_char {
            vec![char_code]
        } else {
            if self.buffer.len() > 1 {
                self.repeating_char = 0;
                start_offset = 0;
            } else {
                self.repeating_char = char_code;
            }
            self.buffer.clone()
        };
        let selected = select.selected.unwrap_or(0);
        let mut index = (selected + start_offset) % count;
        for _ in 0..count {
            if let Some(text) = select.type_ahead_text(index)
                && starts_with_ignoring_case_and_accents(strip_leading_space(text.units()), &prefix)
            {
                return Some(index);
            }
            index = (index + 1) % count;
        }
        None
    }
}

/// `StripLeadingWhiteSpace` in `type_ahead.cc`: no-break spaces and `IsSpaceOrNewline`.
fn strip_leading_space(units: &[u16]) -> &[u16] {
    let start = units
        .iter()
        .position(|&u| u != 0xA0 && !is_space_or_newline(u))
        .unwrap_or(units.len());
    &units[start..]
}

/// WTF's `IsSpaceOrNewline`: ASCII whitespace, or bidi class WS above ASCII.
fn is_space_or_newline(u: u16) -> bool {
    if u <= 0x7F {
        return matches!(u, 0x09..=0x0D | 0x20);
    }
    matches!(u, 0x1680 | 0x2000..=0x200A | 0x2028 | 0x205F | 0x3000)
}

/// `StringImpl::StartsWithIgnoringCaseAndAccents`: an ICU string search at primary strength
/// with the `en-US` collator that matches at index 0.
fn starts_with_ignoring_case_and_accents(text: &[u16], prefix: &[u16]) -> bool {
    let mut end = 0;
    while end < text.len() {
        end += if is_lead_surrogate(text[end])
            && text.get(end + 1).is_some_and(|&u| is_trail_surrogate(u))
        {
            2
        } else {
            1
        };
        if collation::primary_equal_utf16(&text[..end], prefix) {
            return true;
        }
    }
    false
}

fn is_lead_surrogate(u: u16) -> bool {
    (0xD800..0xDC00).contains(&u)
}

fn is_trail_surrogate(u: u16) -> bool {
    (0xDC00..0xE000).contains(&u)
}
