//! The prompt pages as headless models: `src/editRange.ts`, `src/editFov.ts`, `src/renameTab.ts`,
//! `src/export.ts` and, over `asrs_model::unit_conversion_dialog`, `src/unitConversion.ts`. A view
//! shows [`PromptPage::controls`], reports what the user does to them and acts on the
//! [`PromptEvent`]s it gets back; `main.ts`'s side of the port is the caller's.

use asrs_js::ops;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_log::util::clean_float;
use asrs_model::unit_conversion_dialog::{self, DialogEvent, UnitConversionDialog};

use crate::form::{NumberInput, Select, SelectOption};

/// What the page does that main.ts, the user or the view sees.
#[derive(Clone, Debug, PartialEq)]
pub enum PromptEvent {
    /// `messagePort.postMessage(value)`.
    Reply(JsValue),
    /// `alert(message)`.
    Alert(JsString),
    /// An exception thrown out of an event handler.
    Error(JsError),
    /// `input.select()` on the control with this id.
    Select(&'static str),
}

impl From<DialogEvent> for PromptEvent {
    fn from(event: DialogEvent) -> PromptEvent {
        match event {
            DialogEvent::Reply(value) => PromptEvent::Reply(value),
            DialogEvent::Alert(message) => PromptEvent::Alert(message),
            DialogEvent::Error(error) => PromptEvent::Error(error),
        }
    }
}

fn from_dialog(events: Vec<DialogEvent>) -> Vec<PromptEvent> {
    events.into_iter().map(PromptEvent::from).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub enum ControlValue {
    /// `<input type="text">` and its `value`.
    Text(JsString),
    /// `<input type="number">` and its `value`.
    Number(JsString),
    Select(Select),
}

/// One `table` row: the `td.label` text and the form control beside it.
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub id: &'static str,
    pub label: &'static str,
    pub value: ControlValue,
    pub disabled: bool,
}

impl Control {
    fn new(id: &'static str, label: &'static str, value: ControlValue) -> Control {
        Control {
            id,
            label,
            value,
            disabled: false,
        }
    }

    fn number(id: &'static str, label: &'static str, input: &NumberInput) -> Control {
        Control::new(id, label, ControlValue::Number(input.value().clone()))
    }

    fn disabled(mut self, disabled: bool) -> Control {
        self.disabled = disabled;
        self
    }
}

pub trait PromptPage: 'static {
    /// `document.title` up to the dash before the app name.
    fn title(&self) -> &'static str;
    /// The icon in `#confirm`, under `symbols/`.
    fn confirm_icon(&self) -> &'static str {
        "check.svg"
    }
    /// Whether the page has the `div.help-div` link.
    fn has_help(&self) -> bool {
        false
    }
    /// The form controls in document order.
    fn controls(&self) -> Vec<Control>;
    /// Whether the buttons have the `blurred` class.
    fn blurred(&self) -> bool;
    /// `messagePort.onmessage` with this data.
    fn message(&mut self, data: &JsValue) -> Vec<PromptEvent>;
    /// The user edits a text or number field, whose editor now shows `visible`.
    fn edit(&mut self, id: &str, visible: &JsString);
    /// A text or number field's `change` event.
    fn change(&mut self, _id: &str) -> Vec<PromptEvent> {
        Vec::new()
    }
    /// The user selects the option at `index`, which fires the select's `change` event.
    fn pick(&mut self, id: &str, index: usize) -> Vec<PromptEvent>;
    fn click_exit(&mut self) -> Vec<PromptEvent>;
    fn click_confirm(&mut self) -> Vec<PromptEvent>;
    fn click_help(&mut self) -> Vec<PromptEvent> {
        Vec::new()
    }
    /// A `keydown` on the window with this `KeyboardEvent.code`.
    fn key_down(&mut self, code: &str) -> Vec<PromptEvent> {
        if code == "Enter" {
            self.click_confirm()
        } else {
            Vec::new()
        }
    }
}

