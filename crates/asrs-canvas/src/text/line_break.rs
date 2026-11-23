// Ported from Blink, Chromium m142 (BSD-3-Clause, Copyright The Chromium Authors). See THIRD_PARTY_NOTICES.md.

//! Where a line of `white-space: normal` text may end, as Blink's `LazyLineBreakIterator` has
//! it: after spaces, between two ASCII characters by its pair table, and elsewhere by ICU.

use std::collections::VecDeque;

use icu_segmenter::LineSegmenter;
use icu_segmenter::options::{LineBreakOptions, LineBreakStrictness};

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
    !c.is_ascii()
}

/// Bytes past the end of a scan of complex script that may still change its breaks: more than
/// the dictionaries' longest word, 33 characters, takes in UTF-8.
const LOOKAHEAD: usize = 128;

/// Thai, Lao, Burmese and Khmer, which ICU breaks by dictionary.
fn is_complex(c: char) -> bool {
    matches!(c, '\u{e01}'..='\u{edf}' | '\u{1000}'..='\u{109f}' | '\u{1780}'..='\u{17dd}'
        | '\u{a9e0}'..='\u{a9fe}' | '\u{aa60}'..='\u{aa7f}')
}

fn is_hyphen(c: char) -> bool {
    matches!(c, '-' | '|' | '\u{2010}' | '\u{2012}' | '\u{2013}')
}

/// Whether ICU's breaks after `c`, which follows `previous` in the same scan, do not depend on
/// the text before it, and those up to it not on the text after it. True where every state of
/// `icu_segmenter`'s table resolves at `c`'s class. A hyphen or bar opens a rule after a Hebrew
/// letter only, so the second of two resolves. `false` for a character that would resolve only
/// costs a longer scan.
fn settles(previous: Option<char>, c: char) -> bool {
    match c {
        _ if is_hyphen(c) => previous.is_some_and(is_hyphen),
        '!'..='~' | NBSP => true,
        '\u{bc}'..='\u{2ff}' | '\u{370}'..='\u{482}' | '\u{48a}'..='\u{589}' => true,
        '\u{5d0}'..='\u{5ea}' | '\u{620}'..='\u{64a}' | '\u{671}'..='\u{6d5}' => true,
        '\u{904}'..='\u{939}' | '\u{1e00}'..='\u{1fff}' => true,
        '\u{2500}'..='\u{25cb}' | '\u{3041}'..='\u{3096}' => true,
        '\u{30a1}'..='\u{30fa}' | '\u{3400}'..='\u{9fff}' | '\u{ac00}'..='\u{d7a3}' => true,
        _ => false,
    }
}

fn segment(text: &str) -> impl Iterator<Item = usize> {
    let mut options = LineBreakOptions::default();
    options.strictness = Some(LineBreakStrictness::Normal);
    LineSegmenter::new_dictionary(options).segment_str(text)
}

/// The byte offsets at which the next line may start. Blink breaks each line with no regard for
/// the text before it, so they depend on where the current line starts. Spaces are collapsed.
pub struct LineBreaks<'a> {
    text: &'a str,
    /// ICU's breaks by byte offset; empty for ASCII text, which needs none.
    icu: Vec<bool>,
    next: VecDeque<usize>,
    #[cfg(test)]
    scanned: usize,
}

impl<'a> LineBreaks<'a> {
    /// The breaks of a line that starts where `text` does.
    pub fn new(text: &'a str) -> Self {
        let mut icu = Vec::new();
        if text.chars().any(needs_icu) {
            icu.resize(text.len() + 1, false);
            segment(text).for_each(|byte| icu[byte] = true);
        }
        let mut breaks = LineBreaks {
            text,
            icu,
            next: VecDeque::new(),
            #[cfg(test)]
            scanned: 0,
        };
        breaks.next = breaks.between(0, text.len()).into();
        breaks.next.push_back(text.len());
        breaks
    }

    /// Where the line may end, the last of them the end of the text.
    pub fn ends(&self) -> &VecDeque<usize> {
        &self.next
    }

    /// Starts the next line at `start`, which is past the start of the current one.
    pub fn restart(&mut self, start: usize) {
        let second = self.text[start..].char_indices().nth(1);
        let Some(second) = second.map(|(byte, _)| start + byte) else {
            self.next.retain(|&end| end > start);
            return;
        };
        let changed = second.max(self.rescan(start));
        while self.next.len() > 1 && self.next[0] <= changed {
            self.next.pop_front();
        }
        for end in self.between(start, changed).into_iter().rev() {
            self.next.push_front(end);
        }
    }

