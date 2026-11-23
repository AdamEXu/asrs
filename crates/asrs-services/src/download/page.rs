//! `src/download.ts` as a headless model: a view shows its rows, alert and progress bar, reports
//! what the user does, and acts on the [`PageEvent`]s it gets back.

use std::collections::{BTreeMap, HashMap, VecDeque};

use asrs_js::collation;
use asrs_js::math;
use asrs_js::number;
use asrs_js::sort::array_sort;
use asrs_js::string::{JsError, JsString};
use asrs_log::util::zfill;
use asrs_model::preferences::Preferences;

use super::downloader::{ListItem, Progress, WindowMessage};
use super::window::{friendly_error, normalize_address};

pub const FILE_ITEM_HEIGHT_PX: f64 = 25.0;
pub const BOTTOM_FILLER_MARGIN_PX: f64 = 5.0;

/// What the page does that main or the user sees.
#[derive(Clone, Debug, PartialEq)]
pub enum PageEvent {
    /// The `start` message.
    Start { address: JsString, path: JsString },
    /// The `close` message.
    Close,
    /// The `save` message: the selected names in selection order. `None` is the `undefined` a
    /// range reaching past the list selects.
    Save(Vec<Option<String>>),
    /// `alert(message)`.
    Alert(&'static str),
    /// An exception thrown out of an event handler.
    Error(JsError),
}

/// The `img` of a file's row.
#[derive(Clone, Debug, PartialEq)]
pub struct RowIcon {
    /// Relative to `icons/download/`; `None` where the platform sets no `src`.
    pub file: Option<String>,
    pub class: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowItem {
    pub name: String,
    pub size: String,
    pub icon: RowIcon,
}

/// A `div.file-item`; one without an item is a filler row.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub item: Option<RowItem>,
    pub selected: bool,
}

/// `selectedFiles`.
#[derive(Default)]
struct Selection {
    order: BTreeMap<u64, Option<String>>,
    positions: HashMap<Option<String>, VecDeque<u64>>,
    next: u64,
}

impl Selection {
    fn of(names: &[String]) -> Selection {
        let mut selection = Selection::default();
        for name in names {
            selection.push(Some(name.clone()));
        }
        selection
    }

    fn includes(&self, name: &Option<String>) -> bool {
        self.positions.contains_key(name)
    }

    fn push(&mut self, name: Option<String>) {
        self.next += 1;
        self.positions
            .entry(name.clone())
            .or_default()
            .push_back(self.next);
        self.order.insert(self.next, name);
    }

    fn remove(&mut self, name: &Option<String>) {
        let Some(positions) = self.positions.get_mut(name) else {
            return;
        };
        if let Some(first) = positions.pop_front() {
            self.order.remove(&first);
        }
        if positions.is_empty() {
            self.positions.remove(name);
        }
    }

