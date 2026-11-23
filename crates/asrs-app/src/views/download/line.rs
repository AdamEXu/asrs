//! One line of inline text runs in a box with `white-space: nowrap; text-overflow: ellipsis`, as
//! Blink's line truncator cuts it.

use asrs_ui::gpui::{AnyElement, App, IntoElement, ParentElement, Styled, div, px};
use asrs_ui::text::{self, Text, TextStyle, snap_remainder};

const ELLIPSIS: &str = "\u{2026}";

pub struct InlineRun {
    pub text: String,
    /// With `white-space: pre`: the line collapses white space across its runs itself.
    pub style: TextStyle,
    pub color: u32,
    pub underline: bool,
}

/// The runs' texts with each run of white space collapsed to its first space.
fn collapsed(runs: &[InlineRun], mut after_space: bool) -> Vec<String> {
    runs.iter()
        .map(|run| {
            let mut out = String::with_capacity(run.text.len());
            for c in run.text.chars() {
                let space = matches!(c, ' ' | '\t' | '\n' | '\r');
                if !(space && after_space) {
                    out.push(if space { ' ' } else { c });
                }
                after_space = space;
            }
            out
        })
        .collect()
}

/// Elements for the runs, in a box `available` wide whose left edge is `box_left` from the
/// window's and whose first run starts at `left`, after an atomic inline when that is not 0. The
/// ellipsis takes `style`.
#[expect(clippy::too_many_arguments)]
pub fn line(
    runs: &[InlineRun],
    style: &TextStyle,
    left: f64,
    top: f64,
    available: f64,
    box_left: f64,
    scale: f32,
    cx: &mut App,
) -> Vec<AnyElement> {
    let texts = collapsed(runs, left == 0.0);
    let widths: Vec<f64> = runs
        .iter()
        .zip(&texts)
        .map(|(run, text)| text::measure(cx, &run.style, text, scale).0)
        .collect();
    let overflows = left + widths.iter().sum::<f64>() > available;
    let ellipsis = text::measure(cx, style, ELLIPSIS, scale).0;
    let limit = available - ellipsis;
    let place = |x: f64, text: Text| {
        let remainder = snap_remainder((box_left + x) as f32, scale);
        let y = snap_remainder(top as f32, scale);
        div()
            .absolute()
            .left(px(x as f32))
            .top(px(top as f32))
            .child(text.subpixel(remainder, y))
            .into_any_element()
    };
    let mut out = Vec::new();
    let (mut x, mut cut) = (left, false);
    for ((run, text), width) in runs.iter().zip(texts).zip(widths) {
        let mut element = Text::new(text.clone(), run.style).color(run.color);
        if run.underline {
            element = element.underline();
        }
        if cut {
            // It paints nothing, but a capture records its box as the DOM dump does.
            if cfg!(feature = "parity") {
                out.push(place(x, element.cut(0.0)));
            }
        } else if !overflows || x + width <= limit {
            out.push(place(x, element));
        } else {
            cut = true;
            let room = (limit - x).max(0.0);
            let kept = text::cut_width(cx, &run.style, &text, scale, room);
            out.push(place(x, element.cut(room)));
            let dots = Text::new(ELLIPSIS, *style)
                .color(runs[0].color)
                .form_control();
            out.push(place(x + kept, dots));
        }
        x += width;
    }
    out
}
