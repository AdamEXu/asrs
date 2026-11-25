//! How the text of a box is antialiased, from its pixels.

use asrs_parity::pixel::Rgb;

use super::gates::{Rect, TextInk};

/// How far a blend of one coverage may be from the pixel it makes.
const ROUNDING: f64 = 3.0;
/// How far from the background a pixel is ink.
const INK: u8 = 24;
const MIN_INK: usize = 12;
/// The share of inked pixels that need a coverage per channel, above which text is LCD and below
/// which it is grayscale. In between, icons or several colours share the box and it is unclear.
const LCD_SHARE: f64 = 0.10;
const GRAYSCALE_SHARE: f64 = 0.02;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Lcd,
    Grayscale,
}

/// Whether one coverage for all three channels, blending `color` onto `background`, makes `pixel`.
fn one_coverage(pixel: [u8; 3], color: [u8; 3], background: [u8; 3]) -> bool {
    let (mut low, mut high) = (-0.02f64, 1.02f64);
    for c in 0..3 {
        let range = f64::from(color[c]) - f64::from(background[c]);
        let offset = f64::from(pixel[c]) - f64::from(background[c]);
        if range == 0.0 {
            if offset.abs() > ROUNDING {
                return false;
            }
            continue;
        }
        let (a, b) = ((offset - ROUNDING) / range, (offset + ROUNDING) / range);
        low = low.max(a.min(b));
        high = high.min(a.max(b));
    }
    low <= high
}

fn pixels(image: &Rgb, rect: Rect, scale: f64) -> Vec<[u8; 3]> {
    let clamp = |v: f64, max: u32| v.clamp(0.0, f64::from(max)) as usize;
    let (left, right) = (
        clamp((rect[0] * scale).floor(), image.width),
        clamp(((rect[0] + rect[2]) * scale).ceil(), image.width),
    );
    let (top, bottom) = (
        clamp((rect[1] * scale).floor(), image.height),
        clamp(((rect[1] + rect[3]) * scale).ceil(), image.height),
    );
    (top..bottom)
        .flat_map(|y| (left..right).map(move |x| (x, y)))
        .map(|(x, y)| image.data[y * image.width as usize + x])
        .collect()
}

fn most_common(pixels: &[[u8; 3]]) -> Option<[u8; 3]> {
    let mut sorted = pixels.to_vec();
    sorted.sort_unstable();
    let runs = sorted.chunk_by(|a, b| a == b);
    runs.max_by_key(|run| run.len()).map(|run| run[0])
}

/// The grey end a control's text is nearest to.
pub fn contrast(image: &Rgb, rect: Rect, scale: f64) -> [u8; 3] {
    let background = most_common(&pixels(image, rect, scale)).unwrap_or_default();
    [if background[1] < 0x80 { 0xff } else { 0 }; 3]
}

/// `None` for a box with too little ink, or one that is neither.
pub fn mode(image: &Rgb, rect: Rect, scale: f64, color: [u8; 3]) -> Option<Mode> {
    let pixels = pixels(image, rect, scale);
    let background = most_common(&pixels)?;
    let inked: Vec<[u8; 3]> = pixels
        .into_iter()
        .filter(|p| (0..3).any(|c| p[c].abs_diff(background[c]) > INK))
        .collect();
    if inked.len() < MIN_INK {
        return None;
    }
    let per_channel = inked
        .iter()
        .filter(|p| !one_coverage(**p, color, background))
        .count();
    let share = per_channel as f64 / inked.len() as f64;
    if share >= LCD_SHARE {
        Some(Mode::Lcd)
    } else if share <= GRAYSCALE_SHARE {
        Some(Mode::Grayscale)
    } else {
        None
    }
}

/// The mode of every box on both sides: how many are LCD, grayscale and unclear upstream, and
/// those where the port has another.
pub struct Compared {
    pub lcd: usize,
    pub grayscale: usize,
    pub unclear: usize,
    pub differing: Vec<(Rect, Mode, Option<Mode>)>,
}

pub fn compare<'a>(
    inks: impl Iterator<Item = &'a TextInk>,
    reference: &Rgb,
    port: &Rgb,
    scale: f64,
) -> Compared {
    let mut out = Compared {
        lcd: 0,
        grayscale: 0,
        unclear: 0,
        differing: Vec::new(),
    };
    for ink in inks {
        let Some(expected) = mode(reference, ink.rect, scale, ink.color) else {
            out.unclear += 1;
            continue;
        };
        match expected {
            Mode::Lcd => out.lcd += 1,
            Mode::Grayscale => out.grayscale += 1,
        }
        let actual = mode(port, ink.rect, scale, ink.color);
        if actual != Some(expected) {
            out.differing.push((ink.rect, expected, actual));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECT: Rect = [2.0, 2.0, 20.0, 10.0];

    fn image(ink: impl Fn(usize) -> [u8; 3]) -> Rgb {
        let data = (0..24 * 14)
            .map(|i| if i % 4 == 0 { ink(i) } else { [0xff; 3] })
            .collect();
        Rgb {
            width: 24,
            height: 14,
            data,
        }
    }

    #[test]
    fn one_coverage_a_pixel_is_grayscale_and_three_are_lcd() {
        let color = [0x2b, 0x66, 0xa2];
        let blend = |a: [f64; 3]| {
            [0, 1, 2].map(|c| (255.0 + (f64::from(color[c]) - 255.0) * a[c]).round() as u8)
        };
        let gray = image(|i| blend([[0.3, 0.6, 1.0][i % 3]; 3]));
        assert_eq!(mode(&gray, RECT, 1.0, color), Some(Mode::Grayscale));
        let lcd = image(|_| blend([0.2, 0.6, 1.0]));
        assert_eq!(mode(&lcd, RECT, 1.0, color), Some(Mode::Lcd));
        let faded = image(|_| blend([0.15; 3]));
        assert_eq!(mode(&faded, RECT, 1.0, color), Some(Mode::Grayscale));
        let blank = image(|_| [0xff; 3]);
        assert_eq!(mode(&blank, RECT, 1.0, color), None);
    }

    #[test]
    fn a_box_of_the_other_mode_differs() {
        let gray = image(|_| [0x80; 3]);
        let lcd = image(|_| [0x40, 0x80, 0xc0]);
        let inks = [TextInk {
            rect: RECT,
            color: [0x22; 3],
        }];
        let same = compare(inks.iter(), &lcd, &lcd, 1.0);
        assert_eq!((same.lcd, same.grayscale, same.differing.len()), (1, 0, 0));
        let other = compare(inks.iter(), &lcd, &gray, 1.0);
        assert_eq!(other.differing, [(RECT, Mode::Lcd, Some(Mode::Grayscale))]);
    }
}