    fn len(&self) -> usize {
        self.order.len()
    }
}

pub struct DownloadPage {
    platform: String,
    remote_path: JsString,
    address: JsString,
    /// The height of `div.file-list`.
    list_height: f64,
    loading: bool,
    start_time: Option<f64>,
    alert_is_error: bool,
    filenames: Vec<String>,
    selected: Selection,
    last_clicked: Option<usize>,
    last_clicked_select: bool,
    rows: Vec<Row>,
    scrolls: bool,
    blurred: bool,
    alert: Option<JsString>,
    alert_hidden: bool,
    progress_hidden: bool,
    progress: f64,
    details: String,
}

impl Default for DownloadPage {
    fn default() -> DownloadPage {
        DownloadPage {
            platform: String::new(),
            remote_path: JsString::from("undefined"),
            address: JsString::new(),
            list_height: 0.0,
            loading: true,
            start_time: None,
            alert_is_error: false,
            filenames: Vec::new(),
            selected: Selection::default(),
            last_clicked: None,
            last_clicked_select: true,
            rows: Vec::new(),
            scrolls: false,
            blurred: false,
            alert: None,
            alert_hidden: true,
            progress_hidden: true,
            progress: 0.0,
            details: String::new(),
        }
    }
}

impl DownloadPage {
    /// Whether the loading animation shows.
    pub fn loading(&self) -> bool {
        self.loading
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Whether `div.file-list` is `overflow: auto` rather than `hidden`.
    pub fn scrolls(&self) -> bool {
        self.scrolls
    }

    /// The buttons' `blurred` class.
    pub fn blurred(&self) -> bool {
        self.blurred
    }

    /// The HTML of `div.alert-text` while it shows.
    pub fn alert(&self) -> Option<&JsString> {
        self.alert.as_ref().filter(|_| !self.alert_hidden)
    }

    /// The value of the `progress` element and the text of `div.progress-details` while they
    /// show.
    pub fn progress(&self) -> Option<(f64, &str)> {
        (!self.progress_hidden).then_some((self.progress, self.details.as_str()))
    }

    /// `process.platform`.
    pub fn platform(&self) -> &str {
        &self.platform
    }

    pub fn set_platform(&mut self, platform: &str) {
        platform.clone_into(&mut self.platform);
    }

    pub fn set_preferences(&mut self, preferences: &Preferences) -> PageEvent {
        self.remote_path = preferences.remote_path.clone();
        self.address = normalize_address(&preferences.robot_address_for(&self.platform));
        PageEvent::Start {
            address: self.address.clone(),
            path: self.remote_path.clone(),
        }
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.blurred = !focused;
    }

    /// A message of the download service; `now_ms` is the current `Date`. An error is what the
    /// message handler throws.
    pub fn message(&mut self, message: &WindowMessage, now_ms: f64) -> Result<(), JsError> {
        match message {
            WindowMessage::ShowError(message) => {
                self.loading = true;
                self.start_time = None;
                self.rows.clear();
                self.show_alert(friendly_error(message, &self.address, &self.remote_path));
                self.alert_is_error = true;
            }
            WindowMessage::ShowAlert(html) => {
                self.start_time = None;
                self.alert_is_error = false;
                self.show_alert(JsString::from(html.as_str()));
            }
            WindowMessage::SetProgress(progress) => return self.set_progress(progress, now_ms),
            WindowMessage::SetList(list) => self.set_list(list),
        }
        Ok(())
    }

    fn show_alert(&mut self, html: JsString) {
        self.alert = Some(html);
        self.alert_hidden = false;
        self.progress_hidden = true;
    }

    fn set_progress(&mut self, progress: &Progress, now_ms: f64) -> Result<(), JsError> {
        self.alert_hidden = true;
        self.progress_hidden = false;
        self.alert_is_error = false;
        match *progress {
            Progress::Preparing => {
                self.progress = 0.0;
                "Preparing".clone_into(&mut self.details);
            }
            Progress::Finished => {
                self.progress = 1.0;
                "Finished".clone_into(&mut self.details);
            }
            Progress::Transferring { current, total } => {
                let now = now_ms / 1000.0;
                let elapsed = now - *self.start_time.get_or_insert(now);
                let details = details_text(current, total, elapsed);
                let value = if total == 0.0 { 0.0 } else { current / total };
                if !value.is_finite() {
                    return Err(JsError::type_error(
                        "Failed to set the 'value' property on 'HTMLProgressElement': The provided double value is non-finite.",
                    ));
                }
                self.progress = math::min2(math::max2(value, 0.0), 1.0);
                self.details = details;
            }
        }
        Ok(())
    }

    fn set_list(&mut self, list: &[ListItem]) {
        self.loading = false;
        if self.alert_is_error {
            self.alert_hidden = true;
            self.progress_hidden = true;
        }
        let mut list: Vec<(bool, JsString, &ListItem)> = list
            .iter()
            .map(|file| {
                let name = JsString::from(file.name.as_str());
                (is_randomized(&file.name), name, file)
            })
            .collect();
        array_sort(&mut list, |(a_random, a, _), (b_random, b, _)| {
            a_random
                .cmp(b_random)
                .then_with(|| collation::locale_compare(b, a))
        });
        self.filenames = list.iter().map(|(_, _, file)| file.name.clone()).collect();
        self.rows = list
            .iter()
            .map(|(_, _, file)| Row {
                selected: self.selected.includes(&Some(file.name.clone())),
                item: Some(RowItem {
                    name: file.name.clone(),
                    size: size_text(file.size),
                    icon: self.icon(file),
                }),
            })
            .collect();
        self.update_filler();
    }

    fn icon(&self, file: &ListItem) -> RowIcon {
        if file.is_folder {
            return RowIcon {
                file: Some("folder-icon.png".to_owned()),
                class: Some("folder"),
            };
        }
        let extension = file.name.rsplit('.').next().unwrap_or_default();
        let extension = if extension == "wpilogxz" {
            "wpilog"
        } else {
            extension
        };
        match self.platform.as_str() {
            "darwin" => RowIcon {
                file: Some(format!("{extension}-icon-mac.png")),
                class: Some("mac"),
            },
            "linux" | "win32" => RowIcon {
                file: Some(format!("{extension}-icon-linuxwin.png")),
                class: None,
            },
            _ => RowIcon {
                file: None,
                class: None,
            },
        }
    }

    /// The window's `resize` event, with the new height of `div.file-list`.
    pub fn resize(&mut self, list_height: f64) {
        self.list_height = list_height;
        self.update_filler();
    }

    fn update_filler(&mut self) {
        if self.loading {
            return;
        }
        let items = self.rows.iter().filter(|row| row.item.is_some()).count();
        let target = math::ceil(
            (self.list_height - BOTTOM_FILLER_MARGIN_PX - items as f64 * FILE_ITEM_HEIGHT_PX)
                / FILE_ITEM_HEIGHT_PX,
        );
        let target = math::max2(target, 0.0) as usize;
        self.rows.resize(
            items + target,
            Row {
                item: None,
                selected: false,
            },
        );
        self.scrolls = target == 0;
    }

    /// A click on a file's row. An error is what the click handler throws.
    pub fn click(&mut self, index: usize, shift: bool) -> Result<(), JsError> {
        match self.last_clicked {
            Some(last) if shift => {
                for i in index.min(last)..=index.max(last) {
                    let name = self.filenames.get(i).cloned();
                    let included = self.selected.includes(&name);
                    if included == self.last_clicked_select {
                        continue;
                    }
                    if self.last_clicked_select {
                        self.selected.push(name);
                    } else {
                        self.selected.remove(&name);
                    }
                    match self.rows.get_mut(i) {
                        Some(row) => row.selected = self.last_clicked_select,
                        None => {
                            return Err(JsError::type_error(
                                "Cannot read properties of undefined (reading 'classList')",
                            ));
                        }
                    }
                }
            }
            _ => {
                let Some(name) = self.filenames.get(index).cloned() else {
                    return Ok(());
                };
                let name = Some(name);
                let select = !self.selected.includes(&name);
                if select {
                    self.selected.push(name);
                } else {
                    self.selected.remove(&name);
                }
                self.rows[index].selected = select;
                self.last_clicked = Some(index);
                self.last_clicked_select = select;
            }
        }
        Ok(())
    }

    /// The Enter key and the download button.
    pub fn save(&self) -> PageEvent {
        if self.selected.len() == 0 {
            PageEvent::Alert("Please select a log to download.")
        } else {
            PageEvent::Save(self.selected.order.values().cloned().collect())
        }
    }

    /// Cmd or Ctrl and A.
    pub fn select_all(&mut self) {
        let deselect = self.filenames.len() == self.selected.len();
        let names: &[String] = if deselect { &[] } else { &self.filenames };
        self.selected = Selection::of(names);
        for (i, row) in self.rows.iter_mut().enumerate() {
            if deselect {
                row.selected = false;
            } else if i < self.filenames.len() {
                row.selected = true;
            }
        }
    }
}

fn is_randomized(name: &str) -> bool {
    name.contains("TBD")
        || ((name.starts_with("Log_") || name.starts_with("akit_")) && !name.contains('-'))
}

fn size_text(size: Option<f64>) -> String {
    let size = size.unwrap_or(f64::NAN);
    if size < 1e5 {
        return " (<0.1 MB)".to_owned();
    }
    let tenths = math::round(size / 1e5) / 10.0;
    format!(" ({} MB)", number::to_string(tenths))
}

fn details_text(current: f64, total: f64, elapsed: f64) -> String {
    let megabytes = |bytes: f64| number::to_string(math::floor(bytes / 1e6));
    let mut text = format!("{}MB / {}MB", megabytes(current), megabytes(total));
    if elapsed > 0.5 && current > 1e6 {
        let speed = math::round(current / elapsed / 1e6 * 8.0);
        let remaining = math::floor(elapsed / current * (total - current));
        let minutes = math::floor(remaining / 60.0);
        let seconds = remaining - minutes * 60.0;
        let seconds = zfill(&JsString::from(number::to_string(seconds)), 2.0).to_string_lossy();
        text.push_str(&format!(
            " ({}Mb/s, {}m{seconds}s)",
            number::to_string(speed),
            number::to_string(minutes)
        ));
    }
    text
}

/// A stretch of an alert's text.
#[derive(Clone, Debug, PartialEq)]
pub struct AlertRun {
    pub text: String,
    pub underline: bool,
}

/// The text `div.alert-text` shows for `html`: `u` elements and character references as Blink
/// reads them, white space collapsed, and any other tag left out with its content kept.
pub fn alert_runs(html: &JsString) -> Vec<AlertRun> {
    let html = html.to_string_lossy();
    let mut runs: Vec<AlertRun> = Vec::new();
    let mut depth = 0usize;
    let mut space = false;
    let mut push = |c: char, underline: bool, runs: &mut Vec<AlertRun>| {
        let collapsible = matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}');
        if collapsible && (space || runs.is_empty()) {
            return;
        }
        space = collapsible;
        let c = if collapsible { ' ' } else { c };
        match runs.last_mut() {
            Some(run) if run.underline == underline => run.text.push(c),
            _ => runs.push(AlertRun {
                text: String::from(c),
                underline,
            }),
        }
    };
    let mut rest = html.as_str();
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some((tag, after)) = tag(rest)
        {
            match tag {
                Tag::OpenUnderline => depth += 1,
                Tag::CloseUnderline => depth = depth.saturating_sub(1),
                Tag::Other => {}
            }
            rest = after;
        } else if c == '&'
            && let Some((decoded, after)) = character_reference(rest)
        {
            push(decoded, depth > 0, &mut runs);
            rest = after;
        } else {
            push(c, depth > 0, &mut runs);
            rest = &rest[c.len_utf8()..];
        }
    }
    if let Some(last) = runs.last_mut() {
        if last.text.ends_with(' ') {
            last.text.pop();
        }
        if last.text.is_empty() {
            runs.pop();
        }
    }
    runs
}

