use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FilterFn {
    Invert(f64),
    Sepia(f64),
    Grayscale(f64),
    Saturate(f64),
    HueRotate(f64),
    Brightness(f64),
    Contrast(f64),
    Opacity(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct FilterChain(pub Vec<FilterFn>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad filter: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

impl FilterChain {
    pub const NONE: FilterChain = FilterChain(Vec::new());

    /// Parses the CSS `filter` functions upstream uses. `drop-shadow`, `blur` and `url()` are not
    /// colour operations and are rejected.
    pub fn parse(css: &str) -> Result<FilterChain, ParseError> {
        let err = || ParseError(css.to_owned());
        let mut out = Vec::new();
        let mut rest = css.trim();
        while !rest.is_empty() {
            let open = rest.find('(').ok_or_else(err)?;
            let close = rest.find(')').ok_or_else(err)?;
            let name = rest[..open].trim();
            let arg = rest[open + 1..close].trim();
            rest = rest[close + 1..].trim_start();
            let number = |s: &str| s.trim().parse::<f64>().map_err(|_| err());
            let amount = if let Some(p) = arg.strip_suffix('%') {
                number(p)? / 100.0
            } else if let Some(d) = arg.strip_suffix("deg") {
                number(d)?
            } else if arg.is_empty() {
                if name == "hue-rotate" { 0.0 } else { 1.0 }
            } else {
                number(arg)?
            };
            if amount < 0.0 && name != "hue-rotate" {
                return Err(err());
            }
            out.push(match name {
                "invert" => FilterFn::Invert(amount.min(1.0)),
                "sepia" => FilterFn::Sepia(amount.min(1.0)),
                "grayscale" => FilterFn::Grayscale(amount.min(1.0)),
                "opacity" => FilterFn::Opacity(amount.min(1.0)),
                "saturate" => FilterFn::Saturate(amount),
                "hue-rotate" => FilterFn::HueRotate(amount),
                "brightness" => FilterFn::Brightness(amount),
                "contrast" => FilterFn::Contrast(amount),
                _ => return Err(err()),
            });
        }
        Ok(FilterChain(out))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Filters one straight-alpha sRGB colour the way Chromium does for the CSS shorthands: each
    /// function in sRGB on unpremultiplied components in floating point, clamped to [0, 1] after
    /// every step, rounded to 8 bits once at the end. Matches every probe in the oracle's
    /// `ui-icons` scenes exactly (quantizing between steps, as an 8-bit table would, does not).
    pub fn apply(&self, rgba: [u8; 4]) -> [u8; 4] {
        let mut c = [0, 1, 2].map(|i| f64::from(rgba[i]) / 255.0);
        let mut a = f64::from(rgba[3]) / 255.0;
        for f in &self.0 {
            match *f {
                FilterFn::Invert(v) => c = c.map(|x| v + x * (1.0 - 2.0 * v)),
                FilterFn::Brightness(v) => c = c.map(|x| x * v),
                FilterFn::Contrast(v) => c = c.map(|x| v * x + (0.5 - 0.5 * v)),
                FilterFn::Opacity(v) => a *= v,
                FilterFn::Sepia(v) => c = mul(&sepia(v), c),
                FilterFn::Grayscale(v) => c = mul(&grayscale(v), c),
                FilterFn::Saturate(v) => c = mul(&saturate(v), c),
                FilterFn::HueRotate(v) => c = mul(&hue_rotate(v), c),
            }
            c = c.map(|x| x.clamp(0.0, 1.0));
            a = a.clamp(0.0, 1.0);
        }
        let q = |x: f64| (x * 255.0 + 0.5) as u8;
        [q(c[0]), q(c[1]), q(c[2]), q(a)]
    }
}

type Matrix = [[f64; 3]; 3];

fn mul(m: &Matrix, c: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|i| m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2])
}

fn sepia(a: f64) -> Matrix {
    let i = 1.0 - a;
    [
        [0.393 + 0.607 * i, 0.769 - 0.769 * i, 0.189 - 0.189 * i],
        [0.349 - 0.349 * i, 0.686 + 0.314 * i, 0.168 - 0.168 * i],
        [0.272 - 0.272 * i, 0.534 - 0.534 * i, 0.131 + 0.869 * i],
    ]
}

fn grayscale(a: f64) -> Matrix {
    let i = 1.0 - a;
    [
        [
            0.2126 + 0.7874 * i,
            0.7152 - 0.7152 * i,
            0.0722 - 0.0722 * i,
        ],
        [
            0.2126 - 0.2126 * i,
            0.7152 + 0.2848 * i,
            0.0722 - 0.0722 * i,
        ],
        [
            0.2126 - 0.2126 * i,
            0.7152 - 0.7152 * i,
            0.0722 + 0.9278 * i,
        ],
    ]
}

fn saturate(s: f64) -> Matrix {
    [
        [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
    ]
}

fn hue_rotate(deg: f64) -> Matrix {
    let r = deg.to_radians();
    let (s, c) = (r.sin(), r.cos());
    [
        [
            0.213 + c * 0.787 - s * 0.213,
            0.715 - c * 0.715 - s * 0.715,
            0.072 - c * 0.072 + s * 0.928,
        ],
        [
            0.213 - c * 0.213 + s * 0.143,
            0.715 + c * 0.285 + s * 0.140,
            0.072 - c * 0.072 - s * 0.283,
        ],
        [
            0.213 - c * 0.213 - s * 0.787,
            0.715 - c * 0.715 + s * 0.715,
            0.072 + c * 0.928 + s * 0.072,
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_upstream_forms() {
        let chain = FilterChain::parse(
            "invert(32%) sepia(0%) saturate(60%) hue-rotate(224deg) brightness(100%) contrast(95%)",
        )
        .unwrap();
        assert_eq!(chain.0.len(), 6);
        assert_eq!(chain.0[3], FilterFn::HueRotate(224.0));
        assert_eq!(
            FilterChain::parse("hue-rotate(-45deg) brightness(450%)")
                .unwrap()
                .0,
            vec![FilterFn::HueRotate(-45.0), FilterFn::Brightness(4.5)]
        );
        assert_eq!(
            FilterChain::parse("invert(1)").unwrap().0,
            vec![FilterFn::Invert(1.0)]
        );
        assert!(FilterChain::parse("drop-shadow(0px 0.5px 1px #bbb)").is_err());
    }

    #[test]
    fn identity_and_invert() {
        assert_eq!(FilterChain::NONE.apply([1, 2, 3, 4]), [1, 2, 3, 4]);
        let invert = FilterChain::parse("invert(100%)").unwrap();
        assert_eq!(invert.apply([0, 128, 255, 200]), [255, 127, 0, 200]);
    }
}
