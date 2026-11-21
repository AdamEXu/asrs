//! The subset of `simple-statistics` 7.8.7 that `StatisticsController.ts` uses, ported from
//! `node_modules/simple-statistics/src/` (ISC license, see `licenses/simple-statistics.txt`).
//!
//! Array elements stay `JsValue`s so undefined samples and non-number elements keep their JS
//! behaviour: `x[i]` reads past the end return undefined, `typeof x[i]` checks run verbatim, and
//! arithmetic goes through the same coercion rules the operators do.

use asrs_js::math::{self, BigInt};
use asrs_js::number;
use asrs_js::ops;
use asrs_js::sort;
use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;

/// What a call does upstream other than return: throw, or never return.
#[derive(Clone, Debug, PartialEq)]
pub enum StatsError {
    Thrown(JsError),
    /// Upstream spins forever in `quickselect` (see there).
    NeverCompletes,
}

impl From<JsError> for StatsError {
    fn from(e: JsError) -> Self {
        StatsError::Thrown(e)
    }
}

/// For callers that only use the library on paths that cannot reach `NeverCompletes`.
impl From<StatsError> for JsError {
    fn from(e: StatsError) -> Self {
        match e {
            StatsError::Thrown(e) => e,
            StatsError::NeverCompletes => {
                JsError::new("Unmodeled", "simple-statistics quickselect never completes")
            }
        }
    }
}

type Result<T> = std::result::Result<T, StatsError>;

fn num(x: f64) -> JsValue {
    JsValue::Number(x)
}

/// The largest array index (`2^32 - 2`).
const MAX_INDEX: f64 = 4_294_967_294.0;

/// How far past the end `quickselect` may write before the port stops calling it a dense array.
const MAX_GROWTH: usize = 1 << 24;

fn array_index(i: f64) -> Option<usize> {
    (i.fract() == 0.0 && (0.0..=MAX_INDEX).contains(&i)).then_some(i as usize)
}

fn elem(x: &[JsValue], i: f64) -> JsValue {
    array_index(i)
        .and_then(|i| x.get(i))
        .cloned()
        .unwrap_or(JsValue::Undefined)
}

/// An element of the array `quantile` selects in, with the identity `===` compares objects by:
/// the index it had in the copy.
#[derive(Clone)]
struct Elem {
    value: JsValue,
    id: usize,
}

impl Elem {
    const UNDEFINED: Elem = Elem {
        value: JsValue::Undefined,
        id: usize::MAX,
    };

    fn strict_equals(&self, other: &Elem) -> bool {
        let object = |v: &JsValue| {
            matches!(
                v,
                JsValue::Array(_)
                    | JsValue::Object(_)
                    | JsValue::Bytes(_)
                    | JsValue::Date(_)
                    | JsValue::ExtData(..)
            )
        };
        if object(&self.value) && object(&other.value) {
            self.id == other.id
        } else {
            ops::strict_equals(&self.value, &other.value)
        }
    }
}

/// The array `quantile` selects in: `arr[k]` with a `k` that is not an array index (negative,
/// fractional, infinite) reads and writes a named property, which later reads see.
struct SelectArray {
    items: Vec<Elem>,
    named: Vec<(f64, Elem)>,
}

impl SelectArray {
    fn new(x: &[JsValue]) -> Self {
        SelectArray {
            items: x
                .iter()
                .enumerate()
                .map(|(id, value)| Elem {
                    value: value.clone(),
                    id,
                })
                .collect(),
            named: Vec::new(),
        }
    }

    fn len(&self) -> f64 {
        self.items.len() as f64
    }

    fn get(&self, i: f64) -> Elem {
        match array_index(i) {
            Some(i) => self.items.get(i).cloned().unwrap_or(Elem::UNDEFINED),
            None => self
                .named
                .iter()
                .find(|(k, _)| same_key(*k, i))
                .map_or(Elem::UNDEFINED, |(_, v)| v.clone()),
        }
    }

