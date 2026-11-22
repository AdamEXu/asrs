#![allow(clippy::disallowed_methods)]

//! Replays `update/licenses`: upstream's getLicenses.mjs run in scripted working directories.
//! The port runs against the same directory; every SPDX fetch it makes must be the next recorded
//! one (and gets the recorded reply), and every log line and written file must match in order.

mod p07_support;

use std::collections::VecDeque;

use asrs_js::string::JsString;
use asrs_parity::json::Json;
use asrs_parity::vectors;
use asrs_services::licenses::{LicenseHost, SpdxReply, generate};
use p07_support::{Fields, Tally, assert_tree, js_string_value, tree_bytes};

/// The generator's fake working directory: entries in recorded order, a directory listing
/// holds the entries recorded directly under it, and `"."`/`""` is the root.
struct Host {
    ctx: String,
    entries: Vec<(String, Option<Vec<u8>>)>,
    effects: VecDeque<Json>,
    tally: Tally,
}

fn norm(path: &str) -> String {
    path.split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect::<Vec<_>>()
        .join("/")
}

impl Host {
    fn entry(&self, path: &str) -> Option<&Option<Vec<u8>>> {
        let key = norm(path);
        self.entries.iter().find(|(p, _)| *p == key).map(|(_, d)| d)
    }

    fn next(&mut self, kind: &str) -> Fields {
        let effect = self
            .effects
            .pop_front()
            .unwrap_or_else(|| panic!("{}: port made an extra {kind} effect", self.ctx));
        self.tally.record(kind);
        let mut f = Fields::new(&effect, &format!("{}: {kind}", self.ctx));
        assert_eq!(f.string("k"), kind, "{}: effect kind", self.ctx);
        f
    }
}

impl LicenseHost for Host {
    fn exists(&mut self, path: &str) -> bool {
        norm(path).is_empty() || self.entry(path).is_some()
    }

    fn read_dir(&mut self, path: &str) -> Vec<String> {
        let base = norm(path);
        assert!(
            base.is_empty() || matches!(self.entry(path), Some(None)),
            "{}: readdir of {path:?}, which is not a directory",
            self.ctx
        );
        let prefix = if base.is_empty() {
            String::new()
        } else {
            base + "/"
        };
        self.entries
            .iter()
            .filter_map(|(p, _)| p.strip_prefix(&prefix))
            .filter(|rest| !rest.is_empty() && !rest.contains('/'))
            .map(str::to_owned)
            .collect()
    }

    fn is_dir(&mut self, path: &str) -> bool {
        if norm(path).is_empty() {
            return true;
        }
        match self.entry(path) {
            Some(data) => data.is_none(),
            None => panic!("{}: stat of missing {path:?}", self.ctx),
        }
    }

    fn read(&mut self, path: &str) -> Vec<u8> {
        match self.entry(path) {
            Some(Some(data)) => data.clone(),
            _ => panic!("{}: read of {path:?}, which is not a file", self.ctx),
        }
    }

    fn fetch(&mut self, url: &str) -> SpdxReply {
        let mut f = self.next("fetch");
        assert_eq!(f.string("url"), url, "{}: fetch url", self.ctx);
        let mut r = Fields::new(&f.take("reply"), &format!("{}: reply", self.ctx));
        let reply = if r.has("reject") {
            assert!(r.bool("reject"));
            SpdxReply::Rejected
        } else {
            SpdxReply::Response {
                status: r.raw_number("status") as u16,
                body: tree_bytes(&r.tree("body"), &self.ctx),
            }
        };
        r.finish();
        f.finish();
        self.tally.compare("fetch");
        reply
    }

    fn error(&mut self, text: &str) {
        let mut f = self.next("error");
        assert_eq!(f.string("text"), text, "{}: console.error", self.ctx);
        f.finish();
        self.tally.compare("error");
    }

    fn log(&mut self, text: &str) {
        let mut f = self.next("log");
        assert_eq!(f.string("text"), text, "{}: console.log", self.ctx);
        f.finish();
        self.tally.compare("log");
    }

    fn write(&mut self, path: &str, text: &JsString) {
        let mut f = self.next("write");
        assert_eq!(f.string("path"), path, "{}: written path", self.ctx);
        assert_tree(
            &format!("{}: {path}", self.ctx),
            &f.tree("data"),
            &js_string_value(text),
        );
        f.finish();
        self.tally.compare("write");
    }
}

#[test]
fn licenses() {
    let id = "update/licenses";
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{e}"));
    let mut tally = Tally::default();
    for (index, record) in records.iter().enumerate() {
        let ctx = format!("{id} line {}", record.line);
        let mut f = Fields::new(&record.json, &ctx);
        tally.record("case");
        assert_eq!(f.raw_number("case"), index as f64, "{ctx}: case order");
        let name = f.string("name");
        let ctx = format!("{ctx} ({name})");
        let entries = f
            .array("files")
            .iter()
            .map(|entry| {
                let mut e = Fields::new(entry, &ctx);
                let out = if e.has("dir") {
                    (e.string("dir"), None)
                } else {
                    let path = e.string("file");
                    let data = tree_bytes(&e.tree("data"), &ctx);
                    (path, Some(data))
                };
                e.finish();
                out
            })
            .collect();
        let mut host = Host {
            ctx: ctx.clone(),
            entries,
            effects: f.array("effects").into(),
            tally: Tally::default(),
        };
        generate(&mut host);
        assert!(
            host.effects.is_empty(),
            "{ctx}: port stopped before {} recorded effects, first {}",
            host.effects.len(),
            host.effects[0].to_compact()
        );
        host.tally.finish(&ctx);
        tally.compare("case");
        f.finish();
    }
    tally.finish(id);
}
