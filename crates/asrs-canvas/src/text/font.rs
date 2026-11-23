#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Family {
    /// `system-ui` and its aliases `-apple-system` and `BlinkMacSystemFont`.
    System,
    SansSerif,
    Serif,
    Monospace,
    /// `Courier`, which Chromium takes for `Courier New` where only that is installed.
    Courier,
    /// The family of Chromium's `-webkit-small-control` font on macOS, which form controls use.
    Arial,
    /// The first family of Chart.js's default font, which only macOS has.
    HelveticaNeue,
}

/// A canvas `font` resolved to what the glyph backends need. Size and weight are `f32`, as
/// Blink stores them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontSpec {
    pub family: Family,
    pub size: f32,
    pub weight: f32,
    pub italic: bool,
}

impl Default for FontSpec {
    fn default() -> FontSpec {
        FontSpec {
            family: Family::SansSerif,
            size: 10.0,
            weight: 400.0,
            italic: false,
        }
    }
}

pub type FontKey = (Family, u32, u32, bool);

impl FontSpec {
    pub fn key(&self) -> FontKey {
        (
            self.family,
            self.size.to_bits(),
            self.weight.to_bits(),
            self.italic,
        )
    }

    /// Parses the CSS `font` shorthand the way a canvas `font` assignment does. `None` means the
    /// assignment is ignored and the previous font stays. Relative sizes (`em`, `%`, `larger`)
    /// and system font keywords are not supported; upstream sets only `px` sizes.
    pub fn parse(css: &str) -> Option<FontSpec> {
        let mut tokens = Tokens { rest: css.trim() };
        let mut spec = FontSpec {
            family: Family::Serif,
            ..FontSpec::default()
        };
        let (mut style, mut variant, mut weight, mut stretch) = (false, false, false, false);
        let mut prefix = 0;
        let size = loop {
            let word = tokens.word()?;
            let lower = word.to_ascii_lowercase();
            if let Some(size) = font_size(&lower) {
                break size;
            }
            prefix += 1;
            if prefix > 4 {
                return None;
            }
            match lower.as_str() {
                "normal" => {}
                "italic" | "oblique" if !style => {
                    style = true;
                    spec.italic = true;
                }
                "small-caps" if !variant => variant = true,
                "bold" | "bolder" if !weight => {
                    weight = true;
                    spec.weight = 700.0;
                }
                "lighter" if !weight => {
                    weight = true;
                    spec.weight = 100.0;
                }
                "ultra-condensed" | "extra-condensed" | "condensed" | "semi-condensed"
                | "semi-expanded" | "expanded" | "extra-expanded" | "ultra-expanded"
                    if !stretch =>
                {
                    stretch = true;
                }
                _ => match number(&lower) {
                    Some(w) if !weight && (1.0..=1000.0).contains(&w) => {
                        weight = true;
                        spec.weight = w as f32;
                    }
                    _ => return None,
                },
            }
        };
        if !size.is_finite() || size < 0.0 {
            return None;
        }
        spec.size = size as f32;
        if tokens.rest.starts_with('/') {
            tokens.rest = tokens.rest[1..].trim_start();
            tokens.word()?;
        }
        spec.family = families(tokens.rest)?;
        Some(spec)
    }
}

struct Tokens<'a> {
    rest: &'a str,
}

impl<'a> Tokens<'a> {
    fn word(&mut self) -> Option<&'a str> {
        let end = self
            .rest
            .find(|c: char| c.is_ascii_whitespace() || c == '/' || c == ',')
            .unwrap_or(self.rest.len());
        if end == 0 {
            return None;
        }
        let word = &self.rest[..end];
        self.rest = self.rest[end..].trim_start();
        Some(word)
    }
}

fn number(s: &str) -> Option<f64> {
    let valid = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b'e' || b == b'+' || b == b'-');
    if valid { s.parse().ok() } else { None }
}

