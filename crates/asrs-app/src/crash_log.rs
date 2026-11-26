//! Writes panics to `crash.log` in the user-data directory: a packaged app has no terminal that
//! shows them. Upstream has no counterpart.
//!
//! A panic in the hook aborts, and the panicking thread may hold any lock, so the file is written
//! by a thread of its own that the hook waits for only so long.

use backtrace::Backtrace;
use std::cell::Cell;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const FILE: &str = "crash.log";
pub const OLD_FILE: &str = "crash.old.log";

const MAX_FILE: u64 = 256 << 10;
const MAX_MESSAGE: usize = 512;
const MAX_BACKTRACE: usize = 20 << 10;
/// Panics one run writes. They fit in one file, so the first is still in the log or the old one.
const MAX_PANICS: usize = 10;
const WRITE_WAIT: Duration = Duration::from_secs(3);

static PANICS: AtomicUsize = AtomicUsize::new(0);
static WRITING: Mutex<()> = Mutex::new(());

thread_local! {
    static WRITER: Cell<bool> = const { Cell::new(false) };
}

pub struct CrashLog {
    dir: PathBuf,
    heading: String,
    home: Option<String>,
}

impl CrashLog {
    /// `version` heads each entry. `home` is replaced in what is written.
    pub fn new(dir: PathBuf, version: &str, home: Option<String>) -> CrashLog {
        let heading = format!(
            "{version}, {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        );
        CrashLog {
            dir,
            heading,
            home: home.filter(|home| home.len() > 1),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(FILE)
    }

    /// Writes the panic in the hook's `info`; true when it is in the log. Returns within a few
    /// seconds and does not panic.
    pub fn record(&self, info: &PanicHookInfo<'_>) -> bool {
        if WRITER.try_with(Cell::get).unwrap_or(true)
            || PANICS.fetch_add(1, Ordering::Relaxed) >= MAX_PANICS
        {
            return false;
        }
        let payload = info.payload();
        let message = payload
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
            .unwrap_or("(not a message)");
        let mut text = format!("--- {} ---\n{}\n", now(), self.heading);
        let thread = thread::current();
        let name = thread.name().unwrap_or("unnamed");
        let _ = write!(text, "thread '{}' panicked", bounded(name, 64));
        if let Some(location) = info.location() {
            let _ = write!(text, " at {location}");
        }
        let _ = write!(text, ":\n{}\n", without_contents(message));
        // Without symbols, a frame's address is found by its distance to this function.
        let _ = writeln!(text, "CrashLog::new at {:p}", CrashLog::new as *const ());
        let backtrace = match WRITING.try_lock() {
            Ok(_free) => Some(Backtrace::new_unresolved()),
            Err(_) => None,
        };
        let entry = Entry {
            dir: self.dir.clone(),
            home: self.home.clone(),
            text,
            backtrace,
        };
        let (written, done) = mpsc::channel();
        let writer = thread::Builder::new()
            .name("crash-log".to_owned())
            .spawn(move || entry.write(&written));
        writer.is_ok() && wait(&done)
    }
}

/// Records every panic in `log` before the hook installed so far runs, and then tells `then`
/// whether it is there, and whether the process aborts next, whichever thread panicked.
pub fn install(log: CrashLog, then: impl Fn(bool, bool) + Send + Sync + 'static) {
    let hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        let logged = log.record(info);
        hook(info);
        then(logged, aborts(info));
    }));
}

/// `PanicHookInfo::can_unwind` is unstable: these are core's messages for a panic that leaves an
/// `extern` function or a destructor run by another panic.
fn aborts(info: &PanicHookInfo<'_>) -> bool {
    matches!(
        info.payload_as_str(),
        Some("panic in a function that cannot unwind" | "panic in a destructor during cleanup")
    )
}

fn wait(done: &Receiver<bool>) -> bool {
    done.recv_timeout(WRITE_WAIT).unwrap_or(false)
}

struct Entry {
    dir: PathBuf,
    home: Option<String>,
    text: String,
    backtrace: Option<Backtrace>,
}

impl Entry {
    fn write(self, written: &Sender<bool>) {
        let _ = WRITER.try_with(|writer| writer.set(true));
        let _one = WRITING.lock().unwrap_or_else(PoisonError::into_inner);
        let home = self.home.as_deref();
        let file = open(&self.dir).and_then(|mut file| {
            file.write_all(without_home(&self.text, home).as_bytes())?;
            if let Some(backtrace) = &self.backtrace {
                let backtrace = without_home(&format!("{backtrace:#?}"), home);
                file.write_all(bounded(&backtrace, MAX_BACKTRACE).as_bytes())?;
            }
            file.write_all(b"\n\n")?;
            file.sync_all()?;
            Ok(file)
        });
        let _ = written.send(file.is_ok());
    }
}

