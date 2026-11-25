//! `src/hub/SourceList.ts` without the DOM: item states, type choice, type memory, reorder and
//! drop rules, availability, colours and preview text. Every layout read upstream makes goes
//! through [`SourceListDom`], in upstream's order; every rendered value the DOM would show is in
//! [`ItemView`].

pub mod configs;
pub mod layout;
mod preview;

use std::fmt;

use asrs_js::json;
use asrs_js::math;
use asrs_js::ops;
use asrs_js::sort::array_sort;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue, ToJs};
use asrs_log::colors::ensure_theme_contrast;
use asrs_log::log::{Log, StructuredType};

use crate::source_list_config::{
    AutoAdvance, PreviewSetting, SourceListConfig, SourceListItemState,
    SourceListOptionValueConfig, SourceListTypeConfig,
};

pub use preview::NumberPreview;

const DRAG_THRESHOLD_PX: f64 = 5.0;
const DOUBLE_CLICK_MS: f64 = 400.0;

pub(crate) fn eq_str(js: &JsString, s: &str) -> bool {
    js.units().iter().copied().eq(s.encode_utf16())
}

/// An exception upstream throws out of a `SourceList` call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SourceListError {
    /// `throw 'Unknown type "<type>"'` from `updateItem`.
    UnknownType(JsString),
    /// Any other error, as `"<name>: <message>"`.
    Thrown(String),
}

impl SourceListError {
    fn type_error(on: &str, property: &str) -> SourceListError {
        SourceListError::Thrown(format!(
            "TypeError: Cannot read properties of {on} (reading '{property}')"
        ))
    }
}

impl fmt::Display for SourceListError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SourceListError::UnknownType(t) => {
                write!(f, "Unknown type \"{}\"", t.to_string_lossy())
            }
            SourceListError::Thrown(text) => f.write_str(text),
        }
    }
}

impl std::error::Error for SourceListError {}

impl From<JsError> for SourceListError {
    fn from(e: JsError) -> Self {
        SourceListError::Thrown(format!("{}: {}", e.name, e.message))
    }
}

pub type Result<T = ()> = std::result::Result<T, SourceListError>;

/// What upstream reads from `window` while a source list works.
pub struct Env<'a> {
    pub log: &'a Log,
    /// `window.typeMemory`, shared by every source list.
    pub type_memory: &'a mut JsValue,
    /// `prefers-color-scheme: dark`.
    pub dark: bool,
    pub dom: &'a mut dyn SourceListDom,
}

/// A button whose `getBoundingClientRect()` a list reads when it is clicked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListButton {
    Edit,
    Clear,
    /// The type button of the item at this index.
    Type(usize),
}

/// The layout reads a list makes, answered by the view that lays it out. `list` is the list's
/// UUID.
pub trait SourceListDom {
    /// `ROOT.getBoundingClientRect()`.
    fn root_rect(&mut self, list: &str) -> Rect;
    /// `LIST.getBoundingClientRect()`.
    fn list_rect(&mut self, list: &str) -> Rect;
    /// `LIST.children[index].getBoundingClientRect()`.
    fn item_rect(&mut self, list: &str, index: usize) -> Rect;
    /// `LIST.children[index].clientWidth`.
    fn item_client_width(&mut self, list: &str, index: usize) -> f64;
    fn button_rect(&mut self, list: &str, button: ListButton) -> Rect;
    /// `clientWidth` of a `div.type-name` showing `text` inside a bare `div.source-list`
    /// appended to `<body>`, which `updateItem` measures for `--type-width`.
    fn type_name_width(&mut self, text: &str) -> f64;
}

/// The reads of lists whose elements are not rendered (`display: none`, or no view at all):
/// every rect and width is zero, as Blink reports for an element without a layout box.
#[derive(Clone, Copy, Debug, Default)]
pub struct Undisplayed;

impl Undisplayed {
    /// A borrow for an `Env` that lives as long as needed; `Undisplayed` is zero-sized, so
    /// leaking it allocates nothing.
    pub fn dom() -> &'static mut Undisplayed {
        Box::leak(Box::new(Undisplayed))
    }
}

impl SourceListDom for Undisplayed {
    fn root_rect(&mut self, _list: &str) -> Rect {
        Rect::default()
    }
    fn list_rect(&mut self, _list: &str) -> Rect {
        Rect::default()
    }
    fn item_rect(&mut self, _list: &str, _index: usize) -> Rect {
        Rect::default()
    }
    fn item_client_width(&mut self, _list: &str, _index: usize) -> f64 {
        0.0
    }
    fn button_rect(&mut self, _list: &str, _button: ListButton) -> Rect {
        Rect::default()
    }
    fn type_name_width(&mut self, _text: &str) -> f64 {
        0.0
    }
}

/// An event a controller's lists receive from the hub, in the order their listeners run.
#[derive(Clone, Copy, Debug)]
pub enum ListInput<'a> {
    /// A `drag-update` of a sidebar field drag.
    FieldDrag {
        fields: &'a [JsString],
        children: &'a [JsString],
        x: f64,
        y: f64,
        end: bool,
    },
    /// A `drag-update` of a list item drag.
    ItemDrag {
        payload: &'a ItemDragPayload,
        x: f64,
        y: f64,
        end: bool,
    },
    /// `source-list-type-response`.
    TypeResponse {
        uuid: &'a str,
        state: &'a SourceListItemState,
    },
    /// `source-list-clear-response`.
    ClearResponse { uuid: &'a str },
    /// One run of every list's `requestAnimationFrame` loop.
    Frame,
    /// Input on the elements of the list `uuid`.
    Pointer { uuid: &'a str, event: ListPointer },
}

/// Input on one list's elements; coordinates are `MouseEvent` client coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ListPointer {
    /// `mousedown` in the item container.
    Down {
        x: f64,
        y: f64,
    },
    /// `mouseup` in the item container.
    Up,
    /// `mousemove` in the item container.
    Move {
        x: f64,
        y: f64,
    },
    ContextMenu {
        index: usize,
        x: f64,
        y: f64,
    },
    TypeClick(usize),
    HideClick(usize),
    /// A click on an item's type name or key at `new Date().getTime()` = `now`.
    NameClick {
        index: usize,
        now: f64,
    },
    RemoveClick(usize),
    ClearClick,
    HelpClick,
    EditClick,
}

