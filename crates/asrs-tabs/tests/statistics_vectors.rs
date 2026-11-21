//! Direct `fn` vectors for the `simple-statistics` subset in
//! `asrs_tabs::simple_statistics`, requested as `npm:simple-statistics:<fn>`.

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use asrs_js::string::JsError;
use asrs_js::value::JsValue;
use asrs_parity::cve1::Value;
use asrs_parity::utf16::Utf16String;
use asrs_parity::{FnCase, Response, fn_vectors};
use asrs_tabs::simple_statistics as ss;

fn args(case: &FnCase<'_>) -> Result<Vec<JsValue>, String> {
    case.request.args.iter().map(js_json::to_js).collect()
}

fn arr(args: &[JsValue], i: usize) -> Result<Vec<JsValue>, JsError> {
    match args.get(i).unwrap_or(&JsValue::Undefined) {
        JsValue::Array(items) => Ok(items.clone()),
        // `x.forEach`/`x.length` reads upstream does on non-arrays.
        other => Err(JsError::type_error(format!(
            "x.forEach is not a function ({other:?})"
        ))),
    }
}

fn respond(result: Result<JsValue, JsError>) -> Response {
    match result {
        Ok(v) => Response::Ok(js_json::from_js(&v)),
        Err(e) if e.name.is_empty() => {
            Response::Throw(Value::String(Utf16String::from(e.message)))
        }
        Err(e) => Response::Throw(Value::Error(
            Utf16String::from(e.name),
            Utf16String::from(e.message),
        )),
    }
}

fn dispatch(case: &FnCase<'_>) -> Response {
    let Some(name) = case.name().strip_prefix("npm:simple-statistics:") else {
        return Response::Error(format!("no port of {}", case.name()));
    };
    let Ok(args) = args(case) else {
        return Response::Error("bad cve1 argument".to_owned());
    };
    let x = || arr(&args, 0);
    let p = || args.get(1).cloned().unwrap_or(JsValue::Undefined);
    let plain = |v: JsValue| Response::Ok(js_json::from_js(&v));
    respond(match name {
        "mean" => x().and_then(|x| ss::mean(&x)),
        "median" => x().and_then(|x| ss::median(&x)),
        "medianSorted" => x().and_then(|x| ss::median_sorted(&x)),
        "modeSorted" => x().and_then(|x| ss::mode_sorted(&x)),
        "harmonicMean" => x().and_then(|x| ss::harmonic_mean(&x)),
        "rootMeanSquare" => x().and_then(|x| ss::root_mean_square(&x)),
        "sampleVariance" => x().and_then(|x| ss::sample_variance(&x)),
        "sampleStandardDeviation" => x().and_then(|x| ss::sample_standard_deviation(&x)),
        "medianAbsoluteDeviation" => x().and_then(|x| ss::median_absolute_deviation(&x)),
        "interquartileRange" => x().and_then(|x| ss::interquartile_range(&x)),
        "sampleSkewness" => x().and_then(|x| ss::sample_skewness(&x)),
        "logAverage" => x().and_then(|x| ss::log_average(&x)),
        "quantile" => x().and_then(|x| ss::quantile(&x, &p())),
        "quantileSorted" => x().and_then(|x| ss::quantile_sorted(&x, &p())),
        "sumNthPowerDeviations" => x().and_then(|x| ss::sum_nth_power_deviations(&x, &p())),
        "sum" => return x().map(|x| plain(ss::sum(&x))).unwrap_or_else(|e| respond(Err(e))),
        "minSorted" => {
            return x()
                .map(|x| plain(ss::min_sorted(&x)))
                .unwrap_or_else(|e| respond(Err(e)));
        }
        "maxSorted" => {
            return x()
                .map(|x| plain(ss::max_sorted(&x)))
                .unwrap_or_else(|e| respond(Err(e)));
        }
        _ => return Response::Error(format!("no port of {}", case.name())),
    })
}

fn_vectors!(
    simple_statistics,
    &asrs_parity::vectors::platform_vector("tabs/simple-statistics"),
    dispatch
);
