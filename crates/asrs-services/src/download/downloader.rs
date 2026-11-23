//! The download section of `main.ts`. Every call it makes on ssh2, Electron or `fs` is a
//! [`DownloadHost`] call; what upstream receives in callbacks and promises comes back through the
//! `on_*` methods with the token the host returned. Those return `Err` with the message of an
//! exception upstream lets escape from the callback.
//!
//! Upstream registers its callbacks on a client without checking, when they run, that the client is
//! still the current one, and its licenses and help windows also call `downloadStop` when they
//! close. Both are kept.
//!
//! One deviation: upstream saves under whatever name the server sent, and here a save with a name
//! that is not a plain file name ([`is_plain_name`]) fails like any other before it writes anything.

use std::collections::HashMap;

use asrs_js::string::JsString;

use crate::dialog::OpenDialog;
use crate::path::PathStyle;

pub const SYSTEMCORE_USERNAME: &str = "systemcore";
pub const SYSTEMCORE_PASSWORD: &str = "systemcore";
pub const ROBORIO_USERNAME: &str = "lvuser";
pub const ROBORIO_PASSWORD: &str = "";
pub const CONNECT_TIMEOUT_MS: f64 = 3000.0;
pub const RETRY_DELAY_MS: f64 = 1000.0;
pub const REFRESH_INTERVAL_MS: f64 = 5000.0;
pub const SSH_PORT: u16 = 22;

const LOG_EXTENSIONS: [&str; 5] = [".rlog", ".wpilog", ".wpilogxz", ".revlog", ".hoot"];

pub type ClientId = u32;
pub type SftpId = u32;
pub type TimerId = u32;
/// Names a request or dialog whose result is still to come.
pub type Token = u32;

/// `Client.connect`'s options; `forceIPv4` is always on.
#[derive(Clone, Debug, PartialEq)]
pub struct ConnectOptions {
    pub host: String,
    pub port: u16,
    pub ready_timeout_ms: f64,
    pub username: &'static str,
    pub password: &'static str,
}

/// One entry of an SFTP directory listing. `None` is an attribute the server left out.
#[derive(Clone, Debug, PartialEq)]
pub struct DirEntry {
    pub filename: String,
    pub mode: Option<u32>,
    pub size: Option<f64>,
}

impl DirEntry {
    /// `Stats.isDirectory()`.
    pub fn is_directory(&self) -> bool {
        self.mode.is_some_and(|mode| mode & 0o170000 == 0o040000)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ListItem {
    pub name: String,
    /// `None` is `undefined`: the server sent no size.
    pub size: Option<f64>,
    pub is_folder: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Progress {
    /// `0`
    Preparing,
    /// `1`
    Finished,
    Transferring {
        current: f64,
        total: f64,
    },
}

/// A message main posts to the download window.
#[derive(Clone, Debug, PartialEq)]
pub enum WindowMessage {
    SetList(Vec<ListItem>),
    ShowError(String),
    /// HTML.
    ShowAlert(String),
    SetProgress(Progress),
}

/// The `showSaveDialog` call for a single log.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveLogDialog {
    pub title: &'static str,
    pub default_path: String,
    pub properties: &'static [&'static str],
    pub filter_name: &'static str,
    pub extension: String,
}

/// The "Open log?" `showMessageBox` call. Its icon is `WINDOW_ICON`.
#[derive(Clone, Debug, PartialEq)]
pub struct OpenLogBox {
    pub kind: &'static str,
    pub message: &'static str,
    pub detail: String,
    pub buttons: [&'static str; 2],
    pub default_id: usize,
}

/// `Err` is the message of what the call throws.
pub type Thrown = Result<(), String>;

pub trait DownloadHost {
    /// `new Client()` with main's `ready` and `error` listeners, and its `connect`.
    fn connect(&mut self, options: &ConnectOptions) -> ClientId;
    fn end(&mut self, client: ClientId);
    /// The result is [`Downloader::on_sftp`].
    fn sftp(&mut self, client: ClientId) -> Result<Token, String>;
    /// The result is [`Downloader::on_readdir`].
    fn readdir(&mut self, sftp: SftpId, path: &str) -> Result<Token, String>;
    /// The result is [`Downloader::on_got`], after any number of [`Downloader::on_step`].
    fn fast_get(&mut self, sftp: SftpId, remote: &str, local: &str) -> Result<Token, String>;

