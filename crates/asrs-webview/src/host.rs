use std::io::{BufRead, Write};
use std::path::PathBuf;

use gtk::gdk::{self, EventType};
use gtk::glib::{self, Propagation};
use gtk::prelude::*;
use wry::dpi::{PhysicalPosition, PhysicalSize};
use wry::{Rect, WebView, WebViewBuilder, WebViewExtUnix};

use crate::page;
use crate::parent::Parent;
use crate::wire::{FromHost, PageEvent, ToHost};

const USAGE: &str = "usage: asrs-webview <X11 window> dark|light <root> <index> [<embed>...]";

fn send(message: &FromHost) {
    let mut out = std::io::stdout().lock();
    let written = out
        .write_all(message.line().as_bytes())
        .and_then(|()| out.flush());
    if written.is_err() {
        std::process::exit(0);
    }
}

pub fn run() {
    let messages = read();
    let view = match open() {
        Ok(view) => view,
        Err(error) => {
            send(&FromHost::Event(PageEvent::Failed(error)));
            std::process::exit(1);
        }
    };
    send(&FromHost::Ready);
    glib::MainContext::default().spawn_local(async move {
        while let Ok(message) = messages.recv().await {
            if let Err(error) = act(&view, message) {
                eprintln!("asrs-webview: {error}");
            }
        }
        std::process::exit(0);
    });
    gtk::main();
}

fn read() -> async_channel::Receiver<ToHost> {
    let (sender, receiver) = async_channel::unbounded();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if let Some(message) = ToHost::parse(&line) {
                let _ = sender.try_send(message);
            }
        }
    });
    receiver
}

fn open() -> Result<WebView, String> {
    let mut args = std::env::args_os().skip(1);
    let mut text = || args.next()?.into_string().ok();
    let parent = text().and_then(|id| id.parse().ok()).ok_or(USAGE)?;
    let dark = match text().as_deref() {
        Some("dark") => true,
        Some("light") => false,
        _ => return Err(USAGE.to_owned()),
    };
    let root = PathBuf::from(args.next().ok_or(USAGE)?);
    let mut rest = args.map(|arg| arg.into_string().map_err(|_| USAGE.to_owned()));
    let index = rest.next().ok_or(USAGE)??;
    let embeds = rest.collect::<Result<Vec<_>, _>>()?;

    gdk::set_allowed_backends("x11");
    gtk::init().map_err(|e| e.to_string())?;
    set_dark(dark);
    let report = |event| send(&FromHost::Event(event));
    let view = page::builder(WebViewBuilder::new(), root, index, embeds, report)
        .with_visible(false)
        .build_as_child(&Parent(parent))
        .map_err(|e| e.to_string())?;
    view.webview().connect_button_press_event(|widget, event| {
        if event.event_type() == EventType::ButtonPress {
            let scale = f64::from(widget.scale_factor());
            let (x, y) = event.position();
            send(&FromHost::Press(event.button(), [x * scale, y * scale]));
        }
        Propagation::Proceed
    });
    Ok(view)
}

/// WebKitGTK answers `prefers-color-scheme` from GTK's preference.
fn set_dark(dark: bool) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(dark);
    }
}

fn act(view: &WebView, message: ToHost) -> wry::Result<()> {
    match message {
        ToHost::Bounds([x, y, width, height], scale) => {
            // GDK only has integer scales.
            let gdk = f64::from(view.webview().scale_factor().max(1));
            view.zoom(scale / gdk)?;
            view.set_bounds(Rect {
                position: PhysicalPosition::new(x, y).into(),
                size: PhysicalSize::new(width, height).into(),
            })
        }
        ToHost::Visible(visible) => view.set_visible(visible),
        ToHost::Dark(dark) => {
            set_dark(dark);
            Ok(())
        }
        ToHost::Evaluate(script) => view.evaluate_script(&script),
        ToHost::Query(id, script) => view.evaluate_script_with_callback(&script, move |json| {
            send(&FromHost::Result(id, json));
        }),
    }
}
