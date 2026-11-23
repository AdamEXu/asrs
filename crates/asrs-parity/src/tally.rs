use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::cve1::{DecodeError, Value};
use crate::json::Json;
use crate::vectors::{self, Record};

/// Counts, per record kind, what a replay compared against what the vector holds, so a replay
/// that skips records fails. Every record is either compared or of a kind declared in
/// [`Tally::covered_elsewhere`]; [`Tally::finish`] panics unless each kind's counts are equal, and
/// a tally dropped without it panics too.
pub struct Tally {
    id: String,
    recorded: BTreeMap<String, usize>,
    compared: BTreeMap<String, usize>,
    elsewhere: BTreeMap<String, String>,
    finished: bool,
}

impl Tally {
    /// Counts `records` by `kind`. An empty vector panics.
    pub fn new(id: &str, records: &[Record], kind: impl Fn(&Record) -> String) -> Tally {
        assert!(!records.is_empty(), "vectors {id}: no records");
        let mut recorded = BTreeMap::new();
        for record in records {
            *recorded.entry(kind(record)).or_default() += 1;
        }
        Tally {
            id: id.to_owned(),
            recorded,
            compared: BTreeMap::new(),
            elsewhere: BTreeMap::new(),
            finished: false,
        }
    }

    /// [`Tally::new`] for records whose kind is their first key, as the `dom` harness writes them.
    pub fn by_first_key(id: &str, records: &[Record]) -> Tally {
        Tally::new(id, records, first_key)
    }

    /// [`Tally::new`] for records whose kind is the string at `key`.
    pub fn by_field(id: &str, records: &[Record], key: &str) -> Tally {
        Tally::new(id, records, |r| field_kind(id, r, key))
    }

    /// Declares that another harness compares every record of `kind`; `by` names it.
    pub fn covered_elsewhere(&mut self, kind: &str, by: &str) {
        assert!(
            !by.trim().is_empty(),
            "{}: {kind}: covered by what?",
            self.id
        );
        self.elsewhere.insert(kind.to_owned(), by.to_owned());
    }

    /// Counts one record of `kind` as compared. A kind the vector does not hold panics.
    pub fn compared(&mut self, kind: &str) {
        assert!(
            self.recorded.contains_key(kind),
            "vectors {}: compared a {kind:?} record, which the vector does not hold",
            self.id
        );
        *self.compared.entry(kind.to_owned()).or_default() += 1;
    }

    /// Runs `check` on `record`, and counts it as compared when it succeeds having read every one
    /// of the record's fields.
    pub fn compare<E: From<String>>(
        &mut self,
        kind: &str,
        record: &Record,
        check: impl FnOnce(&Fields<'_>) -> Result<(), E>,
    ) -> Result<(), E> {
        let fields = Fields::new(record);
        check(&fields)?;
        fields.finish()?;
        self.compared(kind);
        Ok(())
    }

    /// Panics unless every kind was compared as often as it was recorded; returns the number of
    /// records compared.
    pub fn finish(mut self) -> usize {
        self.finished = true;
        let problems = self.problems();
        assert!(
            problems.is_empty(),
            "vectors {}: not every record was compared:\n  {}",
            self.id,
            problems.join("\n  ")
        );
        self.compared.values().sum()
    }

    fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (kind, n) in &self.recorded {
            let compared = self.compared.get(kind).copied().unwrap_or(0);
            match self.elsewhere.get(kind) {
                None if compared != *n => {
                    out.push(format!("{kind:?}: {n} recorded, {compared} compared"));
                }
                Some(by) if compared > 0 => out.push(format!(
                    "{kind:?}: {compared} compared, but declared as compared by {by}"
                )),
                _ => {}
            }
        }
        for kind in self.elsewhere.keys() {
            if !self.recorded.contains_key(kind) {
                out.push(format!("{kind:?}: declared, and the vector holds none"));
            }
        }
        if self.compared.is_empty() {
            out.push("nothing was compared".to_owned());
        }
        out
    }
}

impl Drop for Tally {
    fn drop(&mut self) {
        if !self.finished && !std::thread::panicking() {
            panic!("vectors {}: the replay never finished its tally", self.id);
        }
    }
}

fn first_key(record: &Record) -> String {
    match &record.json {
        Json::Object(pairs) if !pairs.is_empty() => pairs[0].0.to_string_lossy(),
        _ => panic!("line {}: the record has no first key", record.line),
    }
}

fn field_kind(id: &str, record: &Record, key: &str) -> String {
    record
        .string(key)
        .unwrap_or_else(|| panic!("vectors {id} line {}: no string {key:?}", record.line))
}

/// One record's fields, remembering which were read.
pub struct Fields<'a> {
    record: &'a Record,
    read: RefCell<BTreeSet<String>>,
}

impl<'a> Fields<'a> {
    pub fn new(record: &'a Record) -> Fields<'a> {
        Fields {
            record,
            read: RefCell::default(),
        }
    }

    pub fn line(&self) -> usize {
        self.record.line
    }

    pub fn field(&self, key: &str) -> Option<&'a Json> {
        self.read.borrow_mut().insert(key.to_owned());
        self.record.json.get(key)
    }

