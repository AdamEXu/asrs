//! Text with line breaks in a table cell, as `innerText` leaves it: a text node per line of the
//! string with a `<br>` after each but the last.

use asrs_canvas::text::TextBackend;
use asrs_canvas::text::dom::{self, Lu, WhiteSpace};
use asrs_ui::text::TextStyle;

#[derive(Clone, Debug, PartialEq)]
pub struct CellLine {
    pub text: String,
    /// Where the text starts, from the content box's left edge.
    pub left: Lu,
    pub top: Lu,
    pub width: Lu,
    /// The text box's top, from the content box's.
    pub box_top: Lu,
    pub box_height: Lu,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CellText {
    pub lines: Vec<CellLine>,
    pub width: Lu,
    pub height: Lu,
}

pub const fn px(whole: i32) -> Lu {
    whole * dom::LU_PER_PX
}

/// A CSS length in device `LayoutUnit`s.
pub fn length(css: f64, scale: f32) -> Lu {
    (css as f32 * scale * 64.0) as Lu
}

/// The used width of a `1px` border.
pub fn border(scale: f32) -> Lu {
    px(scale.floor().max(1.0) as i32)
}

pub fn css(lu: Lu, scale: f32) -> f64 {
    f64::from(lu) / 64.0 / f64::from(scale)
}

impl CellText {
    /// Lays `text` out in a content box `available` wide, or at its max-content width.
    pub fn new(
        backend: &mut dyn TextBackend,
        style: &TextStyle,
        text: &str,
        available: Option<Lu>,
        scale: f32,
    ) -> CellText {
        let mut paragraphs: Vec<&str> = text.split('\n').collect();
        let blank = |p: &&str| dom::collapse(p, WhiteSpace::Normal)[0].is_empty();
        if paragraphs.last().is_some_and(blank) {
            paragraphs.pop();
        }
        let (ascent, descent, strut) = dom::line_metrics(backend, style, scale);
        let mut out = CellText::default();
        for paragraph in paragraphs {
            let layout = dom::layout(backend, style, paragraph, available, scale);
            out.width = out.width.max(layout.width);
            for line in &layout.lines {
                out.lines.push(CellLine {
                    text: line.fragments.iter().map(|f| f.text.as_str()).collect(),
                    left: line.fragments.iter().map(|f| f.x).min().unwrap_or(0),
                    top: out.height + line.top,
                    width: line.width,
                    box_top: out.height + line.baseline - ascent,
                    box_height: ascent + descent,
                });
            }
            out.height += if layout.lines.is_empty() {
                strut
            } else {
                layout.height
            };
        }
        out
    }

    /// The width of the widest part of `text` that cannot be broken.
    pub fn min_content(
        backend: &mut dyn TextBackend,
        style: &TextStyle,
        text: &str,
        scale: f32,
    ) -> Lu {
        let mut style = *style;
        style.break_word = false;
        CellText::new(backend, &style, text, Some(0), scale).width
    }
}