/// `typeof event.data === "object" && "isFocused" in event.data`, and whether it is truthy then.
fn focus_message(data: &JsValue) -> Result<Option<bool>, JsError> {
    if data.is_null() {
        return Err(JsError::type_error(
            "Cannot use 'in' operator to search for 'isFocused' in null",
        ));
    }
    if !matches!(data, JsValue::Object(_)) || !ops::has(data, "isFocused")? {
        return Ok(None);
    }
    Ok(Some(ops::to_boolean(&ops::get(data, "isFocused")?)))
}

/// The state editRange, editFov and renameTab share: the triggers are attached, and the exit
/// value known, once main's normal message arrives.
#[derive(Clone, Debug, Default)]
struct Port {
    blurred: bool,
    original: Option<JsValue>,
}

impl Port {
    /// Handles a focus message; `Ok(true)` when `data` was one.
    fn focus(&mut self, data: &JsValue) -> Result<bool, JsError> {
        let focused = focus_message(data)?;
        if let Some(focused) = focused {
            self.blurred = !focused;
        }
        Ok(focused.is_some())
    }

    fn exit(&self) -> Vec<PromptEvent> {
        self.original
            .iter()
            .map(|value| PromptEvent::Reply(value.clone()))
            .collect()
    }
}

fn events(result: Result<Vec<PromptEvent>, JsError>) -> Vec<PromptEvent> {
    result.unwrap_or_else(|e| vec![PromptEvent::Error(e)])
}

/// `cleanFloat(value).toString()`.
fn clean_text(value: &JsValue) -> Result<JsString, JsError> {
    let number = clean_float(ops::value_to_number(value)?);
    Ok(ops::to_string(&JsValue::Number(number)))
}

#[derive(Clone, Debug, Default)]
pub struct EditRange {
    port: Port,
    min: NumberInput,
    max: NumberInput,
}

impl EditRange {
    fn receive(&mut self, data: &JsValue) -> Result<Vec<PromptEvent>, JsError> {
        if self.port.focus(data)? {
            return Ok(Vec::new());
        }
        self.min.set_value(clean_text(&ops::index(data, 0)?)?);
        self.max.set_value(clean_text(&ops::index(data, 1)?)?);
        self.port.original = Some(data.clone());
        Ok(vec![PromptEvent::Select("max")])
    }
}

impl PromptPage for EditRange {
    fn title(&self) -> &'static str {
        "Edit Range"
    }

    fn controls(&self) -> Vec<Control> {
        vec![
            Control::number("max", "Maximum Value", &self.max),
            Control::number("min", "Minimum Value", &self.min),
        ]
    }

    fn blurred(&self) -> bool {
        self.port.blurred
    }

    fn message(&mut self, data: &JsValue) -> Vec<PromptEvent> {
        events(self.receive(data))
    }

    fn edit(&mut self, id: &str, visible: &JsString) {
        match id {
            "max" => self.max.set_visible_value(visible),
            "min" => self.min.set_visible_value(visible),
            _ => {}
        }
    }

    fn pick(&mut self, _id: &str, _index: usize) -> Vec<PromptEvent> {
        Vec::new()
    }

    fn click_exit(&mut self) -> Vec<PromptEvent> {
        self.port.exit()
    }

    fn click_confirm(&mut self) -> Vec<PromptEvent> {
        if self.port.original.is_none() {
            return Vec::new();
        }
        let (min, max) = (self.min.number(), self.max.number());
        vec![if min >= max {
            PromptEvent::Alert(JsString::from("Maximum must be greater than minimum."))
        } else {
            PromptEvent::Reply(JsValue::Array(vec![
                JsValue::Number(min),
                JsValue::Number(max),
            ]))
        }]
    }
}

#[derive(Clone, Debug, Default)]
pub struct EditFov {
    port: Port,
    fov: NumberInput,
}

