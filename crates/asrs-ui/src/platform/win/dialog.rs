//! Electron's `message_box_win.cc` and `file_dialog_win.cc`.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use gpui_kit::{App, Window};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize,
};
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOG_COMMON_BUTTON_FLAGS, TASKDIALOGCONFIG, TASKDIALOGCONFIG_0,
    TD_ERROR_ICON, TD_INFORMATION_ICON, TD_WARNING_ICON, TDCBF_CANCEL_BUTTON, TDCBF_CLOSE_BUTTON,
    TDCBF_NO_BUTTON, TDCBF_OK_BUTTON, TDCBF_RETRY_BUTTON, TDCBF_YES_BUTTON,
    TDF_ALLOW_DIALOG_CANCELLATION, TDF_SIZE_TO_CONTENT, TDF_USE_COMMAND_LINKS,
    TDF_VERIFICATION_FLAG_CHECKED, TaskDialogIndirect,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_ALLOWMULTISELECT, FOS_DONTADDTORECENT, FOS_FILEMUSTEXIST, FOS_FORCEFILESYSTEM,
    FOS_OVERWRITEPROMPT, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog,
    IFileDialog, IFileOpenDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{
    IDCANCEL, IDCLOSE, IDNO, IDOK, IDRETRY, IDYES, MESSAGEBOX_RESULT,
};
use windows::core::{BOOL, HSTRING, Interface, PCWSTR, Result};

use super::super::dialog::{
    FileFilter, MessageBoxOptions, MessageBoxResult, MessageBoxType, OpenDialogOptions,
    SaveDialogOptions,
};

/// Button ids Windows does not use.
const FIRST_BUTTON: i32 = 100;

struct Apartment;

impl Apartment {
    fn new() -> Option<Apartment> {
        // SAFETY: no pointers.
        let result = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        result.is_ok().then_some(Apartment)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: balances the `CoInitializeEx` that made `self`.
        unsafe { CoUninitialize() };
    }
}

/// Runs a dialog's modal loop on a thread of its own, so that the window behind it keeps drawing.
fn show<T: Send + 'static>(
    window: Option<&mut Window>,
    cx: &App,
    dialog: impl FnOnce(HWND) -> T + Send + 'static,
    on_done: impl FnOnce(Option<T>, &mut App) + 'static,
) {
    let owner = window
        .and_then(|window| super::hwnd(window))
        // SAFETY: no pointers.
        .unwrap_or_else(|| unsafe { GetActiveWindow() });
    let owner = owner.0 as isize;
    let (sender, receiver) = async_channel::bounded(1);
    let thread = std::thread::Builder::new()
        .name("dialog".to_owned())
        .spawn(move || {
            let apartment = Apartment::new();
            let result = apartment.as_ref().map(|_| dialog(HWND(owner as _)));
            let _ = sender.send_blocking(result);
        });
    if let Err(e) = thread {
        eprintln!("asrs: showing a dialog: {e}");
    }
    cx.spawn(async move |cx| {
        let result = receiver.recv().await.ok().flatten();
        cx.update(|cx| on_done(result, cx));
    })
    .detach();
}

