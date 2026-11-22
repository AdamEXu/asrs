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

/// A colour as Blink's `Color` holds it for the syntaxes ported here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Parsed {
    /// `kSRGBLegacy` (hex, named, `rgb()`): channels in 0..=255.
    Legacy([f32; 3], f32),
    /// `kHSL`, already through `HSLToSRGB`: channels in 0..=1.
    Hsl([f32; 3], f32),
}

impl Parsed {
    pub fn alpha(self) -> f32 {
        match self {
            Parsed::Legacy(_, a) | Parsed::Hsl(_, a) => a,
        }
    }

    /// `toSkColor4f()`, what a canvas paints with.
    pub fn to_rgba(self) -> Rgba {
        let ([r, g, b], a) = match self {
            Parsed::Legacy(c, a) => (c.map(|v| (f64::from(v) / 255.0) as f32), a),
            Parsed::Hsl(c, a) => (c, a),
        };
        Rgba { r, g, b, a }
    }

    /// The channels `SerializeLegacyColorAsCSSColor` writes.
    pub fn css_channels(self) -> [u8; 3] {
        let channels = match self {
            Parsed::Legacy(c, _) => c,
            Parsed::Hsl(c, _) => c.map(|v| (f64::from(v + 1e-7) * 255.0) as f32),
        };
        channels.map(|v| f64::from(v).clamp(0.0, 255.0).round() as u8)
    }
}

/// Parses a canvas `fillStyle`/`strokeStyle` string the way Blink's CSS colour parser does for
/// the syntaxes painters use: hex, named colours, `transparent`, `rgb[a]()` and `hsl[a]()` in
/// comma and space forms. `None` means the assignment is ignored.
pub fn parse(css: &str) -> Option<Rgba> {
    parse_color(css).map(Parsed::to_rgba)
}

pub fn parse_color(css: &str) -> Option<Parsed> {
    if let Some(parsed) = parse_fast(css) {
        return Some(parsed);
    }
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
        return match name {
            "rgb" | "rgba" => match parse_rgb_tokens(rest) {
                Slow::Valid(parsed) => Some(parsed),
                Slow::Invalid | Slow::Unmodeled => None,
            },
            "hsl" | "hsla" => parse_hsl(rest.strip_suffix(')').unwrap_or(rest)),
            _ => None,
        };
    }
    if s == "transparent" {
        return Some(Parsed::Legacy([0.0; 3], 0.0));
    }
    named(&s).map(|v| Parsed::Legacy(v.map(f32::from), 1.0))
}

/// The `[r, g, b]` of a named colour, `name` in lower case.
pub fn named(name: &str) -> Option<[u8; 3]> {
    let i = NAMED.binary_search_by(|(n, _)| n.cmp(&name)).ok()?;
    let v = NAMED[i].1;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// `Color::ParseHexColor` on the digits after `#`.
pub fn parse_hex(hex: &str) -> Option<Parsed> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(&hex[i..=i], 16).ok();
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    let c: Vec<u8> = match hex.len() {
        3 | 4 => (0..hex.len())
            .map(|i| digit(i).map(|d| d * 17))
            .collect::<Option<_>>()?,
        6 | 8 => (0..hex.len() / 2)
            .map(|i| pair(i * 2))
            .collect::<Option<_>>()?,
        _ => return None,
    };
    let alpha = c.get(3).map_or(1.0, |&a| f32::from(a) / 255.0);
    Some(Parsed::Legacy([c[0], c[1], c[2]].map(f32::from), alpha))
}

fn is_html_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

fn skip_spaces(chars: &[u8]) -> &[u8] {
    &chars[chars.iter().take_while(|b| is_html_space(**b)).count()..]
}

/// `FindLengthOfValidDouble`. Its SIMD variant differs only on a trailing `.`, which every
/// caller here rejects either way.
fn valid_double_length(chars: &[u8]) -> usize {
    let mut seen_mark = false;
    let length = chars
        .iter()
        .take_while(|&&c| {
            c.is_ascii_digit() || (c == b'.' && !std::mem::replace(&mut seen_mark, true))
        })
        .count();
    if length > 0 && chars[length - 1] == b'.' {
        0
    } else {
        length
    }
}

