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
use asrs_js::value::{JsObject, JsValue};

type JsArray = Vec<JsValue>;

type Result<T> = std::result::Result<T, JsError>;

fn num(x: f64) -> JsValue {
    JsValue::Number(x)
}

fn elem(x: &[JsValue], i: f64) -> JsValue {
    if i.fract() == 0.0 && i >= 0.0 && i < x.len() as f64 {
        x[i as usize].clone()
    } else {
        JsValue::Undefined
    }
}

fn set(x: &mut JsArray, i: f64, v: JsValue) {
    if i.fract() == 0.0 && i >= 0.0 && i < (u32::MAX - 1) as f64 {
        let i = i as usize;
        if i >= x.len() {
            x.resize(i + 1, JsValue::Undefined);
        }
        x[i] = v;
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
        return Err(JsError::type_error(
            "Cannot convert a BigInt value to a number",
        ));
    }
    ops::value_to_number(v)
}

fn error(message: &str) -> JsError {
    JsError::new("Error", message)
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
    if x.is_empty() {
        return num(0.0);
    }
    let mut sum = match &x[0] {
        JsValue::Number(v) => *v,
        _ => return num(f64::NAN),
    };
    let mut correction = 0.0;
    for v in &x[1..] {
        let JsValue::Number(xi) = v else {
            return num(f64::NAN);
        };
        let transition = sum + xi;
        if math::abs(sum) >= math::abs(*xi) {
            correction += sum - transition + xi;
        } else {
            correction += xi - transition + sum;
        }
        sum = transition;
    }
    num(sum + correction)
}

pub fn mean(x: &[JsValue]) -> Result<JsValue> {
    if x.is_empty() {
        return Err(error("mean requires at least one data point"));
    }
    let JsValue::Number(s) = sum(x) else {
        unreachable!()
    };
    Ok(num(s / x.len() as f64))
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

fn swap(arr: &mut JsArray, i: f64, j: f64) {
    let a = elem(arr, i);
    let b = elem(arr, j);
    set(arr, i, b);
    set(arr, j, a);
}

fn quickselect(arr: &mut JsArray, k: f64, mut left: f64, mut right: f64) {
    if !truthy(left) {
        left = 0.0;
    }
    if !truthy(right) {
        right = arr.len() as f64 - 1.0;
    }

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
            quickselect(arr, k, new_left, new_right);
        }

        let t = elem(arr, k);
        let mut i = left;
        let mut j = right;
        swap(arr, left, k);

        if gt(&elem(arr, right), &t) {
            swap(arr, left, right);
        }

        while i < j {
            swap(arr, i, j);
            i += 1.0;
            j -= 1.0;

            while lt(&elem(arr, i), &t) {
                i += 1.0;
            }
            while gt(&elem(arr, j), &t) {
                j -= 1.0;
            }
        }

        if ops::strict_equals(&elem(arr, left), &t) {
            swap(arr, left, j);
        } else {
            j += 1.0;
            swap(arr, j, right);
        }

        if j <= k {
            left = j + 1.0;
        }
        if k <= j {
            right = j - 1.0;
        }
    }
}

