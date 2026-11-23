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
use asrs_services::download::window::friendly_error;
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

#[derive(Default)]
struct Host {
    tokens: Token,
    dialogs: Vec<Token>,
    readdirs: Vec<(String, Token)>,
    gets: Vec<(String, String)>,
    dirs: Vec<String>,
    errors: Vec<String>,
    timeouts: Vec<f64>,
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
    fn set_timeout(&mut self, ms: f64) -> TimerId {
        self.timeouts.push(ms);
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
        if let WindowMessage::ShowError(message) = message {
            self.errors.push(message);
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

fn assert_refused(host: &Host, name: &str) {
    assert_eq!(host.errors.len(), 1, "{name:?}: {:?}", host.errors);
    assert!(host.gets.is_empty(), "{name:?}: {:?}", host.gets);
    assert!(host.dialogs.is_empty(), "{name:?}");
    assert_eq!(host.timeouts, [1000.0], "{name:?}: no retry was set");
    assert_eq!(host.intervals_cleared, 1, "{name:?}: still refreshing");
}

#[test]
fn plain_names() {
    for name in UNSAFE {
        assert!(!is_plain_name(name), "{name:?}");
    }
    for name in SAFE {
        assert!(is_plain_name(name), "{name:?}");
    }
}

#[test]
fn a_single_log_is_refused_before_its_dialog() {
    for style in [PathStyle::Posix, PathStyle::Win32] {
        for name in UNSAFE {
            let (mut downloader, mut host) = listed(style, &[entry(name, false)]);
            downloader.save_message(vec![name.to_owned()], &mut host);
            assert_refused(&host, name);
        }
    }
}

#[test]
fn several_logs_are_refused_together() {
    for name in UNSAFE {
        let files = ["good.wpilog", name, "other.rlog"];
        let entries: Vec<DirEntry> = files.iter().map(|f| entry(f, false)).collect();
        let (mut downloader, mut host) = listed(PathStyle::Posix, &entries);
        downloader.save_message(files.map(str::to_owned).to_vec(), &mut host);
        assert_refused(&host, name);
    }
}

#[test]
fn a_folder_is_refused() {
    for name in UNSAFE {
        let (mut downloader, mut host) = listed(PathStyle::Posix, &[entry(name, true)]);
        downloader.save_message(vec![name.to_owned()], &mut host);
        assert_refused(&host, name);
        assert!(host.dirs.is_empty(), "{name:?}: {:?}", host.dirs);
    }
}

#[test]
fn a_folder_with_an_unsafe_file_writes_nothing() {
    for name in UNSAFE.iter().copied().filter(|name| !name.starts_with('.')) {
        let (mut downloader, mut host) = listed(
            PathStyle::Posix,
            &[entry("first.wpilog", false), entry("folder", true)],
        );
        downloader.save_message(
            vec!["first.wpilog".to_owned(), "folder".to_owned()],
            &mut host,
        );
        choose(&mut downloader, &mut host, "/save");
        let (path, token) = host.readdirs.pop().expect("the folder is read");
        assert_eq!(path, "/logs/folder");
        let files = vec![entry("good.wpilog", false), entry(name, false)];
        downloader.on_readdir(token, Ok(files), &mut host);
        assert_refused(&host, name);
        assert!(host.dirs.is_empty(), "{name:?}: {:?}", host.dirs);
    }
}

/// Entries a folder's download leaves out are not judged.
#[test]
fn hidden_entries_and_subfolders_do_not_refuse_a_folder() {
    let (mut downloader, mut host) = listed(PathStyle::Posix, &[entry("folder", true)]);
    downloader.save_message(vec!["folder".to_owned()], &mut host);
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
    assert_eq!(host.errors, [""; 0]);
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
        downloader.save_message(files.map(str::to_owned).to_vec(), &mut host);
        choose(&mut downloader, &mut host, "/save");
        assert_eq!(host.errors, [""; 0], "{name:?}");
        assert_eq!(host.gets.len(), 2, "{name:?}");
        assert_eq!(
            host.gets[1],
            (format!("/logs/{name}"), format!("/save/{name}"))
        );
    }
}

#[test]
fn the_error_shows_the_name_as_text() {
    let name = "<img src=x>&amp;/\u{7}..";
    let (mut downloader, mut host) = listed(PathStyle::Posix, &[entry(name, true)]);
    downloader.save_message(vec![name.to_owned()], &mut host);
    let html = friendly_error(&host.errors[0], &JsString::from("a"), &JsString::from("p"));
    let text: String = alert_runs(&html).into_iter().map(|run| run.text).collect();
    assert_eq!(
        text,
        "Unknown error: The robot sent an unsafe file name: <img src=x>&amp;/\u{fffd}.."
    );
}
