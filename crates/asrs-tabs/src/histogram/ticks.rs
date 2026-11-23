// Ported from Chart.js 4.4.0 (MIT, Copyright (c) 2014-2022 Chart.js Contributors). See THIRD_PARTY_NOTICES.md.

//! `generateTicks` of the linear scale, `autoSkip` and the numeric tick formatter.

use asrs_js::math::{self, max2, min2};
use asrs_js::number::{self, IntlOptions};

#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    pub value: f64,
    pub label: String,
}

pub fn limit(value: f64, min: f64, max: f64) -> f64 {
    max2(min, min2(max, value))
}

pub fn almost_equals(x: f64, y: f64, epsilon: f64) -> bool {
    math::abs(x - y) < epsilon
}

fn nice_num(range: f64) -> f64 {
    let rounded = math::round(range);
    let range = if almost_equals(range, rounded, range / 1000.0) {
        rounded
    } else {
        range
    };
    let nice_range = math::pow(10.0, math::floor(math::log10(range)));
    let fraction = range / nice_range;
    let nice = if fraction <= 1.0 {
        1.0
    } else if fraction <= 2.0 {
        2.0
    } else if fraction <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * nice_range
}

/// `_decimalPlaces`, with NaN for the `undefined` of a non-finite number, which `Math.max` turns
/// into NaN. Upstream loops forever once `e` overflows; the port stops there.
fn decimal_places(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let mut e = 1.0;
    let mut p = 0.0;
    while math::round(x * e) / e != x && f64::is_finite(e) {
        e *= 10.0;
        p += 1.0;
    }
    p
}

/// `generateTicks` with `bounds: "ticks"` and no `min`, `max`, `count` or `precision`.
pub fn generate(step: f64, max_ticks: f64, rmin: f64, rmax: f64) -> Vec<f64> {
    const MIN_SPACING: f64 = 1e-14;
    let unit = if step == 0.0 || step.is_nan() {
        1.0
    } else {
        step
    };
    let max_spaces = max_ticks - 1.0;
    let mut spacing = nice_num((rmax - rmin) / max_spaces / unit) * unit;
    if spacing < MIN_SPACING {
        return vec![rmin, rmax];
    }
    let mut num_spaces = math::ceil(rmax / spacing) - math::floor(rmin / spacing);
    if num_spaces > max_spaces {
        spacing = nice_num(num_spaces * spacing / max_spaces / unit) * unit;
    }
    let mut nice_min = math::floor(rmin / spacing) * spacing;
    let mut nice_max = math::ceil(rmax / spacing) * spacing;
    num_spaces = (nice_max - nice_min) / spacing;
    num_spaces = if almost_equals(num_spaces, math::round(num_spaces), spacing / 1000.0) {
        math::round(num_spaces)
    } else {
        math::ceil(num_spaces)
    };
    let places = max2(decimal_places(spacing), decimal_places(nice_min));
    let factor = math::pow(10.0, places);
    nice_min = math::round(nice_min * factor) / factor;
    nice_max = math::round(nice_max * factor) / factor;
    let mut ticks = Vec::new();
    let mut j = 0.0;
    while j < num_spaces {
        ticks.push(math::round((nice_min + j * spacing) * factor) / factor);
        j += 1.0;
    }
    ticks.push(nice_max);
    ticks
}

fn delta(value: f64, ticks: &[f64]) -> f64 {
    let delta = if ticks.len() > 3 {
        ticks[2] - ticks[1]
    } else {
        ticks[1] - ticks[0]
    };
    if math::abs(delta) >= 1.0 && value != math::floor(value) {
        value - math::floor(value)
    } else {
        delta
    }
}

pub fn format(value: f64, ticks: &[f64]) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let mut scientific = false;
    let mut d = value;
    if ticks.len() > 1 {
        let max_tick = max2(math::abs(ticks[0]), math::abs(ticks[ticks.len() - 1]));
        scientific = max_tick < 1e-4 || max_tick > 1e15;
        d = delta(value, ticks);
    }
    let log = math::log10(math::abs(d));
    let digits = if log.is_nan() {
        1.0
    } else {
        max2(min2(-math::floor(log), 20.0), 0.0)
    } as u32;
    if scientific {
        return number::intl_format_scientific(value, digits);
    }
    number::intl_format(
        value,
        IntlOptions {
            min_fraction: digits,
            max_fraction: digits,
            ..IntlOptions::default()
        },
    )
}

/// `autoSkip` without major ticks.
pub fn auto_skip(ticks: &[Tick], limit: f64) -> Vec<Tick> {
    if limit < 0.0 {
        return Vec::new();
    }
    let spacing = math::ceil(max2(ticks.len() as f64 / limit, 1.0));
    let mut kept = Vec::new();
    let mut count = 0.0;
    let mut next = 0.0;
    for (i, tick) in ticks.iter().enumerate() {
        if i as f64 == next {
            kept.push(tick.clone());
            count += 1.0;
            next = math::round(count * spacing);
        }
    }
    kept
}
