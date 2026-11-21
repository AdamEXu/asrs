// Ported from color-convert 2.0.1 (MIT, Copyright (c) 2011-2016 Heather Arthur). See THIRD_PARTY_NOTICES.md.

//! `color-convert` 2.0.1, the subset upstream uses: `hex.rgb`, `hex.hsl`
//! (hex -> rgb -> hsl) and `hsl.hex` (hsl -> rgb -> hex), plus the one
//! composition in `src/shared/Colors.ts`. Ported from the npm package's
//! `conversions.js` and `index.js`: the exported conversions are `wrapRounded`
//! (each element of an array result is `Math.round`ed); the same function
//! without rounding is exposed as `.raw`.

use crate::math;
use crate::math::int::to_int32;

/// `hex.rgb`: first match of `/[a-f0-9]{6}|[a-f0-9]{3}/i`, three digits expand
/// to six, no match gives black. Elements are already integers, so `wrapRounded`
/// changes nothing.
pub fn hex_rgb(hex: &str) -> [u8; 3] {
    let bytes = hex.as_bytes();
    for start in 0..bytes.len() {
        let rest = &bytes[start..];
        if rest.len() >= 6 && rest[..6].iter().all(|b| b.is_ascii_hexdigit()) {
            let n = u32::from_str_radix(str::from_utf8(&rest[..6]).unwrap_or("0"), 16).unwrap_or(0);
            return [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        }
        if rest.len() >= 3 && rest[..3].iter().all(|b| b.is_ascii_hexdigit()) {
            let nibble = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
            return [
                nibble(rest[0]) * 17,
                nibble(rest[1]) * 17,
                nibble(rest[2]) * 17,
            ];
        }
    }
    [0, 0, 0]
}

/// `hex.hsl`: `Math.round` on each element of `hex.hsl.raw`.
pub fn hex_hsl(hex: &str) -> [f64; 3] {
    let [h, s, l] = hex_hsl_raw(hex);
    [math::round(h), math::round(s), math::round(l)]
}

/// `hex.hsl.raw`: `rgb.hsl` over `hex.rgb`, raw floats.
pub fn hex_hsl_raw(hex: &str) -> [f64; 3] {
    rgb_hsl(hex_rgb(hex).map(f64::from))
}

/// `rgb.hsl` (and `.raw`: the elements are already integral when the inputs
/// are, so the two exports coincide on byte channels): `[h, s * 100, l * 100]`
/// unrounded.
pub fn rgb_hsl(rgb: [f64; 3]) -> [f64; 3] {
    let r = rgb[0] / 255.0;
    let g = rgb[1] / 255.0;
    let b = rgb[2] / 255.0;
    let min = math::min(&[r, g, b]);
    let max = math::max(&[r, g, b]);
    let delta = max - min;
    let h = if max == min {
        0.0
    } else if r == max {
        (g - b) / delta
    } else if g == max {
        2.0 + (b - r) / delta
    } else {
        4.0 + (r - g) / delta
    };
    let h = math::min2(h * 60.0, 360.0);
    let h = if h < 0.0 { h + 360.0 } else { h };
    let l = (min + max) / 2.0;
    let s = if max == min {
        0.0
    } else if l <= 0.5 {
        delta / (max + min)
    } else {
        delta / (2.0 - max - min)
    };
    [h, s * 100.0, l * 100.0]
}

/// `hsl.rgb.raw`: raw floats.
pub fn hsl_rgb(hsl: [f64; 3]) -> [f64; 3] {
    let h = hsl[0] / 360.0;
    let s = hsl[1] / 100.0;
    let l = hsl[2] / 100.0;
    if s == 0.0 {
        let val = l * 255.0;
        return [val, val, val];
    }
    let t2 = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let t1 = 2.0 * l - t2;
    let mut rgb = [0.0; 3];
    for (i, slot) in rgb.iter_mut().enumerate() {
        let mut t3 = h + 1.0 / 3.0 * -(i as f64 - 1.0);
        if t3 < 0.0 {
            t3 += 1.0;
        }
        if t3 > 1.0 {
            t3 -= 1.0;
        }
        let val = if 6.0 * t3 < 1.0 {
            t1 + (t2 - t1) * 6.0 * t3
        } else if 2.0 * t3 < 1.0 {
            t2
        } else if 3.0 * t3 < 2.0 {
            t1 + (t2 - t1) * (2.0 / 3.0 - t3) * 6.0
        } else {
            t1
        };
        *slot = val * 255.0;
    }
    rgb
}

/// `rgb.hex`: `Math.round` each channel, mask to 8 bits, six uppercase digits.
pub fn rgb_hex(rgb: [f64; 3]) -> String {
    let r = to_int32(math::round(rgb[0])) & 0xFF;
    let g = to_int32(math::round(rgb[1])) & 0xFF;
    let b = to_int32(math::round(rgb[2])) & 0xFF;
    format!("{:06X}", (r << 16) + (g << 8) + b)
}

/// `hsl.hex`: `rgb.hex` over `hsl.rgb`. The result is a string, so
/// `wrapRounded`'s array loop never runs on it.
pub fn hsl_hex(hsl: [f64; 3]) -> String {
    rgb_hex(hsl_rgb(hsl))
}

/// `src/shared/Colors.ts ensureThemeContrast` with `darkMode` given
/// (the `window.matchMedia` default is the caller's problem).
pub fn ensure_theme_contrast(color: &str, dark_mode: bool) -> String {
    const DARK_MIN_LIGHTNESS: f64 = 65.0;
    const LIGHT_MAX_LIGHTNESS: f64 = 45.0;
    let stripped = &color[color.char_indices().nth(1).map_or(color.len(), |(i, _)| i)..];
    let mut hsl = hex_hsl(stripped);
    hsl[2] = if dark_mode {
        math::max2(hsl[2], DARK_MIN_LIGHTNESS)
    } else {
        math::min2(hsl[2], LIGHT_MAX_LIGHTNESS)
    };
    format!("#{h}", h = hsl_hex(hsl))
}
