#![allow(clippy::disallowed_methods)]

//! Server-sent names that could leave the save folder are refused in every save flow (#299), where
//! upstream joins them onto it unchecked.

use asrs_js::string::JsString;
use asrs_services::dialog::OpenDialog;
use asrs_services::download::downloader::{
    ClientId, ConnectOptions, DirEntry, DownloadHost, Downloader, OpenLogBox, SaveLogDialog,
    SftpId, Thrown, TimerId, Token, WindowMessage, is_plain_name,
};
use asrs_services::download::page::alert_runs;
use asrs_services::path::PathStyle;

const UNSAFE: [&str; 17] = [
    "",
    ".",
    "..",
    "../x.wpilog",
    "x/../../../y.wpilog",
    "sub/x.wpilog",
    "/etc/x.wpilog",
    "//host/share/x.wpilog",
    "..\\x.wpilog",
    "sub\\x.wpilog",
    "\\x.wpilog",
    "\\\\host\\share\\x.wpilog",
    "C:x.wpilog",
    "c:\\x.wpilog",
    "z:/x.wpilog",
    "x\0.wpilog",
    "a/",
];
const SAFE: [&str; 9] = [
    "x.wpilog",
    "..x.wpilog",
    "x..wpilog",
    "x...wpilog",
    "FRC_20250101_120000_Q1.wpilog",
    "12:30.wpilog",
    "ab:c.wpilog",
    "caf\u{e9} \u{65e5}\u{672c}.hoot",
    "folder",
];
/// What Windows opens as a device or as a stream of another file.
const WINDOWS_UNSAFE: [&str; 15] = [
    "NUL",
    "con.wpilog",
    "Aux .rlog",
    "PRN.a.b.hoot",
    "CONIN$",
    "conout$.wpilog",
    "COM1.wpilog",
    "com9",
    "LPT0.rlog",
    "lpt\u{b2}.wpilog",
    "12:30.wpilog",
    "ab:c.wpilog",
    "x.exe::$DATA",
    ":x.wpilog",
    "x.wpilog:",
];
const WINDOWS_SAFE: [&str; 9] = [
    "CONSOLE.wpilog",
    "xNUL.wpilog",
    "x.con",
    "COM.wpilog",
    "COM10.wpilog",
    "COMx.wpilog",
    "LPT\u{b4}.wpilog",
    "nul_.hoot",
    "FRC_20250101_120000_Q1.wpilog",
];
const STYLES: [PathStyle; 2] = [PathStyle::Posix, PathStyle::Win32];

fn refused(style: PathStyle) -> Vec<&'static str> {
    let windows: &[&str] = match style {
        PathStyle::Posix => &[],
        PathStyle::Win32 => &WINDOWS_UNSAFE,
    };
    UNSAFE.iter().chain(windows).copied().collect()
}

fn some(files: &[&str]) -> Vec<Option<String>> {
    files.iter().map(|file| Some((*file).to_owned())).collect()
}

#[derive(Default)]
struct Host {
    tokens: Token,
    dialogs: Vec<Token>,
    readdirs: Vec<(String, Token)>,
    gets: Vec<(String, String)>,
    dirs: Vec<String>,
    errors: Vec<String>,
    alerts: Vec<String>,
    timeouts: usize,
    intervals_cleared: usize,
}

impl Host {
    fn token(&mut self) -> Token {
        self.tokens += 1;
        self.tokens
    }

    fn dialog(&mut self) -> Token {
        let token = self.token();
        self.dialogs.push(token);
        token
    }
}