fn font_size(word: &str) -> Option<f64> {
    let keyword = match word {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "xxx-large" => Some(48.0),
        _ => None,
    };
    if keyword.is_some() {
        return keyword;
    }
    let units: [(&str, f64); 7] = [
        ("px", 1.0),
        ("pt", 4.0 / 3.0),
        ("pc", 16.0),
        ("in", 96.0),
        ("cm", 96.0 / 2.54),
        ("mm", 96.0 / 25.4),
        ("q", 96.0 / 101.6),
    ];
    units.iter().find_map(|&(unit, factor)| {
        let n = number(word.strip_suffix(unit)?)?;
        Some(if factor == 1.0 { n } else { n * factor })
    })
}

/// The first family in the list a macOS Chromium would find. Named families other than the
/// system aliases are not resolved; with none left, Chromium's standard font (Times) is used.
fn families(list: &str) -> Option<Family> {
    if list.is_empty() {
        return None;
    }
    let mut first = None;
    for item in list.split(',') {
        let item = item.trim();
        if item.is_empty() {
            return None;
        }
        let quoted = item.starts_with('"') || item.starts_with('\'');
        let name = item.trim_matches(|c| c == '"' || c == '\'');
        let family = match name.to_ascii_lowercase().as_str() {
            "system-ui" if !quoted => Some(Family::System),
            "-apple-system" | "blinkmacsystemfont" => Some(Family::System),
            "sans-serif" if !quoted => Some(Family::SansSerif),
            "serif" if !quoted => Some(Family::Serif),
            "monospace" if !quoted => Some(Family::Monospace),
            "helvetica" => Some(Family::SansSerif),
            "times" => Some(Family::Serif),
            "courier" => Some(Family::Courier),
            "arial" => Some(Family::Arial),
            "helvetica neue" if cfg!(target_os = "macos") => Some(Family::HelveticaNeue),
            _ => None,
        };
        first = first.or(family);
    }
    Some(first.unwrap_or(Family::Serif))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(family: Family, size: f32, weight: f32, italic: bool) -> Option<FontSpec> {
        Some(FontSpec {
            family,
            size,
            weight,
            italic,
        })
    }

    #[test]
    fn parses_upstream_fonts() {
        let system = "ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont";
        let cases = [
            (
                format!("12px {system}"),
                spec(Family::System, 12.0, 400.0, false),
            ),
            (
                format!("200 14px {system}"),
                spec(Family::System, 14.0, 200.0, false),
            ),
            (
                format!("bold 12px {system}"),
                spec(Family::System, 12.0, 700.0, false),
            ),
            (
                "10px sans-serif".to_owned(),
                spec(Family::SansSerif, 10.0, 400.0, false),
            ),
            (
                "italic 16px sans-serif".to_owned(),
                spec(Family::SansSerif, 16.0, 400.0, true),
            ),
            (
                "13.5px sans-serif".to_owned(),
                spec(Family::SansSerif, 13.5, 400.0, false),
            ),
            (
                "23.437500000000004px sans-serif".to_owned(),
                spec(Family::SansSerif, 23.4375, 400.0, false),
            ),
        ];
        for (css, expected) in cases {
            assert_eq!(FontSpec::parse(&css), expected, "{css}");
        }
    }

    #[test]
    fn shorthand_grammar() {
        assert_eq!(
            FontSpec::parse("italic small-caps 600 12pt/1.5 \"Helvetica\", serif"),
            spec(Family::SansSerif, 16.0, 600.0, true)
        );
        assert_eq!(
            FontSpec::parse("normal normal 10px 'My Font'"),
            spec(Family::Serif, 10.0, 400.0, false)
        );
        assert_eq!(
            FontSpec::parse("12px \"system-ui\""),
            spec(Family::Serif, 12.0, 400.0, false)
        );
        assert_eq!(FontSpec::parse("0px sans-serif").unwrap().size, 0.0);
        for bad in [
            "",
            "12px",
            "sans-serif",
            "bold bold 12px serif",
            "-1px serif",
            "12 serif",
            "1001 12px serif",
            "12px serif,",
            "normal normal normal normal normal 12px serif",
        ] {
            assert_eq!(FontSpec::parse(bad), None, "{bad:?}");
        }
    }
}