/// What a list asks of the hub after an input, besides the main messages it sends.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ListOutput {
    /// `window.startDrag` for an item drag.
    pub drag: Option<ItemDragStart>,
    /// The edit button's rect, for the controller's `editButtonCallback`.
    pub edit: Option<Rect>,
}

/// A `DOMRect`, as `getBoundingClientRect` returns it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn left(&self) -> f64 {
        math::min2(self.x, self.x + self.width)
    }
    pub fn right(&self) -> f64 {
        math::max2(self.x, self.x + self.width)
    }
    pub fn top(&self) -> f64 {
        math::min2(self.y, self.y + self.height)
    }
    pub fn bottom(&self) -> f64 {
        math::max2(self.y, self.y + self.height)
    }
}

/// A CSS length on the drag highlight.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Length {
    Px(f64),
    Percent(f64),
}

impl Length {
    /// The `style` string upstream assigns.
    pub fn css(self) -> String {
        match self {
            Length::Px(v) => format!("{}px", asrs_js::number::to_string(v)),
            Length::Percent(v) => format!("{}%", asrs_js::number::to_string(v)),
        }
    }
}

/// The green drop indicator. `placement` is `(top, height)`, with `left: 0%` and `width: 100%`,
/// and stays as last set while hidden.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DragHighlight {
    pub hidden: bool,
    pub placement: Option<(Length, Length)>,
}

/// What `updateItem` last rendered for an item.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedItem {
    /// The `type-color` of the icon, after `ensureThemeContrast`.
    pub color: String,
    /// `symbols/sourceList/<symbol>.svg`.
    pub symbol_path: String,
    /// The `type-name` text, e.g. `"Robot/Blue:"`, or empty.
    pub type_name: String,
    pub log_key: JsString,
    /// The key span's `innerText`, where `null` is empty.
    pub key_text: JsString,
    /// False draws the key struck through.
    pub available: bool,
    pub visible: bool,
}

impl RenderedItem {
    pub fn hide_icon(&self) -> &'static str {
        if self.visible {
            "symbols/visibility.svg"
        } else {
            "symbols/visibilityoff.svg"
        }
    }

    pub fn hide_title(&self) -> &'static str {
        if self.visible {
            "Hide field"
        } else {
            "Show field"
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemView {
    /// `None` until `updateItem` has succeeded once (the template's blank state).
    pub rendered: Option<RenderedItem>,
    /// Indented child formatting, fixed when the item is created.
    pub child: bool,
    pub parent_highlight: bool,
    /// `None` until the first preview update; then whether the preview line shows (the item is
    /// 50 px tall instead of 30).
    pub preview_shown: Option<bool>,
    /// The preview text last written. It is kept while the preview is hidden.
    pub preview_text: String,
    /// The key container's `--type-width` in px, once set.
    pub type_width: Option<f64>,
}

#[derive(Clone, Debug)]
struct Item {
    id: u64,
    /// Upstream attaches the item's listeners only after its first `updateItem` succeeds.
    interactive: bool,
    created_as_child: bool,
    last_click: f64,
    view: ItemView,
}

/// What a drag of a list item carries (`startDrag`'s data).
#[derive(Clone, Debug, PartialEq)]
pub struct ItemDragPayload {
    pub uuid: String,
    /// Identifies the config the list was built from (`configStr`).
    pub config_str: String,
    pub index: usize,
    pub is_child: bool,
}

/// `window.startDrag(x, y, offsetX, offsetY, data)` for an item drag.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemDragStart {
    pub x: f64,
    pub y: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    /// The dragged item's `clientWidth`, the width of its copy in `#dragItem`.
    pub client_width: f64,
    pub payload: ItemDragPayload,
}

/// The `source-list-type-prompt` message.
#[derive(Clone, Debug, PartialEq)]
pub struct TypePrompt {
    pub uuid: String,
    pub config: SourceListConfig,
    pub state: SourceListItemState,
    /// The type button's rect, or the pointer as a zero-size rect for a right click.
    pub rect: Rect,
}

/// The `source-list-clear-prompt` message.
#[derive(Clone, Debug, PartialEq)]
pub struct ClearPrompt {
    pub uuid: String,
    /// The clear button's rect.
    pub rect: Rect,
}

#[derive(Clone, Debug)]
struct PendingTypePrompt {
    item: u64,
    original_type: JsString,
    is_child: bool,
}

pub struct SourceList {
    uuid: String,
    config: SourceListConfig,
    config_str: String,
    has_edit_button: bool,
    title: String,
    state: Vec<SourceListItemState>,
    items: Vec<Item>,
    next_item: u64,
    independent_allowed_types: Vec<String>,
    parent_keys: Vec<(String, String)>,
    stopped: bool,
    periodic_alive: bool,
    last_is_dark: Option<bool>,
    mouse_down: Option<(f64, f64)>,
    refresh_last_fields: Vec<JsString>,
    refresh_last_struct_types: JsObject<StructuredType>,
    hand_icon_shown: bool,
    highlight: DragHighlight,
    type_prompt: Option<PendingTypePrompt>,
    clear_prompt: bool,
}