pub fn quantile_sorted(x: &[JsValue], p: &JsValue) -> Result<JsValue> {
    let length = x.len() as f64;
    let idx = length * ops::value_to_number(p)?;

    if x.is_empty() {
        Err(error("quantile requires at least one data point."))
    } else if lt(p, &num(0.0)) || gt(p, &num(1.0)) {
        Err(error("quantiles must be between 0 and 1"))
    } else if ops::strict_equals(p, &num(1.0)) {
        Ok(elem(x, length - 1.0))
    } else if ops::strict_equals(p, &num(0.0)) {
        Ok(elem(x, 0.0))
    } else if math::rem(idx, 1.0) != 0.0 {
        Ok(elem(x, math::ceil(idx) - 1.0))
    } else if x.len().is_multiple_of(2) {
        Ok(num(ops::value_to_number(&add(
            &elem(x, idx - 1.0),
            &elem(x, idx),
        )?)? / 2.0))
    } else {
        Ok(elem(x, idx))
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
    let mean_value = mean(x)?;
    let mut sum = 0.0;
    for v in x {
        let diff = sub(v, &mean_value)?;
        if ops::strict_equals(n, &num(2.0)) {
            sum += diff * diff;
        } else {
            sum += math::pow(diff, math_num(n)?);
        }
    }
    Ok(num(sum))
}

pub fn sample_variance(x: &[JsValue]) -> Result<JsValue> {
    if x.len() < 2 {
        return Err(error("sampleVariance requires at least two data points"));
    }
    let JsValue::Number(deviations) = sum_nth_power_deviations(x, &num(2.0))? else {
        unreachable!()
    };
    Ok(num(deviations / (x.len() as f64 - 1.0)))
}

pub fn sample_standard_deviation(x: &[JsValue]) -> Result<JsValue> {
    let JsValue::Number(v) = sample_variance(x)? else {
        unreachable!()
    };
    Ok(num(math::sqrt(v)))
}

fn quantile_select(arr: &mut JsArray, k: f64, left: f64, right: f64) {
    if math::rem(k, 1.0) == 0.0 {
        quickselect(arr, k, left, right);
    } else {
        let k = math::floor(k);
        quickselect(arr, k, left, right);
        quickselect(arr, k + 1.0, k + 1.0, right);
    }
}

fn multi_quantile_select(arr: &mut JsArray, p: &[JsValue]) -> Result<()> {
    let length = arr.len() as f64;
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
        );
        stack.push(l);
        stack.push(m);
        stack.push(m);
        stack.push(r);
    }
    Ok(())
}

pub fn quantile(x: &[JsValue], p: &JsValue) -> Result<JsValue> {
    let mut copy = x.to_vec();
    if let JsValue::Array(ps) = p {
        multi_quantile_select(&mut copy, ps)?;
        let mut results = Vec::with_capacity(ps.len());
        for p_i in ps {
            results.push(quantile_sorted(&copy, p_i)?);
        }
        return Ok(JsValue::Array(results));
    }
    let idx = quantile_index(copy.len() as f64, p)?;
    let right = copy.len() as f64 - 1.0;
    quantile_select(&mut copy, idx, 0.0, right);
    quantile_sorted(&copy, p)
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
    let JsValue::Number(mean_value) = mean(x)? else {
        unreachable!()
    };
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

pub fn field_stats(
    values: &[JsValue],
    sorted: &[JsValue],
    positive_sorted: &[JsValue],
    geometric_mean: &JsValue,
) -> Result<JsObject> {
    let mut stats = JsObject::new();
    stats.insert("count", num(values.len() as f64));
    stats.insert("min", min_sorted(sorted));
    stats.insert("max", max_sorted(sorted));
    stats.insert("mean", mean(values)?);
    stats.insert("median", median_sorted(sorted)?);
    stats.insert("mode", mode_sorted(sorted)?);
    stats.insert("geometricMean", geometric_mean.clone());
    stats.insert("harmonicMean", harmonic_mean(positive_sorted)?);
    stats.insert("quadraticMean", root_mean_square(values)?);
    stats.insert("standardDeviation", sample_standard_deviation(values)?);
    stats.insert(
        "medianAbsoluteDeviation",
        median_absolute_deviation(values)?,
    );
    stats.insert("interquartileRange", interquartile_range(values)?);
    stats.insert("skewness", sample_skewness(values)?);
    for p in [0.01f64, 0.05, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95, 0.99] {
        let name = match p.to_bits() {
            b if b == 0.01f64.to_bits() => "percentile01",
            b if b == 0.05f64.to_bits() => "percentile05",
            b if b == 0.1f64.to_bits() => "percentile10",
            b if b == 0.25f64.to_bits() => "percentile25",
            b if b == 0.5f64.to_bits() => "percentile50",
            b if b == 0.75f64.to_bits() => "percentile75",
            b if b == 0.9f64.to_bits() => "percentile90",
            b if b == 0.95f64.to_bits() => "percentile95",
            _ => "percentile99",
        };
        stats.insert(name, quantile_sorted(sorted, &num(p))?);
    }
    Ok(stats)
}