    fn set(&mut self, i: f64, v: Elem) -> Result<()> {
        match array_index(i) {
            Some(i) if i < self.items.len() => self.items[i] = v,
            Some(i) if i - self.items.len() <= MAX_GROWTH => {
                self.items.resize(i, Elem::UNDEFINED);
                self.items.push(v);
            }
            Some(_) => {
                return Err(
                    JsError::new("Unmodeled", "quickselect writing far past the end").into(),
                );
            }
            None => match self.named.iter_mut().find(|(k, _)| same_key(*k, i)) {
                Some(slot) => slot.1 = v,
                None => self.named.push((i, v)),
            },
        }
        Ok(())
    }

    fn swap(&mut self, i: f64, j: f64) -> Result<()> {
        let a = self.get(i);
        let b = self.get(j);
        self.set(i, b)?;
        self.set(j, a)
    }
}

/// Property keys of two non-index numbers are equal when their `ToString`s are.
#[allow(clippy::float_cmp)]
fn same_key(a: f64, b: f64) -> bool {
    a == b || (a.is_nan() && b.is_nan())
}

#[allow(clippy::float_cmp)]
fn stuck(x: f64, step: f64) -> bool {
    x + step == x
}

/// `x.length` and `x[i]` for `quantileSorted`, on a plain array or a selected copy.
trait Indexed {
    fn length(&self) -> f64;
    fn at(&self, i: f64) -> JsValue;
}

impl Indexed for [JsValue] {
    fn length(&self) -> f64 {
        self.len() as f64
    }

    fn at(&self, i: f64) -> JsValue {
        elem(self, i)
    }
}

impl Indexed for SelectArray {
    fn length(&self) -> f64 {
        self.len()
    }

    fn at(&self, i: f64) -> JsValue {
        self.get(i).value
    }
}

/// `ToPrimitive` for the values `JSON.parse` and the log model produce.
#[derive(Clone)]
enum Prim {
    Undefined,
    Null,
    Bool(bool),
    Num(f64),
    Str(JsString),
    Big(BigInt),
}

fn primitive(v: &JsValue) -> Prim {
    match v {
        JsValue::Undefined => Prim::Undefined,
        JsValue::Null => Prim::Null,
        JsValue::Bool(b) => Prim::Bool(*b),
        JsValue::Number(x) => Prim::Num(*x),
        JsValue::String(s) => Prim::Str(s.clone()),
        JsValue::BigInt(b) => Prim::Big(b.clone()),
        JsValue::Array(_) | JsValue::Bytes(_) | JsValue::Object(_) | JsValue::ExtData(..) => {
            Prim::Str(ops::to_string(v))
        }
        JsValue::Date(t) => Prim::Num(*t),
    }
}

fn prim_to_f64(p: &Prim) -> Result<f64> {
    Ok(match p {
        Prim::Undefined => f64::NAN,
        Prim::Null => 0.0,
        Prim::Bool(b) => f64::from(u8::from(*b)),
        Prim::Num(x) => *x,
        Prim::Str(s) => number::to_number(s.units()),
        Prim::Big(b) => b.to_f64(),
    })
}

fn rel(a: &JsValue, b: &JsValue) -> Option<std::cmp::Ordering> {
    let a = primitive(a);
    let b = primitive(b);
    if let (Prim::Str(x), Prim::Str(y)) = (&a, &b) {
        return Some(x.units().iter().cmp(y.units().iter()));
    }
    let x = prim_to_f64(&a).ok()?;
    let y = prim_to_f64(&b).ok()?;
    x.partial_cmp(&y)
}

pub fn lt(a: &JsValue, b: &JsValue) -> bool {
    rel(a, b) == Some(std::cmp::Ordering::Less)
}

pub fn gt(a: &JsValue, b: &JsValue) -> bool {
    rel(a, b) == Some(std::cmp::Ordering::Greater)
}

pub fn le(a: &JsValue, b: &JsValue) -> bool {
    matches!(
        rel(a, b),
        Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
    )
}

pub fn ge(a: &JsValue, b: &JsValue) -> bool {
    matches!(
        rel(a, b),
        Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
    )
}

