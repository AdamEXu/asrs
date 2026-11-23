// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Where a line of `white-space: normal` text may end, as Blink's `LazyLineBreakIterator` has
//! it: after spaces, between two ASCII characters by its pair table, and elsewhere by ICU.

use icu_segmenter::LineSegmenter;
use icu_segmenter::options::LineBreakOptions;

const NBSP: char = '\u{a0}';

/// `kAsciiLineBreakTable`, less the digits after a hyphen.
fn ascii_pair(before: char, after: char) -> bool {
    match before {
        '!' | '"' | '#' | '%' | '&' | ')' | '*' | '+' | ',' | '.' | ':' | ';' | '=' | '>'
        | '\\' | ']' | '|' | '}' | '~' => matches!(after, '(' | '<' | '[' | '{'),
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
    breaks(text).collect()
}

/// [`opportunities`], found as they are asked for.
pub fn breaks(text: &str) -> impl Iterator<Item = usize> {
    let mut icu = None;
    let (mut before_last, mut last) = ('\0', None);
    text.char_indices().filter_map(move |(byte, c)| {
        let mut icu_breaks = || {
            let found = icu.get_or_insert_with(|| {
                let options = LineBreakOptions::default();
                LineSegmenter::new_for_non_complex_scripts(options)
                    .segment_str(text)
                    .peekable()
            });
            while found.next_if(|&at| at < byte).is_some() {}
            found.peek() == Some(&byte)
        };
        let here = last.is_some_and(|previous| {
            c != ' ' && (previous == ' ' || may_break(before_last, previous, c, &mut icu_breaks))
        });
        before_last = last.unwrap_or('\0');
        last = Some(c);
        here.then_some(byte)
    })
}

fn may_break(before_last: char, last: char, c: char, icu: &mut dyn FnMut() -> bool) -> bool {
    if last == '-' && c.is_ascii_digit() {
        return before_last.is_ascii_alphanumeric();
    }
    let letter = needs_icu(c) && c.is_alphabetic() && c < '\u{2e80}';
    if last == '-' && letter && matches!(before_last, '\0' | ' ' | '\u{200b}' | NBSP) {
        // LB20a, which icu_segmenter leaves out at the start of text.
        return false;
    }
    if needs_icu(last) || needs_icu(c) {
        return icu();
    }
    last.is_ascii() && c.is_ascii() && ascii_pair(last, c)
}
