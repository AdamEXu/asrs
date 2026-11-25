//! A focused field's selection and baseline: Chromium's selection colour, and glyphs on the rows
//! the unfocused field has them on.

#![allow(clippy::disallowed_methods)]

use std::ops::Range;
use std::sync::Arc;

use asrs_ui::capture::{Capture, HeadlessCapture};
use asrs_ui::input::{FieldKind, FieldStyle, PlainText, TextField};
use asrs_ui::selection::SelectAll;
use asrs_ui::selection::colors::{ForcedActive, SelectionColors};
use asrs_ui::theme::Appearance;
use gpui_kit::{
    AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Window, div, px, rgb, size,
};

const HEIGHT: f32 = 29.0;
const WIDTH: f32 = 200.0;

struct Page {
    body: FocusHandle,
    field: Entity<TextField<PlainText>>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.body)
            .bg(rgb(0xffffff))
            .child(self.field.clone())
    }
}

fn inner(scale: f32) -> (Range<u32>, Range<u32>) {
    let inset = (6.0 * scale) as u32;
    let end = |side: f32| (side * scale) as u32 - inset;
    (inset..end(WIDTH), inset / 2..end(HEIGHT) + inset / 2)
}

fn dark(shot: &Capture, x: u32, y: u32) -> bool {
    shot.pixel(x, y)[1] < 110
}

/// The columns of the glyphs, without the ends, where the caret is.
fn ink_columns(shot: &Capture, scale: f32) -> Range<u32> {
    let (columns, rows) = inner(scale);
    let inked: Vec<u32> = columns
        .filter(|&x| rows.clone().any(|y| dark(shot, x, y)))
        .collect();
    let end = (2.0 * scale) as u32;
    inked[0] + end..inked[inked.len() - 1] - end
}

/// Each row's ink, with rows of nothing around it to shift into.
fn ink(shot: &Capture, columns: &Range<u32>, scale: f32) -> Vec<f32> {
    let (_, rows) = inner(scale);
    let light = |x: u32, y: u32| f32::from(shot.pixel(x, y)[1]);
    let paper = light(columns.end + (20.0 * scale) as u32, rows.end / 2);
    rows.map(|y| columns.clone().map(|x| paper - light(x, y)).sum())
        .collect()
}

/// How many rows lower `b` is than `a`.
fn shift(a: &[f32], b: &[f32]) -> i32 {
    let at = |rows: &[f32], i: i32| usize::try_from(i).ok().and_then(|i| rows.get(i).copied());
    let difference = |by: i32| -> f32 {
        (-4..a.len() as i32 + 4)
            .map(|i| (at(a, i).unwrap_or(0.0) - at(b, i + by).unwrap_or(0.0)).abs())
            .sum()
    };
    (-4..=4)
        .min_by(|&x, &y| difference(x).total_cmp(&difference(y)))
        .expect("a shift")
}

fn over(colors: SelectionColors, behind: [u8; 4]) -> [u8; 3] {
    let c = colors.rgba();
    let blend = |v: f32, b: u8| (v * c.a * 255.0 + f32::from(b) * (1.0 - c.a)).round() as u8;
    [
        blend(c.r, behind[0]),
        blend(c.g, behind[1]),
        blend(c.b, behind[2]),
    ]
}

fn main() {
    if asrs_parity::ci::skip_in_ci("field selection", "a GPU") {
        return;
    }
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("field selection: skipped, no headless renderer on this platform");
        return;
    }
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    capture.context().update(asrs_ui::init);
    capture
        .context()
        .update(|cx| cx.set_global(ForcedActive(true)));
    let styles = [
        ("setting", FieldStyle::setting_block()),
        ("value", FieldStyle::field_value()),
        ("prompt", FieldStyle::prompt()),
        ("search", FieldStyle::sidebar_search(Appearance::Light)),
    ];
    let mut failures = Vec::new();
    for (name, style) in styles {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let window = capture
                .open_window(size(px(WIDTH), px(HEIGHT + 11.0)), scale, |window, cx| {
                    cx.new(|cx| Page {
                        body: cx.focus_handle(),
                        field: cx.new(|cx| {
                            let value = PlainText::new("gjpq");
                            let mut field =
                                TextField::new(value, FieldKind::Text, style, window, cx);
                            field.set_size(WIDTH, HEIGHT);
                            field
                        }),
                    })
                })
                .expect("a window");
            window
                .update(capture.context(), |page, window, cx| {
                    window.activate_window();
                    window.focus(&page.body, cx);
                })
                .expect("open");
            let shot = capture.capture(window.into()).expect("capture");
            let columns = ink_columns(&shot, scale);
            let plain = ink(&shot, &columns, scale);
            window
                .update(capture.context(), |page, window, cx| {
                    let handle = page.field.focus_handle(cx);
                    window.focus(&handle, cx);
                })
                .expect("open");
            let shot = capture.capture(window.into()).expect("capture");
            let focused = ink(&shot, &columns, scale);
            let by = shift(&plain, &focused);
            if by != 0 {
                failures.push(format!("{name} {scale}: {by} rows lower with focus"));
            }
            let below = plain.len() - plain.len() / 3;
            let depth = |rows: &[f32]| rows[below..].iter().sum::<f32>();
            let kept = depth(&focused) / depth(&plain);
            if !(0.8..1.3).contains(&kept) {
                failures.push(format!("{name} {scale}: descenders {kept}"));
            }

            window
                .update(capture.context(), |_, window, cx| {
                    window.dispatch_action(Box::new(SelectAll), cx);
                })
                .expect("open");
            capture.context().run_until_parked();
            let selected = capture.capture(window.into()).expect("capture");
            let colors = SelectionColors::new(true);
            let is_selected = |x: u32, y: u32| {
                let [r, g, b, _] = selected.pixel(x, y);
                let behind = shot.pixel(x, y);
                let want = over(colors, behind);
                let changed = selected.pixel(x, y) != behind;
                changed && [r, g, b].iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 2)
            };
            let found = (0..shot.height)
                .flat_map(|y| (0..shot.width).map(move |x| (x, y)))
                .filter(|&(x, y)| is_selected(x, y))
                .count();
            if found < (150.0 * scale * scale) as usize {
                failures.push(format!("{name} {scale}: {found} px selected"));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
    println!("field selection: ok");
}
