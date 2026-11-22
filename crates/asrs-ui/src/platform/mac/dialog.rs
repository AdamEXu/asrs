//! Electron's `message_box_mac.mm` and `file_dialog_mac.mm`.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use block2::RcBlock;
use gpui_kit::{App, AsyncApp, Window};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAlert, NSAlertStyle, NSControlStateValueOff, NSControlStateValueOn, NSModalResponse,
    NSModalResponseOK, NSOpenPanel, NSSavePanel, NSWindow,
};
use objc2_foundation::{NSArray, NSString, NSURL};

use super::super::dialog::{
    FileFilter, MessageBoxOptions, MessageBoxResult, MessageBoxType, OpenDialogOptions,
    SaveDialogOptions,
};
use super::{ns_window, perform_on_run_loop};

fn ns_string(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// Runs `on_done` with the panel's response once, from wherever AppKit calls the handler.
fn completion<T: 'static>(
    cx: AsyncApp,
    read: impl Fn(NSModalResponse) -> T + 'static,
    on_done: impl FnOnce(T, &mut App) + 'static,
) -> RcBlock<dyn Fn(NSModalResponse)> {
    let on_done = Cell::new(Some(on_done));
    RcBlock::new(move |response: NSModalResponse| {
        let Some(on_done) = on_done.take() else {
            return;
        };
        let value = read(response);
        cx.spawn(async move |cx| cx.update(|cx| on_done(value, cx)))
            .detach();
    })
}

/// Starts a panel as a sheet on `parent`, or app-modal without one.
fn present(
    parent: Option<Retained<NSWindow>>,
    sheet: impl FnOnce(&NSWindow) + 'static,
    modal: impl FnOnce() + 'static,
) {
    match parent {
        Some(parent) => sheet(&parent),
        None => perform_on_run_loop(modal),
    }
}

pub(in super::super) fn message_box(
    window: Option<&Window>,
    cx: &mut App,
    options: MessageBoxOptions,
    on_done: impl FnOnce(MessageBoxResult, &mut App) + 'static,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&ns_string(&options.message));
    alert.setInformativeText(&ns_string(&options.detail));
    match options.kind {
        MessageBoxType::Info => alert.setAlertStyle(NSAlertStyle::Informational),
        MessageBoxType::Warning | MessageBoxType::Error => {
            alert.setAlertStyle(NSAlertStyle::Critical);
        }
        MessageBoxType::None | MessageBoxType::Question => {}
    }
    for (i, title) in options.buttons.iter().enumerate() {
        let title = if title.is_empty() { "(empty)" } else { title };
        alert
            .addButtonWithTitle(&ns_string(title))
            .setTag(i as isize);
    }
    let buttons = alert.buttons();
    let count = buttons.count();
    if let Some(default) = options.default_id.filter(|d| *d < count) {
        buttons.objectAtIndex(default).highlight(true);
        buttons.objectAtIndex(0).setKeyEquivalent(&ns_string(""));
        buttons
            .objectAtIndex(default)
            .setKeyEquivalent(&ns_string("\r"));
    }
    let cancel = options.cancel_id();
    if cancel < count {
        buttons
            .objectAtIndex(cancel)
            .setKeyEquivalent(&ns_string("\u{1b}"));
    }
    if !options.checkbox_label.is_empty() {
        alert.setShowsSuppressionButton(true);
        if let Some(checkbox) = alert.suppressionButton() {
            checkbox.setTitle(&ns_string(&options.checkbox_label));
            checkbox.setState(if options.checkbox_checked {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            });
        }
    }

    let read_alert = alert.clone();
    let handler = completion(
        cx.to_async(),
        move |response| MessageBoxResult {
            response: usize::try_from(response).unwrap_or(0),
            checkbox_checked: read_alert
                .suppressionButton()
                .is_some_and(|b| b.state() == NSControlStateValueOn),
        },
        on_done,
    );
    let sheet_alert = alert.clone();
    let sheet_handler = handler.clone();
    present(
        window.and_then(ns_window),
        move |parent| {
            sheet_alert.beginSheetModalForWindow_completionHandler(parent, Some(&sheet_handler));
        },
        move || {
            let response = alert.runModal();
            handler.call((response,));
        },
    );
}

fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&ns_string(&path.to_string_lossy()))
}

