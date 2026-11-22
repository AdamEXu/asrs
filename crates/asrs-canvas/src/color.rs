// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

/// A parsed CSS colour: straight-alpha channels in 0..=1, as Blink hands them to Skia.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Rgba {
    pub const BLACK: Rgba = Rgba::from_u8(0, 0, 0, 1.0);

    pub const fn from_u8(r: u8, g: u8, b: u8, a: f32) -> Rgba {
        Rgba {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a,
        }
    }

    pub fn to_u8(self) -> [u8; 3] {
        [self.r, self.g, self.b].map(|c| (c * 255.0 + 0.5) as u8)
    }

    pub fn with_alpha_scaled(self, alpha: f32) -> Rgba {
        Rgba {
            a: self.a * alpha,
            ..self
        }
    }
}

/// Parses a canvas `fillStyle`/`strokeStyle` string the way Blink's CSS colour parser does for
/// the syntaxes painters use: hex, named colours, `transparent`, `rgb[a]()` and `hsl[a]()` in
/// comma and space forms. `None` means the assignment is ignored.
pub fn parse(css: &str) -> Option<Rgba> {
    let s = css
        .trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c'))
        .to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(open) = s.find('(') {
        let name = &s[..open];
        // CSS closes a function left open at the end of input, so Blink takes
        // `rgb(0 0 0 / 10%` (Field2dRenderer's FTC grid colour) as written.
        let rest = &s[open + 1..];
        let body = rest.strip_suffix(')').unwrap_or(rest);
        return match name {
            "rgb" | "rgba" => parse_rgb(body),
            "hsl" | "hsla" => parse_hsl(body),
            _ => None,
        };
    }
    if s == "transparent" {
        return Some(Rgba::from_u8(0, 0, 0, 0.0));
    }
    let i = NAMED.binary_search_by(|(n, _)| n.cmp(&s.as_str())).ok()?;
    let v = NAMED[i].1;
    Some(Rgba::from_u8((v >> 16) as u8, (v >> 8) as u8, v as u8, 1.0))
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok();
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let alpha = |v: u8| f32::from(v) / 255.0;
    match hex.len() {
        3 | 4 => {
            let c: Vec<u8> = (0..hex.len())
                .map(|i| digit(i).map(|d| d * 17))
                .collect::<Option<_>>()?;
            Some(Rgba::from_u8(
                c[0],
                c[1],
                c[2],
                c.get(3).map_or(1.0, |&a| alpha(a)),
            ))
        }
        6 | 8 => {
            let c: Vec<u8> = (0..hex.len() / 2)
                .map(|i| pair(i * 2))
                .collect::<Option<_>>()?;
            Some(Rgba::from_u8(
                c[0],
                c[1],
                c[2],
                c.get(3).map_or(1.0, |&a| alpha(a)),
            ))
        }
        _ => None,
    }
}

enum Token {
    Number(f64),
    Percent(f64),
}

fn number(token: &str) -> Option<f64> {
    let valid = !token.is_empty()
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'+' | b'e'));
    if !valid {
        return None;
    }
    token.parse::<f64>().ok().filter(|v| v.is_finite())
}

fn token(raw: &str) -> Option<Token> {
    match raw.strip_suffix('%') {
        Some(p) => number(p).map(Token::Percent),
        None => number(raw).map(Token::Number),
    }
}

fn split_args(body: &str) -> Option<(Vec<&str>, Option<&str>)> {
    let is_space = |c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0c');
    if body.contains(',') {
        let parts: Vec<&str> = body.split(',').map(|p| p.trim_matches(is_space)).collect();
        return match parts.len() {
            3 => Some((parts, None)),
            4 => Some((parts[..3].to_vec(), Some(parts[3]))),
            _ => None,
        };
    }
    let (channels, alpha) = match body.split_once('/') {
        Some((c, a)) => (c, Some(a.trim_matches(is_space))),
        None => (body, None),
    };
    let parts: Vec<&str> = channels.split(is_space).filter(|p| !p.is_empty()).collect();
    (parts.len() == 3).then_some((parts, alpha))
}