fn add(a: &JsValue, b: &JsValue) -> Result<JsValue> {
    if matches!(primitive(a), Prim::Str(_)) || matches!(primitive(b), Prim::Str(_)) {
        let mut s = ops::to_string(a);
        s.push_str(&ops::to_string(b));
        return Ok(JsValue::String(s));
    }
    Ok(num(ops::value_to_number(a)? + ops::value_to_number(b)?))
}

fn sub(a: &JsValue, b: &JsValue) -> Result<f64> {
    Ok(ops::value_to_number(a)? - ops::value_to_number(b)?)
}

/// `ToNumber` at `Math.*` calls, unary `+` and `Number()` sites: BigInt reports a different
/// TypeError than the arithmetic operators do.
fn math_num(v: &JsValue) -> Result<f64> {
    if let JsValue::BigInt(_) = v {
        return Err(JsError::type_error("Cannot convert a BigInt value to a number").into());
    }
    Ok(ops::value_to_number(v)?)
}

fn error(message: &str) -> StatsError {
    StatsError::Thrown(JsError::new("Error", message))
}

pub fn min_sorted(x: &[JsValue]) -> JsValue {
    if x.is_empty() {
        JsValue::Undefined
    } else {
        x[0].clone()
    }
}

pub fn max_sorted(x: &[JsValue]) -> JsValue {
    if x.is_empty() {
        JsValue::Undefined
    } else {
        x[x.len() - 1].clone()
    }
}

pub fn sum(x: &[JsValue]) -> JsValue {
    num(sum_f64(x))
}

fn sum_f64(x: &[JsValue]) -> f64 {
    let Some((first, rest)) = x.split_first() else {
        return 0.0;
    };
    let JsValue::Number(mut sum) = *first else {
        return f64::NAN;
    };
    let mut correction = 0.0;
    for v in rest {
        let JsValue::Number(xi) = *v else {
            return f64::NAN;
        };
        let transition = sum + xi;
        if math::abs(sum) >= math::abs(xi) {
            correction += sum - transition + xi;
        } else {
            correction += xi - transition + sum;
        }
        sum = transition;
    }
    sum + correction
}

pub fn mean(x: &[JsValue]) -> Result<JsValue> {
    Ok(num(mean_f64(x)?))
}

fn mean_f64(x: &[JsValue]) -> Result<f64> {
    if x.is_empty() {
        return Err(error("mean requires at least one data point"));
    }
    Ok(sum_f64(x) / x.len() as f64)
}

fn quantile_index(length: f64, p: &JsValue) -> Result<f64> {
    let idx = length * ops::value_to_number(p)?;
    if ops::strict_equals(p, &num(1.0)) {
        Ok(length - 1.0)
    } else if ops::strict_equals(p, &num(0.0)) {
        Ok(0.0)
    } else if math::rem(idx, 1.0) != 0.0 {
        Ok(math::ceil(idx) - 1.0)
    } else if math::rem(length, 2.0) == 0.0 {
        Ok(idx - 0.5)
    } else {
        Ok(idx)
    }
}

fn truthy(x: f64) -> bool {
    x != 0.0 && !x.is_nan()
}

/// `left = left || 0; right = right || arr.length - 1`.
fn window(arr: &SelectArray, left: f64, right: f64) -> (f64, f64) {
    (
        if truthy(left) { left } else { 0.0 },
        if truthy(right) {
            right
        } else {
            arr.len() - 1.0
        },
    )
}

