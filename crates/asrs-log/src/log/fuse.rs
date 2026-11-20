//! Fuse.js 7.0.0 string-list search (`Fuse#search` over `Array<string>` with the Bitap
//! searcher), as `searchFields` configures it. Extended and logical search, keyed object lists
//! and match indices are not used upstream and are not ported.

use asrs_js::math;
use asrs_js::sort::array_sort;
use asrs_js::string::JsString;

const MAX_BITS: usize = 32;

#[derive(Clone, Debug)]
pub struct FuseOptions {
    pub is_case_sensitive: bool,
    pub find_all_matches: bool,
    pub location: f64,
    pub threshold: f64,
    pub distance: f64,
    pub ignore_location: bool,
    pub ignore_field_norm: bool,
    pub field_norm_weight: f64,
}

impl Default for FuseOptions {
    fn default() -> Self {
        FuseOptions {
            is_case_sensitive: false,
            find_all_matches: false,
            location: 0.0,
            threshold: 0.6,
            distance: 100.0,
            ignore_location: false,
            ignore_field_norm: false,
            field_norm_weight: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FuseResult {
    pub ref_index: usize,
    pub score: f64,
}

struct Record {
    text: JsString,
    index: usize,
    norm: f64,
}

/// A `FuseIndex` built by `setCollection` over a string list.
pub struct Fuse {
    options: FuseOptions,
    records: Vec<Record>,
}

impl Fuse {
    pub fn new(docs: &[JsString], options: FuseOptions) -> Self {
        let mut norm = Norm::new(options.field_norm_weight);
        let records = docs
            .iter()
            .enumerate()
            .filter(|(_, doc)| !doc.trim().is_empty())
            .map(|(index, doc)| Record {
                text: doc.clone(),
                index,
                norm: norm.get(doc),
            })
            .collect();
        Fuse { options, records }
    }

    /// `search(query)` with `shouldSort` and the default `sortFn`: matches by ascending score,
    /// then ascending collection index.
    pub fn search(&self, query: &JsString) -> Vec<FuseResult> {
        let searcher = BitapSearch::new(query, &self.options);
        let mut results: Vec<FuseResult> = self
            .records
            .iter()
            .filter_map(|record| {
                let (is_match, score) = searcher.search_in(&record.text);
                is_match.then(|| FuseResult {
                    ref_index: record.index,
                    score: self.practical_score(score, record.norm),
                })
            })
            .collect();
        array_sort(&mut results, |a, b| {
            if a.score == b.score {
                if a.ref_index < b.ref_index { -1 } else { 1 }
            } else if a.score < b.score {
                -1
            } else {
                1
            }
        });
        results
    }

    fn practical_score(&self, score: f64, norm: f64) -> f64 {
        let exponent = if self.options.ignore_field_norm {
            1.0
        } else {
            norm
        };
        math::pow(score, exponent)
    }
}

struct Norm {
    weight: f64,
    m: f64,
    cache: Vec<(usize, f64)>,
}

impl Norm {
    fn new(weight: f64) -> Self {
        Norm {
            weight,
            m: math::pow(10.0, 3.0),
            cache: Vec::new(),
        }
    }

    fn get(&mut self, value: &JsString) -> f64 {
        let num_tokens = count_space_tokens(value.units());
        if let Some(&(_, n)) = self.cache.iter().find(|(k, _)| *k == num_tokens) {
            return n;
        }
        let norm = 1.0 / math::pow(num_tokens as f64, 0.5 * self.weight);
        let n = math::round(norm * self.m) / self.m;
        self.cache.push((num_tokens, n));
        n
    }
}

/// `value.match(/[^ ]+/g).length`.
fn count_space_tokens(units: &[u16]) -> usize {
    let mut count = 0;
    let mut in_token = false;
    for &unit in units {
        let space = unit == u16::from(b' ');
        if !space && !in_token {
            count += 1;
        }
        in_token = !space;
    }
    count
}

struct Chunk {
    pattern: JsString,
    ascii: [i32; 128],
    other: Vec<(u16, i32)>,
    start_index: usize,
}

impl Chunk {
    fn new(pattern: JsString, start_index: usize) -> Self {
        let units = pattern.units();
        let len = units.len();
        let mut ascii = [0i32; 128];
        let mut other: Vec<(u16, i32)> = Vec::new();
        for (i, &unit) in units.iter().enumerate() {
            let bit = 1i32.wrapping_shl((len - i - 1) as u32);
            if unit < 0x80 {
                ascii[usize::from(unit)] |= bit;
            } else {
                match other.iter_mut().find(|(u, _)| *u == unit) {
                    Some((_, mask)) => *mask |= bit,
                    None => other.push((unit, bit)),
                }
            }
        }
        Chunk {
            pattern,
            ascii,
            other,
            start_index,
        }
    }

    fn char_match(&self, unit: Option<u16>) -> i32 {
        match unit {
            Some(u) if u < 0x80 => self.ascii[usize::from(u)],
            Some(u) => self
                .other
                .iter()
                .find(|(c, _)| *c == u)
                .map_or(0, |&(_, mask)| mask),
            None => 0,
        }
    }
}

struct BitapSearch<'a> {
    options: &'a FuseOptions,
    pattern: JsString,
    chunks: Vec<Chunk>,
}

impl<'a> BitapSearch<'a> {
    fn new(pattern: &JsString, options: &'a FuseOptions) -> Self {
        let pattern = if options.is_case_sensitive {
            pattern.clone()
        } else {
            lower_case(pattern)
        };
        let mut chunks = Vec::new();
        let units = pattern.units();
        let len = units.len();
        let sub = |start: usize, count: usize| {
            JsString::from_units(units[start..(start + count).min(len)].to_vec())
        };
        if len > MAX_BITS {
            let remainder = len % MAX_BITS;
            let end = len - remainder;
            let mut i = 0;
            while i < end {
                chunks.push(Chunk::new(sub(i, MAX_BITS), i));
                i += MAX_BITS;
            }
            if remainder != 0 {
                let start_index = len - MAX_BITS;
                chunks.push(Chunk::new(sub(start_index, len - start_index), start_index));
            }
        } else if len > 0 {
            chunks.push(Chunk::new(pattern.clone(), 0));
        }
        BitapSearch {
            options,
            pattern,
            chunks,
        }
    }

    fn search_in(&self, text: &JsString) -> (bool, f64) {
        let lowered;
        let text = if self.options.is_case_sensitive {
            text
        } else {
            lowered = lower_case(text);
            &lowered
        };
        if self.pattern == *text {
            return (true, 0.0);
        }
        let mut total_score = 0.0;
        let mut has_matches = false;
        for chunk in &self.chunks {
            let (is_match, score) = bitap(
                text,
                chunk,
                self.options.location + chunk.start_index as f64,
                self.options,
            );
            if is_match {
                has_matches = true;
            }
            total_score += score;
        }
        let score = if has_matches {
            total_score / self.chunks.len() as f64
        } else {
            1.0
        };
        (has_matches, score)
    }
}

fn lower_case(text: &JsString) -> JsString {
    let units = text.units();
    if units.iter().all(|&u| u < 0x80) {
        JsString::from_units(
            units
                .iter()
                .map(|&u| {
                    if (0x41..=0x5a).contains(&u) {
                        u + 0x20
                    } else {
                        u
                    }
                })
                .collect(),
        )
    } else {
        text.to_lower_case()
    }
}

fn compute_score(
    pattern_len: usize,
    errors: f64,
    current_location: f64,
    expected_location: f64,
    options: &FuseOptions,
) -> f64 {
    let accuracy = errors / pattern_len as f64;
    if options.ignore_location {
        return accuracy;
    }
    let proximity = (expected_location - current_location).abs();
    if options.distance == 0.0 {
        return if proximity != 0.0 { 1.0 } else { accuracy };
    }
    accuracy + proximity / options.distance
}

/// Fuse's `search(text, pattern, patternAlphabet, options)`: `[isMatch, score]`.
fn bitap(text: &JsString, chunk: &Chunk, location: f64, options: &FuseOptions) -> (bool, f64) {
    let pattern_len = chunk.pattern.len();
    let text_units = text.units();
    let text_len = text_units.len() as f64;
    let expected_location = math::max2(0.0, math::min2(location, text_len));
    let mut current_threshold = options.threshold;
    let mut best_location = expected_location;
    let score_at = |errors: f64, current: f64| {
        compute_score(pattern_len, errors, current, expected_location, options)
    };

    loop {
        let index = text.index_of(&chunk.pattern, Some(best_location));
        if index <= -1 {
            break;
        }
        let score = score_at(0.0, index as f64);
        current_threshold = math::min2(score, current_threshold);
        best_location = index as f64 + pattern_len as f64;
    }

    best_location = -1.0;
    let mut last_bit_arr: Vec<i32> = Vec::new();
    let mut final_score = 1.0;
    let mut bin_max = pattern_len as f64 + text_len;
    let mask = 1i32.wrapping_shl((pattern_len - 1) as u32);

    for i in 0..pattern_len {
        let errors = i as f64;
        let mut bin_min = 0.0;
        let mut bin_mid = bin_max;
        while bin_min < bin_mid {
            if score_at(errors, expected_location + bin_mid) <= current_threshold {
                bin_min = bin_mid;
            } else {
                bin_max = bin_mid;
            }
            bin_mid = math::floor((bin_max - bin_min) / 2.0 + bin_min);
        }
        bin_max = bin_mid;

        let mut start = math::max2(1.0, expected_location - bin_mid + 1.0);
        let finish = if options.find_all_matches {
            text_len
        } else {
            math::min2(expected_location + bin_mid, text_len) + pattern_len as f64
        };

        let size = finish as usize + 2;
        let mut bit_arr = vec![0i32; size];
        bit_arr[size - 1] = 1i32.wrapping_shl(i as u32).wrapping_sub(1);
        let last = |k: usize| last_bit_arr.get(k).copied().unwrap_or(0);

        let mut j = finish;
        while j >= start {
            let ju = j as usize;
            let current_location = j - 1.0;
            let char_match = chunk.char_match(text_units.get(ju - 1).copied());
            bit_arr[ju] = (bit_arr[ju + 1].wrapping_shl(1) | 1) & char_match;
            if i != 0 {
                bit_arr[ju] |= (last(ju + 1) | last(ju)).wrapping_shl(1) | 1 | last(ju + 1);
            }
            if bit_arr[ju] & mask != 0 {
                final_score = score_at(errors, current_location);
                if final_score <= current_threshold {
                    current_threshold = final_score;
                    best_location = current_location;
                    if best_location <= expected_location {
                        break;
                    }
                    start = math::max2(1.0, 2.0 * expected_location - best_location);
                }
            }
            j -= 1.0;
        }

        if score_at(errors + 1.0, expected_location) > current_threshold {
            break;
        }
        last_bit_arr = bit_arr;
    }

    (best_location >= 0.0, math::max2(0.001, final_score))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(text: &str) -> JsString {
        JsString::from(text)
    }

    #[test]
    fn space_tokens() {
        assert_eq!(count_space_tokens(s("a").units()), 1);
        assert_eq!(count_space_tokens(s("  a  b ").units()), 2);
        assert_eq!(count_space_tokens(s("a\tb").units()), 1);
    }

    #[test]
    fn exact_match_scores_zero() {
        let docs = [s("Foo"), s("foo bar"), s("   ")];
        let fuse = Fuse::new(&docs, FuseOptions::default());
        let results = fuse.search(&s("foo"));
        assert_eq!(
            results[0],
            FuseResult {
                ref_index: 0,
                score: 0.0
            }
        );
        assert_eq!(results.len(), 2);
    }
}
