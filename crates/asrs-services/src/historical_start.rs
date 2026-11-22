//! Main's `historical-start` and `historical-start-raw` handlers from `main/electron/main.ts`:
//! the files a historical source asks for, read (and for `.wpilogxz`, decompressed) into the
//! `historical-data` reply, plus the recent-document and `akit-log-path.txt` side effects.

use std::sync::Arc;

use asrs_formats::historical::{MainReply, MainRequest};
use asrs_formats::wpilogxz;
use asrs_js::string::JsString;
use asrs_js::time::{Rng, create_uuid};

use crate::fs::{Fs, ReadError};

pub const XZ_UNSUPPORTED: &str =
    "Compressed WPILOG files (.wpilogxz) are not supported on this platform.";
pub const HOOT_UNSUPPORTED: &str = "Hoot log files are not supported in this build.";
pub const REVLOG_UNSUPPORTED: &str = "REVLOG files are not supported in this build.";

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
    /// The handler threw after replying (an unreadable but openable `.wpilogxz`, such as a
    /// directory, reaches `Readable.from(undefined)`), which Electron reports in its
    /// main-process error dialog.
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

fn reply(uuid: &str, files: Vec<Option<Vec<u8>>>, error: Option<&str>) -> MainReply {
    MainReply {
        uuid: uuid.to_owned(),
        files: files.into_iter().map(|f| f.map(Arc::from)).collect(),
        error: error.map(JsString::from),
    }
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
            return answered(reply(uuid, vec![None], Some("Invalid log extension")));
        }
        let mut name = JsString::from(format!("child_{}.", create_uuid(rng)).as_str());
        name.push_str(extension);
        let temp_path = JsString::from(join(self.temp_dir, &name.to_string_lossy()).as_str());
        if has_nul(&temp_path) {
            return Outcome::default();
        }
        match self.fs.write(&temp_path.to_string_lossy(), data) {
            Ok(()) => self.start(uuid, &temp_path, true),
            Err(e) => answered(reply(uuid, vec![None], Some(&e.message))),
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
        let mut error = None;
        let files = if ends_with(path, ".dslog") {
            if has_nul(path) {
                return outcome;
            }
            vec![read(&fs_path), read(&dsevents_path(&fs_path))]
        } else if ends_with(path, ".hoot") {
            error = Some(HOOT_UNSUPPORTED);
            vec![None]
        } else if ends_with(path, ".revlog") {
            error = Some(REVLOG_UNSUPPORTED);
            vec![None]
        } else if ends_with(path, ".wpilogxz") {
            if !self.lzma {
                error = Some(XZ_UNSUPPORTED);
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
