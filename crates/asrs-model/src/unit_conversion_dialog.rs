//! Upstream's `src/unitConversion.ts` (the "Edit Conversion..." prompt window) as a headless
//! model: the select and input state after the preset arrives, the user's picks, and what
//! confirming, exiting and the window's focus messages do. main.ts's side (opening the window
//! with `unitConversion.preset ?? NoopUnitConversion`, and the reply's `edit-axis` message and
//! `updateRecents`) is `asrs_app::menus::context::Prompt::UnitConversion` and
//! `asrs_services::recent_units`.

use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;

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
        todo!("D08: select value")
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
pub struct UnitConversionDialog {}

impl UnitConversionDialog {
    /// The page after main.ts posts `preset` on the port.
    pub fn open(preset: &JsValue) -> (UnitConversionDialog, Vec<DialogEvent>) {
        let _ = preset;
        todo!("D08: unit conversion dialog")
    }

    pub fn state(&self) -> DialogState {
        todo!("D08: unit conversion dialog")
    }

    /// The user picks the unit type option whose value is `value` (the select's `change` event).
    pub fn pick_unit_type(&mut self, value: &JsString) -> Vec<DialogEvent> {
        let _ = value;
        todo!("D08: unit conversion dialog")
    }

    pub fn pick_from_unit(&mut self, value: &JsString) -> Vec<DialogEvent> {
        let _ = value;
        todo!("D08: unit conversion dialog")
    }

    pub fn pick_to_unit(&mut self, value: &JsString) -> Vec<DialogEvent> {
        let _ = value;
        todo!("D08: unit conversion dialog")
    }

    /// The extra factor field's text changes to `text`.
    pub fn set_extra_factor(&mut self, text: JsString) -> Vec<DialogEvent> {
        let _ = text;
        todo!("D08: unit conversion dialog")
    }

    pub fn click_confirm(&mut self) -> Vec<DialogEvent> {
        todo!("D08: unit conversion dialog")
    }

    pub fn click_exit(&mut self) -> Vec<DialogEvent> {
        todo!("D08: unit conversion dialog")
    }

    /// A `keydown` on the window with this `KeyboardEvent.code`.
    pub fn key_down(&mut self, code: &str) -> Vec<DialogEvent> {
        let _ = code;
        todo!("D08: unit conversion dialog")
    }

    /// main.ts's `{ isFocused }` message on window focus and blur.
    pub fn set_focused(&mut self, focused: bool) -> Vec<DialogEvent> {
        let _ = focused;
        todo!("D08: unit conversion dialog")
    }
}