    pub fn string(&self, key: &str) -> Result<String, String> {
        self.field(key)
            .and_then(Json::as_str)
            .map(|s| s.to_string_lossy())
            .ok_or_else(|| format!("line {}: no string {key:?}", self.record.line))
    }

    /// The CVE-1 value at `key`.
    pub fn value(&self, key: &str) -> Result<Value, DecodeError> {
        self.field(key);
        self.record.value(key)
    }

    /// Marks `key` as read without comparing it, for one only upstream's run uses; `why` says so.
    pub fn ignore(&self, key: &str, why: &str) {
        assert!(!why.trim().is_empty(), "{key:?} ignored for no reason");
        self.field(key);
    }

    /// Fails if the record holds a field that was never read.
    pub fn finish(self) -> Result<(), String> {
        let read = self.read.into_inner();
        let Json::Object(pairs) = &self.record.json else {
            return Err(format!("line {}: not an object", self.record.line));
        };
        let unread: Vec<String> = pairs
            .iter()
            .map(|(k, _)| k.to_string_lossy())
            .filter(|k| !read.contains(k))
            .collect();
        if unread.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "line {}: the replay never read {}",
                self.record.line,
                unread.join(", ")
            ))
        }
    }
}

/// [`vectors::check_records`] over records whose kind is the string at `kind_key`: `check` gets
/// the kind and must read every field of the record, and every record must be compared.
pub fn check_every_field(
    id: &str,
    kind_key: &str,
    mut check: impl FnMut(&str, &Fields<'_>) -> Result<(), String>,
) {
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut tally = Tally::by_field(id, &records, kind_key);
    vectors::check_record_list(id, &records, |record| {
        let kind = field_kind(id, record, kind_key);
        tally.compare(&kind, record, |fields| {
            fields.field(kind_key);
            check(&kind, fields)
        })
    });
    tally.finish();
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use super::*;

    const TEXT: &str = "{\"command\":1,\"value\":2}\n{\"state\":3}\n{\"command\":4,\"value\":5}\n";

    fn records() -> Vec<Record> {
        vectors::parse_records(TEXT).unwrap()
    }

    fn panic_text(f: impl FnOnce()) -> String {
        let payload = catch_unwind(AssertUnwindSafe(f)).unwrap_err();
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .expect("a message")
    }

    #[test]
    fn equal_counts_pass() {
        let mut tally = Tally::by_first_key("t", &records());
        tally.covered_elsewhere("state", "another harness");
        tally.compared("command");
        tally.compared("command");
        assert_eq!(tally.finish(), 2);
    }

    #[test]
    fn a_skipped_record_fails() {
        let mut tally = Tally::by_first_key("t", &records());
        tally.compared("command");
        tally.compared("state");
        let text = panic_text(|| {
            tally.finish();
        });
        assert!(
            text.contains("\"command\": 2 recorded, 1 compared"),
            "{text}"
        );
        assert!(!text.contains("\"state\""), "{text}");
    }

    #[test]
    fn comparing_more_than_recorded_fails() {
        let mut tally = Tally::by_first_key("t", &records());
        for kind in ["command", "command", "command", "state"] {
            tally.compared(kind);
        }
        let text = panic_text(|| {
            tally.finish();
        });
        assert!(text.contains("2 recorded, 3 compared"), "{text}");
    }

    #[test]
    fn a_kind_compared_here_is_not_covered_elsewhere() {
        let mut tally = Tally::by_first_key("t", &records());
        tally.covered_elsewhere("state", "another harness");
        for kind in ["command", "command", "state"] {
            tally.compared(kind);
        }
        let text = panic_text(|| {
            tally.finish();
        });
        assert!(
            text.contains("declared as compared by another harness"),
            "{text}"
        );
    }

    #[test]
    fn declaring_everything_away_compares_nothing() {
        let mut tally = Tally::by_first_key("t", &records());
        tally.covered_elsewhere("state", "another harness");
        tally.covered_elsewhere("command", "another harness");
        let text = panic_text(|| {
            tally.finish();
        });
        assert!(text.contains("nothing was compared"), "{text}");
    }

    #[test]
    fn unknown_kinds_and_unfinished_tallies_panic() {
        let records = records();
        let text = panic_text(|| Tally::by_first_key("t", &records).compared("canvas"));
        assert!(text.contains("which the vector does not hold"), "{text}");
        let text = panic_text(|| drop(Tally::by_first_key("t", &records)));
        assert!(text.contains("never finished its tally"), "{text}");
        let text = panic_text(|| drop(Tally::by_first_key("t", &[])));
        assert!(text.contains("no records"), "{text}");
    }

    #[test]
    fn a_record_with_an_unread_field_is_not_compared() {
        let records = records();
        let mut tally = Tally::by_first_key("t", &records);
        let error = tally
            .compare::<String>("command", &records[0], |f| {
                f.field("command");
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error, "line 1: the replay never read value");
        for record in &records {
            let kind = first_key(record);
            tally
                .compare::<String>(&kind, record, |f| {
                    f.field(&kind);
                    f.ignore("value", "not under test");
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(tally.finish(), 3);
    }
}
