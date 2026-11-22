//! Main's `historical-start` and `historical-start-raw` handlers from `main/electron/main.ts`:
//! the files a historical source asks for, read (and for `.wpilogxz`, decompressed) into the
//! `historical-data` reply, plus the recent-document and `akit-log-path.txt` side effects.

use std::sync::Arc;

use asrs_formats::historical::{MainReply, MainRequest};
use asrs_formats::{revlog, wpilogxz};
use asrs_js::string::JsString;
use asrs_js::time::{Rng, create_uuid};

use crate::fs::{Fs, ReadError};

pub const XZ_UNSUPPORTED: &str =
    "Compressed WPILOG files (.wpilogxz) are not supported on this platform.";
pub const HOOT_UNSUPPORTED: &str = "Hoot log files are not supported in this build.";

/// Whether upstream's `require("lzma-native")` succeeds here: the package ships prebuilt
/// binaries for macOS, Windows x64 and ia32 and Linux x64 and arm64 only, and main reports
/// `.wpilogxz` files as unsupported everywhere else.
pub const UPSTREAM_HAS_LZMA: bool = !((cfg!(windows) && cfg!(target_arch = "aarch64"))
    || (cfg!(target_os = "linux") && cfg!(target_arch = "arm")));

pub struct FileOpen<'a> {
    pub fs: &'a dyn Fs,
    /// `app.getPath("temp")`.
    pub temp_dir: &'a str,
    pub lzma: bool,
}

/// What main did for one request.
#[derive(Default)]
pub struct Outcome {
    /// The `historical-data` reply. `None` where upstream never answers: a path holding a NUL
    /// makes node's `fs` throw inside the async handler, and main swallows the rejection.
    pub reply: Option<MainReply>,
    /// The path for `app.addRecentDocument`.
    pub recent_document: Option<JsString>,
    /// The handler raised an uncaught exception, which Electron reports in its main-process
    /// error dialog: an unreadable but openable `.wpilogxz` (such as a directory) reaches
    /// `Readable.from(undefined)` after the reply, and a `.revlog` that cannot be opened fails
    /// the converter's read stream before anything listens for its `error` event.
    pub uncaught_error: bool,
}

fn join(dir: &str, name: &str) -> String {
    format!("{dir}{}{name}", std::path::MAIN_SEPARATOR)
}

/// `path.join(app.getPath("temp"), "akit-log-path.txt")`.
pub fn akit_path_output(temp_dir: &str) -> String {
    join(temp_dir, "akit-log-path.txt")
}

/// `p.slice(0, p.length - 5) + "dsevents"` for a path ending in `.dslog`.
fn dsevents_path(dslog: &str) -> String {
    format!("{}dsevents", &dslog[..dslog.len() - "dslog".len()])
}

fn has_nul(path: &JsString) -> bool {
    path.units().contains(&0)
}

fn ends_with(path: &JsString, suffix: &str) -> bool {
    path.ends_with(&JsString::from(suffix), None)
}

fn reply(uuid: &str, files: Vec<Option<Vec<u8>>>, error: Option<JsString>) -> MainReply {
    MainReply {
        uuid: uuid.to_owned(),
        files: files.into_iter().map(|f| f.map(Arc::from)).collect(),
        error,
    }
}

const META: [&str; 32] = [
    "\\x00", "\\x01", "\\x02", "\\x03", "\\x04", "\\x05", "\\x06", "\\x07", "\\b", "\\t", "\\n",
    "\\x0B", "\\f", "\\r", "\\x0E", "\\x0F", "\\x10", "\\x11", "\\x12", "\\x13", "\\x14", "\\x15",
    "\\x16", "\\x17", "\\x18", "\\x19", "\\x1A", "\\x1B", "\\x1C", "\\x1D", "\\x1E", "\\x1F",
];

/// node's `util.inspect` of a string (`strEscape` in `lib/internal/util/inspect.js`).
fn inspect_string(s: &JsString) -> JsString {
    let has = |text: &str| s.includes(&JsString::from(text), None);
    let quote = if !has("'") {
        '\''
    } else if !has("\"") {
        '"'
    } else if !has("`") && !has("${") {
        '`'
    } else {
        '\''
    };
    let units = s.units();
    let mut out = JsString::from(quote.to_string().as_str());
    let mut i = 0;
    while i < units.len() {
        let unit = units[i];
        match unit {
            0x00..=0x1f => out.push_str(&JsString::from(META[unit as usize])),
            0x7f..=0x9f => out.push_str(&JsString::from(format!("\\x{unit:02X}").as_str())),
            0x5c => out.push_str(&JsString::from("\\\\")),
            0x27 if quote == '\'' => out.push_str(&JsString::from("\\'")),
            0xd800..=0xdbff if matches!(units.get(i + 1), Some(0xdc00..=0xdfff)) => {
                out.push_str(&JsString::from_units(units[i..i + 2].to_vec()));
                i += 1;
            }
            0xd800..=0xdfff => out.push_str(&JsString::from(format!("\\u{unit:x}").as_str())),
            _ => out.push_str(&JsString::from_units(vec![unit])),
        }
        i += 1;
    }
    out.push_str(&JsString::from(quote.to_string().as_str()));
    out
}