impl DownloadHost for Host {
    fn connect(&mut self, _: &ConnectOptions) -> ClientId {
        1
    }
    fn end(&mut self, _: ClientId) {}
    fn sftp(&mut self, _: ClientId) -> Result<Token, String> {
        Ok(self.token())
    }
    fn readdir(&mut self, _: SftpId, path: &str) -> Result<Token, String> {
        let token = self.token();
        self.readdirs.push((path.to_owned(), token));
        Ok(token)
    }
    fn fast_get(&mut self, _: SftpId, remote: &str, local: &str) -> Result<Token, String> {
        self.gets.push((remote.to_owned(), local.to_owned()));
        Ok(self.token())
    }
    fn set_timeout(&mut self, _: f64) -> TimerId {
        self.timeouts += 1;
        self.token()
    }
    fn clear_timeout(&mut self, _: TimerId) {}
    fn set_interval(&mut self, _: f64) -> TimerId {
        self.token()
    }
    fn clear_interval(&mut self, _: TimerId) {
        self.intervals_cleared += 1;
    }
    fn send(&mut self, message: WindowMessage) {
        match message {
            WindowMessage::ShowError(message) => self.errors.push(message),
            WindowMessage::ShowAlert(html) => self.alerts.push(html),
            WindowMessage::SetList(_) | WindowMessage::SetProgress(_) => {}
        }
    }
    fn show_open_dialog(&mut self, _: &OpenDialog) -> Token {
        self.dialog()
    }
    fn show_save_dialog(&mut self, _: &SaveLogDialog) -> Token {
        self.dialog()
    }
    fn show_message_box(&mut self, _: &OpenLogBox) -> Token {
        self.dialog()
    }
    fn destroy_window(&mut self) {}
    fn focus_hub(&mut self) {}
    fn open_files(&mut self, _: &[String]) {}
    fn exists(&mut self, _: &str) -> bool {
        false
    }
    fn file_size(&mut self, path: &str) -> Result<f64, String> {
        Err(path.to_owned())
    }
    fn mkdir_all(&mut self, path: &str) -> Thrown {
        self.dirs.push(path.to_owned());
        Ok(())
    }
}

fn entry(filename: &str, dir: bool) -> DirEntry {
    DirEntry {
        filename: filename.to_owned(),
        mode: Some(if dir { 0o040755 } else { 0o100644 }),
        size: Some(10.0),
    }
}

/// A downloader with the list of `files` shown.
fn listed(style: PathStyle, files: &[DirEntry]) -> (Downloader, Host) {
    let mut host = Host::default();
    let mut downloader = Downloader::new(style);
    downloader.window_opened();
    downloader.start_message("10.0.0.2", "/logs", &mut host);
    downloader.on_ready(&mut host).expect("ready");
    let sftp = host.tokens;
    downloader
        .on_sftp(sftp, Ok(1), &mut host)
        .expect("the SFTP session");
    while let Some((path, token)) = host.readdirs.pop() {
        let entries = if path == "/logs/" {
            files.to_vec()
        } else {
            Vec::new()
        };
        downloader.on_readdir(token, Ok(entries), &mut host);
    }
    assert!(host.errors.is_empty());
    (downloader, host)
}

/// Answers the save dialog and starts the save's SFTP session.
fn choose(downloader: &mut Downloader, host: &mut Host, path: &str) {
    let dialog = host.dialogs.pop().expect("a save dialog");
    downloader.on_save_location(dialog, Some(path.to_owned()), host);
    let sftp = host.tokens;
    downloader
        .on_sftp(sftp, Ok(2), host)
        .expect("the SFTP session");
}

/// The window says so, nothing is written, and the connection goes on.
fn assert_refused(host: &Host, name: &str) {
    assert_eq!(host.alerts.len(), 1, "{name:?}: {:?}", host.alerts);
    assert_eq!(host.errors, [""; 0], "{name:?}");
    assert!(host.gets.is_empty(), "{name:?}: {:?}", host.gets);
    assert!(host.dirs.is_empty(), "{name:?}: {:?}", host.dirs);
    assert!(host.dialogs.is_empty(), "{name:?}");
    assert_eq!((host.timeouts, host.intervals_cleared), (0, 0), "{name:?}");
}

fn save(downloader: &mut Downloader, files: &[&str], host: &mut Host) {
    downloader
        .save_message(some(files), host)
        .expect("every name is a string");
}

#[test]
fn plain_names() {
    for style in STYLES {
        for name in refused(style) {
            assert!(!is_plain_name(name, style), "{style:?} {name:?}");
        }
        for name in WINDOWS_SAFE {
            assert!(is_plain_name(name, style), "{style:?} {name:?}");
        }
    }
    for name in SAFE.iter().chain(&WINDOWS_UNSAFE) {
        assert!(is_plain_name(name, PathStyle::Posix), "{name:?}");
    }
}

#[test]
fn a_single_log_is_refused_before_its_dialog() {
    for style in STYLES {
        for name in refused(style) {
            let (mut downloader, mut host) = listed(style, &[entry(name, false)]);
            save(&mut downloader, &[name], &mut host);
            assert_refused(&host, name);
        }
    }
}

#[test]
fn several_logs_are_refused_together() {
    for style in STYLES {
        for name in refused(style) {
            let files = ["good.wpilog", name, "other.rlog"];
            let entries: Vec<DirEntry> = files.iter().map(|f| entry(f, false)).collect();
            let (mut downloader, mut host) = listed(style, &entries);
            save(&mut downloader, &files, &mut host);
            assert_refused(&host, name);
        }
    }
}

