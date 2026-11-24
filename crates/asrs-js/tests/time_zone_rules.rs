#![allow(clippy::disallowed_methods)]

//! `js/time-zone-rules`: every UTC offset change from 1830 to 2100 in each zone of tzdata 2025b,
//! as Electron's `Date` has them.

use std::collections::BTreeSet;

use asrs_js::time::{LocalZone, NamedZone};
use asrs_parity::vectors::{check_records, load};
use asrs_parity::{Json, Record};

const END: f64 = 4_102_444_800_000.0;
const SAMPLE: f64 = 45.0 * 86_400_000.0;

fn changes(record: &Record) -> Result<Vec<[f64; 2]>, String> {
    let list = record.field("changes").and_then(Json::as_array);
    list.ok_or("no changes")?
        .iter()
        .map(|change| match change.as_array() {
            Some([ms, offset]) => match (ms.as_f64(), offset.as_f64()) {
                (Some(ms), Some(offset)) => Ok([ms, offset]),
                _ => Err("a change is not two numbers".to_owned()),
            },
            _ => Err("a change is not a pair".to_owned()),
        })
        .collect()
}

fn check(record: &Record) -> Result<(), String> {
    let name = record.string("zone").ok_or("no zone")?;
    let zone = NamedZone::get(&name).map_err(|e| e.to_string())?;
    let changes = changes(record)?;
    if changes.is_empty() {
        return Err(format!("{name}: no offsets"));
    }
    for (i, [from, offset]) in changes.iter().enumerate() {
        let to = changes.get(i + 1).map_or(END, |next| next[0]);
        let mut at = *from;
        loop {
            let actual = zone.offset_ms(at);
            if actual != *offset {
                return Err(format!("{name} at {at}: expected {offset}, got {actual}"));
            }
            if at == to - 1.0 {
                break;
            }
            at = (at + SAMPLE).min(to - 1.0);
        }
    }
    Ok(())
}

#[test]
fn offsets_match_electron() {
    check_records("js/time-zone-rules", check);
}

#[test]
fn every_bundled_zone_is_recorded() {
    assert_eq!(jiff_tzdb::VERSION, Some("2025b"));
    let recorded: BTreeSet<String> = load("js/time-zone-rules")
        .expect("vectors")
        .iter()
        .map(|record| record.string("zone").expect("zone"))
        .collect();
    let bundled: BTreeSet<String> = jiff_tzdb::available()
        .filter(|name| Some(*name) != jiff_tzdb::VERSION)
        .map(str::to_owned)
        .collect();
    assert_eq!(recorded, bundled);
}
