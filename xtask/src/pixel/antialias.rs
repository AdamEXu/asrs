//! How the text of a box is antialiased, from its pixels.

use std::fmt;

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

/// The ink of a box: how many pixels, and the share of them that needs a coverage per channel.
struct Ink {
    pixels: usize,
    per_channel: f64,
}

impl Ink {
    fn of(image: &Rgb, rect: Rect, scale: f64, color: [u8; 3]) -> Ink {
        let pixels = pixels(image, rect, scale);
        let background = most_common(&pixels).unwrap_or_default();
        let inked: Vec<[u8; 3]> = pixels
            .into_iter()
            .filter(|p| (0..3).any(|c| p[c].abs_diff(background[c]) > INK))
            .collect();
        let per_channel = inked
            .iter()
            .filter(|p| !one_coverage(**p, color, background))
            .count();
        Ink {
            pixels: inked.len(),
            per_channel: per_channel as f64 / inked.len().max(1) as f64,
        }
    }

    fn mode(&self) -> Option<Mode> {
        if self.pixels < MIN_INK {
            None
        } else if self.per_channel >= LCD_SHARE {
            Some(Mode::Lcd)
        } else if self.per_channel <= GRAYSCALE_SHARE {
            Some(Mode::Grayscale)
        } else {
            None
        }
    }
}

/// `None` for a box with too little ink, or one that is neither.
pub fn mode(image: &Rgb, rect: Rect, scale: f64, color: [u8; 3]) -> Option<Mode> {
    Ink::of(image, rect, scale, color).mode()
}

/// The mode of every box: how many are LCD and grayscale upstream, how many are unclear on either
/// side, and those the port draws in the other mode, or with ink that is further from the
/// reference's than `LCD_SHARE` where either is unclear.
pub struct Compared {
    pub lcd: usize,
    pub grayscale: usize,
    pub unclear: usize,
    pub differing: Vec<Differing>,
}

#[derive(Debug, PartialEq)]
pub struct Differing {
    pub rect: Rect,
    pub expected: Option<Mode>,
    pub actual: Option<Mode>,
    /// The shares of the ink that need a coverage per channel.
    pub per_channel: [f64; 2],
}

impl fmt::Display for Differing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = |mode: Option<Mode>| mode.map_or("unclear".to_owned(), |m| format!("{m:?}"));
        write!(
            f,
            "{:?} is {} upstream ({:.2}) and {} here ({:.2})",
            self.rect,
            name(self.expected),
            self.per_channel[0],
            name(self.actual),
            self.per_channel[1]
        )
    }
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
        let of = |image| Ink::of(image, ink.rect, scale, ink.color);
        let (theirs, ours) = (of(reference), of(port));
        let (expected, actual) = (theirs.mode(), ours.mode());
        let differs = match (expected, actual) {
            (Some(expected), Some(actual)) => {
                match expected {
                    Mode::Lcd => out.lcd += 1,
                    Mode::Grayscale => out.grayscale += 1,
                }
                actual != expected
            }
            _ => {
                out.unclear += 1;
                theirs.pixels >= MIN_INK
                    && (theirs.per_channel - ours.per_channel).abs() > LCD_SHARE
            }
        };
        if differs {
            out.differing.push(Differing {
                rect: ink.rect,
                expected,
                actual,
                per_channel: [theirs.per_channel, ours.per_channel],
            });
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
        let [differing] = &other.differing[..] else {
            panic!("{:?}", other.differing);
        };
        assert_eq!(
            (differing.expected, differing.actual),
            (Some(Mode::Lcd), Some(Mode::Grayscale))
        );
        let nothing = image(|_| [0xff; 3]);
        let missing = compare(inks.iter(), &lcd, &nothing, 1.0);
        assert_eq!((missing.unclear, missing.differing.len()), (1, 1));
        let unclear = image(|i| match i / 4 % 16 {
            0 => [0x40, 0x80, 0xc0],
            _ => [0x80; 3],
        });
        assert_eq!(mode(&unclear, RECT, 1.0, [0x22; 3]), None);
        assert_eq!(compare(inks.iter(), &lcd, &unclear, 1.0).differing.len(), 1);
        assert!(
            compare(inks.iter(), &unclear, &unclear, 1.0)
                .differing
                .is_empty()
        );
    }
}
