//! Replays `revlog/convert` (the real `parseREVLOG(path, writable)`, called the way main.ts's
//! `historical-start` calls it) through `asrs_services::historical_start`: a `.revlog` reply
//! carries the converted bytes, or `files: [null]` and the rejection's message, and an open
//! failure also raises the uncaught exception the converter's unlistened read stream throws.
//! Converted inputs also go through `historical-start-raw`, as a `log:revlog` child does.

use std::collections::HashMap;

use asrs_formats::historical::MainRequest;
use asrs_js::string::JsString;
use asrs_js::time::Rng;
use asrs_parity::json::Json;
use asrs_parity::vectors::{self, Record};
use asrs_services::fs::{Failure, MemFs};
use asrs_services::historical_start::{FileOpen, Outcome};

const ID: &str = "revlog/convert";
const TEMP: &str = "/fake/temp";
const RAW_LIMIT: usize = 4096;

struct Counter(u32);

impl Rng for Counter {
    fn next_f64(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
        f64::from(self.0 >> 8) / f64::from(1u32 << 24)
    }
}

fn field<'a>(r: &'a Record, k: &str) -> Result<&'a Json, String> {
    r.field(k).ok_or_else(|| format!("no field {k:?}"))
}

fn text(json: &Json) -> Result<JsString, String> {
    json.as_str()
        .map(|s| JsString::from_units(s.units().to_vec()))
        .ok_or_else(|| format!("{} is not a string", json.to_compact()))
}

fn hex(json: &Json) -> Result<Vec<u8>, String> {
    let s = json
        .as_str()
        .ok_or("hex is not a string")?
        .to_string_lossy();
    if !s.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("{e}")))
        .collect()
}

fn input(r: &Record, blobs: &HashMap<u64, Vec<u8>>) -> Result<Vec<u8>, String> {
    let json = field(r, "input")?;
    if let Some(b) = json.get("bytes") {
        return hex(b);
    }
    let id = json.get("blob").and_then(Json::as_f64).ok_or("blob id")? as u64;
    let blob = blobs.get(&id).ok_or_else(|| format!("no blob {id}"))?;
    let len = match json.get("len") {
        Some(len) => len.as_f64().ok_or("len")? as usize,
        None => blob.len(),
    };
    let mut out = blob.get(..len).ok_or("len past blob")?.to_vec();
    if let Some(flips) = json.get("flips") {
        for flip in flips.as_array().ok_or("flips")? {
            let pair = flip.as_array().ok_or("flip")?;
            let offset = pair[0].as_f64().ok_or("flip offset")? as usize;
            out[offset] ^= pair[1].as_f64().ok_or("flip xor")? as u8;
        }
    }
    Ok(out)
}

struct Expected {
    output: Option<Vec<u8>>,
    error: Option<JsString>,
    uncaught: bool,
}

fn expected(r: &Record) -> Result<Expected, String> {
    let output = match field(r, "output")? {
        Json::Null => None,
        json => Some(hex(json)?),
    };
    let error = match field(r, "error")? {
        Json::Null => None,
        json => Some(text(
            json.as_array()
                .and_then(|pair| pair.get(1))
                .ok_or("error is not [name, message]")?,
        )?),
    };
    let uncaught = field(r, "uncaught")?.as_f64().ok_or("uncaught")? != 0.0;
    Ok(Expected {
        output,
        error,
        uncaught,
    })
}

fn check(outcome: Outcome, uuid: &str, want: &Expected) -> Result<(), String> {
    let reply = outcome.reply.ok_or("main never replied")?;
    if reply.uuid != uuid {
        return Err(format!("reply uuid {:?}", reply.uuid));
    }
    if reply.error != want.error {
        return Err(format!(
            "error {:?}, expected {:?}",
            reply.error.as_ref().map(JsString::to_string_lossy),
            want.error.as_ref().map(JsString::to_string_lossy)
        ));
    }
    let files: Vec<Option<&[u8]>> = reply.files.iter().map(|f| f.as_deref()).collect();
    if files != [want.output.as_deref()] {
        let describe = |f: &Option<&[u8]>| f.map(|b| format!("{} bytes", b.len()));
        return Err(format!(
            "files {:?}, expected [{:?}]{}",
            files.iter().map(describe).collect::<Vec<_>>(),
            describe(&want.output.as_deref()),
            match (files.first(), &want.output) {
                (Some(Some(a)), Some(e)) => format!(
                    ", first difference at byte {}",
                    a.iter()
                        .zip(e)
                        .position(|(x, y)| x != y)
                        .unwrap_or(a.len().min(e.len()))
                ),
                _ => String::new(),
            }
        ));
    }
    if outcome.uncaught_error != want.uncaught {
        return Err(format!(
            "uncaught error {}, expected {}",
            outcome.uncaught_error, want.uncaught
        ));
    }
    Ok(())
}

