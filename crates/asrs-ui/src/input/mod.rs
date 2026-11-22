//! Form controls styled from upstream's CSS and Chromium's UA stylesheet and native theme.
//! Values live in a model the view supplies (over `asrs_tabs::form`); the controls only show
//! them, forward what the user does, and report the DOM events upstream listens for.

mod field;
mod model;
mod native;
mod segmented;
mod skia;
mod slider;
pub mod style;

pub use field::{FieldEvent, FieldGeometry, FieldKind, TextField, geometry};
pub use model::{FieldModel, FieldValue, PlainText, RangeModel, number_accepts};
pub use native::{Checkbox, Progress, paint_checkbox, paint_progress, paint_text_field};
pub use segmented::{Segment, SegmentedControl, segment_boxes};
pub use slider::{Slider, SliderEvent, pointer_proportion, thumb_offset};
pub use style::{FieldStyle, Frame, SliderStyle, SplitStyle};