impl EditFov {
    fn receive(&mut self, data: &JsValue) -> Result<Vec<PromptEvent>, JsError> {
        if self.port.focus(data)? {
            return Ok(Vec::new());
        }
        self.fov.set_value(ops::to_string(data));
        self.port.original = Some(data.clone());
        Ok(vec![PromptEvent::Select("fov")])
    }
}

impl PromptPage for EditFov {
    fn title(&self) -> &'static str {
        "Edit FOV"
    }

    fn controls(&self) -> Vec<Control> {
        vec![Control::number("fov", "Camera FOV", &self.fov)]
    }

    fn blurred(&self) -> bool {
        self.port.blurred
    }

    fn message(&mut self, data: &JsValue) -> Vec<PromptEvent> {
        events(self.receive(data))
    }

    fn edit(&mut self, id: &str, visible: &JsString) {
        if id == "fov" {
            self.fov.set_visible_value(visible);
        }
    }

    fn pick(&mut self, _id: &str, _index: usize) -> Vec<PromptEvent> {
        Vec::new()
    }

    fn click_exit(&mut self) -> Vec<PromptEvent> {
        self.port.exit()
    }

    fn click_confirm(&mut self) -> Vec<PromptEvent> {
        let Some(original) = &self.port.original else {
            return Vec::new();
        };
        let value = self.fov.number();
        vec![PromptEvent::Reply(if value.is_nan() {
            original.clone()
        } else {
            JsValue::Number(value)
        })]
    }
}

#[derive(Clone, Debug, Default)]
pub struct RenameTab {
    port: Port,
    name: JsString,
}

impl RenameTab {
    fn receive(&mut self, data: &JsValue) -> Result<Vec<PromptEvent>, JsError> {
        if self.port.focus(data)? {
            return Ok(Vec::new());
        }
        self.name = text_value(&ops::to_string(data));
        self.port.original = Some(data.clone());
        Ok(vec![PromptEvent::Select("name")])
    }
}

impl PromptPage for RenameTab {
    fn title(&self) -> &'static str {
        "Rename Tab"
    }

    fn controls(&self) -> Vec<Control> {
        vec![Control::new(
            "name",
            "Tab Name",
            ControlValue::Text(self.name.clone()),
        )]
    }

    fn blurred(&self) -> bool {
        self.port.blurred
    }

    fn message(&mut self, data: &JsValue) -> Vec<PromptEvent> {
        events(self.receive(data))
    }

    fn edit(&mut self, id: &str, visible: &JsString) {
        if id == "name" {
            self.name = text_value(visible);
        }
    }

    fn pick(&mut self, _id: &str, _index: usize) -> Vec<PromptEvent> {
        Vec::new()
    }

    fn click_exit(&mut self) -> Vec<PromptEvent> {
        self.port.exit()
    }

    fn click_confirm(&mut self) -> Vec<PromptEvent> {
        if self.port.original.is_none() {
            return Vec::new();
        }
        vec![PromptEvent::Reply(JsValue::String(self.name.clone()))]
    }
}

/// A text input's value sanitization: line breaks are stripped.
fn text_value(text: &JsString) -> JsString {
    JsString::from_units(
        text.units()
            .iter()
            .copied()
            .filter(|&u| u != 0x0A && u != 0x0D)
            .collect(),
    )
}

pub const EXPORT_HELP_URL: &str = "https://docs.advantagescope.org/more-features/export/#options";
const SAMPLING_MODE_AKIT: usize = 2;

/// `export.ts` from the `"port"` message on, when its listeners are attached.
#[derive(Clone, Debug)]
pub struct Export {
    blurred: bool,
    format: Select,
    sampling_mode: Select,
    sampling_period: NumberInput,
    period_disabled: bool,
    prefixes: JsString,
    include_generated: Select,
}