/// `GetApplicationName()`.
fn application_name() -> String {
    let exe = std::env::current_exe().ok();
    exe.as_deref()
        .and_then(Path::file_stem)
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// `GetCommonID`: the button Windows has for a label.
fn common_button(label: &str) -> Option<(MESSAGEBOX_RESULT, TASKDIALOG_COMMON_BUTTON_FLAGS)> {
    Some(match label.to_lowercase().as_str() {
        "ok" => (IDOK, TDCBF_OK_BUTTON),
        "yes" => (IDYES, TDCBF_YES_BUTTON),
        "no" => (IDNO, TDCBF_NO_BUTTON),
        "cancel" => (IDCANCEL, TDCBF_CANCEL_BUTTON),
        "retry" => (IDRETRY, TDCBF_RETRY_BUTTON),
        "close" => (IDCLOSE, TDCBF_CLOSE_BUTTON),
        _ => return None,
    })
}

fn pcwstr(text: &HSTRING) -> PCWSTR {
    PCWSTR(text.as_ptr())
}

fn task_dialog(owner: HWND, options: &MessageBoxOptions) -> MessageBoxResult {
    let title = if options.title.is_empty() {
        HSTRING::from(application_name())
    } else {
        HSTRING::from(&options.title)
    };
    let message = HSTRING::from(&options.message);
    let detail = HSTRING::from(&options.detail);
    let checkbox = HSTRING::from(&options.checkbox_label);
    let mut flags = TDF_SIZE_TO_CONTENT | TDF_ALLOW_DIALOG_CANCELLATION;
    let mut common = TASKDIALOG_COMMON_BUTTON_FLAGS(0);
    let mut config = TASKDIALOGCONFIG {
        cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: owner,
        pszWindowTitle: pcwstr(&title),
        Anonymous1: TASKDIALOGCONFIG_0 {
            pszMainIcon: match options.kind {
                MessageBoxType::None => PCWSTR::null(),
                MessageBoxType::Info | MessageBoxType::Question => TD_INFORMATION_ICON,
                MessageBoxType::Warning => TD_WARNING_ICON,
                MessageBoxType::Error => TD_ERROR_ICON,
            },
        },
        ..Default::default()
    };
    if options.detail.is_empty() {
        config.pszContent = pcwstr(&message);
    } else {
        config.pszMainInstruction = pcwstr(&message);
        config.pszContent = pcwstr(&detail);
    }
    if !options.checkbox_label.is_empty() {
        config.pszVerificationText = pcwstr(&checkbox);
        if options.checkbox_checked {
            flags |= TDF_VERIFICATION_FLAG_CHECKED;
        }
    }

    let ok = ["OK".to_owned()];
    let labels = if options.buttons.is_empty() {
        &ok[..]
    } else {
        &options.buttons
    };
    let texts: Vec<HSTRING> = labels.iter().map(HSTRING::from).collect();
    let mut ids = Vec::new();
    let mut buttons = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        match common_button(label) {
            Some((id, flag)) if !ids.contains(&id.0) => {
                common |= flag;
                ids.push(id.0);
            }
            _ => {
                let id = FIRST_BUTTON + index as i32;
                buttons.push(TASKDIALOG_BUTTON {
                    nButtonID: id,
                    pszButtonText: pcwstr(&texts[index]),
                });
                ids.push(id);
            }
        }
    }
    if !buttons.is_empty() {
        config.cButtons = buttons.len() as u32;
        config.pButtons = buttons.as_ptr();
        flags |= TDF_USE_COMMAND_LINKS;
    }
    config.dwFlags = flags;
    config.dwCommonButtons = common;
    if let Some(&id) = options.default_id.and_then(|index| ids.get(index)) {
        config.nDefaultButton = id;
    }

    let mut pressed = 0;
    let mut checked = BOOL::from(options.checkbox_checked);
    // SAFETY: the strings and buttons `config` points to outlive the call.
    let shown =
        unsafe { TaskDialogIndirect(&config, Some(&mut pressed), None, Some(&mut checked)) };
    if let Err(e) = &shown {
        eprintln!("asrs: showing a message box: {e}");
    }
    let response = ids.iter().position(|&id| shown.is_ok() && id == pressed);
    MessageBoxResult {
        response: response.unwrap_or_else(|| options.cancel_id()),
        checkbox_checked: checked.as_bool(),
    }
}

pub(in super::super) fn message_box(
    window: Option<&mut Window>,
    cx: &mut App,
    options: MessageBoxOptions,
    on_done: impl FnOnce(MessageBoxResult, &mut App) + 'static,
) {
    let failed = MessageBoxResult {
        response: options.cancel_id(),
        checkbox_checked: options.checkbox_checked,
    };
    show(
        window,
        cx,
        move |owner| task_dialog(owner, &options),
        move |result, cx| on_done(result.unwrap_or(failed), cx),
    );
}

/// `ConvertFilters`: each filter's name and its patterns, joined by semicolons.
fn filter_patterns(filters: &[FileFilter]) -> Vec<(String, String)> {
    let pattern = |extension: &String| format!("*.{extension}");
    filters
        .iter()
        .map(|filter| {
            let patterns: Vec<String> = filter.extensions.iter().map(pattern).collect();
            (filter.name.clone(), patterns.join(";"))
        })
        .collect()
}

/// The first filter that is not for every file, and the extension names typed without one get.
fn default_filter(filters: &[FileFilter]) -> Option<(usize, &str)> {
    filters.iter().enumerate().find_map(|(index, filter)| {
        let extension = filter.extensions.first()?;
        (extension != "*").then_some((index, extension.as_str()))
    })
}

/// `ApplySettings`.
fn apply(
    dialog: &IFileDialog,
    title: &str,
    button_label: &str,
    default_path: Option<&Path>,
    filters: &[FileFilter],
) -> Result<()> {
    let patterns: Vec<(HSTRING, HSTRING)> = filter_patterns(filters)
        .into_iter()
        .map(|(name, pattern)| (name.into(), pattern.into()))
        .collect();
    let specs: Vec<COMDLG_FILTERSPEC> = patterns
        .iter()
        .map(|(name, pattern)| COMDLG_FILTERSPEC {
            pszName: pcwstr(name),
            pszSpec: pcwstr(pattern),
        })
        .collect();
    // SAFETY: the strings outlive the calls, which copy them.
    unsafe {
        if let Some(path) = default_path {
            let directory = if path.is_dir() {
                Some(path)
            } else {
                if let Some(name) = path.file_name() {
                    dialog.SetFileName(&HSTRING::from(name))?;
                }
                path.parent()
            };
            if path.is_absolute()
                && let Some(directory) = directory
                && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                    &HSTRING::from(directory.as_os_str()),
                    None,
                )
            {
                dialog.SetFolder(&item)?;
            }
        }
        if !title.is_empty() {
            dialog.SetTitle(&HSTRING::from(title))?;
        }
        if !button_label.is_empty() {
            dialog.SetOkButtonLabel(&HSTRING::from(button_label))?;
        }
        if !specs.is_empty() {
            dialog.SetFileTypes(&specs)?;
        }
        if let Some((index, extension)) = default_filter(filters) {
            dialog.SetFileTypeIndex(index as u32 + 1)?;
            dialog.SetDefaultExtension(&HSTRING::from(extension))?;
        }
    }
    Ok(())
}