#[test]
fn a_folder_is_refused() {
    for style in STYLES {
        for name in refused(style) {
            let (mut downloader, mut host) = listed(style, &[entry(name, true)]);
            save(&mut downloader, &[name], &mut host);
            assert_refused(&host, name);
        }
    }
}

#[test]
fn a_folder_with_an_unsafe_file_writes_nothing() {
    for style in STYLES {
        for name in refused(style) {
            if name.starts_with('.') {
                continue;
            }
            let entries = [entry("first.wpilog", false), entry("folder", true)];
            let (mut downloader, mut host) = listed(style, &entries);
            save(&mut downloader, &["first.wpilog", "folder"], &mut host);
            choose(&mut downloader, &mut host, "/save");
            let (path, token) = host.readdirs.pop().expect("the folder is read");
            assert_eq!(path, "/logs/folder");
            let files = vec![entry("good.wpilog", false), entry(name, false)];
            downloader.on_readdir(token, Ok(files), &mut host);
            assert_refused(&host, name);
        }
    }
}

/// Entries a folder's download leaves out are not judged.
#[test]
fn hidden_entries_and_subfolders_do_not_refuse_a_folder() {
    let (mut downloader, mut host) = listed(PathStyle::Posix, &[entry("folder", true)]);
    save(&mut downloader, &["folder"], &mut host);
    choose(&mut downloader, &mut host, "/save");
    let (_, token) = host.readdirs.pop().expect("the folder is read");
    let files = vec![
        entry(".", true),
        entry("..", true),
        entry("../hidden", false),
        entry("a/b", true),
        entry("good.wpilog", false),
    ];
    downloader.on_readdir(token, Ok(files), &mut host);
    assert_eq!(host.alerts, [""; 0]);
    assert_eq!(
        host.gets,
        [(
            "/logs/folder/good.wpilog".to_owned(),
            "/save/folder/good.wpilog".to_owned()
        )]
    );
}

#[test]
fn plain_names_are_saved() {
    for name in SAFE.iter().copied().filter(|name| name.contains('.')) {
        let files = ["first.rlog", name];
        let entries: Vec<DirEntry> = files.iter().map(|f| entry(f, false)).collect();
        let (mut downloader, mut host) = listed(PathStyle::Posix, &entries);
        save(&mut downloader, &files, &mut host);
        choose(&mut downloader, &mut host, "/save");
        assert_eq!(host.alerts, [""; 0], "{name:?}");
        assert_eq!(host.gets.len(), 2, "{name:?}");
        assert_eq!(
            host.gets[1],
            (format!("/logs/{name}"), format!("/save/{name}"))
        );
    }
}

/// `path.extname(undefined)` throws: at once for the first name, and for a later one when the save
/// comes to it, so nothing is downloaded.
#[test]
fn an_undefined_name_ends_the_save() {
    let entries = [entry("a.wpilog", false), entry("folder", true)];
    for files in [vec![None], vec![None, Some("a.wpilog".to_owned())]] {
        let (mut downloader, mut host) = listed(PathStyle::Posix, &entries);
        let thrown = downloader.save_message(files, &mut host);
        assert!(thrown.is_err_and(|e| e.ends_with("Received undefined")));
        assert!(host.dialogs.is_empty());
    }

    let (mut downloader, mut host) = listed(PathStyle::Posix, &entries);
    let mut files = some(&["a.wpilog", "folder"]);
    files.push(None);
    downloader
        .save_message(files, &mut host)
        .expect("the first name is a string");
    choose(&mut downloader, &mut host, "/save");
    let (_, token) = host.readdirs.pop().expect("the folder is read");
    downloader.on_readdir(token, Ok(vec![entry("b.wpilog", false)]), &mut host);
    assert_eq!(host.dirs, ["/save/folder"]);
    assert!(host.gets.is_empty(), "{:?}", host.gets);
    assert!(host.alerts.is_empty() && host.errors.is_empty());
}

#[test]
fn the_alert_shows_the_name_as_text() {
    let name = "<img src=x>&amp;/\u{7}..";
    let (mut downloader, mut host) = listed(PathStyle::Posix, &[entry(name, true)]);
    save(&mut downloader, &[name], &mut host);
    let html = JsString::from(host.alerts[0].as_str());
    let text: String = alert_runs(&html).into_iter().map(|run| run.text).collect();
    assert_eq!(
        text,
        "Not saved: the robot sent an unsafe file name (<img src=x>&amp;/\u{fffd}..)"
    );
}
