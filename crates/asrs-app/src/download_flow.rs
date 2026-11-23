//! `main.ts`'s side of the download window: the window, the [`DownloadService`] behind it, and the
//! dialogs its saves show.

use std::path::PathBuf;

use asrs_model::preferences::Preferences;
use asrs_services::download::downloader::{OpenLogBox, SaveLogDialog, Token, WindowMessage};
use asrs_services::download::page::PageEvent;
use asrs_services::download::{DownloadConfig, DownloadEvent, DownloadService};
use asrs_services::path::PathStyle;
use asrs_ui::gpui::{App, Context, Entity, Global, Window};

use crate::main_process::{MainProcess, process_platform};
use crate::menus::dispatch;
use crate::views::download::{self, DownloadWindow};
use crate::views::prompts;
use crate::windows::{self, FileFilter, MessageBoxOptions, SaveDialogOptions, WindowId, Windows};

/// Where a parity scenario's robot listens, in place of port 22 of 127.0.0.1.
#[cfg(feature = "parity")]
pub struct RobotPort(pub u16);

#[cfg(feature = "parity")]
impl Global for RobotPort {}

/// The service, which outlives each window it is for.
struct Downloads {
    service: DownloadService,
    window: Option<(WindowId, Entity<DownloadWindow>)>,
    /// How many [`DownloadEvent::WindowOpened`] are still to come: what comes before them is for
    /// a window that closed.
    unanswered: usize,
}

impl Global for Downloads {}

impl Downloads {
    /// The window an event of the service is for.
    fn addressed(cx: &App) -> Option<(WindowId, Entity<DownloadWindow>)> {
        let downloads = cx.try_global::<Downloads>()?;
        downloads
            .window
            .clone()
            .filter(|_| downloads.unanswered == 0)
    }
}

/// The open download window's view, for parity scenarios and tests.
#[cfg(feature = "parity")]
pub fn view(cx: &App) -> Option<Entity<DownloadWindow>> {
    let downloads = cx.try_global::<Downloads>()?;
    downloads.window.as_ref().map(|(_, view)| view.clone())
}

fn config(cx: &App) -> DownloadConfig {
    #[cfg(feature = "parity")]
    if let Some(&RobotPort(port)) = cx.try_global::<RobotPort>() {
        let mut config = DownloadConfig::new(PathStyle::host());
        let target = config.target;
        config.target = Box::new(move |options| {
            let mut target = target(options);
            if target.host == "127.0.0.1" {
                target.port = port;
            }
            target
        });
        return config;
    }
    let _ = cx;
    DownloadConfig::new(PathStyle::host())
}

/// Starts the service the first time.
fn start(cx: &mut App) -> std::io::Result<()> {
    if cx.has_global::<Downloads>() {
        return Ok(());
    }
    let (service, mut events) = DownloadService::spawn(config(cx))?;
    cx.set_global(Downloads {
        service,
        window: None,
        unanswered: 0,
    });
    cx.spawn(async move |cx| {
        while let Some(event) = events.next().await {
            cx.update(|cx| handle(event, cx));
        }
    })
    .detach();
    Ok(())
}

fn with_service(cx: &App, f: impl FnOnce(&DownloadService)) {
    if let Some(downloads) = cx.try_global::<Downloads>() {
        f(&downloads.service);
    }
}

/// `openDownload(parent)`.
pub fn open(parent: WindowId, cx: &mut App) {
    if let Err(e) = start(cx) {
        eprintln!("asrs: starting the download service: {e}");
        return;
    }
    let (id, view) = match download::open(cx, parent, process_platform()) {
        Ok(Some(opened)) => opened,
        Ok(None) => return,
        Err(e) => {
            eprintln!("asrs: opening the download window: {e}");
            return;
        }
    };
    let downloads = cx.global_mut::<Downloads>();
    downloads.window = Some((id, view.clone()));
    downloads.unanswered += 1;
    downloads.service.window_opened();
    cx.subscribe(&view, move |_, event, cx| page_event(event, id, cx))
        .detach();
    on_closed(id, cx);
    MainProcess::send_all_preferences(cx);
}

fn on_closed(id: WindowId, cx: &mut App) {
    let Some(window) = Windows::handle(cx, id).map(|handle| handle.window_id()) else {
        return;
    };
    cx.on_window_closed(move |cx, closed| {
        if closed != window {
            return;
        }
        let downloads = cx.global_mut::<Downloads>();
        if downloads
            .window
            .as_ref()
            .is_some_and(|(open, _)| *open == id)
        {
            downloads.window = None;
            downloads.service.window_closed();
        }
    })
    .detach();
}

