#![allow(clippy::disallowed_methods)]

//! Replays `files/asset-download/<zone>` (upstream's real assetDownloader, github constants and
//! `download` package against a scripted GitHub, a fake clock and a temp folder) through
//! `asrs_services::asset_downloader` on a `MemFs`, comparing every recorded field after every op.

use std::collections::{BTreeMap, HashMap, VecDeque};

use asrs_js::time::NamedZone;
use asrs_parity::vectors::check_records;
use asrs_parity::{Json, Record, Utf16String};
use asrs_services::asset_downloader::{
    AssetDownloader, Distribution, DownloadHost, FetchId, FetchOutcome, FetchRequest, HttpOutcome,
    HttpRequest, Rejected, RequestId, Timer,
};
use asrs_services::asset_loader::AssetFolders;
use asrs_services::fs::MemFs;
use asrs_services::path::PathStyle;
use sha2::{Digest, Sha256};

const USER_DATA: &str = "/fake/userData";
const BUNDLED: &str = "/fake/bundledAssets";
const INLINE_LIMIT: usize = 4096;
const OUTPUT_FIELDS: [&str; 8] = [
    "status",
    "timers",
    "fetches",
    "requests",
    "disk",
    "callbacks",
    "files",
    "dirs",
];

fn field<'a>(json: &'a Json, key: &str) -> Result<&'a Json, String> {
    json.get(key).ok_or_else(|| format!("no field {key:?}"))
}

fn text(json: &Json) -> Result<String, String> {
    json.as_str()
        .map(Utf16String::to_string_lossy)
        .ok_or_else(|| format!("{json:?} is not a string"))
}

fn number(json: &Json) -> Result<f64, String> {
    json.as_f64()
        .ok_or_else(|| format!("{json:?} is not a number"))
}

fn array(json: &Json) -> Result<&[Json], String> {
    json.as_array()
        .ok_or_else(|| format!("{json:?} is not an array"))
}

fn pairs(json: &Json) -> Result<Vec<(String, String)>, String> {
    let Json::Object(entries) = json else {
        return Err(format!("{json:?} is not an object"));
    };
    entries
        .iter()
        .map(|(k, v)| Ok((k.to_string_lossy(), text(v)?)))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `hold` in a scripted outcome.
fn held(args: Option<&Json>) -> Result<bool, String> {
    match args.and_then(|a| a.get("hold")) {
        None => Ok(false),
        Some(Json::Bool(hold)) => Ok(*hold),
        Some(other) => Err(format!("hold {other:?} is not a bool")),
    }
}

fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) {
        return Err(format!("odd hex length {}", text.len()));
    }
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn hex_bytes(json: &Json) -> Result<Vec<u8>, String> {
    let Json::Object(entries) = json else {
        return Err(format!("{json:?} is not a byte value"));
    };
    if entries.len() != 1 {
        return Err(format!("byte value has keys other than hex: {json:?}"));
    }
    unhex(&text(field(json, "hex")?)?)
}

fn file_json(bytes: &[u8]) -> Vec<(&'static str, String)> {
    if bytes.len() <= INLINE_LIMIT {
        vec![("hex", hex(bytes))]
    } else {
        vec![
            ("len", bytes.len().to_string()),
            ("sha256", hex(&Sha256::digest(bytes))),
        ]
    }
}

fn parent(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => ".",
    }
}

/// `fs.mkdirSync(name, { recursive: true })` as the oracle's putDir and putFile do it.
fn put_dir_all(fs: &MemFs, path: &str) {
    if path == "/" || path == "." || fs.dirs().iter().any(|d| d == path) {
        return;
    }
    put_dir_all(fs, parent(path));
    fs.put_dir(path);
}

#[derive(Default)]
struct Host {
    fetches: VecDeque<(FetchOutcome, bool)>,
    serves: HashMap<String, VecDeque<(HttpOutcome, bool)>>,
    held_fetch: Option<(FetchId, FetchOutcome)>,
    held: HashMap<String, Vec<(RequestId, HttpOutcome)>>,
    free: Option<f64>,
    callback_fails: bool,
    random: f64,
    fetch_log: Vec<FetchRequest>,
    request_log: Vec<HttpRequest>,
    disk_log: Vec<String>,
    callbacks: usize,
}

impl DownloadHost for Host {
    fn free_space(&mut self, path: &str) -> Result<f64, Rejected> {
        self.disk_log.push(path.to_owned());
        self.free.ok_or(Rejected)
    }

