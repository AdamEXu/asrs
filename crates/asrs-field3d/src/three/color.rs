use std::sync::LazyLock;

use asrs_js::math;
use asrs_js::number::{parse_float, parse_int};
use asrs_js::regex::{Match, RegExp};
use asrs_js::string::JsString;

use super::math_utils::{clamp, euclidean_modulo};

/// A three `Color` in the linear-sRGB working space, with `ColorManagement` enabled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Default for Color {
    fn default() -> Self {
        Color::WHITE
    }
}

pub fn srgb_to_linear(c: f64) -> f64 {
    if c < 0.04045 {
        c * 0.0773993808
    } else {
        math::pow(c * 0.9478672986 + 0.0521327014, 2.4)
    }
}

pub fn linear_to_srgb(c: f64) -> f64 {
    if c < 0.0031308 {
        c * 12.92
    } else {
        1.055 * math::pow(c, 0.41666) - 0.055
    }
}

fn hue2rgb(p: f64, q: f64, mut t: f64) -> f64 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * 6.0 * (2.0 / 3.0 - t);
    }
    p
}

fn regex(pattern: &str) -> RegExp {
    RegExp::new(pattern, "").unwrap_or_else(|e| panic!("bad pattern {pattern}: {e:?}"))
}

static FUNCTION: LazyLock<RegExp> = LazyLock::new(|| regex(r"^(\w+)\(([^\)]*)\)"));
static RGB: LazyLock<RegExp> =
    LazyLock::new(|| regex(r"^\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*(\d*\.?\d+)\s*)?$"));
static RGB_PERCENT: LazyLock<RegExp> =
    LazyLock::new(|| regex(r"^\s*(\d+)\%\s*,\s*(\d+)\%\s*,\s*(\d+)\%\s*(?:,\s*(\d*\.?\d+)\s*)?$"));
static HSL: LazyLock<RegExp> = LazyLock::new(|| {
    regex(r"^\s*(\d*\.?\d+)\s*,\s*(\d*\.?\d+)\%\s*,\s*(\d*\.?\d+)\%\s*(?:,\s*(\d*\.?\d+)\s*)?$")
});
static HEX: LazyLock<RegExp> = LazyLock::new(|| regex(r"^\#([A-Fa-f\d]+)$"));

fn group(m: &Match, input: &[u16], i: usize) -> Vec<u16> {
    m.group(input, i)
        .map(JsString::into_units)
        .unwrap_or_default()
}

impl Color {
    pub const WHITE: Color = Color {
        r: 1.0,
        g: 1.0,
        b: 1.0,
    };

    /// `new Color(hex)`.
    pub fn hex(hex: f64) -> Self {
        let mut c = Color::WHITE;
        c.set_hex(hex);
        c
    }

    /// `new Color(style)`.
    pub fn style(style: &JsString) -> Self {
        let mut c = Color::WHITE;
        c.set_style(style);
        c
    }

    /// `setRGB` in the working color space.
    pub const fn linear(r: f64, g: f64, b: f64) -> Self {
        Color { r, g, b }
    }

    pub fn from_array(a: [f64; 3]) -> Self {
        Color::linear(a[0], a[1], a[2])
    }

    pub fn to_array(self) -> [f64; 3] {
        [self.r, self.g, self.b]
    }

    fn set_srgb(&mut self, r: f64, g: f64, b: f64) {
        self.r = srgb_to_linear(r);
        self.g = srgb_to_linear(g);
        self.b = srgb_to_linear(b);
    }

    pub fn set_hex(&mut self, hex: f64) {
        let hex = math::to_int32(math::floor(hex));
        self.set_srgb(
            f64::from((hex >> 16) & 255) / 255.0,
            f64::from((hex >> 8) & 255) / 255.0,
            f64::from(hex & 255) / 255.0,
        );
    }

    pub fn set_hsl(&mut self, h: f64, s: f64, l: f64) {
        let h = euclidean_modulo(h, 1.0);
        let s = clamp(s, 0.0, 1.0);
        let l = clamp(l, 0.0, 1.0);
        let (r, g, b) = if s == 0.0 {
            (l, l, l)
        } else {
            let p = if l <= 0.5 {
                l * (1.0 + s)
            } else {
                l + s - (l * s)
            };
            let q = (2.0 * l) - p;
            (
                hue2rgb(q, p, h + 1.0 / 3.0),
                hue2rgb(q, p, h),
                hue2rgb(q, p, h - 1.0 / 3.0),
            )
        };
        self.set_srgb(r, g, b);
    }

    pub fn set_style(&mut self, style: &JsString) {
        let input = style.units();
        if let Some(m) = FUNCTION.find_from(input, 0) {
            let name = String::from_utf16_lossy(&group(&m, input, 1));
            let components = group(&m, input, 2);
            let int = |m: &Match, i: usize, max: f64| {
                math::min2(max, parse_int(&group(m, &components, i), 10.0)) / max
            };
            let float = |m: &Match, i: usize| parse_float(&group(m, &components, i));
            match name.as_str() {
                "rgb" | "rgba" => {
                    if let Some(c) = RGB.find_from(&components, 0) {
                        self.set_srgb(int(&c, 1, 255.0), int(&c, 2, 255.0), int(&c, 3, 255.0));
                    } else if let Some(c) = RGB_PERCENT.find_from(&components, 0) {
                        self.set_srgb(int(&c, 1, 100.0), int(&c, 2, 100.0), int(&c, 3, 100.0));
                    }
                }
                "hsl" | "hsla" => {
                    if let Some(c) = HSL.find_from(&components, 0) {
                        self.set_hsl(
                            float(&c, 1) / 360.0,
                            float(&c, 2) / 100.0,
                            float(&c, 3) / 100.0,
                        );
                    }
                }
                _ => {}
            }
        } else if let Some(m) = HEX.find_from(input, 0) {
            let hex = group(&m, input, 1);
            match hex.len() {
                3 => {
                    let digit = |i: usize| parse_int(&hex[i..=i], 16.0) / 15.0;
                    self.set_srgb(digit(0), digit(1), digit(2));
                }
                6 => self.set_hex(parse_int(&hex, 16.0)),
                _ => {}
            }
        } else if !input.is_empty() {
            let lower = style.to_lower_case();
            let Some(name) = lower.as_string() else {
                return;
            };
            if let Ok(i) = KEYWORDS.binary_search_by(|(k, _)| (*k).cmp(name.as_str())) {
                self.set_hex(f64::from(KEYWORDS[i].1));
            } else if name == "constructor" || name == "__proto__" {
                // The keyword table is a plain object, so these lowercase names find an
                // inherited function or object, which `setHex` floors to NaN and then to black.
                self.set_hex(f64::NAN);
            }
        }
    }

    pub fn get_hex(self) -> f64 {
        let channel = |c: f64| math::round(clamp(linear_to_srgb(c) * 255.0, 0.0, 255.0));
        channel(self.r) * 65536.0 + channel(self.g) * 256.0 + channel(self.b)
    }
}

const KEYWORDS: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];