impl SourceList {
    /// `new SourceList(root, config, suppliers, editButtonCallback?, getNumberPreview?)`.
    /// `uuid` is the list's `createUUID()`; supplemental states and the number preview are
    /// passed to the calls that use them.
    pub fn new(config: SourceListConfig, uuid: String, has_edit_button: bool) -> SourceList {
        let config_str = json::stringify(&config.to_js())
            .ok()
            .flatten()
            .unwrap_or_default();
        let mut independent_allowed_types: Vec<String> = Vec::new();
        let mut parent_keys = Vec::new();
        for type_config in &config.types {
            if type_config.child_of.is_none() {
                for source in &type_config.source_types {
                    if !independent_allowed_types.contains(source) {
                        independent_allowed_types.push(source.clone());
                    }
                }
            }
            if let Some(parent_key) = &type_config.parent_key {
                match parent_keys.iter_mut().find(|(k, _)| k == &type_config.key) {
                    Some(entry) => entry.1 = parent_key.clone(),
                    None => parent_keys.push((type_config.key.clone(), parent_key.clone())),
                }
            }
        }
        SourceList {
            uuid,
            title: config.title.clone(),
            config,
            config_str,
            has_edit_button,
            state: Vec::new(),
            items: Vec::new(),
            next_item: 0,
            independent_allowed_types,
            parent_keys,
            stopped: false,
            periodic_alive: true,
            last_is_dark: None,
            mouse_down: None,
            refresh_last_fields: Vec::new(),
            refresh_last_struct_types: JsObject::new(),
            hand_icon_shown: true,
            highlight: DragHighlight {
                hidden: true,
                placement: None,
            },
            type_prompt: None,
            clear_prompt: false,
        }
    }

    pub fn uuid(&self) -> &str {
        &self.uuid
    }

    /// The config as edited by [`SourceList::set_option_values`] (what the type prompt and the
    /// help window receive).
    pub fn config(&self) -> &SourceListConfig {
        &self.config
    }

    pub fn config_str(&self) -> &str {
        &self.config_str
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn set_title(&mut self, title: &str) {
        self.title = title.to_owned();
    }

    /// Edit button shown (a settings callback was given); otherwise the clear and help buttons.
    pub fn has_edit_button(&self) -> bool {
        self.has_edit_button
    }

    pub fn items(&self) -> impl ExactSizeIterator<Item = &ItemView> + '_ {
        self.items.iter().map(|item| &item.view)
    }

    /// The empty-list hand icon (fades in to opacity 0.15 after a second).
    pub fn hand_icon_shown(&self) -> bool {
        self.hand_icon_shown
    }

    pub fn drag_highlight(&self) -> DragHighlight {
        self.highlight
    }

    pub fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// `getState()`.
    pub fn state(&self) -> &[SourceListItemState] {
        &self.state
    }

    /// `getState(true)`: visible items whose field is available, children only while their
    /// parent is.
    pub fn displayed_state(&self, log: &Log) -> Result<Vec<SourceListItemState>> {
        let mut parent_available = false;
        let mut displayed = Vec::new();
        for item in &self.state {
            item.require("logKey")?;
            let available = is_field_available(log, item);
            let type_config = self.type_config(&item.item_type);
            if type_config.is_some_and(|t| t.parent_key.is_some()) {
                parent_available = available;
            }
            let shown = if type_config.is_some_and(|t| t.child_of.is_some()) {
                available && parent_available
            } else {
                available
            };
            if item.visible && shown {
                displayed.push(item.clone());
            }
        }
        Ok(displayed)
    }

    /// `getState(true)` for a command that draws the items.
    pub fn drawn_state(&self, log: &Log) -> Result<Vec<SourceListItemState>> {
        let displayed = self.displayed_state(log)?;
        displayed
            .iter()
            .try_for_each(|item| self.require_modeled(item))?;
        Ok(displayed)
    }

    /// `getState()` for a command that reads the items `shows` picks.
    pub fn state_for_command(
        &self,
        shows: impl Fn(&SourceListItemState) -> bool,
    ) -> Result<&[SourceListItemState]> {
        for item in &self.state {
            item.require("visible")?;
            if item.raw.is_some() && shows(item) {
                self.require_modeled(item)?;
            }
        }
        Ok(&self.state)
    }

    /// The commands read options as strings, so one that draws an item with other options is
    /// not ported, except that reading any of `undefined` or `null` throws.
    fn require_modeled(&self, item: &SourceListItemState) -> Result {
        if item.raw.is_none() {
            return Ok(());
        }
        let Some(type_config) = self.type_config(&item.item_type) else {
            return Ok(());
        };
        let Some(first) = type_config.options.first() else {
            return Ok(());
        };
        item.option(&first.key)?;
        if !item.options_are_strings() {
            return Err(SourceListError::Thrown(
                "Unmodeled: drawing a source item whose options are not strings".to_owned(),
            ));
        }
        Ok(())
    }

    pub fn active_fields(&self) -> Vec<JsString> {
        self.state.iter().map(|item| item.log_key.clone()).collect()
    }

    pub fn stop(&mut self) {
        self.stopped = true;
    }

    pub fn set_state(&mut self, env: &mut Env<'_>, state: Vec<SourceListItemState>) -> Result {
        self.clear();
        for item in state {
            self.add_list_item(env, item, None)?;
        }
        Ok(())
    }

    /// `setState(value)` with whatever a layout holds: `clear()`, then
    /// `value.forEach(addListItem)`.
    pub fn restore(&mut self, env: &mut Env<'_>, value: &JsValue) -> Result {
        self.clear();
        let JsValue::Array(items) = value else {
            ops::get(value, "forEach")?;
            return Err(JsError::type_error("state.forEach is not a function").into());
        };
        let items = items.iter().map(SourceListItemState::restored).collect();
        self.set_state(env, items)
    }

    pub fn clear(&mut self) {
        self.state.clear();
        self.items.clear();
        self.update_hand_icon();
    }

    /// Call when a new set of log fields may be available.
    pub fn refresh(&mut self, env: &mut Env<'_>) -> Result {
        self.state
            .iter()
            .try_for_each(|item| item.require("logKey"))?;
        let displayed: Vec<&JsString> = self.state.iter().map(|item| &item.log_key).collect();
        let mut current: Vec<JsString> = Vec::new();
        for key in env.log.field_keys() {
            if displayed.contains(&key) && !current.contains(key) {
                current.push(key.clone());
            }
        }
        let mut struct_types: JsObject<StructuredType> = JsObject::new();
        for field in &current {
            struct_types.insert(field.clone(), env.log.structured_type(field).clone());
        }
        let should_update = current
            .iter()
            .any(|f| !self.refresh_last_fields.contains(f))
            || self
                .refresh_last_fields
                .iter()
                .any(|f| !current.contains(f))
            || struct_types
                .iter()
                .any(|(key, ty)| self.refresh_last_struct_types.get_key(key) != Some(ty));
        self.refresh_last_fields = current;
        self.refresh_last_struct_types = struct_types;
        if should_update {
            self.update_all_items(env)?;
        }
        Ok(())
    }