/// Never completes when `k` is NaN (neither bound moves), and once `i++` or `j--` stops changing
/// its operand inside the partition loop (`-Infinity`, `+Infinity`, or past `2^53`): `i < j`
/// then holds forever, or a scan loop's condition does. A recursive call with the window it was
/// given is the same call again (nothing moved in between), so upstream recurses until the
/// stack overflows; with a NaN or infinite `k` on a window over 600 that comes first.
fn quickselect(arr: &mut SelectArray, k: f64, left: f64, right: f64) -> Result<()> {
    let (mut left, mut right) = window(arr, left, right);

    while right > left {
        if right - left > 600.0 {
            let n = right - left + 1.0;
            let m = k - left + 1.0;
            let z = math::log(n);
            let s = 0.5 * math::exp((2.0 * z) / 3.0);
            let mut sd = 0.5 * math::sqrt((z * s * (n - s)) / n);
            if m - n / 2.0 < 0.0 {
                sd *= -1.0;
            }
            let new_left = math::max2(left, math::floor(k - (m * s) / n + sd));
            let new_right = math::min2(right, math::floor(k + ((n - m) * s) / n + sd));
            if window(arr, new_left, new_right) == (left, right) {
                return Err(JsError::range("Maximum call stack size exceeded").into());
            }
            quickselect(arr, k, new_left, new_right)?;
        }
        if k.is_nan() {
            return Err(StatsError::NeverCompletes);
        }

        let t = arr.get(k);
        let mut i = left;
        let mut j = right;
        arr.swap(left, k)?;

        if gt(&arr.get(right).value, &t.value) {
            arr.swap(left, right)?;
        }

        while i < j {
            if stuck(i, 1.0) || stuck(j, -1.0) {
                return Err(StatsError::NeverCompletes);
            }
            arr.swap(i, j)?;
            i += 1.0;
            j -= 1.0;

            while lt(&arr.get(i).value, &t.value) {
                if stuck(i, 1.0) {
                    return Err(StatsError::NeverCompletes);
                }
                i += 1.0;
            }
            while gt(&arr.get(j).value, &t.value) {
                if stuck(j, -1.0) {
                    return Err(StatsError::NeverCompletes);
                }
                j -= 1.0;
            }
        }

        if arr.get(left).strict_equals(&t) {
            arr.swap(left, j)?;
        } else {
            j += 1.0;
            arr.swap(j, right)?;
        }

        if j <= k {
            left = j + 1.0;
        }
        if k <= j {
            right = j - 1.0;
        }
    }
    Ok(())
}

pub fn quantile_sorted(x: &[JsValue], p: &JsValue) -> Result<JsValue> {
    quantile_sorted_in(x, p)
}

fn quantile_sorted_in<X: Indexed + ?Sized>(x: &X, p: &JsValue) -> Result<JsValue> {
    let length = x.length();
    let idx = length * ops::value_to_number(p)?;

    if length == 0.0 {
        Err(error("quantile requires at least one data point."))
    } else if lt(p, &num(0.0)) || gt(p, &num(1.0)) {
        Err(error("quantiles must be between 0 and 1"))
    } else if ops::strict_equals(p, &num(1.0)) {
        Ok(x.at(length - 1.0))
    } else if ops::strict_equals(p, &num(0.0)) {
        Ok(x.at(0.0))
    } else if math::rem(idx, 1.0) != 0.0 {
        Ok(x.at(math::ceil(idx) - 1.0))
    } else if math::rem(length, 2.0) == 0.0 {
        Ok(num(ops::value_to_number(&add(
            &x.at(idx - 1.0),
            &x.at(idx),
        )?)? / 2.0))
    } else {
        Ok(x.at(idx))
    }
}

pub fn median_sorted(x: &[JsValue]) -> Result<JsValue> {
    quantile_sorted(x, &num(0.5))
}

pub fn mode_sorted(x: &[JsValue]) -> Result<JsValue> {
    if x.is_empty() {
        return Err(error("mode requires at least one data point"));
    } else if x.len() == 1 {
        return Ok(elem(x, 0.0));
    }
    let mut last = elem(x, 0.0);
    let mut value = num(f64::NAN);
    let mut max_seen = 0.0;
    let mut seen_this = 1.0;
    for i in 1..=x.len() {
        let cur = elem(x, i as f64);
        if !ops::strict_equals(&cur, &last) {
            if seen_this > max_seen {
                max_seen = seen_this;
                value = last;
            }
            seen_this = 1.0;
            last = cur;
        } else {
            seen_this += 1.0;
        }
    }
    Ok(value)
}

