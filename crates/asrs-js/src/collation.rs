//! `String.prototype.localeCompare` as V8 runs it with the `undefined` locale, which resolves to
//! `en-US` in the app (Electron 39: ICU 74.2, CLDR 44.1, Unicode 15.1). `en-US` uses the CLDR
//! root collation at tertiary strength; V8 turns ICU's normalization mode on, which ICU4X always
//! does. The ICU4X 1.4 data is pinned because it is built from the same CLDR and Unicode
//! versions; a newer data set would order newly assigned characters differently.
//!
//! Two ICU4C behaviours are reproduced on top of ICU4X.
//!
//! ICU4C first skips the identical prefix of the two UTF-16 strings, backing up while the code
//! unit at the boundary is in its unsafe-backward set or, in numeric collation, is a digit
//! (`RuleBasedCollator::doCompare`). The check is per code unit, so a supplementary digit behind a
//! safe lead surrogate does not make it back up, and the digit run is split at the boundary:
//! `"1𝟮"` compares as `"𝟮"` against `"1x"`'s `"x"`. That makes numeric comparison intransitive in
//! V8. `icu_prefix` replays the skip with the sets probed from Electron (`tables.rs`).
//!
//! Lone surrogates are the one place ICU4X and ICU4C disagree. ICU4X reads each as U+FFFD; ICU4C
//! gives it the implicit weight of an unassigned code point with its own value: one collation
//! element with primary `unassignedPrimary(cp)` and common secondary and tertiary weights. No
//! assigned character has a primary in the range those weights occupy (between U+D7FF and
//! U+E000), and a surrogate takes part in no contraction or normalization, so it splits the
//! string into independently collated segments. `compare_with_lone_surrogates` uses that to get
//! ICU4C's answer from ICU4X: primaries segment by segment, then the lower levels with every
//! surrogate replaced by U+E000, whose secondary and tertiary weights are the same.

use std::cmp::Ordering;
use std::sync::OnceLock;

use icu_collator::{Collator, CollatorOptions, Numeric, Strength};
use icu_locid::locale;

use crate::string::JsString;

mod tables;

const SURROGATE_STAND_IN: u16 = 0xe000;

#[derive(Clone, Copy)]
enum Kind {
    Plain,
    Numeric,
    PlainPrimary,
    NumericPrimary,
}

fn collator(kind: Kind) -> &'static Collator {
    static COLLATORS: [OnceLock<Collator>; 4] = [const { OnceLock::new() }; 4];
    COLLATORS[kind as usize].get_or_init(|| {
        let mut options = CollatorOptions::new();
        if matches!(kind, Kind::Numeric | Kind::NumericPrimary) {
            options.numeric = Some(Numeric::On);
        }
        if matches!(kind, Kind::PlainPrimary | Kind::NumericPrimary) {
            options.strength = Some(Strength::Primary);
        }
        Collator::try_new(&locale!("en-US").into(), options)
            .expect("compiled collation data covers en-US")
    })
}

/// `a.localeCompare(b)`.
pub fn locale_compare(a: &JsString, b: &JsString) -> Ordering {
    locale_compare_utf16(a.units(), b.units())
}

/// `a.localeCompare(b, undefined, { numeric: true })`.
pub fn locale_compare_numeric(a: &JsString, b: &JsString) -> Ordering {
    locale_compare_numeric_utf16(a.units(), b.units())
}

pub fn locale_compare_utf16(a: &[u16], b: &[u16]) -> Ordering {
    compare(a, b, false)
}

pub fn locale_compare_numeric_utf16(a: &[u16], b: &[u16]) -> Ordering {
    compare(a, b, true)
}

pub fn locale_compare_str(a: &str, b: &str) -> Ordering {
    compare(&utf16(a), &utf16(b), false)
}

