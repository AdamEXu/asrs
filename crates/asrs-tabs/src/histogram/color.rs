// Ported from @kurkle/color 0.3.2 (MIT, Copyright (c) 2018-2021 Jukka Kurkela). See THIRD_PARTY_NOTICES.md.

//! The `@kurkle/color` operations Chart.js uses for hover colours and colour animations, on the
//! colour strings that reach it: hex from `ensureThemeContrast` and earlier results, and
//! [`super::chart::DEFAULT_BORDER`].

use asrs_js::math::{self, max2, min2};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

fn round(v: f64) -> f64 {
    f64::from(math::to_int32(v + 0.5))
}

fn lim(v: f64, l: f64, h: f64) -> f64 {
    max2(min2(v, h), l)
}

fn n2b(v: f64) -> f64 {
    lim(round(v * 255.0), 0.0, 255.0)
}

fn b2n(v: f64) -> f64 {
    lim(round(v / 2.55) / 100.0, 0.0, 1.0)
}

fn low_byte(v: f64) -> f64 {
    f64::from(math::to_int32(v) & 0xff)
}

impl Color {
    pub fn parse(css: &str) -> Option<Color> {
        if css == super::chart::DEFAULT_BORDER {
            return Some(Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 25.5,
            });
        }
        let digits: Vec<f64> = css
            .strip_prefix('#')?
            .chars()
            .map(|c| c.to_digit(16).map(f64::from))
            .collect::<Option<_>>()?;
        let pair = |i: usize| digits[i] * 16.0 + digits[i + 1];
        let twice = |i: usize| digits[i] * 17.0;
        Some(match digits.len() {
            3 | 4 => Color {
                r: twice(0),
                g: twice(1),
                b: twice(2),
                a: if digits.len() == 4 { twice(3) } else { 255.0 },
            },
            6 | 8 => Color {
                r: pair(0),
                g: pair(2),
                b: pair(4),
                a: if digits.len() == 8 { pair(6) } else { 255.0 },
            },
            _ => return None,
        })
    }

    pub fn hex_string(&self) -> String {
        let bytes = [self.r, self.g, self.b, self.a].map(|v| math::to_int32(v) & 0xff);
        let short = bytes.iter().all(|b| b >> 4 == b & 0xf);
        let channels = if self.a < 255.0 { 4 } else { 3 };
        let mut out = "#".to_owned();
        for b in &bytes[..channels] {
            if short {
                out.push_str(&format!("{:X}", b & 0xf));
            } else {
                out.push_str(&format!("{b:02X}"));
            }
        }
        out
    }

    fn to_hsl(self) -> [f64; 3] {
        let (r, g, b) = (self.r / 255.0, self.g / 255.0, self.b / 255.0);
        let max = max2(max2(r, g), b);
        let min = min2(min2(r, g), b);
        let l = (max + min) / 2.0;
        if max == min {
            return [0.0, 0.0, l];
        }
        let d = max - min;
        let s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        let h = if r == max {
            (g - b) / d + if g < b { 6.0 } else { 0.0 }
        } else if g == max {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        };
        [f64::from(math::to_int32(h * 60.0 + 0.5)), s, l]
    }

    fn set_hsl(&mut self, [h, s, l]: [f64; 3]) {
        let a = s * min2(l, 1.0 - l);
        let f = |n: f64| {
            let k = math::rem(n + h / 30.0, 12.0);
            l - a * max2(min2(min2(k - 3.0, 9.0 - k), 1.0), -1.0)
        };
        (self.r, self.g, self.b) = (n2b(f(0.0)), n2b(f(8.0)), n2b(f(4.0)));
    }

    fn saturate(&mut self, ratio: f64) {
        let mut hsl = self.to_hsl();
        hsl[1] = max2(0.0, min2(hsl[1] + hsl[1] * ratio, 1.0));
        self.set_hsl(hsl);
    }

    fn darken(&mut self, ratio: f64) {
        let mut hsl = self.to_hsl();
        hsl[2] = max2(0.0, min2(hsl[2] + hsl[2] * -ratio, 1.0));
        self.set_hsl(hsl);
    }

    /// `this.mix(other, weight)`.
    fn mix(self, other: Color, weight: f64) -> Color {
        let (a1, a2) = (b2n(self.a), b2n(other.a));
        let w = 2.0 * weight - 1.0;
        let a = a1 - a2;
        let w1 = ((if w * a == -1.0 {
            w
        } else {
            (w + a) / (1.0 + w * a)
        }) + 1.0)
            / 2.0;
        let w2 = 1.0 - w1;
        Color {
            r: low_byte(w1 * self.r + w2 * other.r + 0.5),
            g: low_byte(w1 * self.g + w2 * other.g + 0.5),
            b: low_byte(w1 * self.b + w2 * other.b + 0.5),
            a: n2b(weight * a1 + (1.0 - weight) * a2),
        }
    }
}

/// `getHoverColor`. `None` is the `undefined` that `hexString` returns for an invalid colour.
pub fn hover(css: &str) -> Option<String> {
    let mut color = Color::parse(css)?;
    color.saturate(0.5);
    color.darken(0.1);
    Some(color.hex_string())
}

/// The `color` interpolator.
pub fn interpolate(from: &str, to: &str, factor: f64) -> String {
    match (Color::parse(from), Color::parse(to)) {
        (Some(c0), Some(c1)) => c1.mix(c0, factor).hex_string(),
        _ => to.to_owned(),
    }
}
