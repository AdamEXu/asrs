//! JS regular expressions for the patterns upstream applies to log-derived strings.
//!
//! `RegExp` runs a non-unicode, case-sensitive subset of the pattern syntax (literals, escapes,
//! classes, `.`, `^`, `$`, `\b`, groups, lookahead, greedy and lazy quantifiers) with the
//! `g`, `y`, `m` and `s` flags, matching code unit by code unit like V8 without `u`. Flags
//! upstream never uses (`i`, `u`, `v`, `d`) are rejected rather than approximated.
//!
//! The string-level operations (`match_global`, `replace`, `split`, ...) behave as on a fresh
//! literal whose `lastIndex` is 0, which is every upstream call site; `exec` takes `lastIndex`
//! explicitly.
//!
//! `sites` holds direct matchers for each upstream regex, checked against this engine.

mod engine;
mod parse;
pub mod sites;

use crate::string::JsString;
use engine::Matcher;
pub use parse::PatternError;
use parse::{Node, parse};

#[derive(Debug, Clone)]
pub struct RegExp {
    root: Node,
    captures: usize,
    global: bool,
    sticky: bool,
    multiline: bool,
    dot_all: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Match {
    slots: Vec<Option<(usize, usize)>>,
}

impl Match {
    pub fn index(&self) -> usize {
        self.range(0).map_or(0, |r| r.0)
    }

    pub fn end(&self) -> usize {
        self.range(0).map_or(0, |r| r.1)
    }

    /// Number of capture groups, not counting the whole match.
    pub fn captures(&self) -> usize {
        self.slots.len() - 1
    }

    /// Range of group `i` (0 is the whole match); `None` when the group did not participate.
    pub fn range(&self, i: usize) -> Option<(usize, usize)> {
        self.slots.get(i).copied().flatten()
    }

    pub fn group(&self, input: &[u16], i: usize) -> Option<JsString> {
        self.range(i)
            .map(|(s, e)| JsString::from_units(input[s..e].to_vec()))
    }
}

impl RegExp {
    pub fn new(pattern: &str, flags: &str) -> Result<Self, PatternError> {
        let parsed = parse(pattern)?;
        let mut re = RegExp {
            root: parsed.root,
            captures: parsed.captures,
            global: false,
            sticky: false,
            multiline: false,
            dot_all: false,
        };
        for flag in flags.chars() {
            let slot = match flag {
                'g' => &mut re.global,
                'y' => &mut re.sticky,
                'm' => &mut re.multiline,
                's' => &mut re.dot_all,
                _ => return Err(PatternError(format!("unsupported flag {flag:?}"))),
            };
            if *slot {
                return Err(PatternError(format!("duplicate flag {flag:?}")));
            }
            *slot = true;
        }
        Ok(re)
    }

    pub fn global(&self) -> bool {
        self.global
    }

    pub fn sticky(&self) -> bool {
        self.sticky
    }

    pub fn capture_count(&self) -> usize {
        self.captures
    }

    /// A match starting exactly at `start`.
    pub fn match_at(&self, input: &[u16], start: usize) -> Option<Match> {
        if start > input.len() {
            return None;
        }
        let mut matcher = Matcher::new(input, self.captures, self.multiline, self.dot_all);
        if !matcher.match_at(&self.root, start) {
            return None;
        }
        Some(Match {
            slots: matcher.slots,
        })
    }

    /// The leftmost match starting at or after `start`.
    pub fn find_from(&self, input: &[u16], start: usize) -> Option<Match> {
        let mut matcher = Matcher::new(input, self.captures, self.multiline, self.dot_all);
        (start..=input.len())
            .find(|&s| matcher.match_at(&self.root, s))
            .map(|_| Match {
                slots: matcher.slots,
            })
    }

    /// `RegExpBuiltinExec`, reading and updating `lastIndex` as V8 does.
    pub fn exec(&self, input: &[u16], last_index: &mut usize) -> Option<Match> {
        let uses_last_index = self.global || self.sticky;
        let start = if uses_last_index { *last_index } else { 0 };
        if start > input.len() {
            if uses_last_index {
                *last_index = 0;
            }
            return None;
        }
        let found = if self.sticky {
            self.match_at(input, start)
        } else {
            self.find_from(input, start)
        };
        if uses_last_index {
            *last_index = found.as_ref().map_or(0, Match::end);
        }
        found
    }

    /// `RegExp.prototype.test` on a fresh regex.
    pub fn test(&self, input: &[u16]) -> bool {
        self.exec(input, &mut 0).is_some()
    }