    fn set_timeout(&mut self, ms: f64) -> TimerId;
    fn clear_timeout(&mut self, id: TimerId);
    fn set_interval(&mut self, ms: f64) -> TimerId;
    fn clear_interval(&mut self, id: TimerId);

    /// Never called once the window is destroyed.
    fn send(&mut self, message: WindowMessage);
    /// The result is [`Downloader::on_save_location`].
    fn show_open_dialog(&mut self, dialog: &OpenDialog) -> Token;
    /// The result is [`Downloader::on_save_location`].
    fn show_save_dialog(&mut self, dialog: &SaveLogDialog) -> Token;
    /// The result is [`Downloader::on_open_log`].
    fn show_message_box(&mut self, options: &OpenLogBox) -> Token;
    fn destroy_window(&mut self);
    /// `hubWindows[0].focus()`.
    fn focus_hub(&mut self);
    /// The `open-files` message to `hubWindows[0]`, with `merge: false`.
    fn open_files(&mut self, files: &[String]);

    fn exists(&mut self, path: &str) -> bool;
    /// `fs.statSync(path).size`.
    fn file_size(&mut self, path: &str) -> Result<f64, String>;
    /// `fs.mkdirSync(path, { recursive: true })`.
    fn mkdir_all(&mut self, path: &str) -> Thrown;
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Device {
    Systemcore,
    Roborio,
}

struct Listing {
    sftp: SftpId,
    /// Directories and logs still to be added, next first.
    rest: std::vec::IntoIter<DirEntry>,
    items: Vec<ListItem>,
    folder: Option<String>,
}

#[derive(Clone)]
struct Task {
    remote: String,
    local: String,
    /// `NaN` is `undefined`: both make every use of it the same.
    size: f64,
}

struct SaveMany {
    sftp: SftpId,
    save_path: String,
    rest: std::vec::IntoIter<String>,
    folder: Option<String>,
    tasks: Vec<Task>,
}

struct Batch {
    save_path: String,
    total_count: usize,
    complete_count: usize,
    skip_count: usize,
    sizes: Vec<f64>,
    size_total: f64,
}

enum Pending {
    ListSftp,
    ListRoot {
        sftp: SftpId,
    },
    ListFolder(Listing),
    SaveSftp {
        files: Vec<String>,
        save_path: String,
    },
    SaveFolder(SaveMany),
    GetSingle {
        save_path: String,
    },
    GetMany {
        batch: usize,
        index: usize,
    },
}

pub struct Downloader {
    style: PathStyle,
    /// `downloadWindow`: whether one was ever opened, and whether it is destroyed. Upstream never
    /// sets the variable back to `null`.
    window: Option<bool>,
    client: Option<ClientId>,
    retry_timeout: Option<TimerId>,
    refresh_interval: Option<TimerId>,
    address: String,
    path: String,
    sizes: HashMap<String, f64>,
    device: Device,
    auth_failed_once: bool,
    pending: HashMap<Token, Pending>,
    refreshing: HashMap<TimerId, SftpId>,
    batches: Vec<Batch>,
    selecting: HashMap<Token, Vec<String>>,
    asking: HashMap<Token, String>,
}

impl Downloader {
    pub fn new(style: PathStyle) -> Downloader {
        Downloader {
            style,
            window: None,
            client: None,
            retry_timeout: None,
            refresh_interval: None,
            address: String::new(),
            path: String::new(),
            sizes: HashMap::new(),
            device: Device::Systemcore,
            auth_failed_once: false,
            pending: HashMap::new(),
            refreshing: HashMap::new(),
            batches: Vec::new(),
            selecting: HashMap::new(),
            asking: HashMap::new(),
        }
    }

    /// `openDownload` made a window.
    pub fn window_opened(&mut self) {
        self.window = Some(false);
    }

    /// The window closed from its frame: its `close` listener.
    pub fn window_closed(&mut self, host: &mut dyn DownloadHost) {
        self.stop(host);
        self.window = self.window.map(|_| true);
    }

    fn window_alive(&self) -> bool {
        self.window == Some(false)
    }

    fn send(&self, host: &mut dyn DownloadHost, message: WindowMessage) {
        if self.window_alive() {
            host.send(message);
        }
    }

