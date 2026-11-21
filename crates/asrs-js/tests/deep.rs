//! Issue 219: walking a deeply nested `JsValue` must not overflow a small thread stack.
//! Everything here runs on a 2 MiB thread, the size the issue's 50,000-deep array aborted.

use std::thread;

use asrs_js::json;
use asrs_js::ops;
use asrs_js::value::JsValue;

const STACK: usize = 2 * 1024 * 1024;

fn run(test: impl FnOnce() + Send + 'static) {
    thread::Builder::new()
        .stack_size(STACK)
        .spawn(test)
        .unwrap()
        .join()
        .unwrap();
}

fn nested_arrays(depth: usize) -> String {
    "[".repeat(depth) + &"]".repeat(depth)
}

fn nested_objects(depth: usize) -> String {
    "{\"a\":".repeat(depth) + "1" + &"}".repeat(depth)
}

#[test]
fn parse_and_drop_million_deep_arrays() {
    run(|| {
        let value = json::parse_str(&nested_arrays(1_000_000)).unwrap();
        drop(value);
    });
}

#[test]
fn parse_and_drop_million_deep_objects() {
    run(|| {
        let value = json::parse_str(&nested_objects(1_000_000)).unwrap();
        drop(value);
    });
}

#[test]
fn deep_clone_eq_and_debug() {
    run(|| {
        let value = json::parse_str(&nested_arrays(200_000)).unwrap();
        let other = json::parse_str(&nested_arrays(200_000)).unwrap();
        assert_eq!(value, other);
        let clone = value.clone();
        assert_eq!(value, clone);
        let debug = format!("{value:?}");
        assert!(debug.starts_with("Array([Array(["));
    });
}

#[test]
fn deep_compact_stringify_is_unbounded() {
    run(|| {
        // With both `replacer` and `space` undefined V8 uses an iterative serializer, so a
        // million-deep value stringifies rather than throwing.
        let depth = 1_000_000;
        let value = json::parse_str(&nested_arrays(depth)).unwrap();
        let text = json::stringify(&value).unwrap().unwrap();
        assert_eq!(text.len(), 2 * depth);
    });
}

#[test]
fn deep_pretty_stringify_throws_range_error() {
    run(|| {
        let value = json::parse_str(&nested_arrays(20_000)).unwrap();
        for result in [
            json::stringify_pretty(&value),
            json::stringify_gap(&value, ""),
        ] {
            let err = result.unwrap_err();
            assert_eq!(err.name, "RangeError");
            assert_eq!(
                err.message.to_string_lossy(),
                "Maximum call stack size exceeded"
            );
        }
    });
}

#[test]
fn deep_array_join_to_string() {
    run(|| {
        // `Array.prototype.join` descends nested arrays; the string flattens every level.
        let mut value = JsValue::Number(1.0);
        for _ in 0..500_000 {
            value = JsValue::Array(vec![value]);
        }
        assert_eq!(ops::to_string(&value).to_string_lossy(), "1");
    });
}

/// The hand-written `Clone`, `PartialEq` and `Debug` behave as the derived ones did, on both
/// sides of the depth where they switch from recursion to a worklist.
#[test]
fn hand_written_impls_match_derived_semantics() {
    run(|| {
        for depth in [0, 255, 256, 257, 600] {
            let leaf = |doc: &str| json::parse_str(doc).unwrap();
            let wrap = |inner: JsValue| {
                (0..depth).fold(inner, |v, _| JsValue::Array(vec![JsValue::Null, v]))
            };
            let nan = wrap(JsValue::Number(f64::NAN));
            assert_ne!(nan, nan.clone(), "NaN != NaN at {depth}");
            assert_eq!(wrap(JsValue::Number(-0.0)), wrap(JsValue::Number(0.0)));
            assert_ne!(
                wrap(leaf(r#"{"a":1,"b":2}"#)),
                wrap(leaf(r#"{"b":2,"a":1}"#))
            );
            assert_eq!(
                wrap(leaf(r#"{"1":1,"0":2}"#)),
                wrap(leaf(r#"{"0":2,"1":1}"#))
            );
            assert_ne!(wrap(leaf("[1,2]")), wrap(leaf("[1,2,3]")));
            assert_ne!(wrap(JsValue::Null), wrap(JsValue::Undefined));
            let value = wrap(leaf(r#"{"b":[true,"x"],"1":{},"a":null}"#));
            let clone = value.clone();
            assert_eq!(value, clone);
            assert_eq!(format!("{clone:?}"), format!("{value:?}"));
        }
        let value = json::parse_str(r#"[1,"s",{"k":[null,{}]},[]]"#).unwrap();
        assert_eq!(
            format!("{value:?}"),
            r#"Array([Number(1.0), String("s"), Object({"k": Array([Null, Object({})])}), Array([])])"#
        );
    });
}