fn path(item: &IShellItem) -> Result<PathBuf> {
    // SAFETY: the string is copied before it is freed.
    unsafe {
        let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
        let path = OsString::from_wide(name.as_wide());
        CoTaskMemFree(Some(name.0 as _));
        Ok(PathBuf::from(path))
    }
}

/// The paths chosen; cancelling is an error.
fn choose_open(owner: HWND, options: &OpenDialogOptions) -> Result<Vec<PathBuf>> {
    // SAFETY: COM is initialized on this thread and the pointers are this function's own.
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL)?;
        let mut flags = FOS_FORCEFILESYSTEM | FOS_FILEMUSTEXIST;
        if options.open_directory {
            flags |= FOS_PICKFOLDERS;
        }
        if options.multi_selections {
            flags |= FOS_ALLOWMULTISELECT;
        }
        dialog.SetOptions(flags)?;
        apply(
            &dialog.cast()?,
            &options.title,
            &options.button_label,
            options.default_path.as_deref(),
            &options.filters,
        )?;
        dialog.Show(Some(owner))?;
        let items = dialog.GetResults()?;
        (0..items.GetCount()?)
            .map(|index| path(&items.GetItemAt(index)?))
            .collect()
    }
}

fn choose_save(owner: HWND, options: &SaveDialogOptions) -> Result<PathBuf> {
    // SAFETY: COM is initialized on this thread and the pointers are this function's own.
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_ALL)?;
        dialog.SetOptions(
            FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_OVERWRITEPROMPT | FOS_DONTADDTORECENT,
        )?;
        apply(
            &dialog.cast()?,
            &options.title,
            &options.button_label,
            options.default_path.as_deref(),
            &options.filters,
        )?;
        dialog.Show(Some(owner))?;
        path(&dialog.GetResult()?)
    }
}

pub(in super::super) fn open_dialog(
    window: Option<&mut Window>,
    cx: &mut App,
    options: OpenDialogOptions,
    on_done: impl FnOnce(Vec<PathBuf>, &mut App) + 'static,
) {
    show(
        window,
        cx,
        move |owner| choose_open(owner, &options).unwrap_or_default(),
        move |paths, cx| on_done(paths.unwrap_or_default(), cx),
    );
}

pub(in super::super) fn save_dialog(
    window: Option<&mut Window>,
    cx: &mut App,
    options: SaveDialogOptions,
    on_done: impl FnOnce(Option<PathBuf>, &mut App) + 'static,
) {
    show(
        window,
        cx,
        move |owner| choose_save(owner, &options).ok(),
        move |path, cx| on_done(path.flatten(), cx),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(name: &str, extensions: &[&str]) -> FileFilter {
        FileFilter {
            name: name.to_owned(),
            extensions: extensions.iter().map(|&e| e.to_owned()).collect(),
        }
    }

    #[test]
    fn filters_become_patterns() {
        let filters = [
            filter("Robot logs", &["wpilog", "rlog"]),
            filter("All", &["*"]),
        ];
        assert_eq!(
            filter_patterns(&filters),
            [
                ("Robot logs".to_owned(), "*.wpilog;*.rlog".to_owned()),
                ("All".to_owned(), "*.*".to_owned()),
            ]
        );
        assert_eq!(default_filter(&filters), Some((0, "wpilog")));
        assert_eq!(default_filter(&[filter("All", &["*"])]), None);
        assert_eq!(
            default_filter(&[filter("All", &["*"]), filter("CSV", &["csv"])]),
            Some((1, "csv"))
        );
        assert_eq!(default_filter(&[]), None);
    }

    #[test]
    fn labels_windows_knows_are_its_buttons() {
        assert_eq!(common_button("OK").map(|(id, _)| id), Some(IDOK));
        assert_eq!(common_button("cancel").map(|(id, _)| id), Some(IDCANCEL));
        assert!(common_button("Continue").is_none());
    }

    #[test]
    fn a_dialog_takes_its_settings() {
        let _apartment = Apartment::new().expect("COM");
        let directory = std::env::temp_dir();
        let file = directory.join("log.csv");
        // SAFETY: COM is initialized.
        unsafe {
            let dialog: IFileSaveDialog =
                CoCreateInstance(&FileSaveDialog, None, CLSCTX_ALL).expect("a dialog");
            let filters = [filter("Comma-separated values", &["csv"])];
            let dialog: IFileDialog = dialog.cast().expect("a file dialog");
            apply(&dialog, "Title", "Go", Some(&file), &filters).expect("settings");
            let name = dialog.GetFileName().expect("a name");
            assert_eq!(name.to_string().expect("UTF-16"), "log.csv");
            CoTaskMemFree(Some(name.0 as _));
            assert_eq!(dialog.GetFileTypeIndex().expect("an index"), 1);
        }
    }
}