    /// Replaces ICU's breaks by those of a scan from `start` until the two agree for good, which
    /// is at a break of both or a character that [`settles`]. Returns where.
    fn rescan(&mut self, start: usize) -> usize {
        let first = self.text[start..].chars().next();
        if self.icu.is_empty() || self.icu[start] || first.is_some_and(|c| settles(None, c)) {
            return start;
        }
        let mut size = 4 * LOOKAHEAD;
        loop {
            let (end, known, settled) = self.window(start, size);
            let mut breaks = Vec::new();
            let mut agreed = None;
            #[cfg(test)]
            {
                self.scanned += end - start;
            }
            for byte in segment(&self.text[start..end]).map(|byte| start + byte) {
                if byte > known {
                    break;
                }
                if byte > start {
                    breaks.push(byte);
                    if self.icu[byte] {
                        agreed = Some(byte);
                        break;
                    }
                }
            }
            if let Some(agreed) = agreed.or(settled.then_some(known)) {
                self.icu[start + 1..=agreed].fill(false);
                for byte in breaks {
                    self.icu[byte] = true;
                }
                return agreed;
            }
            size *= 2;
        }
    }

    /// How far to scan from `start`, up to where the breaks found are final, and whether ICU's
    /// breaks agree from there on.
    fn window(&self, start: usize, size: usize) -> (usize, usize, bool) {
        let mut previous = None;
        for (offset, c) in self.text[start..].char_indices() {
            let byte = start + offset;
            if offset > 0 && settles(previous, c) {
                return (byte + c.len_utf8(), byte, true);
            }
            if offset >= size && is_complex(c) && previous.is_some_and(is_complex) {
                return (byte, byte - LOOKAHEAD, false);
            }
            previous = Some(c);
        }
        (self.text.len(), self.text.len(), true)
    }

    /// The breaks past `start`, where a line starts, up to `to`.
    fn between(&mut self, start: usize, to: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let (mut before_last, mut last) = ('\0', None);
        for (offset, c) in self.text[start..].char_indices() {
            let byte = start + offset;
            if byte > to {
                break;
            }
            #[cfg(test)]
            {
                self.scanned += c.len_utf8();
            }
            if let Some(previous) = last
                && c != ' '
                && (previous == ' ' || may_break(before_last, previous, c, &self.icu, byte))
            {
                out.push(byte);
            }
            before_last = last.unwrap_or('\0');
            last = Some(c);
        }
        out
    }
}

