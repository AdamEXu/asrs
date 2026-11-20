#![allow(clippy::disallowed_methods)]

use std::cmp::Ordering;

use asrs_js::collation::{locale_compare, locale_compare_numeric};
use asrs_js::number;
use asrs_js::sort::{
    Slot, array_sort, array_sort_slots, array_to_sorted, array_to_sorted_slots, code_unit_cmp,
};
use asrs_js::string::JsString;
use asrs_parity::vectors::check_records;
use asrs_parity::{Json, Record, Value};

#[derive(Clone, Debug)]
enum Val {
    Num(f64),
    Str(JsString),
}

#[derive(Clone, Debug)]
struct Elem {
    id: usize,
    v: Val,
}

impl Elem {
    fn num(&self) -> f64 {
        match &self.v {
            Val::Num(x) => *x,
            Val::Str(_) => panic!("element {} is not a number", self.id),
        }
    }

    fn str(&self) -> &JsString {
        match &self.v {
            Val::Str(s) => s,
            Val::Num(_) => panic!("element {} is not a string", self.id),
        }
    }

    fn to_js_string(&self) -> JsString {
        match &self.v {
            Val::Num(x) => JsString::from(number::to_string(*x)),
            Val::Str(s) => s.clone(),
        }
    }
}

/// The comparator stream of genSort.ts's `caseRng`.
struct CaseRng(u32);

impl CaseRng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x6d2b79f5);
        let mut t = self.0;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        f64::from(t ^ (t >> 14)) / 4294967296.0
    }
}

struct Trace {
    hash: u32,
    calls: u64,
}

impl Trace {
    fn record(&mut self, a: &Elem, b: &Elem) {
        self.calls += 1;
        for x in [a.id as u32, b.id as u32] {
            for shift in [0, 8, 16, 24] {
                self.hash ^= (x >> shift) & 0xff;
                self.hash = self.hash.wrapping_mul(0x0100_0193);
            }
        }
    }
}

const RANDF_VALUES: [f64; 11] = [
    f64::NAN,
    -0.0,
    0.0,
    f64::INFINITY,
    f64::NEG_INFINITY,
    1e-300,
    -1e-300,
    0.5,
    -0.5,
    9007199254740992.0,
    -2147483648.0,
];

fn starts_with_digit(s: &JsString) -> bool {
    s.units().first().is_some_and(|u| (0x30..=0x39).contains(u))
}

fn sign(order: Ordering) -> f64 {
    match order {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    }
}

type Comparator = Box<dyn FnMut(&Elem, &Elem) -> f64>;

fn comparator(kind: &str, seed: u32) -> Comparator {
    match kind {
        "num" | "numNaN" => Box::new(|a, b| a.num() - b.num()),
        "rev" => Box::new(|a, b| {
            if a.num() < b.num() {
                1.0
            } else if a.num() > b.num() {
                -1.0
            } else {
                0.0
            }
        }),
        "mod" => Box::new(|a, b| (a.num() % 3.0) - (b.num() % 3.0)),
        "zero" => Box::new(|_, _| 0.0),
        "neg" => Box::new(|_, _| -1.0),
        "pos" => Box::new(|_, _| 1.0),
        "rand" => {
            let mut rng = CaseRng(seed);
            Box::new(move |_, _| {
                let x = rng.next();
                if x < 1.0 / 3.0 {
                    -1.0
                } else if x < 2.0 / 3.0 {
                    0.0
                } else {
                    1.0
                }
            })
        }
        "randf" => {
            let mut rng = CaseRng(seed);
            Box::new(move |_, _| {
                let x = rng.next();
                if x < 0.5 {
                    RANDF_VALUES[(rng.next() * RANDF_VALUES.len() as f64) as usize]
                } else {
                    x - 0.75
                }
            })
        }
        "pairhash" => Box::new(|a, b| ((a.id * 7919 + b.id * 104729) % 5) as f64 - 2.0),
        "evergreen" => {
            let evergreen = JsString::from("Evergreen");
            Box::new(move |a, b| {
                if *a.str() == evergreen {
                    return 1.0;
                }
                if *b.str() == evergreen {
                    return -1.0;
                }
                match code_unit_cmp(a.str().units(), b.str().units()) {
                    Ordering::Greater => -1.0,
                    Ordering::Less => 1.0,
                    Ordering::Equal => 0.0,
                }
            })
        }
        "robots" => Box::new(|a, b| {
            let order = sign(locale_compare_numeric(a.str(), b.str()));
            if starts_with_digit(a.str()) && starts_with_digit(b.str()) {
                -order
            } else {
                order
            }
        }),
        "locale" => Box::new(|a, b| sign(locale_compare(a.str(), b.str()))),
        "default" => Box::new(|a, b| {
            sign(code_unit_cmp(
                a.to_js_string().units(),
                b.to_js_string().units(),
            ))
        }),
        other => panic!("unknown comparator kind {other:?}"),
    }
}