impl Default for Export {
    fn default() -> Export {
        let select = |options: &[(&str, &str)]| {
            Select::with_options(
                options
                    .iter()
                    .map(|(value, text)| SelectOption::new(Some(value), text))
                    .collect(),
            )
        };
        let mut export = Export {
            blurred: false,
            format: select(&[
                ("csv-table", "CSV (Table)"),
                ("csv-list", "CSV (List)"),
                ("wpilog", "WPILOG"),
                ("mcap", "MCAP"),
            ]),
            sampling_mode: select(&[
                ("changes", "All Changes"),
                ("fixed", "Fixed Period"),
                ("akit", "AdvantageKit Cycles"),
            ]),
            sampling_period: NumberInput::new("20"),
            period_disabled: false,
            prefixes: JsString::new(),
            include_generated: select(&[("true", "Include Generated"), ("false", "Original Only")]),
        };
        export.update_disabled();
        export
    }
}

impl Export {
    fn update_disabled(&mut self) {
        self.period_disabled = self.sampling_mode.value() != JsString::from("fixed");
    }

    fn receive(&mut self, data: &JsValue) -> Result<Vec<PromptEvent>, JsError> {
        if let Some(focused) = focus_message(data)? {
            self.blurred = !focused;
        }
        if matches!(data, JsValue::Object(_)) && ops::has(data, "supportsAkit")? {
            let supports_akit = ops::to_boolean(&ops::get(data, "supportsAkit")?);
            self.sampling_mode
                .set_option_disabled(SAMPLING_MODE_AKIT, !supports_akit);
        }
        Ok(Vec::new())
    }

    fn one_of(value: JsString, known: &[&'static str]) -> JsValue {
        let found = known.iter().find(|k| value == JsString::from(**k));
        JsValue::string(found.unwrap_or(&known[0]))
    }
}

impl PromptPage for Export {
    fn title(&self) -> &'static str {
        "Export"
    }

    fn confirm_icon(&self) -> &'static str {
        "south.svg"
    }

    fn has_help(&self) -> bool {
        true
    }

    fn controls(&self) -> Vec<Control> {
        let select = |id, label, select: &Select| {
            Control::new(id, label, ControlValue::Select(select.clone()))
        };
        vec![
            select("format", "Format", &self.format),
            select("samplingMode", "Timestamps", &self.sampling_mode),
            Control::number("samplingPeriod", "Period (ms)", &self.sampling_period)
                .disabled(self.period_disabled),
            Control::new(
                "prefixes",
                "Prefixes",
                ControlValue::Text(self.prefixes.clone()),
            ),
            select("includeGenerated", "Field Set", &self.include_generated),
        ]
    }

    fn blurred(&self) -> bool {
        self.blurred
    }

    fn message(&mut self, data: &JsValue) -> Vec<PromptEvent> {
        events(self.receive(data))
    }

    fn edit(&mut self, id: &str, visible: &JsString) {
        match id {
            "samplingPeriod" => self.sampling_period.set_visible_value(visible),
            "prefixes" => self.prefixes = text_value(visible),
            _ => {}
        }
    }

    fn change(&mut self, id: &str) -> Vec<PromptEvent> {
        if id == "samplingPeriod" && self.sampling_period.number() <= 0.0 {
            self.sampling_period.set_value(JsString::from("1"));
        }
        Vec::new()
    }

    fn pick(&mut self, id: &str, index: usize) -> Vec<PromptEvent> {
        let index = index as i32;
        match id {
            "format" => self.format.set_selected_index(index),
            "samplingMode" => {
                self.sampling_mode.set_selected_index(index);
                self.update_disabled();
            }
            "includeGenerated" => self.include_generated.set_selected_index(index),
            _ => {}
        }
        Vec::new()
    }

    fn click_exit(&mut self) -> Vec<PromptEvent> {
        vec![PromptEvent::Reply(JsValue::Null)]
    }

    fn click_confirm(&mut self) -> Vec<PromptEvent> {
        let options = ops::object([
            (
                "format",
                Self::one_of(
                    self.format.value(),
                    &["csv-table", "csv-list", "wpilog", "mcap"],
                ),
            ),
            (
                "samplingMode",
                Self::one_of(self.sampling_mode.value(), &["changes", "fixed", "akit"]),
            ),
            (
                "samplingPeriod",
                JsValue::Number(self.sampling_period.number()),
            ),
            ("prefixes", JsValue::String(self.prefixes.clone())),
            (
                "includeGenerated",
                JsValue::Bool(self.include_generated.value() == JsString::from("true")),
            ),
        ]);
        vec![PromptEvent::Reply(options)]
    }

    fn click_help(&mut self) -> Vec<PromptEvent> {
        vec![PromptEvent::Reply(JsValue::string(EXPORT_HELP_URL))]
    }
}

