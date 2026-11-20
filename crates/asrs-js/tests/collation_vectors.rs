use std::cmp::Ordering;
use std::collections::HashMap;
use std::thread;

use asrs_js::collation::{locale_compare_numeric_utf16, locale_compare_utf16};
use asrs_parity::Json;
use asrs_parity::vectors::load;

const SHOWN: usize = 8;

struct Key {
    units: Vec<u16>,
    plain: usize,
    numeric: usize,
}

struct Corpus {
    keys: Vec<Key>,
    plain_exceptions: HashMap<(usize, usize), Ordering>,
    numeric_exceptions: HashMap<(usize, usize), Ordering>,
}

fn number(json: &Json, what: &str) -> f64 {
    json.as_f64()
        .unwrap_or_else(|| panic!("{what} is not a number"))
}

fn exceptions(json: &Json) -> HashMap<(usize, usize), Ordering> {
    json.as_array()
        .expect("exception list")
        .iter()
        .map(|entry| {
            let entry = entry.as_array().expect("exception entry");
            let order = number(&entry[2], "exception result");
            (
                (
                    number(&entry[0], "exception index") as usize,
                    number(&entry[1], "exception index") as usize,
                ),
                order
                    .partial_cmp(&0.0)
                    .expect("exception result is -1, 0 or 1"),
            )
        })
        .collect()
}

fn corpus() -> Corpus {
    let mut corpus = Corpus {
        keys: Vec::new(),
        plain_exceptions: HashMap::new(),
        numeric_exceptions: HashMap::new(),
    };
    for r in load("js/collation").unwrap_or_else(|e| panic!("{e}")) {
        if let Some(list) = r.field("xp") {
            corpus.plain_exceptions = exceptions(list);
            continue;
        }
        if let Some(list) = r.field("xn") {
            corpus.numeric_exceptions = exceptions(list);
            continue;
        }
        let rank = |field: &str| {
            number(
                r.field(field)
                    .unwrap_or_else(|| panic!("line {}: no {field:?} rank", r.line)),
                "rank",
            ) as usize
        };
        corpus.keys.push(Key {
            units: r
                .field("k")
                .and_then(Json::as_str)
                .unwrap_or_else(|| panic!("line {}: no key", r.line))
                .units()
                .to_vec(),
            plain: rank("p"),
            numeric: rank("n"),
        });
    }
    corpus
}

fn escape(units: &[u16]) -> String {
    let mut out = String::new();
    for &u in units {
        if (0x20..0x7f).contains(&u) {
            out.push(char::from(u as u8));
        } else {
            out.push_str(&format!("\\u{{{u:x}}}"));
        }
    }
    out
}

type Exceptions = HashMap<(usize, usize), Ordering>;

struct Form {
    name: &'static str,
    rank: fn(&Key) -> usize,
    exceptions: fn(&Corpus) -> &Exceptions,
    compare: fn(&[u16], &[u16]) -> Ordering,
}

/// Every ordered pair of keys, split across threads; returns the mismatch count and the first few.
fn check_all_pairs(corpus: &Corpus, form: &Form) -> (usize, Vec<String>) {
    let keys = &corpus.keys;
    let exceptions = (form.exceptions)(corpus);
    let threads = thread::available_parallelism().map_or(4, |n| n.get());
    let rows: Vec<usize> = (0..keys.len()).collect();
    let chunk = keys.len().div_ceil(threads);
    thread::scope(|scope| {
        let handles: Vec<_> = rows
            .chunks(chunk)
            .map(|rows| {
                scope.spawn(move || {
                    let mut count = 0;
                    let mut shown = Vec::new();
                    for &i in rows {
                        let a = &keys[i];
                        for (j, b) in keys.iter().enumerate() {
                            let expected = exceptions
                                .get(&(i, j))
                                .copied()
                                .unwrap_or_else(|| (form.rank)(a).cmp(&(form.rank)(b)));
                            let actual = (form.compare)(&a.units, &b.units);
                            if actual != expected {
                                count += 1;
                                if shown.len() < SHOWN {
                                    shown.push(format!(
                                        "keys {i} and {j}: \"{}\" vs \"{}\": expected {expected:?}, got {actual:?}",
                                        escape(&a.units),
                                        escape(&b.units)
                                    ));
                                }
                            }
                        }
                    }
                    (count, shown)
                })
            })
            .collect();
        let mut count = 0;
        let mut shown = Vec::new();
        for handle in handles {
            let (c, s) = handle.join().expect("pair checker thread");
            count += c;
            shown.extend(s);
        }
        shown.truncate(SHOWN);
        (count, shown)
    })
}

fn check(form: Form) {
    let corpus = corpus();
    assert!(corpus.keys.len() >= 5000, "expected at least 5000 keys");
    let (count, shown) = check_all_pairs(&corpus, &form);
    assert!(
        count == 0,
        "{}: {count} of {} ordered pairs differ from V8\n{}",
        form.name,
        corpus.keys.len() * corpus.keys.len(),
        shown.join("\n")
    );
}

#[test]
fn locale_compare_matches_v8_on_every_pair() {
    check(Form {
        name: "localeCompare",
        rank: |k| k.plain,
        exceptions: |c| &c.plain_exceptions,
        compare: locale_compare_utf16,
    });
}

#[test]
fn numeric_locale_compare_matches_v8_on_every_pair() {
    check(Form {
        name: "localeCompare numeric",
        rank: |k| k.numeric,
        exceptions: |c| &c.numeric_exceptions,
        compare: locale_compare_numeric_utf16,
    });
}

#[test]
fn prefix_tables_match_the_probed_sets() {
    let records = load("js/collation-unsafe").unwrap_or_else(|e| panic!("{e}"));
    let generated = include_str!("../src/collation/tables.rs");
    for (field, table) in [("digits", "DIGITS"), ("unsafe", "UNSAFE_BACKWARD")] {
        let ranges = records
            .iter()
            .find_map(|r| r.field(field))
            .and_then(Json::as_array)
            .unwrap_or_else(|| panic!("no {field:?} record"));
        let mut expected = format!("pub(crate) static {table}: &[(u16, u16)] = &[\n");
        for range in ranges {
            let range = range.as_array().expect("range");
            expected.push_str(&format!(
                "    (0x{:04x}, 0x{:04x}),\n",
                number(&range[0], "range start") as u32,
                number(&range[1], "range end") as u32
            ));
        }
        expected.push_str("];\n");
        assert!(
            generated.contains(&expected),
            "src/collation/tables.rs {table} differs from js/collation-unsafe; rerun gen_collation_table.py"
        );
    }
}