/// `ParsePositiveDouble` from Blink's CSS fast paths: the length of the double at the start of
/// `chars` (0 for none) and its value. Only the first seven decimals count. When at least seven
/// bytes of `chars` follow the decimal mark, the SSE2 and NEON paths scale the decimals as one
/// integer by a constant slightly above 1e-7; otherwise they are divided exactly.
pub fn parse_positive_double(chars: &[u8]) -> (usize, f64) {
    const DIV_1E7: f64 = f64::from_bits(1e-7_f64.to_bits() + 1);
    let length = valid_double_length(chars);
    if length == 0 {
        return (0, 0.0);
    }
    let mut value = 0.0;
    let mut position = 0;
    while position < length && chars[position] != b'.' {
        value = value * 10.0 + f64::from(chars[position] - b'0');
        position += 1;
    }
    position += 1;
    if position >= length {
        return (length, value);
    }
    let decimals = &chars[position..position + (length - position).min(7)];
    if chars.len() - position >= 7 {
        let fraction = decimals
            .iter()
            .fold(0u32, |acc, d| acc * 10 + u32::from(d - b'0'))
            * 10u32.pow(7 - decimals.len() as u32);
        return (length, value + f64::from(fraction) * DIV_1E7);
    }
    let (mut fraction, mut scale) = (0.0, 1.0);
    for d in decimals {
        fraction = fraction * 10.0 + f64::from(d - b'0');
        scale *= 10.0;
    }
    (length, value + fraction / scale)
}

#[derive(Clone, Copy, PartialEq)]
enum Terminator {
    Could,
    MustWhitespace,
    MustCharacter,
}

/// `SkipToTerminator`: advances `chars` only on success.
fn skip_to_terminator(chars: &mut &[u8], terminator: u8, status: &mut Terminator) -> bool {
    let mut current = skip_spaces(chars);
    let skipped = current.len() != chars.len();
    match *status {
        Terminator::Could if current.first() == Some(&terminator) => {
            *status = Terminator::MustCharacter;
            current = &current[1..];
        }
        Terminator::Could | Terminator::MustWhitespace => {
            *status = Terminator::MustWhitespace;
            if !skipped {
                return false;
            }
        }
        Terminator::MustCharacter => {
            if current.first() != Some(&terminator) {
                return false;
            }
            current = &current[1..];
        }
    }
    *chars = current;
    true
}

/// `ParseFloatWithMaxValue`: the value (clamped to `max` once its integer digits reach it) and
/// whether a `-` preceded it.
fn float_with_max(chars: &mut &[u8], max: f64) -> Option<(f64, bool)> {
    let mut current = skip_spaces(chars);
    let negative = current.first() == Some(&b'-');
    if negative {
        current = &current[1..];
    }
    if !current.first()?.is_ascii_digit() {
        return None;
    }
    let mut value = 0.0;
    while let Some(d) = current.first().filter(|d| d.is_ascii_digit()) {
        let new_value = value * 10.0 + f64::from(d - b'0');
        current = &current[1..];
        if new_value >= max {
            value = max;
            current = &current[current.iter().take_while(|d| d.is_ascii_digit()).count()..];
            break;
        }
        value = new_value;
    }
    if current.first() == Some(&b'.') {
        let (length, fraction) = parse_positive_double(current);
        if length == 0 {
            return None;
        }
        current = &current[length..];
        value += fraction;
    }
    *chars = current;
    Some((value, negative))
}

#[derive(Clone, Copy, PartialEq)]
enum ChannelUnit {
    Unknown,
    Number,
    Percentage,
}

/// `ParseColorNumberOrPercentage`.
fn color_channel(
    chars: &mut &[u8],
    terminator: u8,
    status: &mut Terminator,
    expect: &mut ChannelUnit,
) -> Option<u8> {
    let mut current = *chars;
    let (mut value, negative) = float_with_max(&mut current, 255.0)?;
    let percent = *current.first()? == b'%';
    if (*expect == ChannelUnit::Percentage && !percent)
        || (*expect == ChannelUnit::Number && percent)
    {
        return None;
    }
    if percent {
        *expect = ChannelUnit::Percentage;
        value = value / 100.0 * 255.0;
        if value > 255.0 {
            value = 255.0;
        }
        current = &current[1..];
    } else {
        *expect = ChannelUnit::Number;
    }
    if !skip_to_terminator(&mut current, terminator, status) {
        return None;
    }
    *chars = current;
    Some(if negative {
        0
    } else {
        value.round().min(255.0) as u8
    })
}

