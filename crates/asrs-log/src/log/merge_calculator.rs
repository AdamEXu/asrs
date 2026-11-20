use asrs_js::math;
use asrs_js::sort::array_sort;

use super::log_core::Log;
use super::robot_state::{
    RobotMode, get_autonomous_data, get_robot_state_ranges, get_utility_data,
};

#[derive(Clone, Copy)]
struct NormalizedStateRange {
    start: f64,
    end: f64,
    mode: RobotMode,
}

#[derive(Clone, Copy)]
struct StateTransition {
    timestamp: f64,
    from_mode: RobotMode,
    to_mode: RobotMode,
}

#[derive(Clone, Copy)]
struct CandidateOffset {
    offset: f64,
    prefer_rising: bool,
}

#[derive(Clone, Copy)]
struct ResidualSample {
    offset: f64,
    is_rising: bool,
}

const START_ENABLED_GAP_THRESHOLD: f64 = 5.0;
const SCORE_EPSILON: f64 = 1e-6;
const INLIER_TOLERANCE: f64 = 1.0;
const TWO_EDGE_MAX_SPAN: f64 = 0.25;
const CLUSTER_SPAN_LIMIT: f64 = 0.1;
const MIN_INLIER_RADIUS: f64 = 0.08;
const MAX_INLIER_RADIUS: f64 = 0.2;
const INLIER_SPAN_MULTIPLIER: f64 = 2.5;

const WEIGHT_MODE_MATCH: f64 = 1.0;
const WEIGHT_MODE_MISMATCH: f64 = 0.25;
const PENALTY_STATE_MISMATCH: f64 = 0.25;
const PENALTY_OUTSIDE_WINDOW: f64 = 0.1;

/// `getMergeOffset`: the best-fit timestamp offset aligning `source` into `target`.
pub fn get_merge_offset(target: &Log, source: &Log) -> f64 {
    let target_range = target.get_timestamp_range();
    let source_range = source.get_timestamp_range();
    let fallback_offset = target_range[0] - source_range[0];

    let target_ranges = get_normalized_ranges(target, target_range);
    let source_ranges = get_normalized_ranges(source, source_range);
    if target_ranges.is_empty() || source_ranges.is_empty() {
        return fallback_offset;
    }

    let both_have_mode_info = has_mode_info(target) && has_mode_info(source);
    let target_transitions = get_transitions(&target_ranges);
    let source_transitions = get_transitions(&source_ranges);

    let Some(coarse_offset) = find_coarse_offset(
        target_range,
        source_range,
        &target_ranges,
        &source_ranges,
        &target_transitions,
        &source_transitions,
        both_have_mode_info,
    ) else {
        return fallback_offset;
    };

    refine_offset_with_transitions(
        coarse_offset,
        &target_transitions,
        &source_transitions,
        both_have_mode_info,
    )
}

/// `hasModeInfo`: whether the log holds autonomous or utility data as well as enabled.
fn has_mode_info(log: &Log) -> bool {
    get_autonomous_data(log).is_some() || get_utility_data(log).is_some()
}

/// `getNormalizedRanges`: the robot state ranges as a contiguous partition of
/// `[log_range[0], log_range[1]]`, or empty when the log is never enabled.
fn get_normalized_ranges(log: &Log, log_range: [f64; 2]) -> Vec<NormalizedStateRange> {
    let raw_ranges = get_robot_state_ranges(log);
    if !raw_ranges.iter().any(|r| r.mode != RobotMode::Disabled) {
        return Vec::new();
    }

    let mut ranges: Vec<NormalizedStateRange> = raw_ranges
        .iter()
        .map(|r| NormalizedStateRange {
            start: r.start,
            end: r.end.unwrap_or_else(|| math::max2(r.start, log_range[1])),
            mode: r.mode,
        })
        .collect();

    if ranges[0].start > log_range[0] {
        if ranges[0].mode == RobotMode::Disabled {
            ranges[0].start = log_range[0];
        } else if ranges[0].start - log_range[0] > START_ENABLED_GAP_THRESHOLD {
            ranges.insert(
                0,
                NormalizedStateRange {
                    start: log_range[0],
                    end: ranges[0].start,
                    mode: RobotMode::Disabled,
                },
            );
        } else {
            ranges[0].start = log_range[0];
        }
    }

    let last = ranges.last_mut().expect("ranges is nonempty");
    if last.end < log_range[1] {
        last.end = log_range[1];
    }

    ranges
}

