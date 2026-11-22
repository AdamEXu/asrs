//! Chromium's native theme (`ui/native_theme/native_theme_base.cc`, refreshed form controls)
//! for the parts upstream leaves at `appearance: auto`: checkbox and progress bar. Upstream sets
//! no `color-scheme`, so they keep their light colours in dark mode.

use gpui_kit::{
    App, Bounds, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    StatefulInteractiveElement, Styled, Window, canvas, div,
};

use crate::input::skia;
use crate::input::style::{
    ACCENT, NATIVE_BORDER, NATIVE_FILL, NATIVE_RADIUS, NATIVE_TRACK_BORDER, native_stroke,
};

/// `PaintTextField`: a white rounded rect, radius 2 zoomed px, in a `#767676` stroke one
/// zoomed pixel wide, both inset by half the stroke.
pub fn paint_text_field(bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let scale = window.scale_factor();
    let (w, h) = skia::device_size(bounds, scale);
    let stroke = f64::from(native_stroke(scale));
    let radius = f64::from(NATIVE_RADIUS * scale);
    let draw = || {
        let i = stroke / 2.0;
        let paint = format!(
            "{} {} stroke-width=\"{stroke}\"",
            skia::paint_attrs("fill", 0xffffffff),
            skia::paint_attrs("stroke", NATIVE_BORDER)
        );
        let body = skia::rect(
            i,
            i,
            f64::from(w) - stroke,
            f64::from(h) - stroke,
            radius,
            &paint,
        );
        skia::svg(w, h, 1.0, &body)
    };
    skia::paint(("text-field", w, h), bounds, draw, window, cx);
}

type Toggle = Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>;

/// `PaintCheckboxRadioCommon` and `PaintCheckbox`: the largest centred square, a white
/// rounded rect in a 1 px `#767676` stroke, or when checked an accent rounded rect with a white
/// check stroked at 16 % of its side.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    disabled: bool,
    on_toggle: Option<Toggle>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Checkbox {
            id: id.into(),
            checked,
            disabled: false,
            on_toggle: None,
        }
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Called with the new `checked` on click, as the `change` event.
    pub fn on_toggle(mut self, f: impl Fn(&bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_toggle = Some(Box::new(f));
        self
    }
}

pub fn paint_checkbox(bounds: Bounds<Pixels>, checked: bool, window: &mut Window, cx: &mut App) {
    let scale = window.scale_factor();
    let (w, h) = skia::device_size(bounds, scale);
    let (cw, ch) = (
        f64::from(w) / f64::from(scale),
        f64::from(h) / f64::from(scale),
    );
    let side = cw.min(ch);
    let (x, y) = ((cw - side) / 2.0, (ch - side) / 2.0);
    let r = f64::from(NATIVE_RADIUS);
    let draw = || {
        let bg = 0.2;
        let white = skia::paint_attrs("fill", 0xffffffff);
        let mut body = skia::rect(x + bg, y + bg, side - 2.0 * bg, side - 2.0 * bg, r, &white);
        if checked {
            body += &skia::rect(x, y, side, side, r, &skia::paint_attrs("fill", ACCENT));
            body += &format!(
                "<path d=\"M{} {} L{} {} L{} {}\" fill=\"none\" {} stroke-width=\"{}\"/>",
                x + side * 0.2,
                y + side * 0.5,
                x + side * 0.4,
                y + side * 0.7,
                x + side - side * 0.2,
                y + side * 0.2,
                skia::paint_attrs("stroke", 0xffffffff),
                side * 0.16
            );
        } else {
            let paint = format!(
                "fill=\"none\" {} stroke-width=\"1\"",
                skia::paint_attrs("stroke", NATIVE_BORDER)
            );
            body += &skia::rect(x + 0.5, y + 0.5, side - 1.0, side - 1.0, r, &paint);
        }
        skia::svg(w, h, f64::from(scale), &body)
    };
    skia::paint(("checkbox", w, h, checked), bounds, draw, window, cx);
}

impl RenderOnce for Checkbox {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let checked = self.checked;
        let mut root = div().id(self.id).size_full().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| paint_checkbox(bounds, checked, window, cx),
            )
            .size_full(),
        );
        if let (Some(f), false) = (self.on_toggle, self.disabled) {
            root = root.on_click(move |_, window, cx| f(&!checked, window, cx));
        }
        root
    }
}

/// `PaintProgressBar` for a determinate `<progress>` at `position` (0 to 1): a pill-shaped
/// `#efefef` track half the box's height, the accent value `floor(width * position)` device
/// px wide (at least 2) clipped to it, and a `#b2b2b2` stroke.
#[derive(IntoElement)]
pub struct Progress {
    position: f64,
}

impl Progress {
    pub fn new(position: f64) -> Self {
        Progress { position }
    }
}

pub fn paint_progress(bounds: Bounds<Pixels>, position: f64, window: &mut Window, cx: &mut App) {
    let scale = window.scale_factor();
    let (w, h) = skia::device_size(bounds, scale);
    let (wf, hf) = (f64::from(w), f64::from(h));
    let thickness = hf * 8.0 / 16.0;
    let top = (hf / 2.0 - thickness / 2.0).max(0.0);
    let bottom = (hf / 2.0 + thickness / 2.0).min(hf);
    let track_h = bottom - top;
    let mut value = (wf * position) as u32;
    if value > 0 {
        value = value.max(2);
    }
    let stroke = f64::from(native_stroke(scale));
    let draw = || {
        let r = wf.min(track_h) / 2.0;
        let fill = skia::rect(
            0.0,
            top,
            wf,
            track_h,
            r,
            &skia::paint_attrs("fill", NATIVE_FILL),
        );
        let mut pixmap = skia::svg(w, h, 1.0, &fill)?;
        if value > 0 {
            let pill = skia::rect(0.0, top, wf, track_h, r, &skia::paint_attrs("fill", ACCENT));
            let mut bar = skia::svg(w, h, 1.0, &pill)?;
            for row in bar.pixels_mut().chunks_mut(w as usize) {
                for p in row.iter_mut().skip(value as usize) {
                    *p = tiny_skia::PremultipliedColorU8::TRANSPARENT;
                }
            }
            skia::over(&mut pixmap, &bar);
        }
        let i = stroke / 2.0;
        let paint = format!(
            "fill=\"none\" {} stroke-width=\"{stroke}\"",
            skia::paint_attrs("stroke", NATIVE_TRACK_BORDER)
        );
        let (bw, bh) = (wf - stroke, track_h - stroke);
        let border = skia::rect(i, top + i, bw, bh, bw.min(bh) / 2.0, &paint);
        skia::over(&mut pixmap, &skia::svg(w, h, 1.0, &border)?);
        Some(pixmap)
    };
    skia::paint(("progress", w, h, value), bounds, draw, window, cx);
}

impl RenderOnce for Progress {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let position = self.position;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| paint_progress(bounds, position, window, cx),
        )
        .size_full()
    }
}