fn may_break(before_last: char, last: char, c: char, icu: &[bool], byte: usize) -> bool {
    if last == '-' && c.is_ascii_digit() {
        return before_last.is_ascii_alphanumeric();
    }
    let letter = needs_icu(c) && c.is_alphabetic() && c < '\u{2e80}';
    if last == '-' && letter && matches!(before_last, '\0' | ' ' | '\u{200b}' | NBSP) {
        // LB20a, which icu_segmenter leaves out at the start of text.
        return false;
    }
    if needs_icu(last) || needs_icu(c) {
        return icu[byte];
    }
    last.is_ascii() && c.is_ascii() && ascii_pair(last, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random(state: &mut u64) -> usize {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        (*state >> 33) as usize
    }

    /// Characters of every class `icu_segmenter`'s rules treat apart, letters weighted.
    const PIECES: &[&str] = &[
        "a",
        "Z",
        "xy",
        "wordy",
        "7",
        "42",
        " ",
        " ",
        " ",
        "-",
        "--",
        "|",
        "\u{a0}",
        "\u{ad}",
        "\u{200b}",
        "\u{200d}",
        "\u{2060}",
        "\"",
        "'",
        "(",
        ")",
        "[",
        "}",
        ",",
        ".",
        ":",
        "/",
        "!",
        "?",
        "%",
        "$",
        "\u{20ac}",
        "\u{e9}",
        "\u{df}",
        "\u{416}",
        "\u{3a9}",
        "\u{5d0}",
        "\u{5d1}",
        "\u{627}",
        "\u{628}",
        "\u{300}",
        "\u{93f}",
        "\u{915}",
        "\u{ab}",
        "\u{bb}",
        "\u{2018}",
        "\u{2019}",
        "\u{201c}",
        "\u{201d}",
        "\u{2010}",
        "\u{2013}",
        "\u{2014}",
        "\u{2026}",
        "\u{3042}",
        "\u{30fc}",
        "\u{3083}",
        "\u{4e2d}",
        "\u{3001}",
        "\u{300c}",
        "\u{300d}",
        "\u{301c}",
        "\u{ac00}",
        "\u{1100}",
        "\u{1161}",
        "\u{1f1fa}",
        "\u{1f1f8}",
        "\u{1f44d}",
        "\u{1f3fd}",
        "\u{1f468}\u{200d}\u{1f469}",
        "\u{2500}",
        "\u{1b05}",
        "\u{1b44}",
        "\u{25cc}",
        "\u{1a20}",
        "\u{fffc}",
        "\u{b4}",
        "\u{0}",
        "\u{85}",
        "\u{e20}\u{e32}\u{e29}\u{e32}\u{e44}\u{e17}\u{e22}",
        "\u{e01}",
        "\u{e49}",
        "\u{e33}",
        "\u{e9e}\u{eb2}\u{eaa}\u{eb2}",
        "\u{1797}\u{17b6}\u{179f}\u{17b6}",
        "\u{1019}\u{103c}\u{1014}\u{103a}\u{1019}\u{102c}",
    ];

    fn assert_matches_fresh_scans(text: &str, starts: impl Iterator<Item = usize>) {
        let mut breaks = LineBreaks::new(text);
        for start in starts {
            breaks.restart(start);
            let fresh: VecDeque<usize> = LineBreaks::new(&text[start..])
                .next
                .into_iter()
                .map(|end| start + end)
                .collect();
            assert_eq!(breaks.next, fresh, "{text:?} from {start}");
        }
    }

    #[test]
    fn a_restart_breaks_as_a_scan_of_the_rest() {
        let ascii: Vec<&str> = PIECES.iter().copied().filter(|p| p.is_ascii()).collect();
        let mut state = 1;
        for round in 0..600 {
            let few: Vec<&str> = (0..1 + random(&mut state) % 8)
                .map(|_| PIECES[random(&mut state) % PIECES.len()])
                .collect();
            let pieces = [PIECES, &few, &ascii][round % 3];
            let text: String = (0..1 + random(&mut state) % 60)
                .map(|_| pieces[random(&mut state) % pieces.len()])
                .collect();
            let mut starts: Vec<usize> =
                text.char_indices().map(|(byte, _)| byte).skip(1).collect();
            let keep = 1 + random(&mut state) % 4;
            starts.retain(|_| random(&mut state).is_multiple_of(keep));
            assert_matches_fresh_scans(&text, starts.into_iter());
        }
    }

    #[test]
    fn long_thai_breaks_as_a_scan_of_the_rest() {
        let mut state = 7;
        let text: String = (0..1000)
            .map(|_| char::from_u32(0xe01 + random(&mut state) as u32 % 0x2e).unwrap())
            .collect();
        for step in [1, 4, 400] {
            assert_matches_fresh_scans(&text, (3 * step..text.len()).step_by(3 * step));
        }
    }

    #[test]
    fn restarts_scan_little() {
        let text = |piece: &str| piece.repeat(200_000 / piece.len());
        let accent = |piece: &str| format!("{}\u{e9}", text(piece));
        for text in [
            text("a"),
            text("word "),
            text("-"),
            accent("a"),
            accent("-"),
            accent("7.5, "),
            accent("|--"),
            text("\u{e9}"),
            text("\u{5d0}"),
            text("\u{4e2d}\u{3001}"),
            text("\u{2013}"),
            text("\u{1f44d} "),
            text("\u{1f1fa}\u{1f1f8}"),
            text("\u{e20}\u{e32}\u{e29}\u{e32}\u{e44}\u{e17}\u{e22}"),
        ] {
            let mut breaks = LineBreaks::new(&text);
            let starts: Vec<usize> = text.char_indices().map(|(byte, _)| byte).collect();
            let mut lines = 0;
            for &start in starts[4..].iter().step_by(4) {
                breaks.restart(start);
                lines += 1;
            }
            assert!(lines > 10_000, "{lines} lines");
            let per_line = (breaks.scanned - text.len()) / lines;
            assert!(
                per_line < 1200,
                "{per_line} bytes a line: {:?}",
                text.chars().next()
            );
        }
    }
}