/// `getTransitions`: mode changes at the internal boundaries of the normalized ranges.
fn get_transitions(ranges: &[NormalizedStateRange]) -> Vec<StateTransition> {
    let mut transitions = Vec::new();
    for i in 0..ranges.len().saturating_sub(1) {
        let from_mode = ranges[i].mode;
        let to_mode = ranges[i + 1].mode;
        if from_mode != to_mode {
            transitions.push(StateTransition {
                timestamp: ranges[i].end,
                from_mode,
                to_mode,
            });
        }
    }
    transitions
}

/// `transitionsCompatible`: matching directions and modes, given whether both logs
/// carry mode information.
fn transitions_compatible(
    a: &StateTransition,
    b: &StateTransition,
    both_have_mode_info: bool,
) -> bool {
    let both_rising = a.from_mode == RobotMode::Disabled && b.from_mode == RobotMode::Disabled;
    let both_falling = a.to_mode == RobotMode::Disabled && b.to_mode == RobotMode::Disabled;
    if both_rising || both_falling {
        return true;
    }

    if a.from_mode != RobotMode::Disabled
        && a.to_mode != RobotMode::Disabled
        && b.from_mode != RobotMode::Disabled
        && b.to_mode != RobotMode::Disabled
    {
        return !both_have_mode_info
            || (a.from_mode == b.from_mode && a.to_mode == b.to_mode);
    }
    false
}

/// `findCoarseOffset`: the candidate with the highest interval cross-correlation
/// score, or `None` when no candidates exist or enabled overlap is zero.
#[expect(clippy::too_many_arguments)]
fn find_coarse_offset(
    target_range: [f64; 2],
    source_range: [f64; 2],
    target_ranges: &[NormalizedStateRange],
    source_ranges: &[NormalizedStateRange],
    target_transitions: &[StateTransition],
    source_transitions: &[StateTransition],
    both_have_mode_info: bool,
) -> Option<f64> {
    let mut candidates: Vec<CandidateOffset> = Vec::new();

    for t_trans in target_transitions {
        for s_trans in source_transitions {
            if transitions_compatible(t_trans, s_trans, both_have_mode_info) {
                candidates.push(CandidateOffset {
                    offset: t_trans.timestamp - s_trans.timestamp,
                    prefer_rising: t_trans.from_mode == RobotMode::Disabled
                        && s_trans.from_mode == RobotMode::Disabled,
                });
            }
        }
    }

    for t_range in target_ranges {
        if t_range.mode == RobotMode::Disabled {
            continue;
        }
        for s_range in source_ranges {
            if s_range.mode == RobotMode::Disabled {
                continue;
            }
            candidates.push(CandidateOffset {
                offset: t_range.start - s_range.start,
                prefer_rising: false,
            });
            candidates.push(CandidateOffset {
                offset: t_range.end - s_range.end,
                prefer_rising: false,
            });
        }
    }

    if candidates.is_empty() {
        return None;
    }

    let total_target_enabled = get_total_enabled_duration(target_ranges);
    let total_source_enabled = get_total_enabled_duration(source_ranges);

    let mut best_offset = candidates[0].offset;
    let mut best_score = f64::NEG_INFINITY;
    let mut best_enabled_overlap = 0.0;
    let mut best_prefer_rising = false;

    for candidate in &candidates {
        let (score, enabled_overlap) = score_candidate_offset(
            candidate.offset,
            target_range,
            source_range,
            target_ranges,
            source_ranges,
            total_target_enabled,
            total_source_enabled,
            both_have_mode_info,
        );
        let is_better_score = score > best_score + SCORE_EPSILON;
        let is_tie_breaking_rising = math::abs(score - best_score) <= SCORE_EPSILON
            && candidate.prefer_rising
            && !best_prefer_rising;

        if is_better_score || is_tie_breaking_rising {
            best_score = score;
            best_offset = candidate.offset;
            best_enabled_overlap = enabled_overlap;
            best_prefer_rising = candidate.prefer_rising;
        }
    }

    if best_enabled_overlap <= 0.0 && total_target_enabled > 0.0 && total_source_enabled > 0.0 {
        return None;
    }

    Some(best_offset)
}

