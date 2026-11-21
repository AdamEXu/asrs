// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

use asrs_js::math;

pub const DEG2RAD: f64 = math::PI / 180.0;
pub const RAD2DEG: f64 = 180.0 / math::PI;

pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    math::max2(min, math::min2(max, value))
}

pub fn euclidean_modulo(n: f64, m: f64) -> f64 {
    math::rem(math::rem(n, m) + m, m)
}

pub fn lerp(x: f64, y: f64, t: f64) -> f64 {
    (1.0 - t) * x + t * y
}

pub fn deg_to_rad(degrees: f64) -> f64 {
    degrees * DEG2RAD
}

pub fn rad_to_deg(radians: f64) -> f64 {
    radians * RAD2DEG
}