fn alpha_value(raw: Option<&str>) -> Option<f32> {
    let a = match raw.map(token) {
        None => 1.0,
        Some(Some(Token::Number(v))) => v,
        Some(Some(Token::Percent(p))) => p / 100.0,
        Some(None) => return None,
    };
    Some(a.clamp(0.0, 1.0) as f32)
}

fn parse_rgb(body: &str) -> Option<Rgba> {
    let (parts, alpha) = split_args(body)?;
    let tokens: Vec<Token> = parts.iter().map(|p| token(p)).collect::<Option<_>>()?;
    let percent = matches!(tokens[0], Token::Percent(_));
    let mut c = [0u8; 3];
    for (slot, t) in c.iter_mut().zip(&tokens) {
        let v = match (t, percent) {
            (Token::Number(v), false) => *v,
            (Token::Percent(p), true) => p * 2.55,
            _ => return None,
        };
        *slot = (v.clamp(0.0, 255.0) + 0.5).floor() as u8;
    }
    Some(Rgba::from_u8(c[0], c[1], c[2], alpha_value(alpha)?))
}

fn parse_hsl(body: &str) -> Option<Rgba> {
    let (parts, alpha) = split_args(body)?;
    let hue = match parts[0]
        .strip_suffix("deg")
        .map_or_else(|| token(parts[0]), token)?
    {
        Token::Number(h) => h,
        Token::Percent(_) => return None,
    };
    let fraction = |raw: &str| match token(raw)? {
        Token::Percent(p) => Some((p / 100.0).clamp(0.0, 1.0)),
        Token::Number(_) => None,
    };
    let (s, l) = (fraction(parts[1])?, fraction(parts[2])?);
    let h = hue.rem_euclid(360.0) / 60.0;
    let t2 = if l <= 0.5 {
        l * (s + 1.0)
    } else {
        l + s - l * s
    };
    let t1 = l * 2.0 - t2;
    let channel = |h: f64| {
        let h = if h < 0.0 {
            h + 6.0
        } else if h >= 6.0 {
            h - 6.0
        } else {
            h
        };
        let v = if h < 1.0 {
            (t2 - t1) * h + t1
        } else if h < 3.0 {
            t2
        } else if h < 4.0 {
            (t2 - t1) * (4.0 - h) + t1
        } else {
            t1
        };
        (v * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8
    };
    Some(Rgba::from_u8(
        channel(h + 2.0),
        channel(h),
        channel(h - 2.0),
        alpha_value(alpha)?,
    ))
}

const NAMED: [(&str, u32); 148] = [
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

#[cfg(test)]
mod tests {
    use super::*;

    fn u8s(css: &str) -> Option<([u8; 3], f32)> {
        parse(css).map(|c| (c.to_u8(), c.a))
    }

    #[test]
    fn named_table_is_sorted() {
        assert!(NAMED.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn parses_the_scene_styles() {
        assert_eq!(u8s("#f00"), Some(([255, 0, 0], 1.0)));
        assert_eq!(u8s("#2B66A2"), Some(([0x2b, 0x66, 0xa2], 1.0)));
        assert_eq!(u8s("#2b66a280"), Some(([0x2b, 0x66, 0xa2], 128.0 / 255.0)));
        assert_eq!(u8s("Yellow"), Some(([255, 255, 0], 1.0)));
        assert_eq!(u8s("rgb(43, 102, 162)"), Some(([43, 102, 162], 1.0)));
        assert_eq!(u8s("rgba(0,0,0,0)"), Some(([0, 0, 0], 0.0)));
        assert_eq!(u8s("rgba(228,139,50,0.25)"), Some(([228, 139, 50], 0.25)));
        assert_eq!(u8s("rgb(10 20 30 / 50%)"), Some(([10, 20, 30], 0.5)));
        assert_eq!(u8s("hsl(120, 50%, 50%)"), Some(([64, 191, 64], 1.0)));
        assert_eq!(u8s("transparent"), Some(([0, 0, 0], 0.0)));
        assert_eq!(u8s("not a color"), None);
        assert_eq!(u8s("#12345"), None);
        assert_eq!(u8s("rgb(1, 2)"), None);
    }
}
