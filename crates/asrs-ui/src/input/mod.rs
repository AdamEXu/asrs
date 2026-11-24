//! Form controls styled from upstream's CSS and Chromium's UA stylesheet and native theme.
//! Values live in a model the view supplies (over `asrs_tabs::form`); the controls only show
//! them, forward what the user does, and report the DOM events upstream listens for.

use gpui_kit::{App, KeyBinding, actions};

mod field;
mod model;
mod native;
mod segmented;
mod select;
pub(crate) mod skia;
mod slider;
mod spin;
pub mod style;

pub use field::{FieldEvent, FieldGeometry, FieldKind, TextField, geometry};
pub use model::{FieldModel, FieldValue, PlainText, RangeModel};
pub use native::{
    Checkbox, PointerState, Progress, paint_checkbox, paint_progress, paint_text_field,
};
pub use segmented::{Segment, SegmentedControl, segment_boxes};
#[cfg(feature = "parity")]
pub use select::ScriptedSelectKeys;
pub use select::{
    Select, SelectEvent, SelectGeometry, SelectItem, SelectKey, SelectKeys, SelectModel,
    SelectState, SelectStep, geometry as select_geometry, paint_select, popup_menu, select_key,
};
pub use slider::{
    RangeControl, Slider, SliderEvent, SliderTrack, paint_in_rounded, pointer_proportion,
    thumb_offset,
};
pub use style::{FieldStyle, Frame, SliderStyle, SplitStyle};

pub use gpui_kit::base::input::{Cut, Paste, Redo, Undo};

gpui_kit::actions!(text_field, [DeleteSelection]);

/// The key context of the components that are an `<input>` in upstream's pages.
const CONTEXT: &str = "HtmlInput";

const NUMBER_CONTEXT: &str = "HtmlInput HtmlNumberInput";

const TEXT_CONTEXT: &str = "HtmlInput HtmlTextInput";

actions!(number_input, [StepUp, StepDown, StepNone]);
actions!(text_input, [ExtendUp, ExtendDown]);

/// The arrow keys step a number input with any modifiers, some of which the editor binds to
/// selecting. Alt and the down arrow do nothing.
pub(crate) fn init(cx: &mut App) {
    let within = Some("HtmlTextInput > Input");
    cx.bind_keys([
        KeyBinding::new("shift-up", ExtendUp, within),
        KeyBinding::new("shift-down", ExtendDown, within),
    ]);
    let within = Some("HtmlNumberInput > Input");
    for held in 1..16 {
        let modifiers: String = ["shift-", "ctrl-", "cmd-", "alt-"]
            .into_iter()
            .enumerate()
            .filter_map(|(bit, modifier)| (held & (1 << bit) != 0).then_some(modifier))
            .collect();
        let key = |key: &str| format!("{modifiers}{key}").leak();
        cx.bind_keys([
            KeyBinding::new(key("up"), StepUp, within),
            match modifiers.ends_with("alt-") {
                true => KeyBinding::new(key("down"), StepNone, within),
                false => KeyBinding::new(key("down"), StepDown, within),
            },
        ]);
    }
}

/// Whether an `<input>` has the window's focus.
pub fn focused(window: &gpui_kit::Window) -> bool {
    window.context_stack().iter().any(|c| c.contains(CONTEXT))
}
