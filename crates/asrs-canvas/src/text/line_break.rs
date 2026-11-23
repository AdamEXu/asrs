//! Where a line of `white-space: normal` text may end, as Blink's `LazyLineBreakIterator` has
//! it: after spaces, between two ASCII characters by its pair table, and elsewhere by ICU.

use icu_segmenter::LineSegmenter;
use icu_segmenter::options::LineBreakOptions;

const NBSP: char = '\u{a0}';

/// `kAsciiLineBreakTable`, less the digits after a hyphen.
fn ascii_pair(before: char, after: char) -> bool {
    match before {
        '!' | '"' | '#' | '%' | '&' | ')' | '*' | '+' | ',' | '.' | ':' | ';' | '=' | '>' | '\\'
        | ']' | '|' | '}' | '~' => matches!(after, '(' | '<' | '[' | '{'),
        '-' => !matches!(
            after,
            '!' | '$' | ')' | ',' | '.' | '/' | ':' | ';' | '?' | ']' | '}'
        ),
        '?' => !matches!(
            after,
            '!' | '"' | '\'' | ')' | ',' | '.' | '/' | ':' | ';' | '?' | ']' | '}'
        ),
        _ => false,
    }
}

fn needs_icu(c: char) -> bool {
    c > '\u{7f}' && c != NBSP
}

/// The byte offsets at which the next line may start when one starts at `text`'s own start: Blink
/// breaks each line with no regard for the text before it. Spaces are collapsed.
pub fn opportunities(text: &str) -> Vec<usize> {
    let icu: Vec<usize> = if text.chars().any(needs_icu) {
        let segmenter = LineSegmenter::new_for_non_complex_scripts(LineBreakOptions::default());
        segmenter.segment_str(text).collect()
    } else {
        Vec::new()
    };
    let mut out = Vec::new();
    let (mut before_last, mut last) = ('\0', None);
    for (byte, c) in text.char_indices() {
        if let Some(previous) = last
            && c != ' '
            && (previous == ' ' || may_break(before_last, previous, c, &icu, byte))
        {
            out.push(byte);
        }
        before_last = last.unwrap_or('\0');
        last = Some(c);
    }
    out
}

fn may_break(before_last: char, last: char, c: char, icu: &[usize], byte: usize) -> bool {
    if last == '-' && c.is_ascii_digit() {
        return before_last.is_ascii_alphanumeric();
    }
    let letter = needs_icu(c) && c.is_alphabetic() && c < '\u{2e80}';
    if last == '-' && letter && matches!(before_last, '\0' | ' ' | '\u{200b}' | NBSP) {
        // LB20a, which icu_segmenter leaves out at the start of text.
        return false;
    }
    if needs_icu(last) || needs_icu(c) {
        return icu.binary_search(&byte).is_ok();
    }
    last.is_ascii() && c.is_ascii() && ascii_pair(last, c)
}