/// The message of node's `ERR_INVALID_ARG_VALUE` for a path holding a NUL.
fn nul_path_message(path: &JsString) -> JsString {
    let mut inspected = inspect_string(path);
    if inspected.units().len() > 128 {
        inspected = JsString::from_units(inspected.units()[..128].to_vec());
        inspected.push_str(&JsString::from("..."));
    }
    let mut message = JsString::from(
        "The argument 'path' must be a string, Uint8Array, or URL without null bytes. Received ",
    );
    message.push_str(&inspected);
    message
}

impl FileOpen<'_> {
    pub fn handle(&self, request: MainRequest, rng: &mut dyn Rng) -> Outcome {
        match request {
            MainRequest::HistoricalStart { uuid, path } => self.start(&uuid, &path, false),
            MainRequest::HistoricalStartRaw {
                uuid,
                data,
                extension,
            } => self.start_raw(&uuid, &data, &extension, rng),
        }
    }

    fn start_raw(
        &self,
        uuid: &str,
        data: &[u8],
        extension: &JsString,
        rng: &mut dyn Rng,
    ) -> Outcome {
        let answered = |reply| Outcome {
            reply: Some(reply),
            ..Outcome::default()
        };
        if ["/", "\\", ".."]
            .iter()
            .any(|s| extension.includes(&JsString::from(*s), None))
        {
            return answered(reply(
                uuid,
                vec![None],
                Some(JsString::from("Invalid log extension")),
            ));
        }
        let mut name = JsString::from(format!("child_{}.", create_uuid(rng)).as_str());
        name.push_str(extension);
        let temp_path = JsString::from(join(self.temp_dir, &name.to_string_lossy()).as_str());
        if has_nul(&temp_path) {
            return Outcome::default();
        }
        match self.fs.write(&temp_path.to_string_lossy(), data) {
            Ok(()) => self.start(uuid, &temp_path, true),
            Err(e) => answered(reply(
                uuid,
                vec![None],
                Some(JsString::from(e.message.as_str())),
            )),
        }
    }

    fn start(&self, uuid: &str, path: &JsString, delete_after_read: bool) -> Outcome {
        let fs_path = path.to_string_lossy();
        let mut outcome = Outcome::default();
        if !delete_after_read {
            outcome.recent_document = Some(path.clone());
            let _ = self
                .fs
                .write(&akit_path_output(self.temp_dir), fs_path.as_bytes());
        }

        let read = |p: &str| self.fs.read_file(p).ok();
        let mut error: Option<JsString> = None;
        let files = if ends_with(path, ".dslog") {
            if has_nul(path) {
                return outcome;
            }
            vec![read(&fs_path), read(&dsevents_path(&fs_path))]
        } else if ends_with(path, ".hoot") {
            error = Some(JsString::from(HOOT_UNSUPPORTED));
            vec![None]
        } else if ends_with(path, ".revlog") {
            if has_nul(path) {
                error = Some(nul_path_message(path));
                vec![None]
            } else {
                let read = match self.fs.read_file(&fs_path) {
                    Ok(bytes) => revlog::convert(&bytes).map_err(|e| e.message),
                    Err(ReadError::Open(e)) => {
                        outcome.uncaught_error = true;
                        Err(e.message)
                    }
                    Err(ReadError::Read(e)) => Err(e.message),
                };
                match read {
                    Ok(wpilog) => vec![Some(wpilog)],
                    Err(message) => {
                        error = Some(JsString::from(message.as_str()));
                        vec![None]
                    }
                }
            }
        } else if ends_with(path, ".wpilogxz") {
            if !self.lzma {
                error = Some(JsString::from(XZ_UNSUPPORTED));
                vec![None]
            } else if has_nul(path) {
                return outcome;
            } else {
                match self.fs.read_file(&fs_path) {
                    Ok(compressed) => vec![Some(wpilogxz::decompress(&compressed))],
                    Err(ReadError::Open(_)) => vec![None],
                    Err(ReadError::Read(_)) => {
                        outcome.uncaught_error = true;
                        vec![None]
                    }
                }
            }
        } else {
            if has_nul(path) {
                return outcome;
            }
            vec![read(&fs_path)]
        };

        if delete_after_read {
            let mut remove = vec![fs_path.clone()];
            if ends_with(path, ".dslog") {
                let events = dsevents_path(&fs_path);
                if self.fs.exists(&events) {
                    remove.push(events);
                }
            }
            for p in remove {
                if self.fs.exists(&p) {
                    self.fs.remove(&p);
                }
            }
        }
        outcome.reply = Some(reply(uuid, files, error));
        outcome
    }
}

/// The files `historical-start` hands back for `path` when it needs no conversion: a `.dslog`
/// also sends its sibling `.dsevents`, and a failed read is a `null` slot.
pub fn read_files(path: &str) -> Vec<Option<Vec<u8>>> {
    let fs = crate::fs::RealFs;
    match path.strip_suffix(".dslog") {
        Some(stem) => vec![
            fs.read_file(path).ok(),
            fs.read_file(&format!("{stem}.dsevents")).ok(),
        ],
        None => vec![fs.read_file(path).ok()],
    }
}
