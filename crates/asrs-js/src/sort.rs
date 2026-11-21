// Ported from V8 14.2 (BSD-3-Clause, Copyright the V8 project authors). See THIRD_PARTY_NOTICES.md.

//! `Array.prototype.sort` and `Array.prototype.toSorted` as V8 14.2 runs them: a port of V8's
//! TimSort (`third_party/v8/builtins/array-sort.tq` and `builtins/array-to-sorted.tq`). The
//! comparator is called with the same arguments in the same order as in V8, so results match
//! for inconsistent comparators too, and comparators with side effects see the same calls.

use std::cmp::Ordering;

use crate::string::JsString;

/// What the comparator returned, read the way V8 reads `comparefn`'s result: only "less than
/// zero" matters. NaN counts as zero.
pub trait SortOrder {
    fn is_less(&self) -> bool;
}

impl SortOrder for f64 {
    fn is_less(&self) -> bool {
        *self < 0.0
    }
}

impl SortOrder for i32 {
    fn is_less(&self) -> bool {
        *self < 0
    }
}

impl SortOrder for Ordering {
    fn is_less(&self) -> bool {
        *self == Ordering::Less
    }
}

/// An array element as `sort` sees it: holes and `undefined` are never passed to the comparator.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Slot<T> {
    Value(T),
    Undefined,
    Hole,
}

/// `array.sort(compare)` on an array without holes or `undefined`.
pub fn array_sort<T, R: SortOrder>(items: &mut [T], mut compare: impl FnMut(&T, &T) -> R) {
    if items.len() < 2 {
        return;
    }
    let order = sorted_indices(
        (0..items.len()).collect(),
        |x, y| compare(&items[x], &items[y]).is_less(),
        Method::Sort,
    );
    permute(items, &order);
}

/// `array.toSorted(compare)` on an array without holes or `undefined`.
pub fn array_to_sorted<T: Clone, R: SortOrder>(
    items: &[T],
    mut compare: impl FnMut(&T, &T) -> R,
) -> Vec<T> {
    let order = sorted_indices(
        (0..items.len()).collect(),
        |x, y| compare(&items[x], &items[y]).is_less(),
        Method::ToSorted,
    );
    order.into_iter().map(|i| items[i].clone()).collect()
}

/// `array.sort(compare)`: values sorted first, then every `undefined`, then the holes.
pub fn array_sort_slots<T, R: SortOrder>(
    items: &mut [Slot<T>],
    mut compare: impl FnMut(&T, &T) -> R,
) {
    if items.len() < 2 {
        return;
    }
    let values: Vec<usize> = (0..items.len())
        .filter(|&i| matches!(items[i], Slot::Value(_)))
        .collect();
    let undefined = items
        .iter()
        .filter(|s| matches!(s, Slot::Undefined))
        .count();
    let order = sorted_indices(
        values,
        |x, y| match (&items[x], &items[y]) {
            (Slot::Value(a), Slot::Value(b)) => compare(a, b).is_less(),
            _ => unreachable!("only values are sorted"),
        },
        Method::Sort,
    );
    let sorted = order.len();
    let mut layout = order;
    layout.extend((0..items.len()).filter(|&i| !matches!(items[i], Slot::Value(_))));
    permute(items, &layout);
    for (i, slot) in items.iter_mut().enumerate().skip(sorted) {
        *slot = if i < sorted + undefined {
            Slot::Undefined
        } else {
            Slot::Hole
        };
    }
}

/// `array.toSorted(compare)`: holes read as `undefined`, so the copy has values then `undefined`.
pub fn array_to_sorted_slots<T: Clone, R: SortOrder>(
    items: &[Slot<T>],
    mut compare: impl FnMut(&T, &T) -> R,
) -> Vec<Slot<T>> {
    let values: Vec<usize> = (0..items.len())
        .filter(|&i| matches!(items[i], Slot::Value(_)))
        .collect();
    let order = sorted_indices(
        values,
        |x, y| match (&items[x], &items[y]) {
            (Slot::Value(a), Slot::Value(b)) => compare(a, b).is_less(),
            _ => unreachable!("only values are sorted"),
        },
        Method::ToSorted,
    );
    let mut out: Vec<Slot<T>> = order.iter().map(|&i| items[i].clone()).collect();
    out.resize_with(items.len(), || Slot::Undefined);
    out
}