pub fn harmonic_mean(x: &[JsValue]) -> Result<JsValue> {
    if x.is_empty() {
        return Err(error("harmonicMean requires at least one data point"));
    }
    let mut reciprocal_sum = 0.0;
    for v in x {
        if le(v, &num(0.0)) {
            return Err(error(
                "harmonicMean requires only positive numbers as input",
            ));
        }
        reciprocal_sum += 1.0 / ops::value_to_number(v)?;
    }
    Ok(num(x.len() as f64 / reciprocal_sum))
}

pub fn root_mean_square(x: &[JsValue]) -> Result<JsValue> {
    if x.is_empty() {
        return Err(error("rootMeanSquare requires at least one data point"));
    }
    let mut sum_of_squares = 0.0;
    for v in x {
        sum_of_squares += math::pow(math_num(v)?, 2.0);
    }
    Ok(num(math::sqrt(sum_of_squares / x.len() as f64)))
}

/// `sumNthPowerDeviations`; the `n === 2` fast path is a plain multiply, not `Math.pow`.
pub fn sum_nth_power_deviations(x: &[JsValue], n: &JsValue) -> Result<JsValue> {
    Ok(num(sum_nth_power_deviations_f64(x, n)?))
}

fn sum_nth_power_deviations_f64(x: &[JsValue], n: &JsValue) -> Result<f64> {
    let mean_value = num(mean_f64(x)?);
    let mut sum = 0.0;
    for v in x {
        let diff = sub(v, &mean_value)?;
        if ops::strict_equals(n, &num(2.0)) {
            sum += diff * diff;
        } else {
            sum += math::pow(diff, math_num(n)?);
        }
    }
    Ok(sum)
}

pub fn sample_variance(x: &[JsValue]) -> Result<JsValue> {
    Ok(num(sample_variance_f64(x)?))
}

fn sample_variance_f64(x: &[JsValue]) -> Result<f64> {
    if x.len() < 2 {
        return Err(error("sampleVariance requires at least two data points"));
    }
    Ok(sum_nth_power_deviations_f64(x, &num(2.0))? / (x.len() as f64 - 1.0))
}

pub fn sample_standard_deviation(x: &[JsValue]) -> Result<JsValue> {
    Ok(num(math::sqrt(sample_variance_f64(x)?)))
}

fn quantile_select(arr: &mut SelectArray, k: f64, left: f64, right: f64) -> Result<()> {
    if math::rem(k, 1.0) == 0.0 {
        quickselect(arr, k, left, right)
    } else {
        let k = math::floor(k);
        quickselect(arr, k, left, right)?;
        quickselect(arr, k + 1.0, k + 1.0, right)
    }
}

fn multi_quantile_select(arr: &mut SelectArray, p: &[JsValue]) -> Result<()> {
    let length = arr.len();
    let mut indices = Vec::with_capacity(p.len() + 2);
    indices.push(0.0);
    for p_i in p {
        indices.push(quantile_index(length, p_i)?);
    }
    indices.push(length - 1.0);
    sort::array_sort(&mut indices, |a, b| a - b);
    let mut stack = vec![0.0, indices.len() as f64 - 1.0];
    while !stack.is_empty() {
        let r = math::ceil(stack.pop().unwrap());
        let l = math::floor(stack.pop().unwrap());
        if r - l <= 1.0 {
            continue;
        }
        let m = math::floor((l + r) / 2.0);
        quantile_select(
            arr,
            indices[m as usize],
            math::floor(indices[l as usize]),
            math::ceil(indices[r as usize]),
        )?;
        stack.push(l);
        stack.push(m);
        stack.push(m);
        stack.push(r);
    }
    Ok(())
}