/// `ParsePercentage`: a percentage as a fraction in 0..=1.
fn percentage(chars: &mut &[u8], terminator: u8, status: &mut Terminator) -> Option<f64> {
    let mut current = *chars;
    let (value, negative) = float_with_max(&mut current, 100.0)?;
    if current.first() != Some(&b'%') {
        return None;
    }
    current = &current[1..];
    let value = if negative {
        0.0
    } else {
        (value * 0.01).min(1.0)
    };
    if !skip_to_terminator(&mut current, terminator, status) {
        return None;
    }
    *chars = current;
    Some(value)
}

/// `ParseAlphaValue`: the alpha as a byte; the value must end `chars`.
fn alpha_byte(chars: &[u8], terminator: u8) -> Option<u8> {
    const TENTHS: [u8; 10] = [0, 26, 51, 77, 102, 128, 153, 179, 204, 230];
    let mut chars = skip_spaces(chars);
    let negative = chars.first() == Some(&b'-');
    if negative {
        chars = &chars[1..];
    }
    let length = chars.len();
    if length < 2 || chars[length - 1] != terminator || !chars[length - 2].is_ascii_digit() {
        return None;
    }
    if !matches!(chars[0], b'0' | b'1' | b'.') {
        let double_length = valid_double_length(chars);
        return (double_length > 0 && chars.get(double_length) == Some(&terminator))
            .then_some(if negative { 0 } else { 255 });
    }
    if length == 2 && chars[0] != b'.' {
        return Some(if !negative && chars[0] == b'1' {
            255
        } else {
            0
        });
    }
    let body = &chars[..length - 1];
    if matches!(body, [b'0', b'.', d] | [b'.', d] if d.is_ascii_digit()) {
        return Some(if negative {
            0
        } else {
            TENTHS[usize::from(body[body.len() - 1] - b'0')]
        });
    }
    let (double_length, alpha) = parse_positive_double(chars);
    if double_length == 0 || chars.get(double_length) != Some(&terminator) {
        return None;
    }
    Some(if negative {
        0
    } else {
        (alpha.min(1.0) * 255.0).round() as u8
    })
}

#[derive(Clone, Copy)]
enum AngleUnit {
    Degrees,
    Gradians,
    Radians,
    Turns,
}

/// `ParseSimpleAngle`: length, value clamped to the float range, and unit (unitless is
/// degrees).
fn simple_angle(chars: &[u8]) -> Option<(usize, f64, AngleUnit)> {
    let negative = chars.first() == Some(&b'-');
    let (length, n) = parse_positive_double(&chars[usize::from(negative)..]);
    if length == 0 {
        return None;
    }
    let n = n.min(f64::from(f32::MAX));
    let (length, number) = if negative {
        (length + 1, -n)
    } else {
        (length, n)
    };
    let rest = &chars[length..];
    let unit_is = |name: &[u8]| {
        rest.len() >= name.len() && rest.iter().zip(name).all(|(c, n)| c | 0x20 == *n)
    };
    Some(if unit_is(b"deg") {
        (length + 3, number, AngleUnit::Degrees)
    } else if unit_is(b"grad") {
        (length + 4, number, AngleUnit::Gradians)
    } else if unit_is(b"rad") {
        (length + 3, number, AngleUnit::Radians)
    } else if unit_is(b"turn") {
        (length + 4, number, AngleUnit::Turns)
    } else {
        (length, number, AngleUnit::Degrees)
    })
}

/// `FastParseColorInternal` outside quirks mode: `#` hex and the `rgb[a]()` and `hsl[a]()`
/// forms its fast path accepts. `None` when Blink hands the string to its full parser.
pub fn parse_fast(css: &str) -> Option<Parsed> {
    let chars = css.as_bytes();
    if chars.len() >= 4 && chars[0] == b'#' {
        return parse_hex(&css[1..]);
    }
    let function = |name: &[u8]| {
        (chars.len() >= 5
            && chars.starts_with(name)
            && (chars[3] == b'(' || (chars[3] == b'a' && chars[4] == b'(')))
            .then(|| &chars[if chars[3] == b'a' { 5 } else { 4 }..])
    };
    if let Some(args) = function(b"rgb") {
        return fast_rgb(args);
    }
    function(b"hsl").and_then(fast_hsl)
}