    /// Every match the global `@@match`/`@@replace` loop visits, from `lastIndex` 0; for a
    /// non-global regex, the single `exec` result.
    pub fn matches(&self, input: &[u16]) -> Vec<Match> {
        let mut out = Vec::new();
        let mut last_index = 0;
        if !self.global {
            out.extend(self.exec(input, &mut last_index));
            return out;
        }
        while let Some(m) = self.exec(input, &mut last_index) {
            if m.index() == m.end() {
                last_index += 1;
            }
            out.push(m);
        }
        out
    }

    /// `String.prototype.match` with a global regex: every matched substring, or `None`.
    pub fn match_global(&self, input: &[u16]) -> Option<Vec<JsString>> {
        let found: Vec<JsString> = self
            .matches(input)
            .iter()
            .filter_map(|m| m.group(input, 0))
            .collect();
        (!found.is_empty()).then_some(found)
    }

    /// `String.prototype.replace` with a callback, called with each match.
    pub fn replace_with(
        &self,
        input: &[u16],
        mut replacer: impl FnMut(&Match) -> JsString,
    ) -> JsString {
        let mut out = Vec::with_capacity(input.len());
        let mut next = 0;
        for m in self.matches(input) {
            let replacement = replacer(&m);
            if m.index() >= next {
                out.extend_from_slice(&input[next..m.index()]);
                out.extend_from_slice(replacement.units());
                next = m.end();
            }
        }
        out.extend_from_slice(&input[next.min(input.len())..]);
        JsString::from_units(out)
    }

    /// `String.prototype.replace` with a replacement template (`GetSubstitution`).
    pub fn replace(&self, input: &[u16], template: &[u16]) -> JsString {
        self.replace_with(input, |m| substitute(input, m, template))
    }

    /// `String.prototype.split` (`RegExp.prototype[@@split]`); `None` entries are captures that
    /// did not participate (`undefined`).
    pub fn split(&self, input: &[u16], limit: Option<u32>) -> Vec<Option<JsString>> {
        let limit = limit.unwrap_or(u32::MAX) as usize;
        let mut out = Vec::new();
        if limit == 0 {
            return out;
        }
        let piece = |a: usize, b: usize| Some(JsString::from_units(input[a..b].to_vec()));
        let size = input.len();
        if size == 0 {
            if self.match_at(input, 0).is_none() {
                out.push(piece(0, 0));
            }
            return out;
        }
        let mut p = 0;
        let mut q = 0;
        while q < size {
            let Some(m) = self.match_at(input, q) else {
                q += 1;
                continue;
            };
            let e = m.end().min(size);
            if e == p {
                q += 1;
                continue;
            }
            out.push(piece(p, q));
            if out.len() == limit {
                return out;
            }
            p = e;
            for i in 1..=m.captures() {
                out.push(m.group(input, i));
                if out.len() == limit {
                    return out;
                }
            }
            q = p;
        }
        out.push(piece(p, size));
        out
    }
}

/// `GetSubstitution` for a regex match without named groups.
pub fn substitute(input: &[u16], m: &Match, template: &[u16]) -> JsString {
    let dollar = u16::from(b'$');
    let digit = |i: usize| {
        template
            .get(i)
            .and_then(|&u| u8::try_from(u).ok())
            .filter(u8::is_ascii_digit)
            .map(|d| usize::from(d - b'0'))
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < template.len() {
        let unit = template[i];
        if unit != dollar || i + 1 >= template.len() {
            out.push(unit);
            i += 1;
            continue;
        }
        let next = template[i + 1];
        if next == dollar {
            out.push(dollar);
            i += 2;
        } else if next == u16::from(b'&') {
            out.extend_from_slice(&input[m.index()..m.end()]);
            i += 2;
        } else if next == u16::from(b'`') {
            out.extend_from_slice(&input[..m.index()]);
            i += 2;
        } else if next == u16::from(b'\'') {
            out.extend_from_slice(&input[m.end().min(input.len())..]);
            i += 2;
        } else if let Some(first) = digit(i + 1) {
            let (mut index, mut len) = match digit(i + 2) {
                Some(second) => (first * 10 + second, 2),
                None => (first, 1),
            };
            if index > m.captures() && len == 2 {
                index = first;
                len = 1;
            }
            if (1..=m.captures()).contains(&index) {
                if let Some((s, e)) = m.range(index) {
                    out.extend_from_slice(&input[s..e]);
                }
            } else {
                out.extend_from_slice(&template[i..i + 1 + len]);
            }
            i += 1 + len;
        } else {
            out.push(unit);
            i += 1;
        }
    }
    JsString::from_units(out)
}
