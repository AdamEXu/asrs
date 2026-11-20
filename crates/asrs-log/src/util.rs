//! `shared/util.ts`. The helpers `asrs-js` already covers (`htmlEncode`,
//! `formatDate`, `createUUID`, `formatTimeWithMS`, `jsonCopy`) are thin
//! forwards so this module keeps upstream's surface.

use std::collections::HashSet;
use std::hash::Hash;

use asrs_js::math::{self, int};
use asrs_js::number;
use asrs_js::string::{self, JsString};
use asrs_js::value::JsValue;
use asrs_js::{json, regex, time};

/// Element access `array[i]` for an arbitrary JS number index: defined only
/// for an integer index in bounds (`-0` reads index 0); anything else is
/// `undefined` and loses every comparison here.
fn element(array: &[f64], index: f64) -> Option<f64> {
    if index >= 0.0 && index.fract() == 0.0 && index < array.len() as f64 {
        array.get(index as usize).copied()
    } else {
        None
    }
}

pub fn arrays_equal<T: PartialEq>(a: &[T], b: &[T]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

/// `lowerBound` for the index hints every upstream call site passes.
pub fn lower_bound(array: &[f64], target: f64, start_index: usize) -> usize {
    lower_bound_js(array, target, start_index as f64) as usize
}

/// `upperBound` for the index hints every upstream call site passes.
pub fn upper_bound(array: &[f64], target: f64, start_index: usize) -> usize {
    upper_bound_js(array, target, start_index as f64) as usize
}

/// `lowerBound` with `startIndex` as any JS number, uncoerced: negative,
/// fractional and NaN hints all reach the binary search. A `-Infinity` hint
/// loops forever whenever `array[0] >= target`, as upstream does.
pub fn lower_bound_js(array: &[f64], target: f64, start_index: f64) -> f64 {
    let length = array.len() as f64;
    let ge = |i: f64| element(array, i).is_some_and(|v| v >= target);
    if length == 0.0 || start_index >= length || !ge(length - 1.0) {
        return length;
    }
    if start_index > 0.0 {
        if ge(start_index) {
            return start_index;
        }
        if start_index + 1.0 < length && ge(start_index + 1.0) {
            return start_index + 1.0;
        }
    }
    let mut low = start_index;
    let mut high = length - 1.0;
    while low < high {
        let mid = f64::from(int::shift_right_unsigned(low + high, 1.0));
        if ge(mid) {
            high = mid;
        } else {
            low = mid + 1.0;
        }
    }
    low
}

/// `upperBound` with `startIndex` as any JS number; see `lower_bound_js`.
pub fn upper_bound_js(array: &[f64], target: f64, start_index: f64) -> f64 {
    let length = array.len() as f64;
    let gt = |i: f64| element(array, i).is_some_and(|v| v > target);
    if length == 0.0 || start_index >= length || !gt(length - 1.0) {
        return length;
    }
    if start_index > 0.0 {
        if gt(start_index) {
            return start_index;
        }
        if start_index + 1.0 < length && gt(start_index + 1.0) {
            return start_index + 1.0;
        }
    }
    let mut low = start_index;
    let mut high = length - 1.0;
    while low < high {
        let mid = f64::from(int::shift_right_unsigned(low + high, 1.0));
        if gt(mid) {
            high = mid;
        } else {
            low = mid + 1.0;
        }
    }
    low
}

pub fn sets_equal<T: Eq + Hash>(a: &HashSet<T>, b: &HashSet<T>) -> bool {
    a == b
}

/// `checkArrayType`: `Array.isArray(value)` and `typeof item === type` on
/// every element.
pub fn check_array_type(value: &JsValue, type_name: &str) -> bool {
    match value.as_array() {
        Some(items) => items.iter().all(|item| item.type_of() == type_name),
        None => false,
    }
}

/// `jsonCopy`: `JSON.parse(JSON.stringify(value))`.
pub fn json_copy(value: &JsValue) -> Result<JsValue, json::JsonError> {
    json::json_copy(value)
}

/// `htmlEncode`.
pub fn html_encode(text: &JsString) -> JsString {
    string::html_encode(text)
}

/// `indexArray`: `Array.from({ length }, (_, i) => i)`. `None` is upstream's
/// `RangeError: Invalid array length` for a length past 2^32 - 1, which
/// geometry readers can hit through a log's `/length` field.
pub fn index_array(length: f64) -> Option<Vec<f64>> {
    let n = string::to_length(length);
    if n > 4_294_967_295.0 {
        return None;
    }
    Some((0..n as usize).map(|i| i as f64).collect())
}

/// `charIsCapital`: `input.charAt(index)` against its own uppercase. The
/// empty string (out of range) equals its uppercase, so out-of-range is true.
pub fn char_is_capital(input: &JsString, index: f64) -> bool {
    let char = input.char_at(index);
    char == char.to_upper_case()
}

pub fn calc_mock_progress(time: f64, max_percent: f64) -> f64 {
    max_percent - max_percent / (0.1 * time + 1.0)
}

/// `shiftColor`: parse `#rrggbb` pairs and shift each channel, clamped to
/// [0, 255] by plain comparisons (NaN survives). `"rgb(r,g,b)"`.
pub fn shift_color(color: &JsString, shift: f64) -> String {
    let mut channels = [0.0; 3];
    if let Some(pairs) = regex::sites::pairs_of_units(&color.slice(1.0, None)) {
        for (i, pair) in pairs.iter().enumerate().take(3) {
            channels[i] = number::parse_int(pair.units(), 16.0);
        }
        for channel in channels.iter_mut().skip(pairs.len()) {
            *channel = f64::NAN;
        }
    }
    let parts: Vec<String> = channels
        .iter()
        .map(|&x| {
            let x = x + shift;
            let x = if x < 0.0 { 0.0 } else { x };
            let x = if x > 255.0 { 255.0 } else { x };
            number::to_string(x)
        })
        .collect();
    format!("rgb({})", parts.join(","))
}

/// `zfill`: prepend `"0"` while `number.length < length`. Upstream throws
/// `RangeError: Invalid string length` past V8's 2^29 - 24 limit; every call
/// site passes a constant length (2, 3 or 8), so that path is not ported.
pub fn zfill(number: &JsString, length: f64) -> JsString {
    let mut out = number.clone();
    while (out.len() as f64) < length {
        let mut padded = JsString::from("0");
        padded.push_str(&out);
        out = padded;
    }
    out
}

/// `cleanFloat`: round to six decimals; `-0` becomes `+0`.
pub fn clean_float(float: f64) -> f64 {
    let output = math::round(float * 1e6) / 1e6;
    if output == 0.0 { 0.0 } else { output }
}

/// `formatTimeWithMS`.
pub fn format_time_with_ms(time_value: f64) -> String {
    time::format_time_with_ms(time_value)
}

/// `formatDate`: `YYYY-MM-DD` in `zone`.
pub fn format_date(epoch_ms: f64, zone: &impl time::LocalZone) -> String {
    time::format_date(epoch_ms, zone)
}

/// `createUUID`: 32 `[a-z0-9]` chars drawn from `rng` as `Math.random`.
pub fn create_uuid(rng: &mut dyn time::Rng) -> String {
    time::create_uuid(rng)
}

pub fn scale_value(value: f64, old_range: [f64; 2], new_range: [f64; 2]) -> f64 {
    ((value - old_range[0]) / (old_range[1] - old_range[0])) * (new_range[1] - new_range[0])
        + new_range[0]
}

pub fn scale_value_clamped(value: f64, old_range: [f64; 2], new_range: [f64; 2]) -> f64 {
    clamp_value(
        (value - old_range[0]) / (old_range[1] - old_range[0]),
        0.0,
        1.0,
    ) * (new_range[1] - new_range[0])
        + new_range[0]
}

/// `ValueScaler`: the same affine map as `scaleValue`, factored once.
pub struct ValueScaler {
    a: f64,
    b: f64,
}

impl ValueScaler {
    pub fn new(old_range: [f64; 2], new_range: [f64; 2]) -> Self {
        let a = (new_range[1] - new_range[0]) / (old_range[1] - old_range[0]);
        let b = new_range[0] - a * old_range[0];
        Self { a, b }
    }

    pub fn calculate(&self, value: f64) -> f64 {
        value * self.a + self.b
    }
}

pub fn clamp_value(value: f64, min: f64, max: f64) -> f64 {
    math::min2(math::max2(value, min), max)
}

pub fn transform_px(origin_px: [f64; 2], rotation_radians: f64, relative_px: [f64; 2]) -> [f64; 2] {
    let hypot = math::hypot(&[relative_px[0], relative_px[1]]);
    let new_angle = math::atan2(relative_px[1], relative_px[0]) + rotation_radians;
    [
        origin_px[0] + math::cos(new_angle) * hypot,
        origin_px[1] - math::sin(new_angle) * hypot,
    ]
}

/// `wrapRadians`: `(radians + PI) % (2 * PI)` shifted into `(-PI, PI]`; the
/// `> 0` else branch makes the `+PI` side inclusive.
pub fn wrap_radians(radians: f64) -> f64 {
    let wrapped = math::rem(radians + math::PI, 2.0 * math::PI);
    if wrapped > 0.0 {
        wrapped - math::PI
    } else {
        wrapped + math::PI
    }
}

pub fn concat_buffers(arrays: &[&[u8]]) -> Vec<u8> {
    let size: usize = arrays.iter().map(|a| a.len()).sum();
    let mut result = Vec::with_capacity(size);
    for array in arrays {
        result.extend_from_slice(array);
    }
    result
}

const MULTIPLIER_LOOKUP: [f64; 11] = [0.0, 1.0, 2.0, 2.0, 5.0, 5.0, 5.0, 5.0, 5.0, 10.0, 10.0];

/// `calcAxisStepSize`. An out-of-range lookup index reads `undefined`, which
/// shows up as NaN here.
pub fn calc_axis_step_size(data_range: [f64; 2], pixel_range: f64, step_size_target: f64) -> f64 {
    let step_count = pixel_range / step_size_target;
    let step_value_approx = (data_range[1] - data_range[0]) / step_count;
    let round_base = math::pow(10.0, math::floor(math::log10(step_value_approx)));
    let index = math::round(step_value_approx / round_base);
    let multiplier = if (0.0..=10.0).contains(&index) {
        MULTIPLIER_LOOKUP[index as usize]
    } else {
        f64::NAN
    };
    round_base * multiplier
}

/// `getSpiralIndex`, the stackoverflow ring-index formula upstream uses to
/// scatter graph colors.
pub fn get_spiral_index(x: f64, y: f64) -> f64 {
    let a = math::max2(math::abs(x), math::abs(y));
    let interior_count = (2.0 * a - 1.0) * (2.0 * a - 1.0);
    let start = [a, -(a - 1.0)];
    let offset_first_row = |start: [f64; 2], p: [f64; 2]| p[1] - start[1] + 1.0;
    if x == a && y >= start[1] {
        offset_first_row(start, [x, y]) + interior_count
    } else if y == a {
        let start2 = [a, a];
        let off1 = offset_first_row(start, start2);
        let off2 = start2[0] - x;
        off1 + off2 + interior_count
    } else if x == -a {
        let start2 = [a, a];
        let off1 = offset_first_row(start, start2);
        let off2 = start2[0] - (-a);
        let off3 = a - y;
        off1 + off2 + off3 + interior_count
    } else {
        let start2 = [a, a];
        let off1 = offset_first_row(start, start2);
        let off2 = start2[0] - (-a);
        let off3 = a - y;
        let off4 = x - (-a);
        off1 + off2 + off3 + off4 + interior_count
    }
}