/// `getTotalEnabledDuration`: the summed duration of all non-disabled ranges.
fn get_total_enabled_duration(ranges: &[NormalizedStateRange]) -> f64 {
    ranges.iter().fold(0.0, |sum, r| {
        sum + if r.mode != RobotMode::Disabled {
            r.end - r.start
        } else {
            0.0
        }
    })
}

/// `scoreCandidateOffset`: sweeps the overlapping window of both logs at `offset`.
#[expect(clippy::too_many_arguments)]
fn score_candidate_offset(
    offset: f64,
    target_range: [f64; 2],
    source_range: [f64; 2],
    target_ranges: &[NormalizedStateRange],
    source_ranges: &[NormalizedStateRange],
    total_target_enabled: f64,
    total_source_enabled: f64,
    both_have_mode_info: bool,
) -> (f64, f64) {
    let window_start = math::max2(target_range[0], source_range[0] + offset);
    let window_end = math::min2(target_range[1], source_range[1] + offset);
    if window_end < window_start {
        return (f64::NEG_INFINITY, 0.0);
    }

    let mut enabled_overlap = 0.0;
    let mut mode_matched_overlap = 0.0;
    let mut mode_mismatched_overlap = 0.0;
    let mut mismatch_duration = 0.0;

    let mut i = 0;
    let mut j = 0;
    while i < target_ranges.len() && j < source_ranges.len() {
        let t_range = &target_ranges[i];
        let s_range = &source_ranges[j];
        let s_start = s_range.start + offset;
        let s_end = s_range.end + offset;

        let overlap_start = math::max(&[window_start, t_range.start, s_start]);
        let overlap_end = math::min(&[window_end, t_range.end, s_end]);
        if overlap_end > overlap_start {
            let duration = overlap_end - overlap_start;
            let t_enabled = t_range.mode != RobotMode::Disabled;
            let s_enabled = s_range.mode != RobotMode::Disabled;
            if t_enabled && s_enabled {
                enabled_overlap += duration;
                if !both_have_mode_info || t_range.mode == s_range.mode {
                    mode_matched_overlap += duration;
                } else {
                    mode_mismatched_overlap += duration;
                }
            } else if t_enabled != s_enabled {
                mismatch_duration += duration;
            }
        }

        if t_range.end < s_end {
            i += 1;
        } else {
            j += 1;
        }
    }

    let outside_enabled = math::max2(
        0.0,
        total_target_enabled + total_source_enabled - 2.0 * enabled_overlap - mismatch_duration,
    );
    let score = mode_matched_overlap * WEIGHT_MODE_MATCH
        + mode_mismatched_overlap * WEIGHT_MODE_MISMATCH
        - mismatch_duration * PENALTY_STATE_MISMATCH
        - outside_enabled * PENALTY_OUTSIDE_WINDOW;

    (score, enabled_overlap)
}

/// `refineOffsetWithTransitions`: consensus clustering over inlier transitions,
/// falling back to the coarse offset when nothing matches.
fn refine_offset_with_transitions(
    coarse_offset: f64,
    target_transitions: &[StateTransition],
    source_transitions: &[StateTransition],
    both_have_mode_info: bool,
) -> f64 {
    let mut residuals =
        match_inlier_transitions(coarse_offset, target_transitions, source_transitions, both_have_mode_info);

    if residuals.is_empty() {
        return coarse_offset;
    }
    if residuals.len() == 1 {
        return residuals[0].offset;
    }

    array_sort(&mut residuals, |a, b| a.offset - b.offset);

    if residuals.len() == 2 {
        let span = residuals[1].offset - residuals[0].offset;
        if span <= TWO_EDGE_MAX_SPAN || residuals[0].is_rising == residuals[1].is_rising {
            return (residuals[0].offset + residuals[1].offset) / 2.0;
        }
        return if residuals[0].is_rising {
            residuals[0].offset
        } else {
            residuals[1].offset
        };
    }

    let (cluster, span) = find_core_cluster(&residuals);
    let core_mean = mean_offset(&cluster);
    let inlier_radius = math::max2(
        MIN_INLIER_RADIUS,
        math::min2(MAX_INLIER_RADIUS, span * INLIER_SPAN_MULTIPLIER),
    );
    let inliers: Vec<ResidualSample> = residuals
        .iter()
        .copied()
        .filter(|item| math::abs(item.offset - core_mean) <= inlier_radius)
        .collect();
    if inliers.is_empty() {
        core_mean
    } else {
        mean_offset(&inliers)
    }
}