/// `unitConversion.ts`, whose selects are empty until main posts the preset.
#[derive(Clone, Debug, Default)]
pub struct UnitConversion {
    dialog: Option<UnitConversionDialog>,
    blurred: bool,
}

impl UnitConversion {
    fn receive(&mut self, data: &JsValue) -> Result<Vec<PromptEvent>, JsError> {
        if let Some(focused) = focus_message(data)? {
            self.blurred = !focused;
            return Ok(Vec::new());
        }
        let (dialog, events) = UnitConversionDialog::open(data);
        self.dialog = Some(dialog);
        Ok(from_dialog(events))
    }

    fn with(
        &mut self,
        f: impl FnOnce(&mut UnitConversionDialog) -> Vec<DialogEvent>,
    ) -> Vec<PromptEvent> {
        from_dialog(self.dialog.as_mut().map(f).unwrap_or_default())
    }
}

fn dialog_select(state: &unit_conversion_dialog::SelectState) -> Select {
    let mut select = Select::with_options(
        state
            .options
            .iter()
            .map(|o| SelectOption::from_inner_text(&o.text))
            .collect(),
    );
    select.set_selected_index(state.selected_index);
    select
}

impl PromptPage for UnitConversion {
    fn title(&self) -> &'static str {
        "Unit Conversion"
    }

    fn controls(&self) -> Vec<Control> {
        let state = self.dialog.as_ref().map(UnitConversionDialog::state);
        let select = |id, label, of: fn(&unit_conversion_dialog::DialogState) -> &_| {
            let state = state.as_ref().map(of);
            Control::new(
                id,
                label,
                ControlValue::Select(state.map(dialog_select).unwrap_or_default()),
            )
            .disabled(state.is_some_and(|s| s.disabled))
        };
        vec![
            select("unitType", "Unit Type", |s| &s.unit_type),
            select("fromUnit", "From Unit", |s| &s.from_unit),
            select("toUnit", "To Unit", |s| &s.to_unit),
            Control::new(
                "extraFactor",
                "Extra Factor",
                ControlValue::Text(
                    state
                        .as_ref()
                        .map(|s| s.extra_factor.clone())
                        .unwrap_or_default(),
                ),
            ),
        ]
    }

    fn blurred(&self) -> bool {
        self.blurred
    }

    fn message(&mut self, data: &JsValue) -> Vec<PromptEvent> {
        events(self.receive(data))
    }

    fn edit(&mut self, id: &str, visible: &JsString) {
        if id == "extraFactor" {
            self.with(|dialog| dialog.set_extra_factor(visible.clone()));
        }
    }

    fn pick(&mut self, id: &str, index: usize) -> Vec<PromptEvent> {
        self.with(|dialog| {
            let state = dialog.state();
            let select = match id {
                "unitType" => &state.unit_type,
                "fromUnit" => &state.from_unit,
                "toUnit" => &state.to_unit,
                _ => return Vec::new(),
            };
            let Some(option) = select.options.get(index) else {
                return Vec::new();
            };
            match id {
                "unitType" => dialog.pick_unit_type(&option.value),
                "fromUnit" => dialog.pick_from_unit(&option.value),
                _ => dialog.pick_to_unit(&option.value),
            }
        })
    }

    fn click_exit(&mut self) -> Vec<PromptEvent> {
        self.with(UnitConversionDialog::click_exit)
    }

    fn click_confirm(&mut self) -> Vec<PromptEvent> {
        self.with(UnitConversionDialog::click_confirm)
    }
}