fn fast_rgb(mut current: &[u8]) -> Option<Parsed> {
    let mut status = Terminator::Could;
    let mut expect = ChannelUnit::Unknown;
    let red = color_channel(&mut current, b',', &mut status, &mut expect)?;
    let green = color_channel(&mut current, b',', &mut status, &mut expect)?;
    let mut strict = Terminator::MustCharacter;
    let (blue, has_alpha) =
        if let Some(blue) = color_channel(&mut current, b',', &mut strict, &mut expect) {
            if status != Terminator::MustCharacter {
                return None;
            }
            (blue, true)
        } else if let Some(blue) = color_channel(&mut current, b'/', &mut strict, &mut expect) {
            if status != Terminator::MustWhitespace {
                return None;
            }
            (blue, true)
        } else {
            (
                color_channel(&mut current, b')', &mut strict, &mut expect)?,
                false,
            )
        };
    let alpha = if has_alpha {
        f32::from(alpha_byte(current, b')')?) / 255.0
    } else if current.is_empty() {
        1.0
    } else {
        return None;
    };
    Some(Parsed::Legacy([red, green, blue].map(f32::from), alpha))
}

fn fast_hsl(current: &[u8]) -> Option<Parsed> {
    let mut current = skip_spaces(current);
    let (length, mut hue, unit) = simple_angle(current)?;
    hue = match unit {
        AngleUnit::Degrees => hue,
        AngleUnit::Radians => hue * (180.0 / std::f64::consts::PI),
        AngleUnit::Gradians => hue * (360.0 / 400.0),
        AngleUnit::Turns => hue * 360.0,
    };
    if hue < 0.0 {
        hue = hue % 360.0 + 360.0;
    } else if hue > 360.0 {
        hue %= 360.0;
    }
    current = &current[length..];
    let mut status = Terminator::Could;
    if !skip_to_terminator(&mut current, b',', &mut status) {
        return None;
    }
    let saturation = percentage(&mut current, b',', &mut status)?;
    let mut strict = Terminator::MustCharacter;
    let (lightness, has_alpha) = if let Some(l) = percentage(&mut current, b',', &mut strict) {
        if status != Terminator::MustCharacter {
            return None;
        }
        (l, true)
    } else if let Some(l) = percentage(&mut current, b'/', &mut strict) {
        if status != Terminator::MustWhitespace {
            return None;
        }
        (l, true)
    } else {
        (percentage(&mut current, b')', &mut strict)?, false)
    };
    let alpha = if has_alpha {
        f32::from(alpha_byte(current, b')')?) * (1.0 / 255.0)
    } else if current.is_empty() {
        1.0
    } else {
        return None;
    };
    Some(hsl(hue as f32, saturation as f32, lightness as f32, alpha))
}

/// `Color::FromHSLA` through `gfx::HSLToSRGB`.
fn hsl(h: f32, s: f32, l: f32, alpha: f32) -> Parsed {
    if s == 0.0 {
        return Parsed::Hsl([l; 3], alpha);
    }
    let f = |n: f32| {
        let k = (f64::from(n + h / 30.0) % 12.0) as f32;
        let a = s * l.min(1.0 - l);
        l - a * (-1.0f32).max((k - 3.0).min(9.0 - k).min(1.0))
    };
    Parsed::Hsl([f(0.0), f(8.0), f(4.0)], alpha)
}

/// What Blink's full parser makes of a function's arguments.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Slow<T> {
    Valid(T),
    Invalid,
    /// Arguments outside the plain number and percentage tokens modelled here.
    Unmodeled,
}

#[derive(Clone, Copy, PartialEq)]
enum ArgToken {
    Number(f64),
    Percent(f64),
    Comma,
    Slash,
}