pub fn locale_compare_numeric_str(a: &str, b: &str) -> Ordering {
    compare(&utf16(a), &utf16(b), true)
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// The number `localeCompare` returns: -1, 0 or 1.
pub fn ordering_to_number(order: Ordering) -> f64 {
    match order {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    }
}

fn compare(a: &[u16], b: &[u16], numeric: bool) -> Ordering {
    let Some(mut skip) = icu_prefix(a, b, numeric) else {
        return Ordering::Equal;
    };
    if skip > 0 && lookback_context(a[skip - 1]) {
        skip -= 1;
    }
    let (a, b) = (&a[skip..], &b[skip..]);
    if !has_lone_surrogate(a) && !has_lone_surrogate(b) {
        let full = collator(if numeric { Kind::Numeric } else { Kind::Plain });
        return full.compare_utf16(a, b);
    }
    compare_with_lone_surrogates(a, b, numeric)
}

/// The identical prefix ICU4C skips, or `None` when the strings are identical.
fn icu_prefix(a: &[u16], b: &[u16], numeric: bool) -> Option<usize> {
    let mut len = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    if len == a.len() && len == b.len() {
        return None;
    }
    let unsafe_at = |units: &[u16], i: usize| {
        units
            .get(i)
            .is_some_and(|&u| is_unsafe_backward(u, numeric))
    };
    if len > 0 && (unsafe_at(a, len) || unsafe_at(b, len)) {
        loop {
            len -= 1;
            if len == 0 || !is_unsafe_backward(a[len], numeric) {
                break;
            }
        }
    }
    Some(len)
}

/// ICU4C starts comparing at the boundary but lets a prefix mapping there read the character
/// before it. ICU4X cannot look back, so that character is kept in both suffixes. The root
/// collation's only prefix mappings are U+00B7 after `L` or `l`, and both letters are starters
/// that never join what follows, so keeping them adds the same elements to each side.
fn lookback_context(unit: u16) -> bool {
    unit == u16::from(b'L') || unit == u16::from(b'l')
}

fn is_unsafe_backward(unit: u16, numeric: bool) -> bool {
    (numeric && in_ranges(tables::DIGITS, unit)) || in_ranges(tables::UNSAFE_BACKWARD, unit)
}

fn in_ranges(ranges: &[(u16, u16)], unit: u16) -> bool {
    ranges
        .binary_search_by(|&(lo, hi)| {
            if hi < unit {
                Ordering::Less
            } else if lo > unit {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        })
        .is_ok()
}

fn compare_with_lone_surrogates(a: &[u16], b: &[u16], numeric: bool) -> Ordering {
    let primary = collator(if numeric {
        Kind::NumericPrimary
    } else {
        Kind::PlainPrimary
    });
    let mut left = Segments::new(a);
    let mut right = Segments::new(b);
    loop {
        let (seg_a, sur_a) = left.next_segment();
        let (seg_b, sur_b) = right.next_segment();
        match primary.compare_utf16(seg_a, seg_b) {
            Ordering::Equal => match (sur_a, sur_b) {
                (None, None) => break,
                (Some(_), None) => return Ordering::Greater,
                (None, Some(_)) => return Ordering::Less,
                (Some(x), Some(y)) if x != y => return x.cmp(&y),
                (Some(_), Some(_)) => {}
            },
            Ordering::Less => {
                return match sur_a {
                    Some(_) if primary.compare_utf16(&with_stand_in(seg_a), seg_b).is_gt() => {
                        Ordering::Greater
                    }
                    _ => Ordering::Less,
                };
            }
            Ordering::Greater => {
                return match sur_b {
                    Some(_) if primary.compare_utf16(seg_a, &with_stand_in(seg_b)).is_lt() => {
                        Ordering::Less
                    }
                    _ => Ordering::Greater,
                };
            }
        }
    }
    let full = collator(if numeric { Kind::Numeric } else { Kind::Plain });
    full.compare_utf16(&replace_lone_surrogates(a), &replace_lone_surrogates(b))
}

fn with_stand_in(segment: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(segment.len() + 1);
    out.extend_from_slice(segment);
    out.push(SURROGATE_STAND_IN);
    out
}

fn is_lead(u: u16) -> bool {
    (0xd800..0xdc00).contains(&u)
}

fn is_trail(u: u16) -> bool {
    (0xdc00..0xe000).contains(&u)
}

fn lone_surrogate_at(units: &[u16], i: usize) -> bool {
    let u = units[i];
    if is_lead(u) {
        !units.get(i + 1).is_some_and(|&next| is_trail(next))
    } else if is_trail(u) {
        !(i > 0 && is_lead(units[i - 1]))
    } else {
        false
    }
}

fn has_lone_surrogate(units: &[u16]) -> bool {
    units.iter().any(|&u| (0xd800..0xe000).contains(&u))
        && (0..units.len()).any(|i| lone_surrogate_at(units, i))
}

fn replace_lone_surrogates(units: &[u16]) -> Vec<u16> {
    (0..units.len())
        .map(|i| {
            if lone_surrogate_at(units, i) {
                SURROGATE_STAND_IN
            } else {
                units[i]
            }
        })
        .collect()
}

/// Splits a string at its lone surrogates: each segment comes with the surrogate that ends it,
/// and the last segment with `None`.
struct Segments<'a> {
    units: &'a [u16],
    start: usize,
}

impl<'a> Segments<'a> {
    fn new(units: &'a [u16]) -> Self {
        Segments { units, start: 0 }
    }

    fn next_segment(&mut self) -> (&'a [u16], Option<u16>) {
        let start = self.start.min(self.units.len());
        for i in start..self.units.len() {
            if lone_surrogate_at(self.units, i) {
                self.start = i + 1;
                return (&self.units[start..i], Some(self.units[i]));
            }
        }
        self.start = self.units.len();
        (&self.units[start..], None)
    }
}