    fn fetch(&mut self, id: FetchId, request: &FetchRequest) -> FetchOutcome {
        self.fetch_log.push(request.clone());
        match self.fetches.pop_front() {
            None => FetchOutcome::NetworkError,
            Some((outcome, false)) => outcome,
            Some((outcome, true)) => {
                self.held_fetch = Some((id, outcome));
                FetchOutcome::Hang
            }
        }
    }

    fn request(&mut self, id: RequestId, request: &HttpRequest) -> HttpOutcome {
        self.request_log.push(request.clone());
        let queue = self.serves.get_mut(&request.url);
        match queue.and_then(VecDeque::pop_front) {
            None => HttpOutcome::Response {
                status: 404,
                headers: Vec::new(),
                body: Vec::new(),
            },
            Some((outcome, false)) => outcome,
            Some((outcome, true)) => {
                let held = self.held.entry(request.url.clone()).or_default();
                held.push((id, outcome));
                HttpOutcome::Hang
            }
        }
    }

    fn random(&mut self) -> f64 {
        self.random
    }

    fn assets_updated(&mut self) -> Result<(), Rejected> {
        self.callbacks += 1;
        if self.callback_fails {
            Err(Rejected)
        } else {
            Ok(())
        }
    }
}

struct Case {
    fs: MemFs,
    downloader: AssetDownloader,
    host: Host,
    now: f64,
}

struct Replay {
    zone: NamedZone,
    case: Option<Case>,
    ops: BTreeMap<String, usize>,
    fields: usize,
}

fn folders() -> AssetFolders {
    AssetFolders::new(PathStyle::Posix, USER_DATA, BUNDLED)
}

