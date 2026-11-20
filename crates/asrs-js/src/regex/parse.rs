//! Parser for the subset of non-unicode ECMAScript pattern syntax the engine runs.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError(pub String);

impl fmt::Display for PatternError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PatternError {}

#[derive(Debug, Clone)]
pub(crate) struct Class {
    ranges: Vec<(u16, u16)>,
    negated: bool,
}

impl Class {
    pub(crate) fn contains(&self, unit: u16) -> bool {
        let inside = self
            .ranges
            .binary_search_by(|&(lo, hi)| {
                if unit < lo {
                    std::cmp::Ordering::Greater
                } else if unit > hi {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .is_ok();
        inside != self.negated
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Empty,
    Unit(u16),
    Class(Class),
    Dot,
    LineStart,
    LineEnd,
    WordBoundary {
        negated: bool,
    },
    Group {
        node: Box<Node>,
        capture: Option<usize>,
    },
    Lookahead {
        node: Box<Node>,
        negated: bool,
    },
    Seq(Vec<Node>),
    Alt(Vec<Node>),
    Repeat(Box<Repeat>),
}

#[derive(Debug, Clone)]
pub(crate) struct Repeat {
    pub(crate) node: Node,
    pub(crate) min: u32,
    pub(crate) max: Option<u32>,
    pub(crate) greedy: bool,
    pub(crate) first_capture: usize,
    pub(crate) capture_count: usize,
}

pub(crate) struct Parsed {
    pub(crate) root: Node,
    pub(crate) captures: usize,
}

pub(crate) fn parse(pattern: &str) -> Result<Parsed, PatternError> {
    let units: Vec<u16> = pattern.encode_utf16().collect();
    let mut parser = Parser {
        units: &units,
        pos: 0,
        captures: 0,
    };
    let root = parser.disjunction()?;
    if parser.pos != units.len() {
        return Err(parser.error("unmatched ')'"));
    }
    Ok(Parsed {
        root,
        captures: parser.captures,
    })
}

const DIGITS: &[(u16, u16)] = &[(0x30, 0x39)];
const WORD: &[(u16, u16)] = &[(0x30, 0x39), (0x41, 0x5a), (0x5f, 0x5f), (0x61, 0x7a)];
const SPACE: &[(u16, u16)] = &[
    (0x09, 0x0d),
    (0x20, 0x20),
    (0xa0, 0xa0),
    (0x1680, 0x1680),
    (0x2000, 0x200a),
    (0x2028, 0x2029),
    (0x202f, 0x202f),
    (0x205f, 0x205f),
    (0x3000, 0x3000),
    (0xfeff, 0xfeff),
];

pub(crate) fn is_word_unit(unit: u16) -> bool {
    WORD.iter().any(|&(lo, hi)| (lo..=hi).contains(&unit))
}

pub(crate) fn is_space_unit(unit: u16) -> bool {
    SPACE.iter().any(|&(lo, hi)| (lo..=hi).contains(&unit))
}

pub(crate) fn is_line_terminator(unit: u16) -> bool {
    matches!(unit, 0x0a | 0x0d | 0x2028 | 0x2029)
}

fn complement(ranges: &[(u16, u16)]) -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    let mut next = 0u32;
    for &(lo, hi) in ranges {
        if u32::from(lo) > next {
            out.push((next as u16, lo - 1));
        }
        next = u32::from(hi) + 1;
    }
    if next <= 0xffff {
        out.push((next as u16, 0xffff));
    }
    out
}

fn normalize(mut ranges: Vec<(u16, u16)>) -> Vec<(u16, u16)> {
    ranges.sort_unstable();
    let mut out: Vec<(u16, u16)> = Vec::with_capacity(ranges.len());
    for (lo, hi) in ranges {
        match out.last_mut() {
            Some(last) if u32::from(lo) <= u32::from(last.1) + 1 => last.1 = last.1.max(hi),
            _ => out.push((lo, hi)),
        }
    }
    out
}

enum ClassAtom {
    Unit(u16),
    Set(Vec<(u16, u16)>),
}

struct Parser<'a> {
    units: &'a [u16],
    pos: usize,
    captures: usize,
}

impl Parser<'_> {
    fn error(&self, message: &str) -> PatternError {
        PatternError(format!("{message} at {}", self.pos))
    }

    fn peek(&self) -> Option<u16> {
        self.units.get(self.pos).copied()
    }

    fn peek_ascii(&self) -> Option<u8> {
        self.peek().and_then(|u| u8::try_from(u).ok())
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek_ascii() == Some(c) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn disjunction(&mut self) -> Result<Node, PatternError> {
        let mut alternatives = vec![self.alternative()?];
        while self.eat(b'|') {
            alternatives.push(self.alternative()?);
        }
        Ok(if alternatives.len() == 1 {
            alternatives.pop().unwrap_or(Node::Empty)
        } else {
            Node::Alt(alternatives)
        })
    }

    fn alternative(&mut self) -> Result<Node, PatternError> {
        let mut terms = Vec::new();
        while let Some(u) = self.peek() {
            if u == u16::from(b'|') || u == u16::from(b')') {
                break;
            }
            terms.push(self.term()?);
        }
        Ok(match terms.len() {
            0 => Node::Empty,
            1 => terms.pop().unwrap_or(Node::Empty),
            _ => Node::Seq(terms),
        })
    }

    fn term(&mut self) -> Result<Node, PatternError> {
        let first_capture = self.captures + 1;
        let atom = match self.peek_ascii() {
            Some(b'^') => {
                self.pos += 1;
                return Ok(Node::LineStart);
            }
            Some(b'$') => {
                self.pos += 1;
                return Ok(Node::LineEnd);
            }
            Some(b'\\') if matches!(self.units.get(self.pos + 1), Some(&u) if u == u16::from(b'b') || u == u16::from(b'B')) =>
            {
                let negated = self.units[self.pos + 1] == u16::from(b'B');
                self.pos += 2;
                return Ok(Node::WordBoundary { negated });
            }
            Some(b'(') => self.group()?,
            Some(b'.') => {
                self.pos += 1;
                Node::Dot
            }
            Some(b'[') => self.class()?,
            Some(b'\\') => match self.escape(false)? {
                ClassAtom::Unit(u) => Node::Unit(u),
                ClassAtom::Set(ranges) => Node::Class(Class {
                    ranges,
                    negated: false,
                }),
            },
            Some(b'*' | b'+' | b'?' | b'{' | b'}' | b']') => {
                return Err(self.error("unsupported syntax"));
            }
            _ => {
                let u = self.units[self.pos];
                self.pos += 1;
                Node::Unit(u)
            }
        };
        let is_lookahead = matches!(atom, Node::Lookahead { .. });
        let Some((min, max)) = self.quantifier()? else {
            return Ok(atom);
        };
        if is_lookahead {
            return Err(self.error("quantified lookahead is not supported"));
        }
        if max.is_some_and(|max| max < min) {
            return Err(self.error("numbers out of order in {} quantifier"));
        }
        let greedy = !self.eat(b'?');
        Ok(Node::Repeat(Box::new(Repeat {
            node: atom,
            min,
            max,
            greedy,
            first_capture,
            capture_count: self.captures + 1 - first_capture,
        })))
    }

    fn quantifier(&mut self) -> Result<Option<(u32, Option<u32>)>, PatternError> {
        Ok(Some(match self.peek_ascii() {
            Some(b'*') => {
                self.pos += 1;
                (0, None)
            }
            Some(b'+') => {
                self.pos += 1;
                (1, None)
            }
            Some(b'?') => {
                self.pos += 1;
                (0, Some(1))
            }
            Some(b'{') => {
                self.pos += 1;
                let min = self.number()?;
                let max = if self.eat(b',') {
                    if self.peek_ascii() == Some(b'}') {
                        None
                    } else {
                        Some(self.number()?)
                    }
                } else {
                    Some(min)
                };
                if !self.eat(b'}') {
                    return Err(self.error("incomplete quantifier"));
                }
                (min, max)
            }
            _ => return Ok(None),
        }))
    }

    fn number(&mut self) -> Result<u32, PatternError> {
        let start = self.pos;
        let mut value: u32 = 0;
        while let Some(d) = self.peek_ascii().filter(u8::is_ascii_digit) {
            value = value.saturating_mul(10).saturating_add(u32::from(d - b'0'));
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.error("incomplete quantifier"));
        }
        Ok(value)
    }

    fn group(&mut self) -> Result<Node, PatternError> {
        self.pos += 1;
        let node = if self.eat(b'?') {
            match self.peek_ascii() {
                Some(b':') => {
                    self.pos += 1;
                    Node::Group {
                        node: Box::new(self.disjunction()?),
                        capture: None,
                    }
                }
                Some(c @ (b'=' | b'!')) => {
                    self.pos += 1;
                    Node::Lookahead {
                        node: Box::new(self.disjunction()?),
                        negated: c == b'!',
                    }
                }
                _ => return Err(self.error("unsupported group")),
            }
        } else {
            self.captures += 1;
            let capture = self.captures;
            Node::Group {
                node: Box::new(self.disjunction()?),
                capture: Some(capture),
            }
        };
        if !self.eat(b')') {
            return Err(self.error("unterminated group"));
        }
        Ok(node)
    }

    fn class(&mut self) -> Result<Node, PatternError> {
        self.pos += 1;
        let negated = self.eat(b'^');
        let mut ranges = Vec::new();
        loop {
            match self.peek_ascii() {
                None if self.peek().is_none() => {
                    return Err(self.error("unterminated character class"));
                }
                Some(b']') => {
                    self.pos += 1;
                    break;
                }
                _ => {}
            }
            let low = self.class_atom()?;
            let is_range = self.peek_ascii() == Some(b'-')
                && !matches!(self.units.get(self.pos + 1), Some(&u) if u == u16::from(b']'))
                && self.pos + 1 < self.units.len();
            if !is_range {
                push_atom(&mut ranges, low);
                continue;
            }
            self.pos += 1;
            let high = self.class_atom()?;
            match (low, high) {
                (ClassAtom::Unit(lo), ClassAtom::Unit(hi)) => {
                    if lo > hi {
                        return Err(self.error("range out of order in character class"));
                    }
                    ranges.push((lo, hi));
                }
                (low, high) => {
                    push_atom(&mut ranges, low);
                    ranges.push((u16::from(b'-'), u16::from(b'-')));
                    push_atom(&mut ranges, high);
                }
            }
        }
        Ok(Node::Class(Class {
            ranges: normalize(ranges),
            negated,
        }))
    }

    fn class_atom(&mut self) -> Result<ClassAtom, PatternError> {
        if self.peek_ascii() == Some(b'\\') {
            return self.escape(true);
        }
        let u = self
            .peek()
            .ok_or_else(|| self.error("unterminated class"))?;
        self.pos += 1;
        Ok(ClassAtom::Unit(u))
    }

    fn escape(&mut self, in_class: bool) -> Result<ClassAtom, PatternError> {
        self.pos += 1;
        let c = self
            .peek_ascii()
            .ok_or_else(|| self.error("unsupported escape"))?;
        self.pos += 1;
        let set = |ranges: &[(u16, u16)], negated: bool| {
            Ok(ClassAtom::Set(if negated {
                complement(ranges)
            } else {
                ranges.to_vec()
            }))
        };
        let unit = match c {
            b'd' | b'D' => return set(DIGITS, c == b'D'),
            b'w' | b'W' => return set(WORD, c == b'W'),
            b's' | b'S' => return set(SPACE, c == b'S'),
            b'n' => 0x0a,
            b'r' => 0x0d,
            b't' => 0x09,
            b'v' => 0x0b,
            b'f' => 0x0c,
            b'b' if in_class => 0x08,
            b'0' if !self.peek_ascii().is_some_and(|d| d.is_ascii_digit()) => 0,
            b'x' => self.hex(2)?,
            b'u' => self.hex(4)?,
            c if c.is_ascii_punctuation() => u16::from(c),
            _ => return Err(self.error("unsupported escape")),
        };
        Ok(ClassAtom::Unit(unit))
    }

    fn hex(&mut self, digits: usize) -> Result<u16, PatternError> {
        let mut value = 0u16;
        for _ in 0..digits {
            let d = self
                .peek_ascii()
                .and_then(|c| char::from(c).to_digit(16))
                .ok_or_else(|| self.error("unsupported escape"))?;
            value = value * 16 + d as u16;
            self.pos += 1;
        }
        Ok(value)
    }
}

fn push_atom(ranges: &mut Vec<(u16, u16)>, atom: ClassAtom) {
    match atom {
        ClassAtom::Unit(u) => ranges.push((u, u)),
        ClassAtom::Set(set) => ranges.extend(set),
    }
}
