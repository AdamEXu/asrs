//! Blink's layout in device px: the layout zoom includes the display's scale, so lengths are
//! rounded to `LayoutUnit`s, 1/64 device px. Boxes of a view are laid out with these, from rects in
//! CSS px, and scaled back, which is exact at every scale.

use asrs_js::math;

/// A length as `LayoutUnit(float)` truncates it.
pub fn lu(v: f64) -> f64 {
    math::trunc(v * 64.0) / 64.0
}

/// The zoomed length of `v` CSS px in a style.
pub fn zoomed(v: f64, scale: f64) -> f64 {
    lu(f64::from((v * scale) as f32))
}

/// `percent` of `v` as `MinimumValueForLength` resolves it, in single precision.
pub fn percent(v: f64, percent: f64) -> f64 {
    lu(f64::from((v * percent / 100.0) as f32))
}

/// The device px of a laid out CSS length, which is a whole number of LayoutUnits but for the
/// division by the scale.
pub fn device(v: f64, scale: f64) -> f64 {
    math::floor(v * scale * 64.0 + 0.5) / 64.0
}

/// `v` on the device pixel grid, in CSS px.
pub fn snap(v: f64, scale: f64) -> f64 {
    math::floor(device(v, scale) + 0.5) / scale
}

/// The device px of a border `width` CSS px wide: Blink floors it to whole ones, at least one.
pub fn border(width: f64, scale: f64) -> f64 {
    math::max2(math::floor(width * scale), 1.0)
}

/// What a line of `content` is moved by to the middle of `available`.
pub fn centred(available: f64, content: f64) -> f64 {
    lu(math::max2(available - content, 0.0) / 2.0)
}