/// JS `<` on strings: UTF-16 code units compared in order, a proper prefix first.
pub fn code_unit_cmp(a: &[u16], b: &[u16]) -> Ordering {
    a.cmp(b)
}

/// The comparator `sort()` uses when given none, for string elements (`SortCompareDefault`).
pub fn default_compare(a: &JsString, b: &JsString) -> Ordering {
    code_unit_cmp(a.units(), b.units())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Method {
    Sort,
    ToSorted,
}

const MIN_GALLOP: usize = 7;

/// Sorts `work` (indices of the elements to sort, already compacted) and returns it. `less(x, y)`
/// is `Compare(x, y) < 0`.
fn sorted_indices(
    work: Vec<usize>,
    less: impl FnMut(usize, usize) -> bool,
    method: Method,
) -> Vec<usize> {
    let mut state = TimSort {
        work,
        temp: Vec::new(),
        runs: Vec::new(),
        min_gallop: MIN_GALLOP,
        less,
    };
    let length = state.work.len();
    if method == Method::Sort && length < 8 {
        state.binary_insertion_sort(0, 0, length);
    } else {
        state.sort(length);
    }
    state.work
}

/// Reorders `items` so that position `k` holds the element that was at `order[k]`.
fn permute<T>(items: &mut [T], order: &[usize]) {
    let mut done = vec![false; items.len()];
    for start in 0..items.len() {
        if done[start] {
            continue;
        }
        let mut k = start;
        loop {
            done[k] = true;
            let source = order[k];
            if source == start {
                break;
            }
            items.swap(k, source);
            k = source;
        }
    }
}

struct TimSort<F> {
    work: Vec<usize>,
    temp: Vec<usize>,
    runs: Vec<(usize, usize)>,
    min_gallop: usize,
    less: F,
}

enum MergeEnd {
    Succeed,
    CopyRest,
}

impl<F: FnMut(usize, usize) -> bool> TimSort<F> {
    fn sort(&mut self, length: usize) {
        if length < 2 {
            return;
        }
        let mut remaining = length;
        let mut low = 0;
        let min_run = compute_min_run_length(remaining);
        while remaining != 0 {
            let mut run = self.count_and_make_run(low, low + remaining);
            if run < min_run {
                let forced = min_run.min(remaining);
                self.binary_insertion_sort(low, low + run, low + forced);
                run = forced;
            }
            self.runs.push((low, run));
            self.merge_collapse();
            low += run;
            remaining -= run;
        }
        self.merge_force_collapse();
    }

    fn binary_insertion_sort(&mut self, low: usize, start: usize, high: usize) {
        let mut start = if low == start { start + 1 } else { start };
        while start < high {
            let mut left = low;
            let mut right = start;
            let pivot = self.work[right];
            while left < right {
                let mid = left + ((right - left) >> 1);
                if (self.less)(pivot, self.work[mid]) {
                    right = mid;
                } else {
                    left = mid + 1;
                }
            }
            self.work.copy_within(left..start, left + 1);
            self.work[left] = pivot;
            start += 1;
        }
    }

    fn count_and_make_run(&mut self, low_arg: usize, high: usize) -> usize {
        let low = low_arg + 1;
        if low == high {
            return 1;
        }
        let mut run = 2;
        let element_low = self.work[low];
        let element_low_pred = self.work[low - 1];
        let descending = (self.less)(element_low, element_low_pred);
        let mut previous = element_low;
        for idx in low + 1..high {
            let current = self.work[idx];
            let is_less = (self.less)(current, previous);
            if descending != is_less {
                break;
            }
            previous = current;
            run += 1;
        }
        if descending {
            self.work[low_arg..low_arg + run].reverse();
        }
        run
    }

    fn run_invariant_established(&self, n: usize) -> bool {
        if n < 2 {
            return true;
        }
        self.runs[n - 2].1 > self.runs[n - 1].1 + self.runs[n].1
    }

    fn merge_collapse(&mut self) {
        while self.runs.len() > 1 {
            let mut n = self.runs.len() - 2;
            if !self.run_invariant_established(n + 1) || !self.run_invariant_established(n) {
                if self.runs[n - 1].1 < self.runs[n + 1].1 {
                    n -= 1;
                }
                self.merge_at(n);
            } else if self.runs[n].1 <= self.runs[n + 1].1 {
                self.merge_at(n);
            } else {
                break;
            }
        }
    }

    fn merge_force_collapse(&mut self) {
        while self.runs.len() > 1 {
            let mut n = self.runs.len() - 2;
            if n > 0 && self.runs[n - 1].1 < self.runs[n + 1].1 {
                n -= 1;
            }
            self.merge_at(n);
        }
    }

    fn merge_at(&mut self, i: usize) {
        let (mut base_a, mut length_a) = self.runs[i];
        let (base_b, length_b) = self.runs[i + 1];
        self.runs[i].1 = length_a + length_b;
        self.runs.remove(i + 1);

        let key_right = self.work[base_b];
        let k = gallop_right(&mut self.less, &self.work, key_right, base_a, length_a, 0);
        base_a += k;
        length_a -= k;
        if length_a == 0 {
            return;
        }

        let key_left = self.work[base_a + length_a - 1];
        let length_b = gallop_left(
            &mut self.less,
            &self.work,
            key_left,
            base_b,
            length_b,
            length_b - 1,
        );
        if length_b == 0 {
            return;
        }

        if length_a <= length_b {
            self.merge_low(base_a, length_a, base_b, length_b);
        } else {
            self.merge_high(base_a, length_a, base_b, length_b);
        }
    }

    fn merge_low(&mut self, base_a: usize, length_a: usize, base_b: usize, length_b: usize) {
        let mut length_a = length_a;
        let mut length_b = length_b;
        self.temp.clear();
        self.temp
            .extend_from_slice(&self.work[base_a..base_a + length_a]);
        let work = &mut self.work;
        let temp = &self.temp;
        let less = &mut self.less;

        let mut dest = base_a;
        let mut cursor_temp = 0;
        let mut cursor_b = base_b;

        work[dest] = work[cursor_b];
        dest += 1;
        cursor_b += 1;

        let end = 'merge: {
            length_b -= 1;
            if length_b == 0 {
                break 'merge MergeEnd::Succeed;
            }
            if length_a == 1 {
                break 'merge MergeEnd::CopyRest;
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut wins_a = 0;
                let mut wins_b = 0;
                loop {
                    if less(work[cursor_b], temp[cursor_temp]) {
                        work[dest] = work[cursor_b];
                        dest += 1;
                        cursor_b += 1;
                        wins_b += 1;
                        length_b -= 1;
                        wins_a = 0;
                        if length_b == 0 {
                            break 'merge MergeEnd::Succeed;
                        }
                        if wins_b >= min_gallop {
                            break;
                        }
                    } else {
                        work[dest] = temp[cursor_temp];
                        dest += 1;
                        cursor_temp += 1;
                        wins_a += 1;
                        length_a -= 1;
                        wins_b = 0;
                        if length_a == 1 {
                            break 'merge MergeEnd::CopyRest;
                        }
                        if wins_a >= min_gallop {
                            break;
                        }
                    }
                }

                min_gallop += 1;
                let mut first = true;
                while wins_a >= MIN_GALLOP || wins_b >= MIN_GALLOP || first {
                    first = false;
                    min_gallop = (min_gallop - 1).max(1);
                    self.min_gallop = min_gallop;

                    wins_a = gallop_right(less, temp, work[cursor_b], cursor_temp, length_a, 0);
                    if wins_a > 0 {
                        work[dest..dest + wins_a]
                            .copy_from_slice(&temp[cursor_temp..cursor_temp + wins_a]);
                        dest += wins_a;
                        cursor_temp += wins_a;
                        length_a -= wins_a;
                        if length_a == 1 {
                            break 'merge MergeEnd::CopyRest;
                        }
                        if length_a == 0 {
                            break 'merge MergeEnd::Succeed;
                        }
                    }
                    work[dest] = work[cursor_b];
                    dest += 1;
                    cursor_b += 1;
                    length_b -= 1;
                    if length_b == 0 {
                        break 'merge MergeEnd::Succeed;
                    }

                    wins_b = gallop_left(less, work, temp[cursor_temp], cursor_b, length_b, 0);
                    if wins_b > 0 {
                        work.copy_within(cursor_b..cursor_b + wins_b, dest);
                        dest += wins_b;
                        cursor_b += wins_b;
                        length_b -= wins_b;
                        if length_b == 0 {
                            break 'merge MergeEnd::Succeed;
                        }
                    }
                    work[dest] = temp[cursor_temp];
                    dest += 1;
                    cursor_temp += 1;
                    length_a -= 1;
                    if length_a == 1 {
                        break 'merge MergeEnd::CopyRest;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        };

        match end {
            MergeEnd::Succeed => {
                if length_a > 0 {
                    work[dest..dest + length_a]
                        .copy_from_slice(&temp[cursor_temp..cursor_temp + length_a]);
                }
            }
            MergeEnd::CopyRest => {
                work.copy_within(cursor_b..cursor_b + length_b, dest);
                work[dest + length_b] = temp[cursor_temp];
            }
        }
    }

    // Cursors run backwards and can step one below index 0, as in V8, so they are signed.
    fn merge_high(&mut self, base_a: usize, length_a: usize, base_b: usize, length_b: usize) {
        let mut length_a = length_a as isize;
        let mut length_b = length_b as isize;
        self.temp.clear();
        self.temp
            .extend_from_slice(&self.work[base_b..base_b + length_b as usize]);
        let work = &mut self.work;
        let temp = &self.temp;
        let less = &mut self.less;
        let at = |i: isize| i as usize;

        let base_a = base_a as isize;
        let mut dest = base_b as isize + length_b - 1;
        let mut cursor_temp = length_b - 1;
        let mut cursor_a = base_a + length_a - 1;

        work[at(dest)] = work[at(cursor_a)];
        dest -= 1;
        cursor_a -= 1;

        let end = 'merge: {
            length_a -= 1;
            if length_a == 0 {
                break 'merge MergeEnd::Succeed;
            }
            if length_b == 1 {
                break 'merge MergeEnd::CopyRest;
            }
            let mut min_gallop = self.min_gallop as isize;
            loop {
                let mut wins_a: isize = 0;
                let mut wins_b: isize = 0;
                loop {
                    if less(temp[at(cursor_temp)], work[at(cursor_a)]) {
                        work[at(dest)] = work[at(cursor_a)];
                        dest -= 1;
                        cursor_a -= 1;
                        wins_a += 1;
                        length_a -= 1;
                        wins_b = 0;
                        if length_a == 0 {
                            break 'merge MergeEnd::Succeed;
                        }
                        if wins_a >= min_gallop {
                            break;
                        }
                    } else {
                        work[at(dest)] = temp[at(cursor_temp)];
                        dest -= 1;
                        cursor_temp -= 1;
                        wins_b += 1;
                        length_b -= 1;
                        wins_a = 0;
                        if length_b == 1 {
                            break 'merge MergeEnd::CopyRest;
                        }
                        if wins_b >= min_gallop {
                            break;
                        }
                    }
                }

                min_gallop += 1;
                let mut first = true;
                while wins_a >= MIN_GALLOP as isize || wins_b >= MIN_GALLOP as isize || first {
                    first = false;
                    min_gallop = (min_gallop - 1).max(1);
                    self.min_gallop = min_gallop as usize;

                    let k = gallop_right(
                        less,
                        work,
                        temp[at(cursor_temp)],
                        at(base_a),
                        at(length_a),
                        at(length_a - 1),
                    ) as isize;
                    wins_a = length_a - k;
                    if wins_a > 0 {
                        dest -= wins_a;
                        cursor_a -= wins_a;
                        work.copy_within(at(cursor_a + 1)..at(cursor_a + 1 + wins_a), at(dest + 1));
                        length_a -= wins_a;
                        if length_a == 0 {
                            break 'merge MergeEnd::Succeed;
                        }
                    }
                    work[at(dest)] = temp[at(cursor_temp)];
                    dest -= 1;
                    cursor_temp -= 1;
                    length_b -= 1;
                    if length_b == 1 {
                        break 'merge MergeEnd::CopyRest;
                    }

                    let k = gallop_left(
                        less,
                        temp,
                        work[at(cursor_a)],
                        0,
                        at(length_b),
                        at(length_b - 1),
                    ) as isize;
                    wins_b = length_b - k;
                    if wins_b > 0 {
                        dest -= wins_b;
                        cursor_temp -= wins_b;
                        work[at(dest + 1)..at(dest + 1 + wins_b)].copy_from_slice(
                            &temp[at(cursor_temp + 1)..at(cursor_temp + 1 + wins_b)],
                        );
                        length_b -= wins_b;
                        if length_b == 1 {
                            break 'merge MergeEnd::CopyRest;
                        }
                        if length_b == 0 {
                            break 'merge MergeEnd::Succeed;
                        }
                    }
                    work[at(dest)] = work[at(cursor_a)];
                    dest -= 1;
                    cursor_a -= 1;
                    length_a -= 1;
                    if length_a == 0 {
                        break 'merge MergeEnd::Succeed;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop as usize;
            }
        };

        match end {
            MergeEnd::Succeed => {
                if length_b > 0 {
                    let to = dest - (length_b - 1);
                    work[at(to)..at(to + length_b)].copy_from_slice(&temp[..at(length_b)]);
                }
            }
            MergeEnd::CopyRest => {
                dest -= length_a;
                cursor_a -= length_a;
                work.copy_within(at(cursor_a + 1)..at(cursor_a + 1 + length_a), at(dest + 1));
                work[at(dest)] = temp[at(cursor_temp)];
            }
        }
    }
}

fn compute_min_run_length(n: usize) -> usize {
    let mut n = n;
    let mut r = 0;
    while n >= 64 {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}

/// The offset in `0..=length` where `key` belongs in the sorted `array[base..base + length]`,
/// left of any equal elements. Calls `less(element, key)`.
fn gallop_left(
    less: &mut impl FnMut(usize, usize) -> bool,
    array: &[usize],
    key: usize,
    base: usize,
    length: usize,
    hint: usize,
) -> usize {
    let (length, hint) = (length as isize, hint as isize);
    let at = |i: isize| array[base + i as usize];
    let mut last_ofs: isize = 0;
    let mut offset: isize = 1;

    if less(at(hint), key) {
        let max_ofs = length - hint;
        while offset < max_ofs {
            if !less(at(hint + offset), key) {
                break;
            }
            last_ofs = offset;
            offset = (offset << 1) + 1;
        }
        offset = offset.min(max_ofs);
        last_ofs += hint;
        offset += hint;
    } else {
        let max_ofs = hint + 1;
        while offset < max_ofs {
            if less(at(hint - offset), key) {
                break;
            }
            last_ofs = offset;
            offset = (offset << 1) + 1;
        }
        offset = offset.min(max_ofs);
        let tmp = last_ofs;
        last_ofs = hint - offset;
        offset = hint - tmp;
    }

    last_ofs += 1;
    while last_ofs < offset {
        let m = last_ofs + ((offset - last_ofs) >> 1);
        if less(at(m), key) {
            last_ofs = m + 1;
        } else {
            offset = m;
        }
    }
    offset as usize
}

/// Like `gallop_left`, but right of any equal elements. Calls `less(key, element)`.
fn gallop_right(
    less: &mut impl FnMut(usize, usize) -> bool,
    array: &[usize],
    key: usize,
    base: usize,
    length: usize,
    hint: usize,
) -> usize {
    let (length, hint) = (length as isize, hint as isize);
    let at = |i: isize| array[base + i as usize];
    let mut last_ofs: isize = 0;
    let mut offset: isize = 1;

    if less(key, at(hint)) {
        let max_ofs = hint + 1;
        while offset < max_ofs {
            if !less(key, at(hint - offset)) {
                break;
            }
            last_ofs = offset;
            offset = (offset << 1) + 1;
        }
        offset = offset.min(max_ofs);
        let tmp = last_ofs;
        last_ofs = hint - offset;
        offset = hint - tmp;
    } else {
        let max_ofs = length - hint;
        while offset < max_ofs {
            if less(key, at(hint + offset)) {
                break;
            }
            last_ofs = offset;
            offset = (offset << 1) + 1;
        }
        offset = offset.min(max_ofs);
        last_ofs += hint;
        offset += hint;
    }

    last_ofs += 1;
    while last_ofs < offset {
        let m = last_ofs + ((offset - last_ofs) >> 1);
        if less(key, at(m)) {
            offset = m;
        } else {
            last_ofs = m + 1;
        }
    }
    offset as usize
}