impl Replay {
    fn step(&mut self, record: &Record) -> Result<(), String> {
        let op = record.string("op").ok_or("record has no op")?;
        let Json::Object(keys) = &record.json else {
            return Err("record is not an object".to_owned());
        };
        for (key, _) in keys {
            let key = key.to_string_lossy();
            if !["case", "step", "op", "args"].contains(&key.as_str())
                && !OUTPUT_FIELDS.contains(&key.as_str())
            {
                return Err(format!("unknown record field {key:?}"));
            }
        }
        let args = record.field("args");
        let arg = |key: &str| -> Result<&Json, String> { field(args.ok_or("no args")?, key) };
        if op == "reset" {
            let distribution = match text(arg("distribution")?)?.as_str() {
                "FRC6328" => Distribution::Frc6328,
                "WPILib" => Distribution::WpiLib,
                other => return Err(format!("distribution {other} is not modeled")),
            };
            let fs = MemFs::new();
            fs.put_dir(USER_DATA);
            self.case = Some(Case {
                fs,
                downloader: AssetDownloader::new(&folders(), distribution),
                host: Host {
                    free: Some(1e12),
                    random: number(arg("random")?)?,
                    ..Host::default()
                },
                now: number(arg("now")?)?,
            });
        } else {
            let zone = &self.zone;
            let case = self.case.as_mut().ok_or("op before reset")?;
            match op.as_str() {
                "putDir" => put_dir_all(&case.fs, &text(arg("name")?)?),
                "putFile" => {
                    let name = text(arg("name")?)?;
                    put_dir_all(&case.fs, parent(&name));
                    case.fs.put(&name, &hex_bytes(arg("data")?)?);
                }
                "queueFetch" => {
                    let outcome = if args.and_then(|a| a.get("network")).is_some() {
                        FetchOutcome::NetworkError
                    } else if args.and_then(|a| a.get("hang")).is_some() {
                        FetchOutcome::Hang
                    } else {
                        FetchOutcome::Response {
                            status: number(arg("status")?)? as u16,
                            body: match arg("body")? {
                                Json::Null => None,
                                body => Some(hex_bytes(body)?),
                            },
                        }
                    };
                    case.host.fetches.push_back((outcome, held(args)?));
                }
                "serve" => {
                    let url = text(arg("url")?)?;
                    let outcome = if args.and_then(|a| a.get("reset")).is_some() {
                        HttpOutcome::Reset
                    } else if args.and_then(|a| a.get("hang")).is_some() {
                        HttpOutcome::Hang
                    } else if let Some(code) = args.and_then(|a| a.get("error")) {
                        HttpOutcome::Error(text(code)?)
                    } else {
                        let status = number(arg("status")?)? as u16;
                        let headers = pairs(arg("headers")?)?;
                        let body = hex_bytes(arg("body")?)?;
                        match args.and_then(|a| a.get("cut")).map(text).transpose()? {
                            None => HttpOutcome::Response {
                                status,
                                headers,
                                body,
                            },
                            Some(cut) => HttpOutcome::Truncated {
                                status,
                                headers,
                                body,
                                code: match cut.as_str() {
                                    "fin" => None,
                                    "rst" => Some("ECONNRESET".to_owned()),
                                    other => return Err(format!("cut {other} is not modeled")),
                                },
                            },
                        }
                    };
                    let queue = case.host.serves.entry(url).or_default();
                    queue.push_back((outcome, held(args)?));
                }
                "release" => {
                    let url = text(arg("url")?)?;
                    let waiting = case.host.held.get_mut(&url).ok_or("nothing is held")?;
                    if waiting.is_empty() {
                        return Err(format!("nothing holds {url}"));
                    }
                    let older = args.and_then(|a| a.get("older")).is_some();
                    let at = if older { 0 } else { waiting.len() - 1 };
                    let (id, outcome) = waiting.remove(at);
                    case.downloader
                        .answered(id, outcome, &case.fs, &mut case.host);
                }
                "releaseFetch" => {
                    let (id, outcome) = case.host.held_fetch.take().ok_or("no fetch is held")?;
                    case.downloader
                        .fetched(id, outcome, &case.fs, &mut case.host, zone);
                }
                "setDisk" => {
                    case.host.free = match arg("free")? {
                        Json::Null => Some(f64::NAN),
                        Json::Number(n) => Some(*n),
                        other if text(other)? == "throw" => None,
                        other => return Err(format!("free {other:?} is not modeled")),
                    };
                }
                "setCallback" => {
                    case.host.callback_fails = match arg("fails")? {
                        Json::Bool(b) => *b,
                        other => return Err(format!("fails {other:?} is not a bool")),
                    };
                }
                "start" => case
                    .downloader
                    .start(case.now, &case.fs, &mut case.host, zone),
                "status" => {}
                "advance" => {
                    let to = number(arg("to")?)?;
                    case.downloader.advance(to, &case.fs, &mut case.host, zone);
                    case.now = to;
                }
                other => return Err(format!("op {other} is not modeled")),
            }
        }
        let case = self.case.as_mut().ok_or("no case")?;
        let compared = compare(record, case)?;
        if compared != OUTPUT_FIELDS.len() {
            return Err(format!(
                "compared {compared} fields, recorded {}",
                OUTPUT_FIELDS.len()
            ));
        }
        self.fields += compared;
        *self.ops.entry(op).or_default() += 1;
        Ok(())
    }
}

fn mismatch(name: &str, expected: impl std::fmt::Debug, actual: impl std::fmt::Debug) -> String {
    format!("{name}: oracle {expected:?}\n port {actual:?}")
}