    /// The window's `start` message.
    pub fn start_message(&mut self, address: &str, path: &str, host: &mut dyn DownloadHost) {
        if !self.window_alive() {
            return;
        }
        self.device = Device::Systemcore;
        self.auth_failed_once = false;
        address.clone_into(&mut self.address);
        path.clone_into(&mut self.path);
        if !self.path.ends_with('/') {
            self.path.push('/');
        }
        self.start(host);
    }

    /// The window's `close` message.
    pub fn close_message(&mut self, host: &mut dyn DownloadHost) {
        if !self.window_alive() {
            return;
        }
        self.destroy_window(host);
        self.stop(host);
    }

    /// The window's `save` message.
    pub fn save_message(&mut self, files: Vec<String>, host: &mut dyn DownloadHost) {
        if !self.window_alive() {
            return;
        }
        let Some(first) = files.first() else {
            return;
        };
        if let Some(name) = files.iter().find(|name| !is_plain_name(name)) {
            self.error(&unsafe_name(name), host);
            return;
        }
        let extension = self.extension(first);
        let token = if files.len() > 1 || extension.is_empty() {
            host.show_open_dialog(&OpenDialog {
                title: "Select save location for robot logs".to_owned(),
                message: None,
                button_label: Some("Save"),
                properties: &["openDirectory", "createDirectory", "dontAddToRecent"],
                filters: None,
            })
        } else {
            let extension = extension[1..].to_owned();
            host.show_save_dialog(&SaveLogDialog {
                title: "Select save location for robot log",
                default_path: first.clone(),
                properties: &[
                    "createDirectory",
                    "showOverwriteConfirmation",
                    "dontAddToRecent",
                ],
                filter_name: match extension.as_str() {
                    "wpilog" => "WPILib robot log",
                    "wpilogxz" => "Compressed WPILib robot log",
                    "rlog" => "Robot log",
                    "hoot" => "Hoot robot log",
                    "revlog" => "REV Robotics CAN log",
                    _ => "",
                },
                extension,
            })
        };
        self.selecting.insert(token, files);
    }

    fn extension(&self, name: &str) -> String {
        self.style.extname(&JsString::from(name)).to_string_lossy()
    }

    fn join(&self, dir: &str, name: &str) -> String {
        self.style
            .join(&[&JsString::from(dir), &JsString::from(name)])
            .to_string_lossy()
    }

    fn destroy_window(&mut self, host: &mut dyn DownloadHost) {
        if self.window_alive() {
            self.window = Some(true);
            host.destroy_window();
        }
    }

    /// `downloadStart`
    fn start(&mut self, host: &mut dyn DownloadHost) {
        self.clear_timers(host);
        if let Some(client) = self.client {
            host.end(client);
        }
        self.sizes.clear();
        let (username, password) = match self.device {
            Device::Systemcore => (SYSTEMCORE_USERNAME, SYSTEMCORE_PASSWORD),
            Device::Roborio => (ROBORIO_USERNAME, ROBORIO_PASSWORD),
        };
        self.client = Some(host.connect(&ConnectOptions {
            host: self.address.clone(),
            port: SSH_PORT,
            ready_timeout_ms: CONNECT_TIMEOUT_MS,
            username,
            password,
        }));
    }

    fn clear_timers(&mut self, host: &mut dyn DownloadHost) {
        if let Some(id) = self.retry_timeout {
            host.clear_timeout(id);
        }
        self.clear_refresh(host);
    }

    fn clear_refresh(&mut self, host: &mut dyn DownloadHost) {
        if let Some(id) = self.refresh_interval {
            self.refreshing.remove(&id);
            host.clear_interval(id);
        }
    }

    /// `downloadStop`
    pub fn stop(&mut self, host: &mut dyn DownloadHost) {
        if let Some(client) = self.client {
            host.end(client);
        }
        self.device = Device::Systemcore;
        self.auth_failed_once = false;
        self.clear_timers(host);
    }

    /// `downloadError`
    fn error(&mut self, message: &str, host: &mut dyn DownloadHost) {
        if self.window.is_none() {
            return;
        }
        self.device = Device::Systemcore;
        self.auth_failed_once = false;
        self.send(host, WindowMessage::ShowError(message.to_owned()));
        self.clear_refresh(host);
        self.retry_timeout = Some(host.set_timeout(RETRY_DELAY_MS));
    }