pub fn quantile(x: &[JsValue], p: &JsValue) -> Result<JsValue> {
    let mut copy = SelectArray::new(x);
    if let JsValue::Array(ps) = p {
        multi_quantile_select(&mut copy, ps)?;
        let mut results = Vec::with_capacity(ps.len());
        for p_i in ps {
            results.push(quantile_sorted_in(&copy, p_i)?);
        }
        return Ok(JsValue::Array(results));
    }
    let idx = quantile_index(copy.len(), p)?;
    let right = copy.len() - 1.0;
    quantile_select(&mut copy, idx, 0.0, right)?;
    quantile_sorted_in(&copy, p)
}

pub fn median(x: &[JsValue]) -> Result<JsValue> {
    Ok(num(math_num(&quantile(x, &num(0.5))?)?))
}

pub fn median_absolute_deviation(x: &[JsValue]) -> Result<JsValue> {
    let median_value = ops::value_to_number(&median(x)?)?;
    let mut deviations = Vec::with_capacity(x.len());
    for v in x {
        deviations.push(num(math::abs(ops::value_to_number(v)? - median_value)));
    }
    median(&deviations)
}

pub fn interquartile_range(x: &[JsValue]) -> Result<JsValue> {
    let q1 = quantile(x, &num(0.75))?;
    let q2 = quantile(x, &num(0.25))?;
    if let (JsValue::Number(a), JsValue::Number(b)) = (&q1, &q2) {
        Ok(num(a - b))
    } else {
        Ok(JsValue::Undefined)
    }
}

pub fn sample_skewness(x: &[JsValue]) -> Result<JsValue> {
    if x.len() < 3 {
        return Err(error("sampleSkewness requires at least three data points"));
    }
    let mean_value = mean_f64(x)?;
    let mut sum_squared_deviations = 0.0;
    let mut sum_cubed_deviations = 0.0;
    for v in x {
        let temp = ops::value_to_number(v)? - mean_value;
        sum_squared_deviations += temp * temp;
        sum_cubed_deviations += temp * temp * temp;
    }
    let bessel_correction = x.len() as f64 - 1.0;
    let s = math::sqrt(sum_squared_deviations / bessel_correction);
    let n = x.len() as f64;
    let cubed_s = math::pow(s, 3.0);
    Ok(num(
        (n * sum_cubed_deviations) / ((n - 1.0) * (n - 2.0) * cubed_s)
    ))
}

/// `logAverage` from `StatisticsController.ts`.
pub fn log_average(x: &[JsValue]) -> Result<JsValue> {
    if x.is_empty() {
        return Err(error("logAverage requires at least one data point"));
    }
    let mut value = 0.0;
    for v in x {
        if lt(v, &num(0.0)) {
            return Err(error(
                "logAverage requires only non-negative numbers as input",
            ));
        }
        value += math::log(math_num(v)?);
    }
    Ok(num(math::exp(value / x.len() as f64)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nums(xs: &[f64]) -> Vec<JsValue> {
        xs.iter().copied().map(num).collect()
    }

    #[test]
    fn quantile_with_a_nan_probability_never_completes() {
        let x = nums(&[3.0, 1.0, 2.0]);
        for p in [
            num(f64::NAN),
            JsValue::Undefined,
            JsValue::String("x".into()),
        ] {
            assert_eq!(quantile(&x, &p), Err(StatsError::NeverCompletes));
        }
        assert_eq!(
            quantile(&x, &num(f64::NEG_INFINITY)),
            Err(StatsError::NeverCompletes)
        );
    }

    #[test]
    fn quantile_past_the_floyd_rivest_window_overflows_the_stack_instead() {
        let big: Vec<JsValue> = (0..2000).map(|i| num(f64::from(i % 7))).collect();
        for p in [f64::NAN, f64::INFINITY] {
            assert_eq!(
                quantile(&big, &num(p)),
                Err(StatsError::Thrown(JsError::range(
                    "Maximum call stack size exceeded"
                )))
            );
        }
    }

    #[test]
    fn quantile_with_a_nan_probability_returns_on_one_element() {
        assert_eq!(
            quantile(&nums(&[4.0]), &num(f64::NAN)),
            Ok(JsValue::Undefined)
        );
    }
}