/// The tokens of `rgb(` arguments up to the closing `)` (or the end of input).
fn rgb_arg_tokens(args: &str) -> Slow<Vec<ArgToken>> {
    let b = args.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let digit_at = |j: usize| b.get(j).is_some_and(u8::is_ascii_digit);
        let starts_number = c.is_ascii_digit()
            || (c == b'.' && digit_at(i + 1))
            || (matches!(c, b'+' | b'-')
                && (digit_at(i + 1) || (b.get(i + 1) == Some(&b'.') && digit_at(i + 2))));
        if is_html_space(c) {
            i += 1;
        } else if c == b',' {
            tokens.push(ArgToken::Comma);
            i += 1;
        } else if c == b'/' && b.get(i + 1) != Some(&b'*') {
            tokens.push(ArgToken::Slash);
            i += 1;
        } else if c == b')' {
            let rest = &args[i + 1..];
            if rest.contains("/*") {
                return Slow::Unmodeled;
            }
            if !rest.bytes().all(is_html_space) {
                return Slow::Invalid;
            }
            break;
        } else if starts_number {
            let start = i;
            i += usize::from(matches!(c, b'+' | b'-'));
            while digit_at(i) {
                i += 1;
            }
            if b.get(i) == Some(&b'.') && digit_at(i + 1) {
                i += 1;
                while digit_at(i) {
                    i += 1;
                }
            }
            if matches!(b.get(i), Some(b'e' | b'E')) {
                let sign = usize::from(matches!(b.get(i + 1), Some(b'+' | b'-')));
                if digit_at(i + 1 + sign) {
                    i += 1 + sign;
                    while digit_at(i) {
                        i += 1;
                    }
                }
            }
            let Ok(value) = args[start..i].parse::<f64>() else {
                return Slow::Unmodeled;
            };
            let name_start = |j: usize| {
                b.get(j)
                    .is_some_and(|&n| n.is_ascii_alphabetic() || n == b'_' || n >= 0x80)
            };
            let dimension = name_start(i)
                || (b.get(i) == Some(&b'-') && (name_start(i + 1) || b.get(i + 1) == Some(&b'-')))
                || b.get(i) == Some(&b'\\');
            if dimension {
                return Slow::Invalid;
            }
            if b.get(i) == Some(&b'%') {
                tokens.push(ArgToken::Percent(value));
                i += 1;
            } else {
                tokens.push(ArgToken::Number(value));
            }
        } else if b".+-!;#<>=?@$&*^~|`".contains(&c) {
            return Slow::Invalid;
        } else {
            return Slow::Unmodeled;
        }
    }
    Slow::Valid(tokens)
}