    /// A client's `ready` event.
    pub fn on_ready(&mut self, host: &mut dyn DownloadHost) -> Thrown {
        self.auth_failed_once = false;
        if let Some(client) = self.client {
            let token = host.sftp(client)?;
            self.pending.insert(token, Pending::ListSftp);
        }
        Ok(())
    }

    /// A client's `error` event.
    pub fn on_error(&mut self, message: &str, host: &mut dyn DownloadHost) {
        self.device = match self.device {
            Device::Systemcore => Device::Roborio,
            Device::Roborio => Device::Systemcore,
        };
        if self.auth_failed_once {
            self.auth_failed_once = false;
            self.error(message, host);
        } else {
            self.auth_failed_once = true;
            self.clear_refresh(host);
            self.start(host);
        }
    }

    /// A timeout or interval the host was asked for fired.
    pub fn on_timer(&mut self, id: TimerId, host: &mut dyn DownloadHost) -> Thrown {
        if let Some(sftp) = self.refreshing.get(&id).copied() {
            self.read_files(sftp, host)
        } else {
            self.start(host);
            Ok(())
        }
    }

    fn read_files(&mut self, sftp: SftpId, host: &mut dyn DownloadHost) -> Thrown {
        let token = host.readdir(sftp, &self.path)?;
        self.pending.insert(token, Pending::ListRoot { sftp });
        Ok(())
    }