fn open(dir: &Path) -> io::Result<File> {
    fs::create_dir_all(dir)?;
    let path = dir.join(FILE);
    if fs::metadata(&path).is_ok_and(|file| file.len() >= MAX_FILE) {
        let rotated = fs::rename(&path, dir.join(OLD_FILE));
        if rotated.is_err() {
            return OpenOptions::new().write(true).truncate(true).open(&path);
        }
    }
    OpenOptions::new().create(true).append(true).open(&path)
}

fn now() -> String {
    let now = SystemTime::now().duration_since(UNIX_EPOCH);
    asrs_js::json::to_iso_string(now.unwrap_or_default().as_millis() as f64)
}

/// The longest start of `text` of at most `max` bytes.
fn bounded(text: &str, max: usize) -> &str {
    let end = (0..=max.min(text.len())).rev();
    let mut starts = end.filter_map(|end| text.get(..end));
    starts.next().unwrap_or_default()
}

/// The message without the string a `str` index panic quotes or the bytes of a `FromUtf8Error`,
/// which can come from an opened file.
fn without_contents(message: &str) -> String {
    let quotes = message.contains("byte index") || message.contains("char boundary");
    let content = [
        quotes.then(|| message.find('`')).flatten(),
        quotes.then(|| message.find("; it is inside")).flatten(),
        message.find("when slicing `"),
        message.find("bytes: ["),
        message.find("bytes: b\""),
    ];
    let end = content.into_iter().flatten().min();
    let kept = message.get(..end.unwrap_or(message.len()));
    let kept = kept.unwrap_or_default();
    let short = bounded(kept, MAX_MESSAGE);
    let cut = if short.len() < message.len() {
        "..."
    } else {
        ""
    };
    format!("{short}{cut}")
}

fn without_home(text: &str, home: Option<&str>) -> String {
    let Some(home) = home else {
        return text.to_owned();
    };
    let escaped = home.replace('\\', "\\\\");
    let slashes = home.replace('\\', "/");
    let mut out = text.to_owned();
    for home in [escaped.as_str(), home, slashes.as_str()] {
        out = replace_ignoring_case(&out, home);
    }
    out
}

fn replace_ignoring_case(text: &str, home: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(first) = rest.chars().next() {
        let same = rest
            .get(..home.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(home));
        let tail = if same { rest.get(home.len()..) } else { None };
        if let Some(tail) = tail {
            out.push('~');
            rest = tail;
        } else {
            out.push(first);
            rest = rest.get(first.len_utf8()..).unwrap_or_default();
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_contents_are_left_out() {
        for (message, kept) in [
            ("index out of bounds: the len is 1", None),
            (
                "end byte index 9 is out of bounds of `secret`",
                Some("end byte index 9 is out of bounds of "),
            ),
            (
                "byte index 1 is not a char boundary; it is inside 'x' of `secret`",
                Some("byte index 1 is not a char boundary"),
            ),
            (
                "begin <= end (4 <= 2) when slicing `secret`",
                Some("begin <= end (4 <= 2) "),
            ),
            (
                "called `Result::unwrap()` on an `Err` value: FromUtf8Error { bytes: [1, 2] }",
                Some("called `Result::unwrap()` on an `Err` value: FromUtf8Error { "),
            ),
        ] {
            let expected = kept.map_or(message.to_owned(), |kept| format!("{kept}..."));
            assert_eq!(without_contents(message), expected);
        }
        assert_eq!(
            without_contents(&"\u{e9}".repeat(400)),
            format!("{}...", "\u{e9}".repeat(256))
        );
        let odd = format!("a{}", "\u{e9}".repeat(400));
        assert!(without_contents(&odd).len() <= MAX_MESSAGE + 3);
    }

    #[test]
    fn the_home_directory_is_left_out() {
        let home = Some("C:\\Users\\Ada");
        for (text, expected) in [
            ("C:\\Users\\Ada\\a.wpilog", "~\\a.wpilog"),
            ("\"c:\\\\users\\\\ada\\\\a.wpilog\"", "\"~\\\\a.wpilog\""),
            ("C:/Users/Ada/a.wpilog \u{e9}", "~/a.wpilog \u{e9}"),
            ("D:\\logs", "D:\\logs"),
        ] {
            assert_eq!(without_home(text, home), expected);
        }
        assert_eq!(without_home("/Users/ada", None), "/Users/ada");
    }
}
