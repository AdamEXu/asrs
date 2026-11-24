//! Where text can be split between characters: caret positions in a shaped run and word ends.

use std::cmp::Ordering;
use std::ops::Range;

use icu_segmenter::options::WordBreakInvariantOptions;
use icu_segmenter::{GraphemeClusterSegmenter, WordSegmenter};

use super::dom::{boundaries, utf16_len};
use super::layout::normalize_space;
use super::{FontSpec, TextBackend};

/// The byte offsets between `text`'s characters as a reader sees them, ends included, each with
/// its distance from the start in device px, for `text` shaped alone.
pub fn carets(
    backend: &mut dyn TextBackend,
    font: &FontSpec,
    text: &str,
    scale: f32,
) -> Vec<(usize, f32)> {
    backend.set_device_scale(scale);
    let shaped = backend.shape(font, &normalize_space(text), scale);
    let at = boundaries(&shaped, utf16_len(text));
    let mut unit = 0;
    let mut from = 0;
    GraphemeClusterSegmenter::new()
        .segment_str(text)
        .map(|byte| {
            unit += utf16_len(&text[from..byte]);
            from = byte;
            (byte, at[unit] * scale)
        })
        .collect()
}

/// The advance of a space in device px.
pub fn space_width(backend: &mut dyn TextBackend, font: &FontSpec, scale: f32) -> f32 {
    backend.set_device_scale(scale);
    backend.space_width(font, scale)
}

/// The stretch between the word boundaries around the character at byte `at` of `text`.
pub fn word_at(text: &str, at: usize) -> Range<usize> {
    let segmenter = WordSegmenter::new_dictionary(WordBreakInvariantOptions::default());
    let mut start = 0;
    let mut word = None;
    for boundary in segmenter.segment_str(text) {
        if boundary > at {
            word = Some(start..boundary);
            break;
        }
        start = boundary;
    }
    let mut word = word.unwrap_or(start..text.len());
    let from = word.start;
    for (stop, _) in text[word.clone()].match_indices('.') {
        let stop = from + stop;
        if between_digits(text, stop) {
            continue;
        }
        match at.cmp(&stop) {
            Ordering::Less => word.end = word.end.min(stop),
            Ordering::Equal => return stop..stop + 1,
            Ordering::Greater => word.start = word.start.max(stop + 1),
        }
    }
    word
}

/// Chromium's word rules, unlike the standard ones, end a word at any other full stop.
fn between_digits(text: &str, stop: usize) -> bool {
    let before = text[..stop].chars().next_back();
    let after = text[stop + 1..].chars().next();
    [before, after].into_iter().flatten().all(char::is_numeric)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_chromiums() {
        let text = "\"Idle\" +1.5 don't, naïve";
        let words = |at: usize| &text[word_at(text, at)];
        assert_eq!(words(0), "\"");
        assert_eq!(words(2), "Idle");
        assert_eq!(words(6), " ");
        assert_eq!(words(7), "+");
        assert_eq!(words(9), "1.5");
        assert_eq!(words(13), "don't");
        assert_eq!(words(text.len() - 1), "naïve");
        assert_eq!(word_at("www.x.org", 4), 4..5);
        assert_eq!(word_at("www.x.org", 5), 5..6);
        assert_eq!(word_at("www.x.org", 0), 0..3);
        assert_eq!(word_at("www.x.org", 8), 6..9);
    }
}
