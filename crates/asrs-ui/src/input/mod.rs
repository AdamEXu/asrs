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

/// The key context of the components that are an `<input>` in upstream's pages.
const CONTEXT: &str = "HtmlInput";

const NUMBER_CONTEXT: &str = "HtmlInput HtmlNumberInput";

actions!(number_input, [StepUp, StepDown]);

/// The arrow keys step a number input with these modifiers too, which the editor binds to
/// selecting. Alt and the down arrow do nothing.
pub(crate) fn init(cx: &mut App) {
    let within = Some("HtmlNumberInput > Input");
    cx.bind_keys(["shift-up", "ctrl-up", "alt-up"].map(|k| KeyBinding::new(k, StepUp, within)));
    cx.bind_keys(["shift-down", "ctrl-down"].map(|k| KeyBinding::new(k, StepDown, within)));
}

/// Whether an `<input>` has the window's focus.
pub fn focused(window: &gpui_kit::Window) -> bool {
    window.context_stack().iter().any(|c| c.contains(CONTEXT))
}
