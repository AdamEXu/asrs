#![allow(clippy::disallowed_methods)]

//! Replays `video/timer-*` vectors: upstream's VideoProcessor with the real tesseract.js over the
//! frames of `parity/specs/video/timer/*.clip` (format in `oracle/tools/timerClips.mjs`).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc;

use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_ocr::{OcrError, TIMER_RECTS, TimerReader, Worker};
use asrs_parity::cve1::Value;
use asrs_parity::json::{self, Json};
use asrs_parity::vectors::{self, Record};
use asrs_services::video::{
    Handle, LogArg, LogLevel, MessageBox, OpenDialog, VideoConfig, VideoEvent, VideoHost,
    VideoProcessor,
};

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

struct Clip {
    stderr: Vec<Vec<u8>>,
    frames: BTreeMap<u32, Vec<u8>>,
}

fn field<'a>(json: &'a Json, key: &str) -> &'a Json {
    json.get(key)
        .unwrap_or_else(|| panic!("no field {key:?} in {}", json.to_compact()))
}

fn text(json: &Json) -> String {
    json.as_str()
        .and_then(|s| s.as_string())
        .unwrap_or_else(|| panic!("not a string: {}", json.to_compact()))
}

fn number(json: &Json) -> f64 {
    json.as_f64()
        .unwrap_or_else(|| panic!("not a number: {}", json.to_compact()))
}

fn array(json: &Json) -> &[Json] {
    json.as_array()
        .unwrap_or_else(|| panic!("not an array: {}", json.to_compact()))
}

fn tree(json: &Json) -> Value {
    Value::from_json(json).unwrap_or_else(|e| panic!("{}: {e:?}", json.to_compact()))
}

fn tree_number(json: &Json) -> f64 {
    tree(json)
        .as_f64()
        .unwrap_or_else(|| panic!("not a number: {}", json.to_compact()))
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

fn read_clip(name: &str) -> Clip {
    let path = vectors::workspace_root().join(format!("parity/specs/video/timer/{name}.clip"));
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let header_end = 4 + u32::from_le_bytes(data[..4].try_into().expect("4 bytes")) as usize;
    let header = std::str::from_utf8(&data[4..header_end]).expect("UTF-8 header");
    let header = json::parse(header).expect("JSON header");
    let stderr = array(field(&header, "stderr"))
        .iter()
        .map(|chunk| text(chunk).into_bytes())
        .collect();
    let mut at = header_end;
    let mut frames = BTreeMap::new();
    for entry in array(field(&header, "frames")) {
        let [frame, length] = array(entry) else {
            panic!("bad frame entry in {name}");
        };
        let end = at + number(length) as usize;
        frames.insert(number(frame) as u32, data[at..end].to_vec());
        at = end;
    }
    assert_eq!(at, data.len(), "{name}: trailing bytes");
    Clip { stderr, frames }
}

fn outcome(json: &Json) -> Result<String, OcrError> {
    match (json.get("text"), json.get("error")) {
        (Some(value), None) => Ok(text(value)),
        (None, Some(value)) => Err(OcrError(text(value))),
        _ => panic!("neither text nor error: {}", json.to_compact()),
    }
}

fn apply_edit(image: &[u8], edit: &Json) -> Vec<u8> {
    let Json::Object(pairs) = edit else {
        panic!("edit is not an object");
    };
    let [(key, value)] = &pairs[..] else {
        assert!(pairs.is_empty(), "edit with several keys");
        return image.to_vec();
    };
    match key.as_string().as_deref() {
        Some("keep") => image[..number(value) as usize].to_vec(),
        Some("zeros") => vec![0; number(value) as usize],
        Some("insert") => [&image[..2], &hex(&text(value)), &image[2..]].concat(),
        _ => panic!("unknown edit {}", edit.to_compact()),
    }
}

struct RectCase {
    line: usize,
    image: Vec<u8>,
    rectangle: [f64; 4],
    result: Result<String, OcrError>,
    fresh: Result<String, OcrError>,
}

fn run_rect_group(cases: &[RectCase]) -> usize {
    let mut worker = Worker::new().expect("worker");
    for case in cases {
        let RectCase {
            line,
            image,
            rectangle,
            ..
        } = case;
        assert_eq!(
            worker.recognize(image, *rectangle),
            case.result,
            "line {line}, worker with history, {rectangle:?}"
        );
        let mut fresh = Worker::new().expect("worker");
        assert_eq!(
            fresh.recognize(image, *rectangle),
            case.fresh,
            "line {line}, fresh worker, {rectangle:?}"
        );
    }
    cases.len()
}

#[test]
fn rectangles() {
    let records = vectors::load("video/timer-rects").expect("vectors");
    let mut clips: BTreeMap<String, Clip> = BTreeMap::new();
    let mut groups: BTreeMap<u32, Vec<RectCase>> = BTreeMap::new();
    let mut rect_tables = 0;
    let mut expected = 0;
    for record in &records {
        let get = |key: &str| {
            record
                .field(key)
                .unwrap_or_else(|| panic!("line {}: no field {key:?}", record.line))
        };
        match text(get("k")).as_str() {
            "timerRects" => {
                let table: Vec<[f64; 4]> = array(get("v"))
                    .iter()
                    .map(|rect| {
                        let values: Vec<f64> = array(rect).iter().map(tree_number).collect();
                        values.try_into().expect("four numbers")
                    })
                    .collect();
                let port: Vec<[f64; 4]> =
                    TIMER_RECTS.iter().map(|r| r.rectangle(1.0, 1.0)).collect();
                assert_eq!(table, port);
                rect_tables += 1;
            }
            "rect" => {
                let name = text(get("clip"));
                let clip = clips
                    .entry(name.clone())
                    .or_insert_with(|| read_clip(&name));
                let frame = number(get("frame")) as u32;
                let values: Vec<f64> = array(get("rectangle")).iter().map(tree_number).collect();
                groups
                    .entry(number(get("worker")) as u32)
                    .or_default()
                    .push(RectCase {
                        line: record.line,
                        image: apply_edit(&clip.frames[&frame], get("edit")),
                        rectangle: values.try_into().expect("four numbers"),
                        result: outcome(get("result")),
                        fresh: outcome(get("fresh")),
                    });
                expected += 1;
            }
            other => panic!("line {}: unknown record {other:?}", record.line),
        }
    }
    assert_eq!(rect_tables, 1);
    let groups: Vec<Vec<RectCase>> = groups.into_values().collect();
    let threads = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let compared: usize = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                std::thread::Builder::new()
                    .stack_size(8 << 20)
                    .spawn_scoped(scope, || {
                        let mut compared = 0;
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            match groups.get(i) {
                                Some(group) => compared += run_rect_group(group),
                                None => return compared,
                            }
                        }
                    })
                    .expect("thread")
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|e| std::panic::resume_unwind(e))
            })
            .sum()
    });
    assert_eq!(compared, expected);
}

