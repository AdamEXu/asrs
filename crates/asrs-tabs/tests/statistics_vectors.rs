//! Vectors for the `simple-statistics` subset in `asrs_tabs::simple_statistics`: the direct `fn`
//! requests (`npm:simple-statistics:<fn>`, per platform because `sampleSkewness` and
//! `sumNthPowerDeviations` reach the C library's `pow`) and the seeded `genSimpleStatistics`
//! arrays.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code, clippy::disallowed_methods)]
mod js_json;

use std::collections::HashMap;

use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_parity::cve1::Value;
use asrs_parity::utf16::Utf16String;
use asrs_parity::vectors::Record;
use asrs_parity::{FnCase, Response, fn_vectors, record_vectors};
use asrs_tabs::simple_statistics::{self as ss, StatsError};

/// `ss[name](...args)`, or `None` when the port has no such function.
fn call(name: &str, args: &[JsValue]) -> Option<Result<JsValue, StatsError>> {
    let p = args.get(1).cloned().unwrap_or(JsValue::Undefined);
    let Some(JsValue::Array(x)) = args.first() else {
        return Some(Err(StatsError::Thrown(JsError::new(
            "Unmodeled",
            "x is not an array",
        ))));
    };
    Some(match name {
        "mean" => ss::mean(x),
        "median" => ss::median(x),
        "medianSorted" => ss::median_sorted(x),
        "modeSorted" => ss::mode_sorted(x),
        "harmonicMean" => ss::harmonic_mean(x),
        "rootMeanSquare" => ss::root_mean_square(x),
        "sampleVariance" => ss::sample_variance(x),
        "sampleStandardDeviation" => ss::sample_standard_deviation(x),
        "medianAbsoluteDeviation" => ss::median_absolute_deviation(x),
        "interquartileRange" => ss::interquartile_range(x),
        "sampleSkewness" => ss::sample_skewness(x),
        "logAverage" => ss::log_average(x),
        "quantile" => ss::quantile(x, &p),
        "quantileSorted" => ss::quantile_sorted(x, &p),
        "sumNthPowerDeviations" => ss::sum_nth_power_deviations(x, &p),
        "sum" => Ok(ss::sum(x)),
        "minSorted" => Ok(ss::min_sorted(x)),
        "maxSorted" => Ok(ss::max_sorted(x)),
        _ => return None,
    })
}

fn thrown(e: &JsError) -> Value {
    if e.name.is_empty() {
        Value::String(Utf16String::from(e.message.clone()))
    } else {
        Value::Error(
            Utf16String::from(e.name),
            Utf16String::from(e.message.clone()),
        )
    }
}

fn dispatch(case: &FnCase<'_>) -> Response {
    let Some(name) = case.name().strip_prefix("npm:simple-statistics:") else {
        return Response::Error(format!("no port of {}", case.name()));
    };
    let Ok(args) = case
        .request
        .args
        .iter()
        .map(js_json::to_js)
        .collect::<Result<Vec<_>, _>>()
    else {
        return Response::Error("bad cve1 argument".to_owned());
    };
    match call(name, &args) {
        None => Response::Error(format!("no port of {}", case.name())),
        Some(Ok(v)) => Response::Ok(js_json::from_js(&v)),
        Some(Err(StatsError::NeverCompletes)) => Response::Error("never completes".to_owned()),
        Some(Err(StatsError::Thrown(e))) if e.name == "Unmodeled" => Response::Error(e.message),
        Some(Err(StatsError::Thrown(e))) => Response::Throw(thrown(&e)),
    }
}

fn_vectors!(
    simple_statistics,
    &asrs_parity::vectors::platform_vector("tabs/simple-statistics"),
    dispatch
);

fn random_case(arrays: &mut HashMap<String, JsValue>, record: &Record) -> Result<(), String> {
    let id = record.value("a").map_err(|e| e.0)?.encode();
    if record.field("x").is_some() {
        arrays.insert(id, js_json::to_js(&record.value("x").map_err(|e| e.0)?)?);
        return Ok(());
    }
    let name = record.string("fn").ok_or("record has no fn")?;
    let x = arrays
        .get(&id)
        .cloned()
        .ok_or_else(|| format!("array {id} is not defined"))?;
    let mut args = vec![x];
    if record.field("p").is_some() {
        args.push(js_json::to_js(&record.value("p").map_err(|e| e.0)?)?);
    }
    let got = call(&name, &args).ok_or_else(|| format!("no port of {name}"))?;
    let expected = if record.field("never").is_some() {
        "never completes".to_owned()
    } else if record.field("throw").is_some() {
        format!("throw {}", record.value("throw").map_err(|e| e.0)?.encode())
    } else {
        format!("ok {}", record.value("ok").map_err(|e| e.0)?.encode())
    };
    let actual = match got {
        Ok(v) => format!("ok {}", js_json::from_js(&v).encode()),
        Err(StatsError::NeverCompletes) => "never completes".to_owned(),
        Err(StatsError::Thrown(e)) if e.name == "Unmodeled" => format!("unmodeled {}", e.message),
        Err(StatsError::Thrown(e)) => format!("throw {}", thrown(&e).encode()),
    };
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "{name} p={}:\n  expected {expected}\n  actual   {actual}",
            record.value("p").map(|p| p.encode()).unwrap_or_default()
        ))
    }
}

record_vectors!(simple_statistics_random, "tabs/simple-statistics-random", {
    let mut arrays = HashMap::new();
    move |record: &Record| random_case(&mut arrays, record)
});
