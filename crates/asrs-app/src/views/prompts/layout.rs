//! Where `prompts.css` and Blink's table layout put a prompt's parts, in client coordinates.

use asrs_ui::input::style::{CONTROL_FONT, device_border};
use asrs_ui::input::{FieldStyle, Frame};
use asrs_ui::text::{self, TextStyle};
use asrs_ui::theme::FONT_FAMILY;

use asrs_ui::gpui::App;

const TABLE_INSET: f64 = 10.0;
const ROW_HEIGHT: f64 = 27.0;
const CONTROL_MARGIN: f64 = 5.0;
const TITLE_HEIGHT: f64 = 34.0;
const SUBTITLE_HEIGHT: f64 = 27.0;
/// `td.title > hr`: its `margin-top` and its two 1 px borders.
const RULE_TOP: f64 = 5.0;
pub const RULE_HEIGHT: f64 = 2.0;
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

/// `td.title`'s text, under its `hr`.
pub fn title_style() -> TextStyle {
    let font = format!("900 14px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "27px"),
        ("white-space", "nowrap"),
    ])
    .expect("prompt title style")
}

/// `td.subtitle`.
pub fn subtitle_style() -> TextStyle {
    let font = format!("italic 12px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "20px"),
        ("white-space", "nowrap"),
    ])
    .expect("prompt subtitle style")
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

/// What a `table` row holds: a control this tall, or one of the headings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RowKind {
    Control(f64),
    Title,
    Subtitle,
}

/// The boxes of one `table` row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Row {
    /// `td.label`, `td.input` and the control in it.
    Control {
        label: [f64; 4],
        input: [f64; 4],
        control: [f64; 4],
    },
    /// `td.title`, its `hr` and the line under it.
    Title {
        cell: [f64; 4],
        rule: [f64; 4],
        line: [f64; 4],
    },
    /// `td.subtitle`, whose text is at its top.
    Subtitle { cell: [f64; 4] },
}

/// The table in a page `width` wide: the label column is as wide as its widest label, the other
/// takes the rest, each control is centred in its row, and headings span both columns.
pub fn rows(width: f64, label_widths: &[f64], kinds: &[RowKind]) -> Vec<Row> {
    let label_width = label_widths.iter().copied().fold(0.0, f64::max);
    let input_x = TABLE_INSET + label_width;
    let table_width = width - 2.0 * TABLE_INSET;
    let input_width = table_width - label_width;
    let mut y = TABLE_INSET;
    kinds
        .iter()
        .map(|kind| {
            let top = y;
            match *kind {
                RowKind::Control(height) => {
                    y += ROW_HEIGHT;
                    Row::Control {
                        label: [TABLE_INSET, top, label_width, ROW_HEIGHT],
                        input: [input_x, top, input_width, ROW_HEIGHT],
                        control: [
                            input_x + CONTROL_MARGIN,
                            top + (ROW_HEIGHT - height) / 2.0,
                            input_width - CONTROL_MARGIN,
                            height,
                        ],
                    }
                }
                RowKind::Title => {
                    y += TITLE_HEIGHT;
                    let line_top = RULE_TOP + RULE_HEIGHT;
                    Row::Title {
                        cell: [TABLE_INSET, top, table_width, TITLE_HEIGHT],
                        rule: [TABLE_INSET, top + RULE_TOP, table_width, RULE_HEIGHT],
                        line: [
                            TABLE_INSET,
                            top + line_top,
                            table_width,
                            TITLE_HEIGHT - line_top,
                        ],
                    }
                }
                RowKind::Subtitle => {
                    y += SUBTITLE_HEIGHT;
                    Row::Subtitle {
                        cell: [TABLE_INSET, top, table_width, SUBTITLE_HEIGHT],
                    }
                }
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
            &[19.0, 19.0, 19.0, 21.0].map(RowKind::Control),
        );
        let boxes = |i: usize| match got[i] {
            Row::Control {
                label,
                input,
                control,
            } => (label, input, control),
            other => panic!("{other:?}"),
        };
        assert_eq!(boxes(0).0, [10.0, 10.0, 77.78125, 27.0]);
        assert_eq!(boxes(0).1, [87.78125, 10.0, 202.21875, 27.0]);
        assert_eq!(boxes(2).2, [92.78125, 68.0, 197.21875, 19.0]);
        assert_eq!(boxes(3).2, [92.78125, 94.0, 197.21875, 21.0]);
    }
}