#[derive(Debug, PartialEq)]
enum Output {
    Read { frame: u32, width: f64, height: f64 },
    Data(String),
}

const DIALOG: Handle = 1;
const FFMPEG: Handle = 2;
const FIRST_READ: Handle = 100;

#[derive(Default)]
struct Host {
    random: VecDeque<f64>,
    reads: u64,
    outputs: VecDeque<Output>,
}

impl VideoHost for Host {
    fn exists(&mut self, path: &str) -> bool {
        path == "/user/ffmpeg"
    }
    fn rm_recursive(&mut self, path: &str) -> Result<(), JsError> {
        panic!("rm {path}");
    }
    fn mkdir_recursive(&mut self, _path: &str) -> Result<(), JsError> {
        Ok(())
    }
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, JsError> {
        panic!("read {path}");
    }
    fn read_file_sync(&mut self, path: &str) -> Result<Vec<u8>, JsError> {
        panic!("read {path}");
    }
    fn unlink(&mut self, path: &str) -> Result<(), JsError> {
        panic!("unlink {path}");
    }
    fn rename(&mut self, from: &str, _to: &str) -> Result<(), JsError> {
        panic!("rename {from}");
    }
    fn chmod(&mut self, path: &str, _mode: u32) -> Result<(), JsError> {
        panic!("chmod {path}");
    }
    fn sha256_hex(&mut self, _data: &[u8]) -> String {
        panic!("sha256");
    }
    fn random(&mut self) -> f64 {
        self.random.pop_front().expect("recorded random numbers")
    }
    fn read_clipboard(&mut self) -> JsString {
        panic!("clipboard");
    }
    fn window_destroyed(&mut self) -> bool {
        panic!("isDestroyed");
    }
    fn spawn(
        &mut self,
        command: &str,
        _args: &[JsString],
        _cwd: Option<&str>,
    ) -> Result<Handle, JsError> {
        assert_eq!(command, "/user/ffmpeg");
        Ok(FFMPEG)
    }
    fn kill(&mut self, _process: Handle) -> bool {
        panic!("kill");
    }
    fn message_box(&mut self, options: &MessageBox) -> Handle {
        panic!("message box {options:?}");
    }
    fn open_dialog(&mut self, _options: &OpenDialog) -> Handle {
        DIALOG
    }
    fn download(&mut self, url: &str, _folder: &str, _filename: &str) -> Handle {
        panic!("download {url}");
    }
    fn fetch(&mut self, _: &JsString, _: &[(JsString, JsValue)], _: f64) -> Handle {
        panic!("fetch");
    }
    fn popup_menu(&mut self, _labels: &[JsString], _x: f64, _y: f64) -> Handle {
        panic!("menu");
    }
    fn set_timeout(&mut self, _ms: f64) -> Handle {
        panic!("setTimeout");
    }
    fn read_timer_text(&mut self, path: &JsString, width: f64, height: f64) -> Handle {
        let path = path.to_string_lossy();
        let frame = path[path.len() - 12..]
            .strip_suffix(".jpg")
            .and_then(|digits| digits.parse().ok())
            .unwrap_or_else(|| panic!("not a frame path: {path}"));
        self.outputs.push_back(Output::Read {
            frame,
            width,
            height,
        });
        self.reads += 1;
        FIRST_READ + self.reads - 1
    }
    fn log(&mut self, _level: LogLevel, args: &[LogArg]) {
        panic!("log {args:?}");
    }
    fn send(&mut self, data: JsValue) {
        self.outputs
            .push_back(Output::Data(js_json::from_js(&data).encode()));
    }
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn check_clip(name: &str, auto_end_frame: f64) {
    let records = vectors::load(&format!("video/timer-{name}")).expect("vectors");
    let clip = read_clip(name);
    let dir = TempDir(std::env::temp_dir().join(format!("asrs-ocr-{name}-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).expect("temp dir");
    for (frame, data) in &clip.frames {
        std::fs::write(dir.0.join(format!("{frame:08}.jpg")), data).expect("frame");
    }
    let reader = TimerReader::new();
    let mut rect_worker = Worker::new().expect("worker");

    let get = |record: &Record, key: &str| -> Json {
        record
            .field(key)
            .unwrap_or_else(|| panic!("{name} line {}: no field {key:?}", record.line))
            .clone()
    };
    let mut host = Host::default();
    let mut processor = VideoProcessor::new(VideoConfig {
        platform: "darwin".to_owned(),
        arch: "arm64".to_owned(),
        user_data: "/user".to_owned(),
        temp: "/var/folders/xy/4k2qz8_n1x3c0000gn/T".to_owned(),
        yt_dlp_binary: "/yt-dlp".to_owned(),
        window_icon: None,
    });
    let mut reads: Vec<Output> = Vec::new();
    let mut texts = 0;
    let mut chunks = clip.stderr.iter();
    let mut last_data = String::new();
    for record in &records {
        let context = format!("{name} line {}", record.line);
        let kind = text(&get(record, "k"));
        if !matches!(kind.as_str(), "read" | "data") {
            assert_eq!(host.outputs, [], "{context}: unrecorded output");
        }
        match kind.as_str() {
            "clip" => {
                assert_eq!(text(&get(record, "name")), name);
                host.random = array(&get(record, "random"))
                    .iter()
                    .map(tree_number)
                    .collect();
                processor.prepare(
                    &mut host,
                    &JsString::from("timer"),
                    &JsValue::Number(0.0),
                    &JsValue::Null,
                    &JsValue::Null,
                );
            }
            "openDialog" => processor.handle(
                &mut host,
                VideoEvent::OpenDialogClosed {
                    dialog: DIALOG,
                    canceled: false,
                    file_paths: vec!["/Users/frc/Movies/match.mp4".to_owned()],
                },
            ),
            "stderr" => {
                let data = hex(&text(&get(record, "data")));
                assert_eq!(Some(&data), chunks.next(), "{context}");
                processor.handle(
                    &mut host,
                    VideoEvent::Stderr {
                        process: FFMPEG,
                        data,
                    },
                );
            }
            "exit" | "close" => {
                let code = Some(number(&get(record, "code")) as i32);
                let event = if kind == "exit" {
                    VideoEvent::Exit {
                        process: FFMPEG,
                        code,
                        signal: None,
                    }
                } else {
                    VideoEvent::Close {
                        process: FFMPEG,
                        code,
                        signal: None,
                    }
                };
                processor.handle(&mut host, event);
            }
            "read" => {
                let expected = Output::Read {
                    frame: number(&get(record, "frame")) as u32,
                    width: tree_number(&get(record, "width")),
                    height: tree_number(&get(record, "height")),
                };
                assert_eq!(
                    number(&get(record, "id")) as usize,
                    reads.len(),
                    "{context}"
                );
                assert_eq!(
                    host.outputs.pop_front().as_ref(),
                    Some(&expected),
                    "{context}"
                );
                reads.push(expected);
            }
            "data" => {
                let expected = tree(&get(record, "v")).encode();
                assert_eq!(
                    host.outputs.pop_front(),
                    Some(Output::Data(expected.clone())),
                    "{context}"
                );
                last_data = expected;
            }
            "text" => {
                let id = number(&get(record, "id")) as usize;
                let Output::Read {
                    frame,
                    width,
                    height,
                } = reads[id]
                else {
                    unreachable!("reads only holds reads");
                };
                let rects: Vec<String> = array(&get(record, "rects")).iter().map(text).collect();
                let port: Vec<String> = TIMER_RECTS
                    .iter()
                    .map(|rect| {
                        rect_worker
                            .recognize(&clip.frames[&frame], rect.rectangle(width, height))
                            .expect("text")
                    })
                    .collect();
                assert_eq!(port, rects, "{context}: frame {frame}");
                let (sender, receiver) = mpsc::channel();
                reader.read_timer_text(
                    dir.0.join(format!("{frame:08}.jpg")),
                    width,
                    height,
                    move |result| sender.send(result).expect("receiver is alive"),
                );
                let port = receiver.recv().expect("a result").expect("text");
                assert_eq!(port, text(&get(record, "text")), "{context}: frame {frame}");
                processor.handle(
                    &mut host,
                    VideoEvent::TimerText {
                        job: FIRST_READ + id as Handle,
                        text: JsString::from(port.as_str()),
                    },
                );
                texts += 1;
            }
            other => panic!("{context}: unknown record {other:?}"),
        }
    }
    assert_eq!(host.outputs, []);
    assert_eq!(chunks.next(), None, "{name}: stderr left");
    assert!(texts > 0 && texts == reads.len(), "{name}: {texts} texts");
    let found = Value::decode(&last_data).expect("CVE-1");
    assert_eq!(
        found.get("autoEndFrame").and_then(Value::as_f64),
        Some(auto_end_frame),
        "{name}"
    );
}

#[test]
fn banner_lower_2025() {
    check_clip("banner-lower-2025", 493.0);
}

#[test]
fn small_2023_at_29_97_fps() {
    check_clip("small-2023-2997", 493.0);
}

#[test]
fn web_lower_2025_at_a_low_bitrate() {
    check_clip("web-lower-2025-lowbitrate", 411.0);
}

#[test]
fn banner_lower_in_a_720p_source() {
    check_clip("banner-lower-720p", 193.0);
}

#[test]
fn queued_reads_interleave_and_a_failed_one_costs_only_itself() {
    let name = "banner-lower-720p";
    let clip = read_clip(name);
    let dir = TempDir(std::env::temp_dir().join(format!("asrs-ocr-queue-{}", std::process::id())));
    std::fs::create_dir_all(&dir.0).expect("temp dir");
    for (frame, data) in &clip.frames {
        std::fs::write(dir.0.join(format!("{frame:08}.jpg")), data).expect("frame");
    }
    let damaged = dir.0.join("damaged.jpg");
    std::fs::write(&damaged, [0; 1000]).expect("frame");

    let mut reads: BTreeMap<u64, (u32, f64, f64)> = BTreeMap::new();
    let mut expected: BTreeMap<u64, Option<String>> = BTreeMap::new();
    for record in vectors::load(&format!("video/timer-{name}")).expect("vectors") {
        let get = |key: &str| record.field(key).expect("a field").clone();
        match text(&get("k")).as_str() {
            "read" => {
                reads.insert(
                    number(&get("id")) as u64,
                    (
                        number(&get("frame")) as u32,
                        tree_number(&get("width")),
                        tree_number(&get("height")),
                    ),
                );
            }
            "text" => {
                expected.insert(number(&get("id")) as u64, Some(text(&get("text"))));
            }
            _ => {}
        }
    }
    assert!(reads.len() > 8 && reads.len() == expected.len());

    let reader = TimerReader::new();
    let (sender, receiver) = mpsc::channel();
    let submit = |id: u64, path: PathBuf, width: f64, height: f64| {
        let sender = sender.clone();
        reader.read_timer_text(path, width, height, move |result| {
            sender.send((id, result.ok())).expect("receiver is alive");
        });
    };
    for (id, (frame, width, height)) in &reads {
        submit(*id, dir.0.join(format!("{frame:08}.jpg")), *width, *height);
        submit(1000 + id, damaged.clone(), *width, *height);
        submit(2000 + id, dir.0.join("missing.jpg"), *width, *height);
        expected.insert(1000 + id, None);
        expected.insert(2000 + id, None);
    }
    let results: BTreeMap<u64, Option<String>> = receiver.iter().take(expected.len()).collect();
    assert_eq!(results, expected);
}

#[test]
fn a_missing_frame_is_an_error() {
    let reader = TimerReader::new();
    let (sender, receiver) = mpsc::channel();
    reader.read_timer_text("/nonexistent/00000001.jpg", 1920.0, 1080.0, move |result| {
        sender.send(result).expect("receiver is alive");
    });
    assert!(receiver.recv().expect("a result").is_err());
}