fn input(record: &Record) -> Result<Vec<Slot<Elem>>, String> {
    let Value::Array(items) = record.value("a").map_err(|e| e.to_string())? else {
        return Err("\"a\" is not an array".to_owned());
    };
    items
        .into_iter()
        .enumerate()
        .map(|(id, item)| match item {
            Value::Undefined => Ok(Slot::Undefined),
            Value::Hole => Ok(Slot::Hole),
            Value::Number(bits) => Ok(Slot::Value(Elem {
                id,
                v: Val::Num(f64::from_bits(bits)),
            })),
            Value::String(s) => Ok(Slot::Value(Elem {
                id,
                v: Val::Str(JsString::from_units(s.units().to_vec())),
            })),
            other => Err(format!("element {id}: unexpected {other:?}")),
        })
        .collect()
}

fn layout(slots: &[Slot<Elem>]) -> String {
    let parts: Vec<String> = slots
        .iter()
        .map(|s| match s {
            Slot::Value(e) => e.id.to_string(),
            Slot::Undefined => "u".to_owned(),
            Slot::Hole => "h".to_owned(),
        })
        .collect();
    parts.join(",")
}

fn expected_layout(record: &Record) -> Result<String, String> {
    let Value::Array(items) = record.value("o").map_err(|e| e.to_string())? else {
        return Err("\"o\" is not an array".to_owned());
    };
    let parts: Result<Vec<String>, String> = items
        .into_iter()
        .map(|item| match item {
            Value::Number(bits) => Ok((f64::from_bits(bits) as usize).to_string()),
            Value::Undefined => Ok("u".to_owned()),
            Value::Hole => Ok("h".to_owned()),
            other => Err(format!("unexpected layout entry {other:?}")),
        })
        .collect();
    Ok(parts?.join(","))
}

fn number_field(record: &Record, key: &str) -> Result<f64, String> {
    record
        .field(key)
        .and_then(Json::as_f64)
        .ok_or_else(|| format!("no number {key:?}"))
}

fn check_sort_record(record: &Record) -> Result<(), String> {
    let method = record.string("m").ok_or("no method")?;
    let kind = record.string("c").ok_or("no comparator kind")?;
    let seed = number_field(record, "s")? as u32;
    let slots = input(record)?;
    let mut trace = Trace {
        hash: 0x811c_9dc5,
        calls: 0,
    };
    let mut compare = comparator(&kind, seed);
    let mut traced = |a: &Elem, b: &Elem| {
        trace.record(a, b);
        compare(a, b)
    };
    let actual = match method.as_str() {
        "sort" => {
            let mut slots = slots;
            array_sort_slots(&mut slots, &mut traced);
            slots
        }
        "toSorted" => array_to_sorted_slots(&slots, &mut traced),
        other => return Err(format!("unknown method {other:?}")),
    };
    let expected = expected_layout(record)?;
    let actual = layout(&actual);
    let calls = number_field(record, "nc")? as u64;
    let hash = number_field(record, "t")? as u32;
    if actual != expected || trace.calls != calls || trace.hash != hash {
        return Err(format!(
            "{method} with {kind} on {} elements:\n  expected [{expected}] after {calls} calls (trace {hash:08x})\n  actual   [{actual}] after {} calls (trace {:08x})",
            slots_len(record),
            trace.calls,
            trace.hash
        ));
    }
    Ok(())
}

fn slots_len(record: &Record) -> usize {
    record
        .field("a")
        .and_then(Json::as_array)
        .map_or(0, <[Json]>::len)
}

#[test]
fn sort_and_to_sorted_match_v8() {
    check_records("js/sort", check_sort_record);
}

#[test]
fn plain_slices_sort_like_slots() {
    let values: Vec<f64> = (0..200).map(|i| f64::from((i * 37) % 23)).collect();
    let mut sorted = values.clone();
    array_sort(&mut sorted, |a: &f64, b: &f64| a - b);
    let copy = array_to_sorted(&values, |a: &f64, b: &f64| a - b);
    let mut expected = values.clone();
    let mut slots: Vec<Slot<f64>> = expected.drain(..).map(Slot::Value).collect();
    array_sort_slots(&mut slots, |a: &f64, b: &f64| a - b);
    let from_slots: Vec<f64> = slots
        .into_iter()
        .map(|s| match s {
            Slot::Value(v) => v,
            _ => panic!("no holes were given"),
        })
        .collect();
    assert_eq!(sorted, from_slots);
    assert_eq!(copy, from_slots);
}
