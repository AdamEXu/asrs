//! Replay of the `read` and `write` records a Blink `dom` run with `"geometry": "real"` writes
//! (oracle/README.md, "Blink backend"). Blink laid the page out; every geometry read upstream
//! made is a `{"read": name, "el": path, "value": <CVE-1>}` record and every scroll position
//! write a `{"write": name, "el": path, "value": <CVE-1>}` record, in the order upstream made
//! them. A port's headless model asks [`ScriptedReads`] for the same reads in the same order
//! and reports its writes to it; any read or write upstream did not make, made in another
//! order, on another element or, for a write, with another value, fails the replay, and so do
//! records the port never asked for.

use std::collections::VecDeque;

use crate::cve1::Value;
use crate::vectors::Record;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

impl Access {
    fn key(self) -> &'static str {
        match self {
            Access::Read => "read",
            Access::Write => "write",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GeometryRecord {
    pub access: Access,
    pub name: String,
    /// `tag.class:index` per element from `<body>` down, joined by `>`.
    pub el: String,
    /// A rect is `[x, y, width, height]`; a scalar is a number.
    pub value: Value,
    pub line: usize,
}

impl GeometryRecord {
    pub fn parse(record: &Record) -> Result<Option<GeometryRecord>, String> {
        let access = match (record.field("read"), record.field("write")) {
            (Some(_), None) => Access::Read,
            (None, Some(_)) => Access::Write,
            (None, None) => return Ok(None),
            (Some(_), Some(_)) => {
                return Err(format!("line {}: both read and write", record.line));
            }
        };
        let field = |key: &str| {
            record
                .string(key)
                .ok_or_else(|| format!("line {}: {key} is not a string", record.line))
        };
        Ok(Some(GeometryRecord {
            access,
            name: field(access.key())?,
            el: field("el")?,
            value: record
                .value("value")
                .map_err(|e| format!("line {}: {e}", record.line))?,
            line: record.line,
        }))
    }

    /// Whether `target` names this record's element: a tail of its path, segment by segment,
    /// where a target segment `tag.class:index` matches a recorded one with the same tag and
    /// index and at least those classes (`div.data-table-container:2`, `tbody:0>tr:3` matches
    /// `...>tbody:0>tr.selected:3`).
    pub fn is_on(&self, target: &str) -> bool {
        let recorded: Vec<&str> = self.el.split('>').collect();
        let wanted: Vec<&str> = target.split('>').collect();
        wanted.len() <= recorded.len()
            && recorded[recorded.len() - wanted.len()..]
                .iter()
                .zip(&wanted)
                .all(|(r, w)| segment_matches(r, w))
    }
}

fn segment_matches(recorded: &str, wanted: &str) -> bool {
    let split = |s: &'_ str| -> Option<(Vec<String>, String)> {
        let (name, index) = s.rsplit_once(':')?;
        let mut parts = name.split('.').map(str::to_owned);
        let tag = parts.next()?;
        let mut classes: Vec<String> = parts.collect();
        classes.push(format!("<{tag}>"));
        Some((classes, index.to_owned()))
    };
    match (split(recorded), split(wanted)) {
        (Some((have, i)), Some((want, j))) => i == j && want.iter().all(|c| have.contains(c)),
        _ => false,
    }
}

#[derive(Debug, Default)]
pub struct ScriptedReads {
    queue: VecDeque<GeometryRecord>,
    error: Option<String>,
    recorded: usize,
}

impl ScriptedReads {
    pub fn from_records(records: &[Record]) -> Result<ScriptedReads, String> {
        let mut queue = VecDeque::new();
        for record in records {
            if let Some(g) = GeometryRecord::parse(record)? {
                queue.push_back(g);
            }
        }
        Ok(ScriptedReads {
            recorded: queue.len(),
            queue,
            error: None,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// How many records the port has asked for so far.
    pub fn consumed(&self) -> usize {
        self.recorded - self.queue.len()
    }

    fn fail(&mut self, message: String) {
        self.error.get_or_insert(message);
    }

    fn take(&mut self, access: Access, name: &str, target: &str) -> Option<GeometryRecord> {
        if self.error.is_some() {
            return None;
        }
        match self.queue.pop_front() {
            Some(g) if g.access == access && g.name == name && g.is_on(target) => Some(g),
            Some(g) => {
                self.fail(format!(
                    "the port made {} {name} on {target}; upstream's next was {} {} on {} (line {})",
                    access.key(),
                    g.access.key(),
                    g.name,
                    g.el,
                    g.line
                ));
                None
            }
            None => {
                self.fail(format!(
                    "the port made {} {name} on {target} after upstream's last",
                    access.key()
                ));
                None
            }
        }
    }

    /// Upstream's value for the next read, which must be `name` on `target`.
    pub fn read(&mut self, name: &str, target: &str) -> Value {
        self.take(Access::Read, name, target)
            .map_or(Value::Undefined, |g| g.value)
    }

    pub fn read_f64(&mut self, name: &str, target: &str) -> f64 {
        let value = self.read(name, target);
        value.as_f64().unwrap_or_else(|| {
            if self.error.is_none() {
                self.fail(format!(
                    "{name} on {target} recorded {value:?}, not a number"
                ));
            }
            f64::NAN
        })
    }

    /// A `getBoundingClientRect` read: `[x, y, width, height]`; NaNs after a mismatch.
    pub fn read_rect(&mut self, target: &str) -> [f64; 4] {
        let value = self.read("getBoundingClientRect", target);
        match &value {
            Value::Array(items) if items.len() == 4 => {
                let mut out = [f64::NAN; 4];
                for (slot, item) in out.iter_mut().zip(items) {
                    *slot = item.as_f64().unwrap_or(f64::NAN);
                }
                out
            }
            _ => {
                if self.error.is_none() {
                    self.fail(format!(
                        "getBoundingClientRect on {target} recorded {value:?}"
                    ));
                }
                [f64::NAN; 4]
            }
        }
    }

    /// The port wrote `value` to `name` on `target`; upstream's next record must be that write.
    pub fn write(&mut self, name: &str, target: &str, value: &Value) {
        if let Some(g) = self.take(Access::Write, name, target)
            && g.value != *value
        {
            self.fail(format!(
                "the port wrote {name} = {value:?} on {target}; upstream wrote {:?} (line {})",
                g.value, g.line
            ));
        }
    }

    pub fn write_f64(&mut self, name: &str, target: &str, value: f64) {
        self.write(name, target, &Value::Number(value.to_bits()));
    }

    /// Ok with the number of records compared when every one was asked for, in order.
    pub fn finish(mut self) -> Result<usize, String> {
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        if let Some(g) = self.queue.front() {
            return Err(format!(
                "upstream made {} more geometry accesses than the port, starting with {} {} on {} (line {})",
                self.queue.len(),
                g.access.key(),
                g.name,
                g.el,
                g.line
            ));
        }
        Ok(self.recorded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vectors::parse_records;

    const N25: &str = r#"{"n":"4039000000000000"}"#;

    fn reads() -> ScriptedReads {
        let text = format!(
            "{{\"read\":\"scrollTop\",\"el\":\"body:9>div.c:0\",\"value\":{N25}}}\n\
             {{\"main\":\"x\"}}\n\
             {{\"read\":\"getBoundingClientRect\",\"el\":\"body:9>div.c:0>tr:2\",\"value\":[{N25},{N25},{N25},{N25}]}}\n\
             {{\"write\":\"scrollTop\",\"el\":\"body:9>div.c:0\",\"value\":{N25}}}\n"
        );
        ScriptedReads::from_records(&parse_records(&text).unwrap()).unwrap()
    }

    #[test]
    fn answers_in_order_and_counts() {
        let mut r = reads();
        assert_eq!(r.read_f64("scrollTop", "div.c:0"), 25.0);
        assert_eq!(r.read_rect("div:0>tr:2"), [25.0; 4]);
        r.write_f64("scrollTop", "body:9>div.c:0", 25.0);
        assert_eq!(r.finish(), Ok(3));
    }

    #[test]
    fn rejects_other_order_targets_values_and_leftovers() {
        let mut r = reads();
        r.read_rect("tr:2");
        assert!(
            r.finish()
                .unwrap_err()
                .contains("upstream's next was read scrollTop")
        );

        let mut r = reads();
        r.read_f64("scrollTop", "c:0");
        assert!(
            r.finish().is_err(),
            "a tail must start at an element boundary"
        );

        let mut r = reads();
        r.read_f64("scrollTop", "div.c.selected:0");
        assert!(r.finish().is_err(), "a target class the element lacks");

        let mut r = reads();
        r.read_f64("scrollTop", "div.c:0");
        r.read_rect("tr:2");
        r.write_f64("scrollTop", "div.c:0", 0.0);
        assert!(r.finish().unwrap_err().contains("upstream wrote"));

        let mut r = reads();
        r.read_f64("scrollTop", "div.c:0");
        assert!(r.finish().unwrap_err().contains("2 more geometry accesses"));

        let mut r = reads();
        r.read_f64("scrollTop", "div.c:0");
        r.read_rect("tr:2");
        r.write_f64("scrollTop", "div.c:0", 25.0);
        r.read_f64("clientHeight", "div.c:0");
        assert!(r.finish().unwrap_err().contains("after upstream's last"));
    }
}