fn start(r: &Record, blobs: &HashMap<u64, Vec<u8>>, uuid: &str) -> Result<(), String> {
    let path = text(field(r, "path")?)?;
    let fs_path = path.to_string_lossy();
    let mem = MemFs::new();
    match r.string("setup").as_deref() {
        Some("file") => mem.put(&fs_path, &input(r, blobs)?),
        Some("missing") | Some("none") => {}
        Some("directory") => {
            mem.put(&fs_path, &[]);
            mem.set_failure(&fs_path, Some(Failure::ReadOpened));
        }
        Some("unreadable") => {
            mem.put(&fs_path, &input(r, blobs)?);
            mem.set_failure(&fs_path, Some(Failure::Read));
        }
        other => return Err(format!("unknown setup {other:?}")),
    }
    let open = FileOpen {
        fs: &mem,
        temp_dir: TEMP,
        lzma: true,
    };
    let outcome = open.handle(
        MainRequest::HistoricalStart {
            uuid: uuid.to_owned(),
            path: path.clone(),
        },
        &mut Counter(1),
    );
    if outcome.recent_document.as_ref() != Some(&path) {
        return Err("recent document is not the path".into());
    }
    check(outcome, uuid, &expected(r)?)
}

fn start_raw(r: &Record, bytes: Vec<u8>, uuid: &str) -> Result<(), String> {
    let mem = MemFs::new();
    mem.put_dir(TEMP);
    let open = FileOpen {
        fs: &mem,
        temp_dir: TEMP,
        lzma: true,
    };
    let outcome = open.handle(
        MainRequest::HistoricalStartRaw {
            uuid: uuid.to_owned(),
            data: bytes.into(),
            extension: JsString::from("revlog"),
        },
        &mut Counter(7),
    );
    check(outcome, uuid, &expected(r)?)?;
    let left: Vec<String> = mem.dump().into_iter().map(|(name, _)| name).collect();
    if !left.is_empty() {
        return Err(format!("temporary files left behind: {left:?}"));
    }
    Ok(())
}

#[test]
fn historical_start_revlog_vectors() {
    let records = vectors::load(ID).unwrap_or_else(|e| panic!("{e}"));
    let mut blobs: HashMap<u64, Vec<u8>> = HashMap::new();
    let mut failures = Vec::new();
    let (mut cases, mut compared, mut raw) = (0, 0, 0);
    for r in &records {
        match r.string("kind").as_deref() {
            Some("blob") => {
                let id = field(r, "blob")
                    .ok()
                    .and_then(Json::as_f64)
                    .expect("blob id") as u64;
                let bytes = hex(field(r, "bytes").expect("blob bytes")).expect("blob hex");
                blobs.insert(id, bytes);
            }
            Some("case") => {
                cases += 1;
                let name = r.string("name").unwrap_or_default();
                let uuid = format!("case-{cases}");
                match start(r, &blobs, &uuid) {
                    Ok(()) => compared += 1,
                    Err(e) => failures.push(format!("line {} ({name}): {e}", r.line)),
                }
                if r.string("setup").as_deref() == Some("file") {
                    let bytes = input(r, &blobs).expect("case input");
                    if bytes.len() <= RAW_LIMIT {
                        raw += 1;
                        if let Err(e) = start_raw(r, bytes, &uuid) {
                            failures.push(format!("line {} ({name}, raw): {e}", r.line));
                        }
                    }
                }
            }
            other => panic!("line {}: unknown record kind {other:?}", r.line),
        }
    }
    if !failures.is_empty() {
        let shown: Vec<&str> = failures.iter().take(8).map(String::as_str).collect();
        panic!(
            "vectors {ID}: {} of {cases} cases differ\n\n{}",
            failures.len(),
            shown.join("\n\n")
        );
    }
    assert!(cases > 0 && raw > 0, "vectors {ID}: nothing compared");
    assert_eq!(compared, cases, "vectors {ID}: compared != recorded");
}
