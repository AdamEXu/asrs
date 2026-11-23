//! Form controls styled from upstream's CSS and Chromium's UA stylesheet and native theme.
//! Values live in a model the view supplies (over `asrs_tabs::form`); the controls only show
//! them, forward what the user does, and report the DOM events upstream listens for.

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
pub use select::{
    Select, SelectEvent, SelectGeometry, SelectItem, SelectKey, SelectModel, SelectState,
    geometry as select_geometry, paint_select, popup_menu, select_key,
};
pub use slider::{
    RangeControl, Slider, SliderEvent, SliderTrack, paint_in_rounded, pointer_proportion,
    thumb_offset,
};
pub use style::{FieldStyle, Frame, SliderStyle, SplitStyle};

/// The key context of the components that are an `<input>` in upstream's pages.
const CONTEXT: &str = "HtmlInput";

/// Whether an `<input>` has the window's focus.
pub fn focused(window: &gpui_kit::Window) -> bool {
    window.context_stack().iter().any(|c| c.contains(CONTEXT))
}