/// `matchInlierTransitions`: greedy 1-to-1 pairing of compatible transitions
/// within `INLIER_TOLERANCE` of `coarse_offset`.
fn match_inlier_transitions(
    coarse_offset: f64,
    target_transitions: &[StateTransition],
    source_transitions: &[StateTransition],
    both_have_mode_info: bool,
) -> Vec<ResidualSample> {
    struct Pair {
        t_index: usize,
        s_index: usize,
        offset: f64,
        error: f64,
        exact_mode_match: bool,
        is_rising: bool,
    }

    let mut candidate_pairs: Vec<Pair> = Vec::new();
    for (t_index, t_trans) in target_transitions.iter().enumerate() {
        for (s_index, s_trans) in source_transitions.iter().enumerate() {
            if !transitions_compatible(t_trans, s_trans, both_have_mode_info) {
                continue;
            }
            let offset = t_trans.timestamp - s_trans.timestamp;
            let error = math::abs(offset - coarse_offset);
            if error <= INLIER_TOLERANCE {
                candidate_pairs.push(Pair {
                    t_index,
                    s_index,
                    offset,
                    error,
                    exact_mode_match: !both_have_mode_info
                        || (t_trans.from_mode == s_trans.from_mode
                            && t_trans.to_mode == s_trans.to_mode),
                    is_rising: t_trans.from_mode == RobotMode::Disabled
                        && s_trans.from_mode == RobotMode::Disabled,
                });
            }
        }
    }

    array_sort(&mut candidate_pairs, |a, b| {
        if a.exact_mode_match != b.exact_mode_match {
            if a.exact_mode_match { -1.0 } else { 1.0 }
        } else {
            a.error - b.error
        }
    });

    let mut used_target = std::collections::HashSet::new();
    let mut used_source = std::collections::HashSet::new();
    let mut matched = Vec::new();

    for pair in &candidate_pairs {
        if !used_target.contains(&pair.t_index) && !used_source.contains(&pair.s_index) {
            used_target.insert(pair.t_index);
            used_source.insert(pair.s_index);
            matched.push(ResidualSample {
                offset: pair.offset,
                is_rising: pair.is_rising,
            });
        }
    }

    matched
}

/// `findCoreCluster`: the largest offset cluster within `CLUSTER_SPAN_LIMIT`,
/// or the shortest-half window when no tight cluster exists.
fn find_core_cluster(residuals: &[ResidualSample]) -> (Vec<ResidualSample>, f64) {
    let mut cluster: Vec<ResidualSample> = Vec::new();
    let mut best_span = f64::INFINITY;

    for i in 0..residuals.len() {
        for j in i..residuals.len() {
            let span = residuals[j].offset - residuals[i].offset;
            if span > CLUSTER_SPAN_LIMIT {
                break;
            }
            let count = j - i + 1;
            if count > cluster.len() || (count == cluster.len() && span < best_span) {
                cluster = residuals[i..=j].to_vec();
                best_span = span;
            }
        }
    }

    if cluster.len() >= 2 {
        return (cluster, best_span);
    }

    let half_size = residuals.len() / 2 + 1;
    let mut best_span = f64::INFINITY;
    for i in 0..=residuals.len() - half_size {
        let span = residuals[i + half_size - 1].offset - residuals[i].offset;
        if span < best_span {
            best_span = span;
            cluster = residuals[i..i + half_size].to_vec();
        }
    }

    (cluster, best_span)
}

/// `meanOffset`: the arithmetic mean of the residual offsets, summed in order.
fn mean_offset(samples: &[ResidualSample]) -> f64 {
    samples.iter().fold(0.0, |sum, item| sum + item.offset) / samples.len() as f64
}