/// The download window's `set-preferences` in `sendAllPreferences`.
pub fn send_preferences(preferences: &Preferences, cx: &mut App) {
    let Some((_, view)) = cx
        .try_global::<Downloads>()
        .and_then(|downloads| downloads.window.clone())
    else {
        return;
    };
    view.update(cx, |view, cx| view.set_preferences(preferences, cx));
}

pub fn stop(cx: &App) {
    with_service(cx, DownloadService::stop);
}

/// `handleDownloadMessage`, and the page's alerts.
fn page_event(event: &PageEvent, id: WindowId, cx: &mut App) {
    match event {
        PageEvent::Start { address, path } => with_service(cx, |service| {
            service.start(&address.to_string_lossy(), &path.to_string_lossy());
        }),
        PageEvent::Close => with_service(cx, DownloadService::close),
        PageEvent::Save(files) => {
            let files = files.iter().flatten().cloned().collect();
            with_service(cx, |service| service.save(files));
        }
        PageEvent::Alert(message) => prompts::alert((*message).to_owned(), id, cx),
    }
}

fn handle(event: DownloadEvent, cx: &mut App) {
    match event {
        DownloadEvent::WindowOpened => cx.global_mut::<Downloads>().unanswered -= 1,
        DownloadEvent::Window(message) => send(&message, cx),
        DownloadEvent::ChooseFolder { token, dialog } => {
            choose(token, cx, |window, cx, done| {
                let options = dispatch::open_dialog_options(&dialog);
                windows::open_dialog(window, cx, options, move |paths, cx| {
                    done(paths.into_iter().next(), cx);
                });
            });
        }
        DownloadEvent::ChooseFile { token, dialog } => {
            choose(token, cx, |window, cx, done| {
                windows::save_dialog(window, cx, save_options(&dialog), done);
            });
        }
        DownloadEvent::AskOpenLog { token, options } => ask_open_log(token, &options, cx),
        DownloadEvent::DestroyWindow => {
            if let Some((id, _)) = Downloads::addressed(cx) {
                cx.global_mut::<Downloads>().window = None;
                windows::close(cx, id);
            }
        }
        DownloadEvent::FocusHub => {
            if let Some((hub, _)) = MainProcess::hub_windows(cx).first() {
                MainProcess::focus(cx, *hub);
            }
        }
        DownloadEvent::OpenFiles(files) => {
            if let Some((hub, _)) = MainProcess::hub_windows(cx).first() {
                MainProcess::send_files(cx, *hub, files);
            }
        }
        DownloadEvent::Uncaught(message) => eprintln!("asrs: download: {message}"),
    }
}

fn send(message: &WindowMessage, cx: &mut App) {
    let Some((id, view)) = Downloads::addressed(cx) else {
        return;
    };
    if let Some(handle) = Windows::handle(cx, id) {
        let message = |view: &mut DownloadWindow,
                       window: &mut Window,
                       cx: &mut Context<'_, DownloadWindow>| {
            view.message(message, window, cx);
        };
        let _ = handle.update(cx, |_, window, cx| {
            view.update(cx, |view, cx| message(view, window, cx));
        });
    }
}

type Chosen = Box<dyn FnOnce(Option<PathBuf>, &mut App)>;

/// Shows a save location dialog on the window and answers the service with its choice.
fn choose(token: Token, cx: &mut App, show: impl FnOnce(Option<&mut Window>, &mut App, Chosen)) {
    let done: Chosen = Box::new(move |path, cx| {
        let path = path.map(|path| path.to_string_lossy().into_owned());
        with_service(cx, |service| service.save_location(token, path));
    });
    match Downloads::addressed(cx) {
        Some((id, _)) => dispatch::with_window(Some(id), cx, |window, cx| show(window, cx, done)),
        None => done(None, cx),
    }
}

fn save_options(dialog: &SaveLogDialog) -> SaveDialogOptions {
    SaveDialogOptions {
        title: dialog.title.to_owned(),
        default_path: Some(PathBuf::from(&dialog.default_path)),
        filters: vec![FileFilter {
            name: dialog.filter_name.to_owned(),
            extensions: vec![dialog.extension.clone()],
        }],
        create_directory: dialog.properties.contains(&"createDirectory"),
        ..SaveDialogOptions::default()
    }
}

fn ask_open_log(token: Token, options: &OpenLogBox, cx: &mut App) {
    let options = MessageBoxOptions {
        kind: dispatch::message_box_type(options.kind),
        message: options.message.to_owned(),
        detail: options.detail.clone(),
        buttons: options.buttons.iter().map(|b| (*b).to_owned()).collect(),
        default_id: Some(options.default_id),
        ..MessageBoxOptions::default()
    };
    if let Some((id, _)) = Downloads::addressed(cx) {
        dispatch::show_message_box(Some(id), options, cx, move |response, cx| {
            with_service(cx, |service| service.open_log(token, response));
        });
    }
}
