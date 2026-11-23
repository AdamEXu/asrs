//! Where `prompts.css` and Blink's table layout put a prompt's parts, in client coordinates.

use asrs_ui::input::style::{CONTROL_FONT, device_border};
use asrs_ui::input::{FieldStyle, Frame};
use asrs_ui::text::{self, TextStyle};
use asrs_ui::theme::FONT_FAMILY;

use asrs_ui::gpui::App;

const TABLE_INSET: f64 = 10.0;
const ROW_HEIGHT: f64 = 27.0;
const CONTROL_MARGIN: f64 = 5.0;
pub const BUTTON_SIZE: f64 = 30.0;
pub const BUTTON_BOTTOM: f64 = 8.0;
pub const EXIT_RIGHT: f64 = 43.0;
pub const CONFIRM_RIGHT: f64 = 8.0;
pub const HELP_RIGHT: f64 = 78.0;
pub const HELP_WIDTH: f64 = 50.0;

/// `td.label`.
pub fn label_style() -> TextStyle {
    let font = format!("14px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "27px"),
        ("white-space", "nowrap"),
        ("text-align", "right"),
    ])
    .expect("prompt label style")
}

/// `div.help-div > a`.
pub fn help_style() -> TextStyle {
    let font = format!("16px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([("font", font.as_str()), ("white-space", "nowrap")])
        .expect("prompt help style")
}

/// The border-box heights of a text or number input and of a select.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlHeights {
    pub input: f64,
    pub select: f64,
}

impl ControlHeights {
    pub fn measure(cx: &mut App, scale: f32) -> ControlHeights {
        let zoom = f64::from(scale);
        let style = FieldStyle::native();
        let border = match style.frame {
            Frame::Native => f64::from(style.border) * zoom,
            Frame::Css { .. } => f64::from(device_border(style.border, scale)),
        };
        let padding = f64::from(style.padding[0] + style.padding[2]) * zoom;
        let line = text::layout(cx, &TextStyle::new(style.font), "0", scale, None).height;
        let (ascent, descent) = text::font_metrics(cx, &TextStyle::new(CONTROL_FONT), scale);
        let select_border = f64::from(device_border(1.0, scale));
        ControlHeights {
            input: (f64::from(line) / 64.0 + padding + 2.0 * border) / zoom,
            select: (f64::from(ascent + descent) + 2.0 * zoom.trunc() + 2.0 * select_border) / zoom,
        }
    }
}

/// The boxes of one `table` row: `td.label`, `td.input` and the control in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub label: [f64; 4],
    pub input: [f64; 4],
    pub control: [f64; 4],
}

/// The table in a page `width` wide: the label column is as wide as its widest label, the other
/// takes the rest, and each control is centred in its row.
pub fn rows(width: f64, label_widths: &[f64], control_heights: &[f64]) -> Vec<Row> {
    let label_width = label_widths.iter().copied().fold(0.0, f64::max);
    let input_x = TABLE_INSET + label_width;
    let input_width = width - 2.0 * TABLE_INSET - label_width;
    control_heights
        .iter()
        .enumerate()
        .map(|(i, height)| {
            let y = TABLE_INSET + ROW_HEIGHT * i as f64;
            Row {
                label: [TABLE_INSET, y, label_width, ROW_HEIGHT],
                input: [input_x, y, input_width, ROW_HEIGHT],
                control: [
                    input_x + CONTROL_MARGIN,
                    y + (ROW_HEIGHT - height) / 2.0,
                    input_width - CONTROL_MARGIN,
                    *height,
                ],
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `getBoundingClientRect` of unitConversion.html's cells and controls at 1x in Electron.
    #[test]
    fn unit_conversion_rows() {
        let got = rows(
            300.0,
            &[60.0, 70.0, 53.0, 77.78125],
            &[19.0, 19.0, 19.0, 21.0],
        );
        assert_eq!(got[0].label, [10.0, 10.0, 77.78125, 27.0]);
        assert_eq!(got[0].input, [87.78125, 10.0, 202.21875, 27.0]);
        assert_eq!(got[2].control, [92.78125, 68.0, 197.21875, 19.0]);
        assert_eq!(got[3].control, [92.78125, 94.0, 197.21875, 21.0]);
    }
}