/// `ColorFunctionParser::ConsumeFunctionalSyntaxColor` for `rgb(`/`rgba(` arguments made only
/// of number and percentage tokens. A comma after the first or second channel switches to the
/// legacy syntax, which then needs commas for the rest and one channel type throughout.
/// Channels and alpha are stored as floats, legacy alpha quantized to n/255.
pub fn parse_rgb_tokens(args: &str) -> Slow<Parsed> {
    use ArgToken::{Comma, Number, Percent, Slash};
    let tokens = match rgb_arg_tokens(args) {
        Slow::Valid(tokens) => tokens,
        Slow::Invalid => return Slow::Invalid,
        Slow::Unmodeled => return Slow::Unmodeled,
    };
    let mut rest = tokens.as_slice();
    let mut take = |want: &dyn Fn(&ArgToken) -> bool| match rest.split_first() {
        Some((t, tail)) if want(t) => {
            rest = tail;
            Some(*t)
        }
        _ => None,
    };
    let is_value = |t: &ArgToken| matches!(t, Number(_) | Percent(_));
    let mut channels = [Comma; 3];
    let mut legacy = false;
    for (i, channel) in channels.iter_mut().enumerate() {
        let Some(t) = take(&is_value) else {
            return Slow::Invalid;
        };
        *channel = t;
        if i < 2 {
            let comma = take(&|t| *t == Comma).is_some();
            if legacy && !comma {
                return Slow::Invalid;
            }
            legacy |= comma;
        }
    }
    let separator = if legacy { Comma } else { Slash };
    let alpha = if take(&|t| *t == separator).is_some() {
        let Some(a) = take(&is_value) else {
            return Slow::Invalid;
        };
        Some(a)
    } else {
        None
    };
    if !rest.is_empty() {
        return Slow::Invalid;
    }
    if legacy
        && !(channels.iter().all(|t| matches!(t, Number(_)))
            || channels.iter().all(|t| matches!(t, Percent(_))))
    {
        return Slow::Invalid;
    }
    let channels = channels.map(|t| {
        let v = match t {
            Percent(p) => p / 100.0 * 255.0,
            Number(v) => v,
            Comma | Slash => unreachable!("channels are values"),
        };
        (if v.is_finite() {
            v.clamp(0.0, 255.0)
        } else if v > 0.0 {
            255.0
        } else {
            0.0
        }) as f32
    });
    let alpha = alpha.map_or(1.0, |t| {
        let a = match t {
            Percent(p) => p / 100.0,
            Number(v) => v,
            Comma | Slash => unreachable!("alpha is a value"),
        };
        ((a.clamp(0.0, 1.0) * 255.0).round() / 255.0) as f32
    });
    Slow::Valid(Parsed::Legacy(channels, alpha))
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

/// The full parser's `hsl()` for the forms the fast path declines, approximated: plain or
/// `deg` hue, percentage saturation and lightness clamped to 0..=1.
fn parse_hsl(body: &str) -> Option<Parsed> {
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
    let hue = (hue % 360.0 + 360.0) % 360.0;
    Some(hsl(hue as f32, s as f32, l as f32, alpha_value(alpha)?))
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
        assert_eq!(
            u8s("rgba(228,139,50,0.25)"),
            Some(([228, 139, 50], 64.0 / 255.0))
        );
        assert_eq!(
            u8s("rgb(10 20 30 / 50%)"),
            Some(([10, 20, 30], 128.0 / 255.0))
        );
        assert_eq!(u8s("hsl(120, 50%, 50%)"), Some(([64, 191, 64], 1.0)));
        assert_eq!(u8s("transparent"), Some(([0, 0, 0], 0.0)));
        assert_eq!(u8s("not a color"), None);
        assert_eq!(u8s("#12345"), None);
        assert_eq!(u8s("rgb(1, 2)"), None);
    }

    #[test]
    fn percentage_channels_scale_like_blink() {
        assert_eq!(u8s("rgb(100%, 50%, 0%)"), Some(([255, 128, 0], 1.0)));
    }

    #[test]
    fn closes_a_function_left_open_like_blink() {
        let black = Some(([0, 0, 0], 1.0));
        assert_eq!(u8s("rgb(0 0 0 / 10%"), Some(([0, 0, 0], 26.0 / 255.0)));
        assert_eq!(u8s("rgb(0 0 0 / 10%\t"), Some(([0, 0, 0], 26.0 / 255.0)));
        assert_eq!(u8s("rgb(0,0,0"), black);
        assert_eq!(u8s("rgb(0 0 0 "), black);
        assert_eq!(u8s(" RGB(1 2 3"), Some(([1, 2, 3], 1.0)));
        assert_eq!(u8s("rgb(1 2 3 / 50"), Some(([1, 2, 3], 1.0)));
        assert_eq!(u8s("rgb(1 2 3 / .5"), Some(([1, 2, 3], 128.0 / 255.0)));
        assert_eq!(u8s("rgba(1,2,3,0.5"), Some(([1, 2, 3], 128.0 / 255.0)));
        assert_eq!(u8s("rgb(10% 20% 30%"), Some(([26, 51, 77], 1.0)));
        assert_eq!(u8s("hsl(120, 50%, 50%"), Some(([64, 191, 64], 1.0)));
        assert_eq!(u8s("hsl(120 50% 50% / 0.5"), Some(([64, 191, 64], 0.5)));
        assert_eq!(u8s("hsla(120,50%,50%,0.5"), Some(([64, 191, 64], 0.5)));
        for invalid in [
            "rgb(0 0 0 /",
            "rgb(0,0,0,",
            "rgb(",
            "rgb(0 0",
            "rgb(0 0 0)x",
            "rgb(0 0 0))",
            "#fff(",
            "red(",
            "undefined",
        ] {
            assert_eq!(u8s(invalid), None, "{invalid}");
        }
    }
}
