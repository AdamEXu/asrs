//! Upstream's `src/unitConversion.ts` (the "Edit Conversion..." prompt window) as a headless
//! model: the select and input state after the preset arrives, the user's picks, and what
//! confirming, exiting and the window's focus messages do. main.ts's side (opening the window
//! with `unitConversion.preset ?? NoopUnitConversion`, and the reply's `edit-axis` message and
//! `updateRecents`) is `asrs_app::menus::context::Prompt::UnitConversion` and
//! `asrs_services::recent_units`.

use asrs_js::number;
use asrs_js::string::{JsError, JsString};
use asrs_js::time::{NamedZone, date_to_string};
use asrs_js::value::{JsObject, JsValue};
use asrs_log::units::UNIT_GROUPS;

use crate::mathjs::{self, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct SelectOption {
    pub text: JsString,
    pub value: JsString,
}

/// A `<select>`: its options, `selectedIndex` (`-1` when nothing is selected) and `disabled`.
#[derive(Clone, Debug, PartialEq)]
pub struct SelectState {
    pub options: Vec<SelectOption>,
    pub selected_index: i32,
    pub disabled: bool,
}

impl SelectState {
    /// `select.value`: the selected option's value, or `""`.
    pub fn value(&self) -> JsString {
        match usize::try_from(self.selected_index)
            .ok()
            .and_then(|i| self.options.get(i))
        {
            Some(option) => option.value.clone(),
            None => JsString::new(),
        }
    }

    fn empty() -> Self {
        SelectState {
            options: Vec::new(),
            selected_index: -1,
            disabled: false,
        }
    }

    /// `select.value = v`: the first option with value `v` is selected, or none.
    fn set_value(&mut self, value: &JsString) {
        self.selected_index = self
            .options
            .iter()
            .position(|o| o.value == *value)
            .map_or(-1, |i| i as i32);
    }

    /// Removing every `<option>` child: nothing is selected afterwards.
    fn clear_options(&mut self) {
        self.options.clear();
        self.selected_index = -1;
    }

    /// `select.appendChild(option)` with `option.innerText = text`. Appending to an empty
    /// select selects the first option.
    fn append_option(&mut self, text: &JsString) {
        let was_empty = self.options.is_empty();
        self.options.push(SelectOption {
            text: text.clone(),
            value: text.clone(),
        });
        if was_empty {
            self.selected_index = 0;
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DialogState {
    pub unit_type: SelectState,
    pub from_unit: SelectState,
    pub to_unit: SelectState,
    pub extra_factor: JsString,
    /// Whether each of the two buttons (exit, confirm) has the `blurred` class.
    pub blurred: [bool; 2],
}

/// What the page does that main.ts or the user sees.
#[derive(Clone, Debug, PartialEq)]
pub enum DialogEvent {
    /// `messagePort.postMessage(value)`: the conversion preset, or `null` from exit.
    Reply(JsValue),
    /// `alert(message)`.
    Alert(JsString),
    /// An exception thrown out of the port's message handler.
    Error(JsError),
}

#[derive(Clone, Debug)]
pub struct UnitConversionDialog {
    unit_type: SelectState,
    from_unit: SelectState,
    to_unit: SelectState,
    extra_factor: JsString,
    blurred: [bool; 2],
    /// Whether the exit, confirm and Enter listeners are attached. They are added only after
    /// `EXTRA_FACTOR.value` is set, so a throw during preset handling leaves them off; the
    /// unit type `change` listener is attached earlier and always works.
    triggers_attached: bool,
}

impl UnitConversionDialog {
    /// The page after main.ts posts `preset` on the port.
    pub fn open(preset: &JsValue) -> (UnitConversionDialog, Vec<DialogEvent>) {
        let mut dialog = UnitConversionDialog {
            unit_type: SelectState::empty(),
            from_unit: SelectState::empty(),
            to_unit: SelectState::empty(),
            extra_factor: JsString::new(),
            blurred: [false, false],
            triggers_attached: false,
        };
        let mut events = Vec::new();

        let none = JsString::from("none");
        dialog.unit_type.append_option(&none);
        for &(group, _) in UNIT_GROUPS {
            dialog.unit_type.append_option(&JsString::from(group));
        }

        // `preset.type`: reading a property of null or undefined throws.
        let preset_type = match preset {
            JsValue::Null => {
                events.push(DialogEvent::Error(JsError::type_error(
                    "Cannot read properties of null (reading 'type')",
                )));
                return (dialog, events);
            }
            JsValue::Undefined => {
                events.push(DialogEvent::Error(JsError::type_error(
                    "Cannot read properties of undefined (reading 'type')",
                )));
                return (dialog, events);
            }
            _ => get_property(preset, "type"),
        };
        if preset_type.is_null() {
            dialog.unit_type.set_value(&none);
        } else {
            dialog.unit_type.set_value(&to_string(&preset_type));
        }
        if let Err(e) = dialog.update_unit_options() {
            events.push(DialogEvent::Error(e));
            return (dialog, events);
        }
        if !preset_type.is_null() {
            let from = to_string(&get_property(preset, "from"));
            dialog.from_unit.set_value(&from);
            let to = to_string(&get_property(preset, "to"));
            dialog.to_unit.set_value(&to);
        }
        match to_string_method(&get_property(preset, "factor")) {
            Ok(text) => dialog.extra_factor = text_input_value(&text),
            Err(e) => {
                events.push(DialogEvent::Error(e));
                return (dialog, events);
            }
        }
        dialog.triggers_attached = true;
        (dialog, events)
    }

    pub fn state(&self) -> DialogState {
        DialogState {
            unit_type: self.unit_type.clone(),
            from_unit: self.from_unit.clone(),
            to_unit: self.to_unit.clone(),
            extra_factor: self.extra_factor.clone(),
            blurred: self.blurred,
        }
    }

    /// `updateUnitOptions`: rebuild `fromUnit` and `toUnit` for `unitType`'s current value.
    /// `Object.keys(Units.UNIT_GROUPS[type])` throws for a value that is not a group.
    fn update_unit_options(&mut self) -> Result<(), JsError> {
        self.from_unit.clear_options();
        self.to_unit.clear_options();
        let unit_type = self.unit_type.value();
        if eq_str(&unit_type, "none") {
            self.from_unit.disabled = true;
            self.to_unit.disabled = true;
            self.from_unit.append_option(&JsString::from("NA"));
            self.to_unit.append_option(&JsString::from("NA"));
            return Ok(());
        }
        self.from_unit.disabled = false;
        self.to_unit.disabled = false;
        let Some(&(_, units)) = UNIT_GROUPS
            .iter()
            .find(|(name, _)| eq_str(&unit_type, name))
        else {
            return Err(JsError::type_error(
                "Cannot convert undefined or null to object",
            ));
        };
        for (index, &(unit, _)) in units.iter().enumerate() {
            let text = JsString::from(unit);
            self.from_unit.append_option(&text);
            self.to_unit.append_option(&text);
            if index == 1 {
                self.to_unit.set_value(&text);
            }
        }
        Ok(())
    }

    /// The user picks the unit type option whose value is `value` (the select's `change` event).
    pub fn pick_unit_type(&mut self, value: &JsString) -> Vec<DialogEvent> {
        self.unit_type.set_value(value);
        match self.update_unit_options() {
            Ok(()) => Vec::new(),
            Err(e) => vec![DialogEvent::Error(e)],
        }
    }

    pub fn pick_from_unit(&mut self, value: &JsString) -> Vec<DialogEvent> {
        self.from_unit.set_value(value);
        Vec::new()
    }

    pub fn pick_to_unit(&mut self, value: &JsString) -> Vec<DialogEvent> {
        self.to_unit.set_value(value);
        Vec::new()
    }

    /// The extra factor field's text changes to `text`.
    pub fn set_extra_factor(&mut self, text: JsString) -> Vec<DialogEvent> {
        self.extra_factor = text_input_value(&text);
        Vec::new()
    }

    pub fn click_confirm(&mut self) -> Vec<DialogEvent> {
        if self.triggers_attached {
            self.confirm()
        } else {
            Vec::new()
        }
    }

    pub fn click_exit(&mut self) -> Vec<DialogEvent> {
        if self.triggers_attached {
            vec![DialogEvent::Reply(JsValue::Null)]
        } else {
            Vec::new()
        }
    }

    /// A `keydown` on the window with this `KeyboardEvent.code`.
    pub fn key_down(&mut self, code: &str) -> Vec<DialogEvent> {
        if self.triggers_attached && code == "Enter" {
            self.confirm()
        } else {
            Vec::new()
        }
    }

    /// main.ts's `{ isFocused }` message on window focus and blur.
    pub fn set_focused(&mut self, focused: bool) -> Vec<DialogEvent> {
        self.blurred = [!focused, !focused];
        Vec::new()
    }

    /// The `confirm` function: evaluate the extra factor and post the preset, or alert.
    fn confirm(&mut self) -> Vec<DialogEvent> {
        let factor = match mathjs::evaluate(&self.extra_factor) {
            Ok(Value::Number(x)) => x,
            _ => {
                return vec![DialogEvent::Alert(JsString::from(
                    "Failed to parse extra factor.",
                ))];
            }
        };
        let unit_type = self.unit_type.value();
        let is_none = eq_str(&unit_type, "none");
        let mut conversion = JsObject::new();
        conversion.insert(
            "type",
            if is_none {
                JsValue::Null
            } else {
                JsValue::String(unit_type)
            },
        );
        conversion.insert("factor", JsValue::Number(factor));
        if !is_none {
            conversion.insert("from", JsValue::String(self.from_unit.value()));
            conversion.insert("to", JsValue::String(self.to_unit.value()));
        }
        vec![DialogEvent::Reply(JsValue::Object(conversion))]
    }
}

fn eq_str(s: &JsString, t: &str) -> bool {
    s.units().iter().copied().eq(t.encode_utf16())
}

/// `preset[key]`: `undefined` for primitives and absent properties.
fn get_property(preset: &JsValue, key: &str) -> JsValue {
    match preset {
        JsValue::Object(object) => object.get(key).cloned().unwrap_or(JsValue::Undefined),
        // ExtData is a `{type, data}` object for property reads.
        JsValue::ExtData(kind, _) if key == "type" => JsValue::Number(f64::from(*kind)),
        _ => JsValue::Undefined,
    }
}

/// `value.toString()`: `TypeError` on null and undefined, ToString otherwise.
fn to_string_method(value: &JsValue) -> Result<JsString, JsError> {
    match value {
        JsValue::Null => Err(JsError::type_error(
            "Cannot read properties of null (reading 'toString')",
        )),
        JsValue::Undefined => Err(JsError::type_error(
            "Cannot read properties of undefined (reading 'toString')",
        )),
        _ => Ok(to_string(value)),
    }
}

/// ECMAScript `ToString`: array elements and typed array elements join with `,`.
fn to_string(value: &JsValue) -> JsString {
    match value {
        JsValue::Undefined => JsString::from("undefined"),
        JsValue::Null => JsString::from("null"),
        JsValue::Bool(b) => JsString::from(if *b { "true" } else { "false" }),
        JsValue::Number(x) => JsString::from(number::to_string(*x)),
        JsValue::String(s) => s.clone(),
        JsValue::Array(items) => {
            let mut out = JsString::new();
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(&JsString::from(","));
                }
                if !matches!(item, JsValue::Null | JsValue::Undefined) {
                    out.push_str(&to_string(item));
                }
            }
            out
        }
        JsValue::Bytes(bytes) => {
            let mut out = JsString::new();
            for (i, byte) in bytes.iter().enumerate() {
                if i > 0 {
                    out.push_str(&JsString::from(","));
                }
                out.push_str(&JsString::from(number::to_string(f64::from(*byte))));
            }
            out
        }
        JsValue::BigInt(b) => JsString::from(format!("{b}")),
        JsValue::Date(t) => JsString::from(date_to_string(*t, &NamedZone::system())),
        JsValue::Object(_) | JsValue::ExtData(..) => JsString::from("[object Object]"),
    }
}

/// Assigning to a text input's `.value`: newlines are stripped.
fn text_input_value(text: &JsString) -> JsString {
    JsString::from_units(
        text.units()
            .iter()
            .copied()
            .filter(|&u| u != 0x000a && u != 0x000d)
            .collect(),
    )
}