fn url_path(url: &NSURL) -> Option<PathBuf> {
    url.isFileURL()
        .then(|| url.path())
        .flatten()
        .map(|p| PathBuf::from(p.to_string()))
}

/// `SetupDialog`: labels, the default directory and file name, and the allowed types.
fn setup(
    panel: &NSSavePanel,
    title: &str,
    button_label: &str,
    message: &str,
    default_path: Option<&Path>,
    filters: &[FileFilter],
) {
    if !title.is_empty() {
        panel.setTitle(Some(&ns_string(title)));
    }
    if !button_label.is_empty() {
        panel.setPrompt(Some(&ns_string(button_label)));
    }
    if !message.is_empty() {
        panel.setMessage(Some(&ns_string(message)));
    }
    let mut directory = None;
    let mut file_name = None;
    if let Some(path) = default_path {
        if path.is_dir() {
            directory = Some(path.to_path_buf());
        } else {
            if path.is_absolute() {
                directory = path.parent().map(Path::to_path_buf);
            }
            file_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        }
    }
    if filters.is_empty() {
        panel.setAllowsOtherFileTypes(true);
    } else {
        set_allowed_file_types(panel, filters);
    }
    panel.setExtensionHidden(false);
    if let Some(directory) = directory {
        panel.setDirectoryURL(Some(&file_url(&directory)));
    }
    if let Some(name) = file_name {
        panel.setNameFieldStringValue(&ns_string(&name));
    }
}

/// `SetAllowedFileTypes`: every filter's extensions, unless one of them is `*`.
#[allow(deprecated)]
fn set_allowed_file_types(panel: &NSSavePanel, filters: &[FileFilter]) {
    let mut types: Vec<&str> = Vec::new();
    for extension in filters.iter().flat_map(|f| &f.extensions) {
        if extension == "*" {
            return;
        }
        if !types.contains(&extension.as_str()) {
            types.push(extension);
        }
    }
    let types: Vec<Retained<NSString>> = types.into_iter().map(ns_string).collect();
    panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&types)));
}

pub(in super::super) fn open_dialog(
    window: Option<&Window>,
    cx: &mut App,
    options: OpenDialogOptions,
    on_done: impl FnOnce(Vec<PathBuf>, &mut App) + 'static,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let panel = NSOpenPanel::openPanel(mtm);
    setup(
        &panel,
        &options.title,
        &options.button_label,
        &options.message,
        options.default_path.as_deref(),
        &options.filters,
    );
    panel.setCanChooseFiles(options.open_file);
    if options.open_directory {
        panel.setCanChooseDirectories(true);
    }
    if options.create_directory {
        panel.setCanCreateDirectories(true);
    }
    if options.multi_selections {
        panel.setAllowsMultipleSelection(true);
    }
    let read_panel = panel.clone();
    let handler = completion(
        cx.to_async(),
        move |response| {
            if response != NSModalResponseOK {
                return Vec::new();
            }
            read_panel
                .URLs()
                .iter()
                .filter_map(|url| url_path(&url))
                .collect()
        },
        on_done,
    );
    begin(
        window.and_then(ns_window),
        Retained::into_super(panel),
        handler,
    );
}

pub(in super::super) fn save_dialog(
    window: Option<&Window>,
    cx: &mut App,
    options: SaveDialogOptions,
    on_done: impl FnOnce(Option<PathBuf>, &mut App) + 'static,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let panel = NSSavePanel::savePanel(mtm);
    setup(
        &panel,
        &options.title,
        &options.button_label,
        &options.message,
        options.default_path.as_deref(),
        &options.filters,
    );
    if options.create_directory {
        panel.setCanCreateDirectories(true);
    }
    let read_panel = panel.clone();
    let handler = completion(
        cx.to_async(),
        move |response| {
            (response == NSModalResponseOK)
                .then(|| read_panel.URL())
                .flatten()
                .and_then(|url| url_path(&url))
        },
        on_done,
    );
    begin(window.and_then(ns_window), panel, handler);
}

fn begin(
    parent: Option<Retained<NSWindow>>,
    panel: Retained<NSSavePanel>,
    handler: RcBlock<dyn Fn(NSModalResponse)>,
) {
    let modal_panel = panel.clone();
    let modal_handler = handler.clone();
    present(
        parent,
        move |parent| panel.beginSheetModalForWindow_completionHandler(parent, &handler),
        move || modal_panel.beginWithCompletionHandler(&modal_handler),
    );
}
