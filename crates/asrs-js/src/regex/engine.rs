//! Backtracking matcher following the ECMAScript pattern semantics (22.2.2) in continuation
//! passing style. Capture slots are mutated in place and every write is undone when its
//! continuation fails, so a failed branch leaves the state as it found it.

use super::parse::{Node, Repeat, is_line_terminator, is_word_unit};

pub(crate) type Slots = Vec<Option<(usize, usize)>>;

type Cont<'k, 'a> = &'k mut dyn FnMut(&mut Matcher<'a>, usize) -> bool;

pub(crate) struct Matcher<'a> {
    input: &'a [u16],
    multiline: bool,
    dot_all: bool,
    pub(crate) slots: Slots,
}

impl<'a> Matcher<'a> {
    pub(crate) fn new(input: &'a [u16], captures: usize, multiline: bool, dot_all: bool) -> Self {
        Matcher {
            input,
            multiline,
            dot_all,
            slots: vec![None; captures + 1],
        }
    }

    pub(crate) fn match_at(&mut self, root: &'a Node, start: usize) -> bool {
        self.slots.iter_mut().for_each(|s| *s = None);
        let mut end = start;
        let matched = self.m(root, start, &mut |_, p| {
            end = p;
            true
        });
        if matched {
            self.slots[0] = Some((start, end));
        }
        matched
    }

    fn single(&self, node: &Node, unit: u16) -> Option<bool> {
        Some(match node {
            Node::Unit(u) => *u == unit,
            Node::Class(class) => class.contains(unit),
            Node::Dot => self.dot_all || !is_line_terminator(unit),
            _ => return None,
        })
    }

    fn m(&mut self, node: &'a Node, pos: usize, k: Cont<'_, 'a>) -> bool {
        match node {
            Node::Empty => k(self, pos),
            Node::Unit(_) | Node::Class(_) | Node::Dot => match self.input.get(pos) {
                Some(&unit) if self.single(node, unit) == Some(true) => k(self, pos + 1),
                _ => false,
            },
            Node::LineStart => {
                let ok = pos == 0 || (self.multiline && is_line_terminator(self.input[pos - 1]));
                ok && k(self, pos)
            }
            Node::LineEnd => {
                let ok = pos == self.input.len()
                    || (self.multiline && is_line_terminator(self.input[pos]));
                ok && k(self, pos)
            }
            Node::WordBoundary { negated } => {
                let before = pos > 0 && is_word_unit(self.input[pos - 1]);
                let after = self.input.get(pos).is_some_and(|&u| is_word_unit(u));
                ((before != after) != *negated) && k(self, pos)
            }
            Node::Group {
                node,
                capture: None,
            } => self.m(node, pos, k),
            Node::Group {
                node,
                capture: Some(i),
            } => {
                let i = *i;
                self.m(node, pos, &mut |me, end| {
                    let old = me.slots[i];
                    me.slots[i] = Some((pos, end));
                    if k(me, end) {
                        return true;
                    }
                    me.slots[i] = old;
                    false
                })
            }
            Node::Lookahead { node, negated } => {
                let saved = self.slots.clone();
                let found = self.m(node, pos, &mut |_, _| true);
                if found == *negated {
                    self.slots = saved;
                    return false;
                }
                if k(self, pos) {
                    return true;
                }
                self.slots = saved;
                false
            }
            Node::Seq(nodes) => self.seq(nodes, pos, k),
            Node::Alt(alternatives) => {
                for alt in alternatives {
                    if self.m(alt, pos, &mut *k) {
                        return true;
                    }
                }
                false
            }
            Node::Repeat(repeat) => {
                if matches!(repeat.node, Node::Unit(_) | Node::Class(_) | Node::Dot) {
                    self.repeat_single(repeat, pos, k)
                } else {
                    self.repeat(repeat, repeat.min, repeat.max, pos, k)
                }
            }
        }
    }

    fn seq(&mut self, nodes: &'a [Node], pos: usize, k: Cont<'_, 'a>) -> bool {
        match nodes.split_first() {
            None => k(self, pos),
            Some((first, rest)) => self.m(first, pos, &mut |me, p| me.seq(rest, p, k)),
        }
    }

    /// RepeatMatcher for an atom that always consumes exactly one code unit and holds no
    /// captures: the empty check never fires, so trying the counts in order is equivalent and
    /// keeps the stack flat.
    fn repeat_single(&mut self, r: &'a Repeat, pos: usize, k: Cont<'_, 'a>) -> bool {
        let limit = r.max.map_or(usize::MAX, |m| m as usize);
        let min = r.min as usize;
        let mut n = 0;
        while n < limit {
            match self.input.get(pos + n) {
                Some(&unit) if self.single(&r.node, unit) == Some(true) => n += 1,
                _ => break,
            }
        }
        if n < min {
            return false;
        }
        if r.greedy {
            (min..=n).rev().any(|i| k(self, pos + i))
        } else {
            (min..=n).any(|i| k(self, pos + i))
        }
    }

    fn repeat(
        &mut self,
        r: &'a Repeat,
        min: u32,
        max: Option<u32>,
        pos: usize,
        k: Cont<'_, 'a>,
    ) -> bool {
        if max == Some(0) {
            return k(self, pos);
        }
        let range = r.first_capture..r.first_capture + r.capture_count;
        if !r.greedy && min == 0 && k(self, pos) {
            return true;
        }
        let saved: Vec<_> = self.slots[range.clone()].to_vec();
        self.slots[range.clone()].iter_mut().for_each(|s| *s = None);
        let matched = self.m(&r.node, pos, &mut |me, end| {
            if min == 0 && end == pos {
                return false;
            }
            me.repeat(r, min.saturating_sub(1), max.map(|m| m - 1), end, &mut *k)
        });
        if matched {
            return true;
        }
        self.slots[range].copy_from_slice(&saved);
        min == 0 && r.greedy && k(self, pos)
    }
}