    /// Replaces the values of one option and moves items with a value that no longer exists
    /// to the first one.
    pub fn set_option_values(
        &mut self,
        env: &mut Env<'_>,
        type_key: &str,
        option: &str,
        values: Vec<SourceListOptionValueConfig>,
    ) -> Result {
        let Some(type_config) = self.config.types.iter_mut().find(|t| t.key == type_key) else {
            return Ok(());
        };
        let Some(option_config) = type_config.options.iter_mut().find(|o| o.key == option) else {
            return Ok(());
        };
        option_config.values = values.clone();
        for index in 0..self.state.len() {
            let item = &self.state[index];
            item.require("type")?;
            if !eq_str(&item.item_type, type_key) {
                continue;
            }
            let value = item.option(option)?;
            let valid = values.iter().any(|x| is_str(&value, &x.key));
            if !valid {
                let first = values
                    .first()
                    .ok_or_else(|| SourceListError::type_error("undefined", "key"))?;
                self.state[index].set_option(option, &first.key)?;
                self.update_item(env, index)?;
            }
        }
        Ok(())
    }

    /// Adds a field with the best type for it, if any type accepts it. `supplemental` holds
    /// the states of the lists whose use counts are shared with this one.
    pub fn add_field(
        &mut self,
        env: &mut Env<'_>,
        supplemental: &[&[SourceListItemState]],
        log_key: &JsString,
        parent_index: Option<usize>,
    ) -> Result {
        let log_type = env.log.get_type(log_key).map(|t| t.name());
        let structured_type = env.log.get_structured_type(log_key).cloned();

        let memory = match &self.config.type_memory_id {
            Some(id) if !id.is_empty() => remembered(env.type_memory, id, log_key)?,
            _ => None,
        };

        let with_supplemental: Vec<&SourceListItemState> = self
            .state
            .iter()
            .chain(supplemental.iter().flat_map(|s| s.iter()))
            .collect();
        let nullish = with_supplemental.iter().find(|item| item.is_nullish());
        struct Possible<'c> {
            config: &'c SourceListTypeConfig,
            log_type: JsString,
            uses: usize,
        }
        let parent_type_key = match parent_index {
            Some(i) if !self.config.types.is_empty() => Some(
                self.state
                    .get(i)
                    .ok_or_else(|| SourceListError::type_error("undefined", "type"))?
                    .item_type
                    .clone(),
            ),
            _ => None,
        };
        let parent_key = parent_type_key
            .as_ref()
            .and_then(|t| self.parent_key(t))
            .map(str::to_owned);
        let mut possible: Vec<Possible<'_>> = Vec::new();
        for type_config in &self.config.types {
            let skip = match parent_index {
                None => type_config.child_of.is_some(),
                Some(_) => type_config.child_of.as_deref() != parent_key.as_deref(),
            };
            if skip {
                continue;
            }
            let accepts = |t: &str| type_config.source_types.iter().any(|s| s == t);
            let final_type = match (&structured_type, log_type) {
                (Some(st), _) if st.as_string().is_some_and(|s| accepts(&s)) => st.clone(),
                (_, Some(lt)) if accepts(lt) => JsString::from(lt),
                _ => continue,
            };
            if final_type.is_empty() {
                continue;
            }
            nullish.map_or(Ok(()), |item| item.require("type"))?;
            possible.push(Possible {
                config: type_config,
                log_type: final_type,
                uses: with_supplemental
                    .iter()
                    .filter(|item| eq_str(&item.item_type, &type_config.key))
                    .count(),
            });
        }

        if possible.is_empty() {
            return Ok(());
        }
        if self.config.auto_advance == AutoAdvance::Type(true) {
            array_sort(&mut possible, |a, b| a.uses as f64 - b.uses as f64);
        }
        if let Some(memory) = &memory {
            let remembered = ops::get(memory, "type")?;
            if let Some(i) = possible
                .iter()
                .position(|p| is_str(&remembered, &p.config.key))
            {
                let chosen = possible.remove(i);
                possible.insert(0, chosen);
            }
        }
        let best = &possible[0];

