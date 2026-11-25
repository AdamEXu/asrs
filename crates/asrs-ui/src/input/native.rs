//! Chromium's native theme (`ui/native_theme/native_theme_base.cc`, refreshed form controls)
//! for the parts upstream leaves at `appearance: auto`: checkbox and progress bar. Upstream sets
//! no `color-scheme`, so they keep their light colours in dark mode.

use asrs_canvas::Pixmap;
use gpui_kit::{
    App, Bounds, ElementId, InteractiveElement, IntoElement, ParentElement, Pixels, RenderOnce,
    StatefulInteractiveElement, Styled, Window, canvas, div,
};

use crate::input::skia;
use crate::input::style::{
    ACCENT, DISABLED_BACKGROUND, DISABLED_BORDER, LIGHTEN_LAYER, NATIVE_BORDER, NATIVE_FILL,
    NATIVE_RADIUS, NATIVE_TRACK_BORDER, native_stroke,
};
use crate::svg::Backend;

/// `PaintTextField`: a white rounded rect, radius 2 zoomed px, in a `#767676` stroke one
/// zoomed pixel wide, both inset by half the stroke. Disabled, the fill is a translucent white
/// over a lightening layer and the stroke is translucent.
pub fn paint_text_field(
    bounds: Bounds<Pixels>,
    disabled: bool,
    pointer: PointerState,
    window: &mut Window,
    cx: &mut App,
) {
    let border_color = match pointer {
        PointerState::Normal => NATIVE_BORDER,
        PointerState::Hovered => HOVERED_BORDER,
        PointerState::Pressed => PRESSED_BORDER,
    };
    let scale = window.scale_factor();
    let (w, h) = skia::device_size(bounds, scale);
    let stroke = f64::from(native_stroke(scale));
    let radius = f64::from(NATIVE_RADIUS * scale);
    let draw_disabled = || {
        let i = stroke / 2.0;
        let (fw, fh) = (f64::from(w) - stroke, f64::from(h) - stroke);
        let lighten = skia::rect(
            i,
            i,
            fw,
            fh,
            radius,
            &skia::paint_attrs("fill", LIGHTEN_LAYER),
        );
        let mut pixmap = skia::svg(w, h, 1.0, &lighten)?;
        skia::fill_aliased_rrect(&mut pixmap, [i, i, fw, fh], radius, DISABLED_BACKGROUND);
        if Backend::HOST == Backend::Raster {
            let rect = [i, i, fw, fh];
            skia::stroke_aliased_rrect(&mut pixmap, rect, radius, stroke, DISABLED_BORDER);
            return Some(pixmap);
        }
        let paint = format!(
            "fill=\"none\" {} stroke-width=\"{stroke}\"",
            skia::paint_attrs("stroke", DISABLED_BORDER)
        );
        let border = skia::rect(i, i, fw, fh, radius, &paint);
        skia::over(&mut pixmap, &skia::svg(w, h, 1.0, &border)?);
        Some(pixmap)
    };
    let draw = || {
        if disabled {
            return draw_disabled();
        }
        let i = stroke / 2.0;
        if Backend::HOST == Backend::Raster {
            let rect = [i, i, f64::from(w) - stroke, f64::from(h) - stroke];
            let mut pixmap = Pixmap::new(w, h)?;
            skia::fill_aliased_rrect(&mut pixmap, rect, radius, 0xffffffff);
            skia::stroke_aliased_rrect(&mut pixmap, rect, radius, stroke, border_color);
            return Some(pixmap);
        }
        let paint = format!(
            "{} {} stroke-width=\"{stroke}\"",
            skia::paint_attrs("fill", 0xffffffff),
            skia::paint_attrs("stroke", border_color)
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
    let key = ("text-field", w, h, disabled, pointer);
    skia::paint(key, bounds, draw, window, cx);
}

/// What the pointer does to a control, which the theme's colours follow.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PointerState {
    #[default]
    Normal,
    Hovered,
    Pressed,
}

const HOVERED_BORDER: u32 = 0x4f4f4fff;
const PRESSED_BORDER: u32 = 0x8d8d8dff;
const HOVERED_ACCENT: u32 = 0x005cc8ff;
const PRESSED_ACCENT: u32 = 0x3793ffff;

type Toggle = Box<dyn Fn(&bool, &mut Window, &mut App) + 'static>;

/// `PaintCheckboxRadioCommon` and `PaintCheckbox`: the largest centred square, a white
/// rounded rect in a 1 px `#767676` stroke, or when checked an accent rounded rect with a white
/// check stroked at 16 % of its side.
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    checked: bool,
    disabled: bool,
    pointer: PointerState,
    on_toggle: Option<Toggle>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>, checked: bool) -> Self {
        Checkbox {
            id: id.into(),
            checked,
            disabled: false,
            pointer: PointerState::Normal,
            on_toggle: None,
        }
    }

    pub fn pointer(mut self, pointer: PointerState) -> Self {
        self.pointer = pointer;
        self
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

pub fn paint_checkbox(
    bounds: Bounds<Pixels>,
    checked: bool,
    pointer: PointerState,
    window: &mut Window,
    cx: &mut App,
) {
    let (accent, border) = match pointer {
        PointerState::Normal => (ACCENT, NATIVE_BORDER),
        PointerState::Hovered => (HOVERED_ACCENT, HOVERED_BORDER),
        PointerState::Pressed => (PRESSED_ACCENT, PRESSED_BORDER),
    };
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
            body += &skia::rect(x, y, side, side, r, &skia::paint_attrs("fill", accent));
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
                skia::paint_attrs("stroke", border)
            );
            body += &skia::rect(x + 0.5, y + 0.5, side - 1.0, side - 1.0, r, &paint);
        }
        skia::svg(w, h, f64::from(scale), &body)
    };
    skia::paint(
        ("checkbox", w, h, checked, pointer),
        bounds,
        draw,
        window,
        cx,
    );
}

impl RenderOnce for Checkbox {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (checked, pointer) = (self.checked, self.pointer);
        let mut root = div().id(self.id).size_full().child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| paint_checkbox(bounds, checked, pointer, window, cx),
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
        let track = pixmap.clone();
        if value > 0 {
            let pill = skia::rect(0.0, top, wf, track_h, r, &skia::paint_attrs("fill", ACCENT));
            let mut bar = skia::svg(w, h, 1.0, &pill)?;
            for row in bar.pixels_mut().chunks_mut(w as usize) {
                for p in row.iter_mut().skip(value as usize) {
                    *p = tiny_skia::PremultipliedColorU8::TRANSPARENT;
                }
            }
            if Backend::HOST == Backend::Raster {
                skia::clipped(&mut bar, &track);
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
        let mut border = skia::svg(w, h, 1.0, &border)?;
        if Backend::HOST == Backend::Raster {
            skia::clipped(&mut border, &track);
        }
        skia::over(&mut pixmap, &border);
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