fn compare(record: &Record, case: &mut Case) -> Result<usize, String> {
    let json = &record.json;
    let mut compared = 0;
    let mut errors = Vec::new();
    let mut check = |name: &str, result: Result<(), String>| {
        compared += 1;
        if let Err(e) = result {
            errors.push(format!("{name}: {e}"));
        }
    };

    check("status", {
        let expected = text(field(json, "status")?)?;
        let actual = case.downloader.status().to_owned();
        if expected == actual {
            Ok(())
        } else {
            Err(mismatch("text", expected, actual))
        }
    });

    check("timers", {
        let expected = array(field(json, "timers")?)?
            .iter()
            .map(|t| Ok((number(field(t, "due")?)?, number(field(t, "delay")?)?)))
            .collect::<Result<Vec<_>, String>>()?;
        let mut actual: Vec<(f64, f64)> = case
            .downloader
            .timers()
            .into_iter()
            .map(|Timer { due_ms, delay_ms }| (due_ms, delay_ms))
            .collect();
        actual.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        let same = expected.len() == actual.len()
            && expected
                .iter()
                .zip(&actual)
                .all(|(e, a)| e.0.to_bits() == a.0.to_bits() && e.1.to_bits() == a.1.to_bits());
        if same {
            Ok(())
        } else {
            Err(mismatch("(due, delay)", expected, actual))
        }
    });

    check("fetches", {
        let expected = array(field(json, "fetches")?)?
            .iter()
            .map(|f| {
                Ok(FetchRequest {
                    url: text(field(f, "url")?)?,
                    method: text(field(f, "method")?)?,
                    headers: pairs(field(f, "headers")?)?,
                    timeout_ms: number(field(f, "timeoutMs")?)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let actual = std::mem::take(&mut case.host.fetch_log);
        if expected == actual {
            Ok(())
        } else {
            Err(mismatch("calls", expected, actual))
        }
    });

    check("requests", {
        let expected = array(field(json, "requests")?)?
            .iter()
            .map(|r| {
                Ok(HttpRequest {
                    url: text(field(r, "url")?)?,
                    method: text(field(r, "method")?)?,
                    headers: pairs(field(r, "headers")?)?,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        let mut actual = std::mem::take(&mut case.host.request_log);
        actual.sort_by(|a, b| a.url.cmp(&b.url));
        if expected == actual {
            Ok(())
        } else {
            Err(mismatch("requests", expected, actual))
        }
    });

    check("disk", {
        let expected = array(field(json, "disk")?)?
            .iter()
            .map(text)
            .collect::<Result<Vec<_>, String>>()?;
        let actual = std::mem::take(&mut case.host.disk_log);
        if expected == actual {
            Ok(())
        } else {
            Err(mismatch("checkDiskSpace paths", expected, actual))
        }
    });

    check("callbacks", {
        let expected = number(field(json, "callbacks")?)? as usize;
        let actual = std::mem::take(&mut case.host.callbacks);
        if expected == actual {
            Ok(())
        } else {
            Err(mismatch("count", expected, actual))
        }
    });

    check("files", {
        let mut expected = Vec::new();
        for file in array(field(json, "files")?)? {
            let name = text(field(file, "name")?)?;
            let value = match file.get("len") {
                Some(len) => vec![
                    ("len", (number(len)? as usize).to_string()),
                    ("sha256", text(field(file, "sha256")?)?),
                ],
                None => vec![("hex", text(field(file, "hex")?)?)],
            };
            expected.push((name, value));
        }
        expected.sort();
        let mut actual: Vec<_> = case
            .fs
            .dump()
            .into_iter()
            .map(|(name, bytes)| (name, file_json(&bytes)))
            .collect();
        actual.sort();
        if expected == actual {
            Ok(())
        } else {
            Err(mismatch("tree", expected, actual))
        }
    });

    check("dirs", {
        let mut expected = array(field(json, "dirs")?)?
            .iter()
            .map(text)
            .collect::<Result<Vec<_>, String>>()?;
        expected.sort();
        let mut actual = case.fs.dirs();
        actual.sort();
        if expected == actual {
            Ok(())
        } else {
            Err(mismatch("tree", expected, actual))
        }
    });

    if errors.is_empty() {
        Ok(compared)
    } else {
        Err(errors.join("\n"))
    }
}

fn replay(zone_name: &str) {
    let id = format!("files/asset-download/{zone_name}");
    let records = asrs_parity::vectors::load(&id).unwrap_or_else(|e| panic!("{e}"));
    let mut recorded: BTreeMap<String, usize> = BTreeMap::new();
    for record in &records {
        *recorded
            .entry(record.string("op").unwrap_or_default())
            .or_default() += 1;
    }
    let mut replay = Replay {
        zone: NamedZone::get(zone_name).expect("zone resolves"),
        case: None,
        ops: BTreeMap::new(),
        fields: 0,
    };
    check_records(&id, |record| replay.step(record));
    assert_eq!(
        replay.ops, recorded,
        "compared ops differ from recorded ops"
    );
    assert_eq!(
        replay.fields,
        records.len() * OUTPUT_FIELDS.len(),
        "compared fields differ from recorded fields"
    );
    for op in [
        "start",
        "advance",
        "status",
        "serve",
        "queueFetch",
        "release",
        "releaseFetch",
    ] {
        assert!(
            recorded.get(op).copied().unwrap_or(0) > 0,
            "no {op} records"
        );
    }
}

#[test]
fn asset_download_utc() {
    replay("UTC");
}

#[test]
fn asset_download_los_angeles() {
    replay("America/Los_Angeles");
}

#[test]
fn asset_download_kolkata() {
    replay("Asia/Kolkata");
}

#[test]
fn asset_download_auckland() {
    replay("Pacific/Auckland");
}
