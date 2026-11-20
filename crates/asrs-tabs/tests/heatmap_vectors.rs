#![allow(clippy::disallowed_methods)]

#[path = "../../asrs-cli/src/heatmap.rs"]
#[allow(dead_code)]
mod heatmap;

use std::collections::HashMap;

use asrs_parity::Value;
use asrs_parity::vectors::{Record, check_records};
use asrs_tabs::heatmap::Heatmap;

// The vector records one `{fixture, update, args, ok, throw?}` line per update, generated
// by oracle/src/genHeatmap.ts against upstream Heatmap.ts plus the pinned h337 fork.
// `ok` holds {created, setData, store}; `throw` is present only when update threw.

fn check_update(state: &mut HashMap<String, Heatmap>, record: &Record) -> Result<(), String> {
    let fixture = record
        .string("fixture")
        .ok_or_else(|| "record has no fixture".to_owned())?;
    let heatmap = state.entry(fixture).or_default();
    let args = record.value("args").map_err(|e| format!("bad args: {e}"))?;
    let expected_ok = record.value("ok").map_err(|e| format!("bad ok: {e}"))?;
    let expected_throw = match record.field("throw") {
        Some(json) => Value::from_json(json).map_err(|e| format!("bad throw: {e}"))?,
        None => Value::Null,
    };
    let (ok, throw) =
        heatmap::update_record(heatmap, &args).map_err(|e| format!("update failed: {e}"))?;
    let (ok, expected_ok) = (ok.encode(), expected_ok.encode());
    if ok != expected_ok {
        return Err(format!("expected {expected_ok}\nactual   {ok}"));
    }
    let (throw, expected_throw) = (throw.encode(), expected_throw.encode());
    if throw != expected_throw {
        return Err(format!(
            "expected throw {expected_throw}\nactual          {throw}"
        ));
    }
    Ok(())
}

#[test]
fn heatmap() {
    let mut state = HashMap::new();
    check_records("tabs/heatmap", |record| check_update(&mut state, record));
}