    /// The result of [`DownloadHost::sftp`].
    pub fn on_sftp(
        &mut self,
        token: Token,
        result: Result<SftpId, String>,
        host: &mut dyn DownloadHost,
    ) -> Thrown {
        match (self.pending.remove(&token), result) {
            (Some(Pending::ListSftp), Ok(sftp)) => {
                let id = host.set_interval(REFRESH_INTERVAL_MS);
                self.refresh_interval = Some(id);
                self.refreshing.insert(id, sftp);
                self.read_files(sftp, host)
            }
            (Some(Pending::SaveSftp { files, save_path }), Ok(sftp)) => {
                self.save(files, save_path, sftp, host)
            }
            (Some(Pending::ListSftp | Pending::SaveSftp { .. }), Err(message)) => {
                self.error(&message, host);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// The result of [`DownloadHost::readdir`].
    pub fn on_readdir(
        &mut self,
        token: Token,
        result: Result<Vec<DirEntry>, String>,
        host: &mut dyn DownloadHost,
    ) {
        match self.pending.remove(&token) {
            Some(Pending::ListRoot { sftp }) => match result {
                Err(message) => self.error(&message, host),
                Ok(list) => {
                    let shown: Vec<DirEntry> = list
                        .into_iter()
                        .filter(|file| {
                            !file.filename.starts_with('.')
                                && (file.is_directory()
                                    || LOG_EXTENSIONS.iter().any(|e| file.filename.ends_with(e)))
                        })
                        .collect();
                    self.list(
                        Listing {
                            sftp,
                            rest: shown.into_iter(),
                            items: Vec::new(),
                            folder: None,
                        },
                        host,
                    );
                }
            },
            Some(Pending::ListFolder(mut listing)) => {
                let name = listing.folder.take().expect("a folder was being read");
                let mut total = 0.0;
                for file in result.unwrap_or_default() {
                    if !file.is_directory() && !file.filename.starts_with('.') {
                        total += file.size.unwrap_or(f64::NAN);
                    }
                }
                listing.items.push(folder_item(name, total));
                self.list(listing, host);
            }
            Some(Pending::SaveFolder(mut save)) => {
                let folder = save.folder.take().expect("a folder was being read");
                if let Ok(files) = result {
                    let saved = |file: &&DirEntry| is_saved(file);
                    let unsafe_file = files
                        .iter()
                        .filter(saved)
                        .find(|file| !is_plain_name(&file.filename));
                    if let Some(file) = unsafe_file {
                        self.error(&unsafe_name(&file.filename), host);
                        return;
                    }
                    self.add_folder(&mut save, &folder, files, host);
                }
                self.collect(save, host);
            }
            _ => {}
        }
    }

    fn list(&mut self, mut listing: Listing, host: &mut dyn DownloadHost) {
        while let Some(file) = listing.rest.next() {
            if !file.is_directory() {
                listing.items.push(ListItem {
                    name: file.filename,
                    size: file.size,
                    is_folder: false,
                });
                continue;
            }
            match host.readdir(listing.sftp, &format!("{}{}", self.path, file.filename)) {
                Ok(token) => {
                    listing.folder = Some(file.filename);
                    self.pending.insert(token, Pending::ListFolder(listing));
                    return;
                }
                Err(_) => listing.items.push(folder_item(file.filename, 0.0)),
            }
        }
        self.send(host, WindowMessage::SetList(listing.items.clone()));
        for item in listing.items {
            self.sizes.insert(item.name, item.size.unwrap_or(f64::NAN));
        }
    }

    /// The answer to a save location dialog; `None` when it was cancelled.
    pub fn on_save_location(
        &mut self,
        token: Token,
        path: Option<String>,
        host: &mut dyn DownloadHost,
    ) {
        let Some(files) = self.selecting.remove(&token) else {
            return;
        };
        let Some(save_path) = path.filter(|path| !path.is_empty()) else {
            return;
        };
        // "Not connected" is thrown inside a promise here, and main ignores unhandled rejections.
        if let Some(client) = self.client
            && let Ok(token) = host.sftp(client)
        {
            self.pending
                .insert(token, Pending::SaveSftp { files, save_path });
        }
    }

    fn save(
        &mut self,
        files: Vec<String>,
        save_path: String,
        sftp: SftpId,
        host: &mut dyn DownloadHost,
    ) -> Thrown {
        self.send(host, WindowMessage::SetProgress(Progress::Preparing));
        if files.len() == 1 && !self.extension(&files[0]).is_empty() {
            let remote = format!("{}{}", self.path, files[0]);
            if let Ok(token) = host.fast_get(sftp, &remote, &save_path) {
                self.pending.insert(token, Pending::GetSingle { save_path });
            }
        } else {
            self.collect(
                SaveMany {
                    sftp,
                    save_path,
                    rest: files.into_iter(),
                    folder: None,
                    tasks: Vec::new(),
                },
                host,
            );
        }
        Ok(())
    }

    fn add_folder(
        &mut self,
        save: &mut SaveMany,
        folder: &str,
        files: Vec<DirEntry>,
        host: &mut dyn DownloadHost,
    ) {
        let local = self.join(&save.save_path, folder);
        if !host.exists(&local) && host.mkdir_all(&local).is_err() {
            return;
        }
        for file in files {
            if is_saved(&file) {
                save.tasks.push(Task {
                    remote: format!("{}{folder}/{}", self.path, file.filename),
                    local: self.join(&local, &file.filename),
                    size: file.size.unwrap_or(f64::NAN),
                });
            }
        }
    }

    fn collect(&mut self, mut save: SaveMany, host: &mut dyn DownloadHost) {
        while let Some(file) = save.rest.next() {
            let remote = format!("{}{file}", self.path);
            if !self.extension(&file).is_empty() {
                save.tasks.push(Task {
                    remote,
                    local: self.join(&save.save_path, &file),
                    size: self.sizes.get(&file).copied().unwrap_or(0.0),
                });
            } else if let Ok(token) = host.readdir(save.sftp, &remote) {
                save.folder = Some(file);
                self.pending.insert(token, Pending::SaveFolder(save));
                return;
            }
        }
        self.transfer(save, host);
    }

    fn transfer(&mut self, save: SaveMany, host: &mut dyn DownloadHost) {
        let total_count = save.tasks.len();
        let mut batch = Batch {
            save_path: save.save_path,
            total_count,
            complete_count: 0,
            skip_count: 0,
            sizes: vec![0.0; total_count],
            size_total: 0.0,
        };
        let mut downloads = Vec::new();
        for (index, task) in save.tasks.into_iter().enumerate() {
            let saved = host.exists(&task.local)
                && host.file_size(&task.local).is_ok_and(|s| s >= task.size);
            if saved {
                batch.complete_count += 1;
                batch.skip_count += 1;
            } else {
                batch.size_total += task.size;
                downloads.push((task, index));
            }
        }
        if downloads.is_empty() {
            self.send(
                host,
                WindowMessage::ShowAlert("No new logs found.".to_owned()),
            );
            return;
        }
        let batch_index = self.batches.len();
        self.batches.push(batch);
        for (task, index) in downloads {
            // A throw leaves the forEach.
            let Ok(token) = host.fast_get(save.sftp, &task.remote, &task.local) else {
                return;
            };
            self.pending.insert(
                token,
                Pending::GetMany {
                    batch: batch_index,
                    index,
                },
            );
        }
    }

    /// `fastGet`'s `step`.
    pub fn on_step(
        &mut self,
        token: Token,
        transferred: f64,
        total: f64,
        host: &mut dyn DownloadHost,
    ) {
        let progress = match self.pending.get(&token) {
            Some(Pending::GetSingle { .. }) => Progress::Transferring {
                current: transferred,
                total,
            },
            Some(Pending::GetMany { batch, index }) => {
                let batch = &mut self.batches[*batch];
                batch.sizes[*index] = transferred;
                Progress::Transferring {
                    current: batch.sizes.iter().fold(0.0, |a, b| a + b),
                    total: batch.size_total,
                }
            }
            _ => return,
        };
        self.send(host, WindowMessage::SetProgress(progress));
    }

    /// The result of [`DownloadHost::fast_get`].
    pub fn on_got(
        &mut self,
        token: Token,
        result: Result<(), String>,
        host: &mut dyn DownloadHost,
    ) {
        match (self.pending.remove(&token), result) {
            (Some(Pending::GetSingle { .. } | Pending::GetMany { .. }), Err(message)) => {
                self.error(&message, host);
            }
            (Some(Pending::GetSingle { save_path }), Ok(())) => {
                self.send(host, WindowMessage::SetProgress(Progress::Finished));
                let name = self
                    .style
                    .basename(&JsString::from(save_path.as_str()))
                    .to_string_lossy();
                let token = host.show_message_box(&OpenLogBox {
                    kind: "question",
                    message: "Open log?",
                    detail: format!("Would you like to open the log file \"{name}\"?"),
                    buttons: ["Open", "Skip"],
                    default_id: 0,
                });
                self.asking.insert(token, save_path);
            }
            (Some(Pending::GetMany { batch, .. }), Ok(())) => {
                let batch = &mut self.batches[batch];
                batch.complete_count += 1;
                if batch.complete_count < batch.total_count {
                    return;
                }
                let plural = |count: usize| if count == 1 { "" } else { "s" };
                let message = if batch.skip_count > 0 {
                    let new_count = batch.complete_count - batch.skip_count;
                    format!(
                        "Saved {new_count} new log{} ({} skipped) to <u>{}</u>",
                        plural(new_count),
                        batch.skip_count,
                        batch.save_path
                    )
                } else {
                    format!(
                        "Saved {} log{} to <u>{}</u>",
                        batch.complete_count,
                        plural(batch.complete_count),
                        batch.save_path
                    )
                };
                self.send(host, WindowMessage::SetProgress(Progress::Finished));
                self.send(host, WindowMessage::ShowAlert(message));
            }
            _ => {}
        }
    }

    /// The answer to the "Open log?" box: the index of the pressed button.
    pub fn on_open_log(&mut self, token: Token, response: usize, host: &mut dyn DownloadHost) {
        let Some(save_path) = self.asking.remove(&token) else {
            return;
        };
        if response == 0 {
            self.destroy_window(host);
            self.stop(host);
            host.focus_hub();
            host.open_files(&[save_path]);
        }
    }
}

/// Whether a folder's entry is downloaded with it.
fn is_saved(file: &DirEntry) -> bool {
    !file.is_directory() && !file.filename.starts_with('.')
}

/// Whether `name` names a file in whichever folder it is joined to, on every platform: no path
/// separator, drive prefix or NUL, and not empty, `.` or `..`.
pub fn is_plain_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    !matches!(name, "" | "." | "..") && !drive && !name.contains(['/', '\\', '\0'])
}

/// The `show-error` message for a refused name; the window reads it as HTML.
fn unsafe_name(name: &str) -> String {
    let mut shown = String::new();
    for c in name.chars() {
        match c {
            '&' => shown.push_str("&amp;"),
            '<' => shown.push_str("&lt;"),
            '>' => shown.push_str("&gt;"),
            c if c.is_control() => shown.push('\u{fffd}'),
            c => shown.push(c),
        }
    }
    format!("The robot sent an unsafe file name: {shown}")
}

fn folder_item(name: String, size: f64) -> ListItem {
    ListItem {
        name,
        size: Some(size),
        is_folder: true,
    }
}