        let mut options: JsObject<JsString> = JsObject::new();
        for option_config in &best.config.options {
            let value = if self.config.auto_advance
                == AutoAdvance::Option(option_config.key.clone())
            {
                let mut counts: Vec<(&SourceListOptionValueConfig, usize)> = Vec::new();
                for value in &option_config.values {
                    let mut uses = 0;
                    for item in &with_supplemental {
                        if item.has_option(&option_config.key)?
                            && is_str(&item.option(&option_config.key)?, &value.key)
                        {
                            uses += 1;
                        }
                    }
                    counts.push((value, uses));
                }
                array_sort(&mut counts, |a, b| a.1 as f64 - b.1 as f64);
                let first = counts
                    .first()
                    .ok_or_else(|| SourceListError::type_error("undefined", "valueConfig"))?;
                JsString::from(first.0.key.as_str())
            } else if let Some(remembered) = remembered_option(memory.as_ref(), &option_config.key)?
                .filter(|v| option_config.values.iter().any(|x| eq_str(v, &x.key)))
            {
                remembered
            } else {
                let first = option_config
                    .values
                    .first()
                    .ok_or_else(|| SourceListError::type_error("undefined", "key"))?;
                JsString::from(first.key.as_str())
            };
            options.insert(option_config.key.as_str(), value);
        }
        let state = SourceListItemState {
            item_type: JsString::from(best.config.key.as_str()),
            log_key: log_key.clone(),
            log_type: best.log_type.clone(),
            visible: true,
            options,
            raw: None,
        };
        match parent_index {
            Some(parent) => {
                let mut insert = parent + 1;
                while insert < self.state.len() && self.is_child(insert)? {
                    insert += 1;
                }
                self.add_list_item(env, state, Some(insert))
            }
            None => self.add_list_item(env, state, None),
        }
    }

    /// A `drag-update` whose data came from a source list item. `source` is the list the item
    /// was dragged from when that is not this one.
    pub fn handle_item_drag(
        &mut self,
        env: &mut Env<'_>,
        payload: &ItemDragPayload,
        source: Option<&mut SourceList>,
        x: f64,
        y: f64,
        end: bool,
    ) -> Result {
        if payload.config_str != self.config_str {
            return Ok(());
        }
        let root = env.dom.root_rect(&self.uuid);
        if x < root.left() || x > root.right() || y < root.top() || y > root.bottom() {
            self.highlight.hidden = true;
            return Ok(());
        }
        let same_list = payload.uuid == self.uuid;
        let count = self.items.len();
        let is_valid_target = |list: &SourceList, target: usize| -> Result<bool> {
            Ok(if payload.is_child {
                same_list
                    && list.parent_of(payload.index as isize)? == list.parent_of(target as isize)?
            } else {
                !(target < count && list.is_child(target)?)
            })
        };

        let mut closest_dist = f64::INFINITY;
        let mut closest_index = 0;
        for index in 0..count {
            if !is_valid_target(self, index)? {
                continue;
            }
            let dist = (y - env.dom.item_rect(&self.uuid, index).top()).abs();
            if dist < closest_dist {
                closest_dist = dist;
                closest_index = index;
            }
        }
        if count > 0 && is_valid_target(self, count)? {
            let dist = (y - env.dom.item_rect(&self.uuid, count - 1).bottom()).abs();
            if dist < closest_dist {
                closest_index = count;
            }
        }

        if !end {
            self.highlight.hidden = false;
            let mut highlight_y = if count == 0 {
                env.dom.list_rect(&self.uuid).top()
            } else if closest_index < count {
                env.dom.item_rect(&self.uuid, closest_index).top()
            } else {
                env.dom.item_rect(&self.uuid, count - 1).bottom()
            };
            highlight_y -= env.dom.root_rect(&self.uuid).top();
            highlight_y -= 6.0;
            self.highlight.placement = Some((Length::Px(highlight_y), Length::Px(12.0)));
            return Ok(());
        }

        self.highlight.hidden = true;
        let mut start_index = payload.index;
        let mut end_index = closest_index;
        let item_count = if payload.is_child {
            1
        } else {
            let mut n = 0;
            loop {
                n += 1;
                if start_index + n >= self.state.len() || !self.is_child(start_index + n)? {
                    break n;
                }
            }
        };
        let remove_error = || {
            SourceListError::Thrown(
                "TypeError: Failed to execute 'removeChild' on 'Node': parameter 1 is not of type 'Node'."
                    .to_owned(),
            )
        };
        match source {
            Some(source) if !same_list => {
                for _ in 0..item_count {
                    if start_index >= source.state.len() {
                        return Err(remove_error());
                    }
                    source.items.remove(start_index);
                    let state = source.state.remove(start_index);
                    self.add_list_item(env, state, Some(end_index))?;
                    end_index += 1;
                }
            }
            _ if !same_list => {}
            _ => {
                for _ in 0..item_count {
                    if end_index != start_index && start_index >= self.state.len() {
                        return Err(remove_error());
                    }
                    if end_index < start_index {
                        self.items.remove(start_index);
                        let state = self.state.remove(start_index);
                        self.add_list_item(env, state, Some(end_index))?;
                        start_index += 1;
                        end_index += 1;
                    } else if end_index > start_index {
                        self.items.remove(start_index);
                        let state = self.state.remove(start_index);
                        self.add_list_item(env, state, Some(end_index - 1))?;
                    }
                }
            }
        }
        Ok(())
    }

    /// A `drag-update` whose data are sidebar fields (`fields` plus their `children`).
    #[allow(clippy::too_many_arguments)]
    pub fn handle_field_drag(
        &mut self,
        env: &mut Env<'_>,
        supplemental: &[&[SourceListItemState]],
        fields: &[JsString],
        children: &[JsString],
        x: f64,
        y: f64,
        end: bool,
    ) -> Result {
        let dragged: Vec<JsString> = if self.config.allow_children_from_drag {
            fields.iter().chain(children).cloned().collect()
        } else {
            fields.to_vec()
        };
        let list_rect = env.dom.root_rect(&self.uuid);
        if list_rect.width == 0.0
            || list_rect.height == 0.0
            || x < list_rect.left()
            || x > list_rect.right()
            || y < list_rect.top()
            || y > list_rect.bottom()
        {
            self.highlight.hidden = true;
            self.clear_parent_highlights();
            return Ok(());
        }

        let mut parent_index = None;
        for i in 0..self.items.len() {
            let r = env.dom.item_rect(&self.uuid, i);
            let within = x > r.left() && x < r.right() && y > r.top() && y < r.bottom();
            if within && self.parent_key(&self.state[i].item_type).is_some() {
                parent_index = Some(i);
            }
        }

        self.state
            .iter()
            .try_for_each(|item| item.require("type"))?;
        let log = env.log;
        let is_type_valid = |source_types: &[&str]| {
            dragged.iter().any(|field| {
                let log_type = log.get_type(field).map(|t| t.name());
                let structured = log.get_structured_type(field).and_then(|s| s.as_string());
                log_type.is_some_and(|t| source_types.contains(&t))
                    || structured.is_some_and(|s| source_types.contains(&s.as_str()))
            })
        };
        let is_type_valid_as_child = |list: &SourceList, parent_type: &JsString| {
            let Some(parent_key) = list.parent_key(parent_type) else {
                return false;
            };
            let mut allowed: Vec<&str> = Vec::new();
            for type_config in &list.config.types {
                if type_config.child_of.as_deref() == Some(parent_key) {
                    allowed.extend(type_config.source_types.iter().map(String::as_str));
                }
            }
            is_type_valid(&allowed)
        };
        let independent: Vec<&str> = self
            .independent_allowed_types
            .iter()
            .map(String::as_str)
            .collect();
        let valid_as_root = is_type_valid(&independent);
        let valid_as_child =
            parent_index.is_some_and(|p| is_type_valid_as_child(self, &self.state[p].item_type));

        for i in 0..self.state.len() {
            if is_type_valid_as_child(self, &self.state[i].item_type)
                && let Some(item) = self.items.get_mut(i)
            {
                item.view.parent_highlight = true;
            }
        }

        if end {
            self.highlight.hidden = true;
            if !valid_as_child {
                parent_index = None;
            }
            if parent_index.is_some() || valid_as_root {
                for field in &dragged {
                    self.add_field(env, supplemental, field, parent_index)?;
                }
            }
            self.clear_parent_highlights();
        } else if let Some(parent) = parent_index.filter(|_| valid_as_child) {
            let top = math::max2(
                env.dom.item_rect(&self.uuid, parent).top() - list_rect.top(),
                0.0,
            );
            let bottom = math::min2(
                env.dom.item_rect(&self.uuid, parent).bottom() - list_rect.top(),
                list_rect.height,
            );
            self.highlight.placement = Some((Length::Px(top), Length::Px(bottom - top)));
            self.highlight.hidden = false;
        } else if valid_as_root {
            self.highlight.placement = Some((Length::Percent(0.0), Length::Percent(100.0)));
            self.highlight.hidden = false;
        } else {
            self.highlight.hidden = true;
            self.clear_parent_highlights();
        }
        Ok(())
    }

    /// One run of the list's animation-frame loop: re-renders every item when the theme
    /// changed, then updates the previews. An error stops the loop for good, as upstream's
    /// `requestAnimationFrame` chain breaks.
    pub fn frame(
        &mut self,
        env: &mut Env<'_>,
        time: Option<f64>,
        mut number_preview: Option<NumberPreview<'_>>,
    ) -> Result {
        if self.stopped || !self.periodic_alive {
            return Ok(());
        }
        let result = (|| {
            if self.last_is_dark != Some(env.dark) {
                self.last_is_dark = Some(env.dark);
                self.update_all_items(env)?;
            }
            let count = self.state.len().min(self.items.len());
            for index in 0..count {
                let state = &self.state[index];
                state.require("type")?;
                let available = is_field_available(env.log, state);
                let preview_setting = self
                    .type_config(&state.item_type)
                    .map_or(PreviewSetting::Unset, |t| t.preview_type);
                let text = preview::preview_text(
                    env.log,
                    state,
                    available,
                    preview_setting,
                    time,
                    &self.uuid,
                    &mut number_preview,
                )?;
                let view = &mut self.items[index].view;
                view.preview_shown = Some(text.is_some());
                if let Some(text) = text
                    && text != view.preview_text
                {
                    view.preview_text = text;
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.periodic_alive = false;
        }
        result
    }

    /// Mouse button pressed inside the item container.
    pub fn mouse_down(&mut self, x: f64, y: f64) {
        self.mouse_down = Some((x, y));
    }

    pub fn mouse_up(&mut self) {
        self.mouse_down = None;
    }

    /// Mouse moved inside the item container. Returns the item drag to start, if any.
    pub fn mouse_move(
        &mut self,
        dom: &mut dyn SourceListDom,
        x: f64,
        y: f64,
    ) -> Result<Option<ItemDragStart>> {
        let Some((down_x, down_y)) = self.mouse_down else {
            return Ok(None);
        };
        if (x - down_x).abs() < DRAG_THRESHOLD_PX && (y - down_y).abs() < DRAG_THRESHOLD_PX {
            return Ok(None);
        }
        let mut index = None;
        for i in 0..self.items.len() {
            let r = dom.item_rect(&self.uuid, i);
            if down_x >= r.left()
                && down_x <= r.right()
                && down_y >= r.top()
                && down_y <= r.bottom()
            {
                index = Some(i);
            }
        }
        self.mouse_down = None;
        let Some(index) = index else {
            return Ok(None);
        };
        let client_width = dom.item_client_width(&self.uuid, index);
        let rect = dom.item_rect(&self.uuid, index);
        Ok(Some(ItemDragStart {
            x,
            y,
            offset_x: x - rect.left(),
            offset_y: y - rect.top(),
            client_width,
            payload: ItemDragPayload {
                uuid: self.uuid.clone(),
                config_str: self.config_str.clone(),
                index,
                is_child: self.is_child(index)?,
            },
        }))
    }

    /// The type button of an item was clicked. Returns the prompt to send to main; its answer
    /// goes to [`SourceList::type_response`].
    pub fn prompt_type(&mut self, dom: &mut dyn SourceListDom, index: usize) -> Option<TypePrompt> {
        if !self.items.get(index).is_some_and(|item| item.interactive) {
            return None;
        }
        let rect = dom.button_rect(&self.uuid, ListButton::Type(index));
        self.prompt(index, rect)
    }

    fn prompt(&mut self, index: usize, rect: Rect) -> Option<TypePrompt> {
        let item = self.items.get(index).filter(|item| item.interactive)?;
        let state = self.state.get(index)?.clone();
        self.type_prompt = Some(PendingTypePrompt {
            item: item.id,
            original_type: state.item_type.clone(),
            is_child: item.created_as_child,
        });
        Some(TypePrompt {
            uuid: self.uuid.clone(),
            config: self.config.clone(),
            state,
            rect,
        })
    }

    /// Right click on an item at the pointer's client `x`, `y`: cancels a pending item drag
    /// and prompts for the type.
    pub fn context_menu(&mut self, index: usize, x: f64, y: f64) -> Option<TypePrompt> {
        if !self.items.get(index).is_some_and(|item| item.interactive) {
            return None;
        }
        self.mouse_down = None;
        self.prompt(
            index,
            Rect {
                x,
                y,
                width: 0.0,
                height: 0.0,
            },
        )
    }

    /// `source-list-type-response` for this list.
    pub fn type_response(&mut self, env: &mut Env<'_>, new_state: SourceListItemState) -> Result {
        let Some(prompt) = self.type_prompt.take() else {
            return Ok(());
        };
        let position = self.items.iter().position(|item| item.id == prompt.item);
        match position {
            Some(index) => {
                self.state[index] = new_state.clone();
                self.update_item(env, index)?;
            }
            None => {
                self.render(env, &new_state)?;
                self.remember(env, &new_state)?;
            }
        }
        if prompt.is_child {
            return Ok(());
        }
        let original_parent = self.parent_key(&prompt.original_type).map(str::to_owned);
        let new_parent = self.parent_key(&new_state.item_type).map(str::to_owned);
        if original_parent == new_parent {
            return Ok(());
        }
        let index = position.map_or(0, |i| i + 1);
        if !self.is_child(index)? {
            return Ok(());
        }
        let mut child_count = 0;
        while index + child_count < self.state.len() {
            child_count += 1;
            if !self.is_child(index + child_count)? {
                break;
            }
        }
        self.state.drain(index..index + child_count);
        for _ in 0..child_count {
            self.items.remove(index);
            self.update_hand_icon();
        }
        Ok(())
    }

    /// The hide button of an item.
    pub fn toggle_hidden(&mut self, env: &mut Env<'_>, index: usize) -> Result {
        let Some(item) = self.items.get(index).filter(|item| item.interactive) else {
            return Ok(());
        };
        let is_child = item.created_as_child;
        let visible = !self.state[index].visible;
        self.state[index].set_visible(visible);
        self.update_item(env, index)?;
        if !is_child {
            let mut search = index + 1;
            while search < self.state.len() {
                if !self.is_child(search)? {
                    break;
                }
                self.state[search].set_visible(visible);
                self.update_item(env, search)?;
                search += 1;
            }
        } else if visible
            && let Some(parent) = self.parent_of(index as isize)?
            && !self.state[parent].visible
        {
            self.state[parent].set_visible(true);
            self.update_item(env, parent)?;
        }
        Ok(())
    }

    /// A click on an item's type name or key; two within 400 ms toggle visibility.
    pub fn name_click(&mut self, env: &mut Env<'_>, index: usize, now: f64) -> Result {
        let Some(item) = self.items.get_mut(index).filter(|item| item.interactive) else {
            return Ok(());
        };
        if now - item.last_click < DOUBLE_CLICK_MS {
            self.toggle_hidden(env, index)?;
            self.items[index].last_click = 0.0;
            Ok(())
        } else {
            item.last_click = now;
            Ok(())
        }
    }

    /// The remove button of an item: removes it, and a parent's children with it.
    pub fn remove(&mut self, index: usize) -> Result {
        let Some(item) = self.items.get(index).filter(|item| item.interactive) else {
            return Ok(());
        };
        let is_child = item.created_as_child;
        let mut count = 0;
        while index + count < self.state.len() {
            count += 1;
            if is_child || !self.is_child(index + count)? {
                break;
            }
        }
        self.state.drain(index..index + count);
        self.items.drain(index..index + count);
        self.update_hand_icon();
        Ok(())
    }

    /// The clear button: main is asked to confirm with `source-list-clear-prompt`.
    pub fn request_clear(&mut self, dom: &mut dyn SourceListDom) -> ClearPrompt {
        let rect = dom.button_rect(&self.uuid, ListButton::Clear);
        self.clear_prompt = true;
        ClearPrompt {
            uuid: self.uuid.clone(),
            rect,
        }
    }

    /// The edit button: the rect upstream passes to the controller's `editButtonCallback`.
    pub fn edit_click(&self, dom: &mut dyn SourceListDom) -> Rect {
        dom.button_rect(&self.uuid, ListButton::Edit)
    }

    /// `source-list-clear-response` for this list.
    pub fn clear_response(&mut self) {
        if std::mem::take(&mut self.clear_prompt) {
            self.clear();
        }
    }

    fn type_config(&self, key: &JsString) -> Option<&SourceListTypeConfig> {
        self.config.types.iter().find(|t| eq_str(key, &t.key))
    }

    fn parent_key(&self, type_key: &JsString) -> Option<&str> {
        self.parent_keys
            .iter()
            .find(|(k, _)| eq_str(type_key, k))
            .map(|(_, v)| v.as_str())
    }

    fn is_child(&self, index: usize) -> Result<bool> {
        let Some(item) = self.state.get(index) else {
            return Ok(false);
        };
        item.require("type")?;
        let type_config = self.type_config(&item.item_type);
        Ok(type_config.is_some_and(|t| t.child_of.is_some()))
    }

    /// The nearest non-child item above `index`.
    fn parent_of(&self, index: isize) -> Result<Option<usize>> {
        let mut parent = index - 1;
        while parent >= 0 && self.is_child(parent as usize)? {
            parent -= 1;
        }
        Ok((parent >= 0).then_some(parent as usize))
    }

    fn update_hand_icon(&mut self) {
        self.hand_icon_shown = self.items.is_empty();
    }

    fn clear_parent_highlights(&mut self) {
        for item in &mut self.items {
            item.view.parent_highlight = false;
        }
    }

    fn add_list_item(
        &mut self,
        env: &mut Env<'_>,
        state: SourceListItemState,
        insert_index: Option<usize>,
    ) -> Result {
        let item = Item {
            id: self.next_item,
            interactive: false,
            created_as_child: false,
            last_click: 0.0,
            view: ItemView::default(),
        };
        self.next_item += 1;
        let index = match insert_index {
            Some(i) if i < self.items.len() => i,
            _ => self.items.len(),
        };
        self.items.insert(index, item);
        self.state.insert(index.min(self.state.len()), state);
        self.update_item(env, index)?;
        self.update_hand_icon();
        let is_child = self
            .type_config(&self.state[index].item_type)
            .is_some_and(|t| t.child_of.is_some());
        let item = &mut self.items[index];
        item.interactive = true;
        item.created_as_child = is_child;
        item.view.child = is_child;
        Ok(())
    }

    fn update_all_items(&mut self, env: &mut Env<'_>) -> Result {
        let count = self.state.len().min(self.items.len());
        for index in 0..count {
            self.update_item(env, index)?;
        }
        Ok(())
    }

    fn update_item(&mut self, env: &mut Env<'_>, index: usize) -> Result {
        let (rendered, type_width) = self.render(env, &self.state[index])?;
        let view = &mut self.items[index].view;
        view.rendered = Some(rendered);
        view.type_width = Some(type_width);
        self.remember(env, &self.state[index])
    }

    /// The end of `updateItem`, which throws with the item already drawn.
    fn remember(&self, env: &mut Env<'_>, state: &SourceListItemState) -> Result {
        let Some(id) = &self.config.type_memory_id else {
            return Ok(());
        };
        let entry = ops::object([
            ("type", state.member("type")?),
            ("options", state.member("options")?),
        ]);
        remember(env.type_memory, id, &state.log_key, entry)
    }

    /// `updateItem`'s computation; also the `--type-width`.
    fn render(
        &self,
        env: &mut Env<'_>,
        state: &SourceListItemState,
    ) -> Result<(RenderedItem, f64)> {
        state.require("type")?;
        let Some(type_config) = self.type_config(&state.item_type) else {
            let item_type = state.member("type")?;
            return Err(SourceListError::UnknownType(ops::to_string(&item_type)));
        };

        let color = if type_config.color.starts_with('#') {
            match (&type_config.dark_color, env.dark) {
                (Some(dark), true) => dark.clone(),
                _ => type_config.color.clone(),
            }
        } else {
            let option = state.option(&type_config.color)?;
            let option = match &option {
                JsValue::String(option) => option,
                JsValue::Undefined => {
                    return Err(SourceListError::type_error("undefined", "startsWith"));
                }
                JsValue::Null => return Err(SourceListError::type_error("null", "startsWith")),
                _ => {
                    return Err(SourceListError::Thrown(
                        "TypeError: state.options[typeConfig.color].startsWith is not a function"
                            .to_owned(),
                    ));
                }
            };
            if option.units().first() == Some(&(b'#' as u16)) {
                option.to_string_lossy()
            } else if env.dark {
                "#ffffff".to_owned()
            } else {
                "#000000".to_owned()
            }
        };
        let color = ensure_theme_contrast(&color, env.dark);

        let mut components: Vec<&str> = Vec::new();
        if type_config.show_in_type_name {
            components.push(&type_config.display);
        }
        for option_config in &type_config.options {
            if option_config.show_in_type_name {
                let key = state.option(&option_config.key)?;
                let value = option_config.values.iter().find(|v| is_str(&key, &v.key));
                components.extend(value.map(|v| v.display.as_str()));
            }
        }
        let type_name = format!(
            "{}{}",
            components.join("/"),
            if components.is_empty() { "" } else { ":" }
        );

        let mut type_width = env.dom.type_name_width(&type_name);
        if type_width > 0.0 {
            type_width += 3.0;
        }

        let rendered = RenderedItem {
            color,
            symbol_path: format!("symbols/sourceList/{}.svg", type_config.symbol),
            type_name,
            log_key: state.log_key.clone(),
            key_text: match state.raw.as_ref().and_then(|raw| raw.get("logKey")) {
                Some(JsValue::Null) => JsString::new(),
                _ => state.log_key.clone(),
            },
            available: is_field_available(env.log, state),
            visible: state.visible,
        };
        Ok((rendered, type_width))
    }
}

fn is_str(value: &JsValue, s: &str) -> bool {
    value.as_str().is_some_and(|value| eq_str(value, s))
}

/// `isFieldAvailable`: the field still has the type the item was added with. A `null` `logType`
/// is the structured type of any field without one.
pub fn is_field_available(log: &Log, item: &SourceListItemState) -> bool {
    if item.raw.is_none() {
        return log
            .get_type(&item.log_key)
            .is_some_and(|t| eq_str(&item.log_type, t.name()))
            || log.get_structured_type(&item.log_key) == Some(&item.log_type);
    }
    let log_type = item.member("logType").unwrap_or_default();
    let structured = match log.get_structured_type(&item.log_key) {
        Some(structured) => structured.to_js(),
        None => JsValue::Null,
    };
    log.get_type(&item.log_key)
        .is_some_and(|t| is_str(&log_type, t.name()))
        || ops::strict_equals(&structured, &log_type)
}

/// The entry `addField` finds for `log_key` with its two `in` checks: `None` for `null`. An
/// inherited member (`constructor`) has neither a `type` nor `options`.
fn remembered(memory: &JsValue, id: &str, log_key: &JsString) -> Result<Option<JsValue>> {
    if !ops::has(memory, id)? {
        return Ok(None);
    }
    let entries = ops::get(memory, id)?;
    if !ops::has_key(&entries, log_key)? {
        return Ok(None);
    }
    let entry = match ops::has_own(&entries, log_key)? {
        true => ops::get_key(&entries, log_key)?,
        false => JsValue::Object(JsObject::new()),
    };
    Ok(Some(entry).filter(|entry| !entry.is_null()))
}

/// `memory.options[key]` when it is a string, behind `key in memory.options`.
fn remembered_option(memory: Option<&JsValue>, key: &str) -> Result<Option<JsString>> {
    let Some(memory) = memory else {
        return Ok(None);
    };
    let options = ops::get(memory, "options")?;
    Ok(match ops::has(&options, key)? {
        true => ops::get(&options, key)?.into_string(),
        false => None,
    })
}

/// `updateItem`'s `window.typeMemory[id][logKey] = entry`.
fn remember(memory: &mut JsValue, id: &str, log_key: &JsString, entry: JsValue) -> Result {
    let key = JsString::from(id);
    if !ops::has(memory, id)? {
        ops::set_key(memory, &key, JsValue::Object(JsObject::new()))?;
    }
    match memory {
        JsValue::Object(ids) => match ids.get_units_mut(key.units()) {
            Some(entries) => Ok(ops::set_key(entries, log_key, entry)?),
            None => Ok(()),
        },
        _ => Ok(()),
    }
}