enum Tag {
    OpenUnderline,
    CloseUnderline,
    Other,
}

/// The tag `text` starts with and what follows it; `None` when its `<` is text.
fn tag(text: &str) -> Option<(Tag, &str)> {
    let body = &text[1..];
    let (closing, body) = match body.strip_prefix('/') {
        Some(body) => (true, body),
        None => (false, body),
    };
    let starts_name = body.starts_with(|c: char| c.is_ascii_alphabetic());
    let bogus_comment = closing && !body.is_empty() || body.starts_with(['!', '?']);
    if !starts_name && !bogus_comment {
        return None;
    }
    let (inside, after) = body.split_once('>').unwrap_or((body, ""));
    let name_end = inside
        .find(|c: char| c.is_ascii_whitespace() || c == '/')
        .unwrap_or(inside.len());
    let underline = starts_name && inside[..name_end].eq_ignore_ascii_case("u");
    let tag = match (underline, closing) {
        (true, false) => Tag::OpenUnderline,
        (true, true) => Tag::CloseUnderline,
        _ => Tag::Other,
    };
    Some((tag, after))
}

fn character_reference(text: &str) -> Option<(char, &str)> {
    let (name, after) = text[1..].split_once(';')?;
    let decoded = match name {
        "amp" | "AMP" => '&',
        "lt" | "LT" => '<',
        "gt" | "GT" => '>',
        "quot" | "QUOT" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        _ => {
            let digits = name.strip_prefix('#')?;
            let (digits, radix) = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => (hex, 16),
                None => (digits, 10),
            };
            let code = u32::from_str_radix(digits, radix).ok()?;
            char::from_u32(code).filter(|c| !c.is_control())?
        }
    };
    Some((decoded, after))
}
