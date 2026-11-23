#![allow(clippy::disallowed_methods)]

//! File routing and the export flow in the app, headless: `main_process::start` in a fresh home
//! directory, then menu clicks, hub messages and the export prompt's own input, with the dialogs
//! answered by a script. What is decided is `routing_vectors`' business; this checks that the
//! decisions reach the windows, and that the files the app writes are upstream's (`export/runs`).

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use asrs_app::hub::HubEvent;
use asrs_app::main_process::{self, MainProcess};
use asrs_app::menus::bridge::MenuAction;
use asrs_app::menus::dispatch::dispatch;
use asrs_app::menus::model::Command;
use asrs_app::windows::{self, WindowId, WindowKind, Windows};
use asrs_hub::hub::HubModel;
use asrs_js::ops;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::tab_type::TabType;
use asrs_parity::Value;
use asrs_parity::vectors;
use asrs_services::ds_layout::default_ds_layout;
use asrs_ui::gpui::{
    AnyWindowHandle, HeadlessAppContext, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, NoopTextSystem, PlatformInput, point, px,
};
use asrs_ui::platform::dialog::{Reply, Script, Shown};

const HOME: &str = "ASRS_EXPORT_FLOW_HOME";
const TIMEOUT: Duration = Duration::from_secs(60);

fn in_fresh_home(test: &str) {
    let home = std::env::temp_dir().join(format!("asrs-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("home");
    let mut child = Process::new(std::env::current_exe().expect("the test binary"));
    child.args(["--exact", test, "--ignored", "--nocapture"]);
    for variable in [
        "HOME",
        "APPDATA",
        "XDG_CONFIG_HOME",
        "TMPDIR",
        "TEMP",
        "TMP",
        HOME,
    ] {
        child.env(variable, &home);
    }
    let status = child.status().expect("the test binary");
    let _ = std::fs::remove_dir_all(&home);
    assert!(status.success(), "{test}: {status}");
}

#[test]
fn routing_in_the_app() {
    in_fresh_home("routing_in_a_fresh_home");
}

#[test]
fn a_ds_log_argument_asks_at_startup() {
    in_fresh_home("startup_in_a_fresh_home");
}

#[test]
fn exports_in_the_app() {
    in_fresh_home("exports_in_a_fresh_home");
}

struct AppUnderTest {
    cx: HeadlessAppContext,
    home: PathBuf,
    events: async_channel::Sender<String>,
}

impl AppUnderTest {
    fn start(argv: &[String], replies: &[Reply]) -> Option<AppUnderTest> {
        let home = PathBuf::from(std::env::var_os(HOME)?);
        let mut cx = HeadlessAppContext::new(Arc::new(NoopTextSystem::new()));
        let (events, received) = async_channel::unbounded();
        cx.update(|cx| {
            asrs_ui::init(cx);
            windows::init(cx);
            cx.set_global(Script {
                replies: replies.to_vec().into(),
                ..Script::default()
            });
            main_process::start(cx, argv, received);
        });
        let mut app = AppUnderTest { cx, home, events };
        app.settle();
        Some(app)
    }

    fn settle(&mut self) {
        self.cx.run_until_parked();
    }

    fn advance(&mut self, ms: u64) {
        self.cx.advance_clock(Duration::from_millis(ms));
        self.settle();
    }

    fn until(&mut self, what: &str, mut done: impl FnMut(&mut AppUnderTest) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(start.elapsed() < TIMEOUT, "waiting for {what}");
            self.advance(10);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn hubs(&mut self) -> Vec<WindowId> {
        self.cx.update(|cx| Windows::manager(cx).hubs().to_vec())
    }

    fn handle(&mut self, window: WindowId) -> AnyWindowHandle {
        self.cx
            .update(|cx| Windows::handle(cx, window))
            .expect("open")
    }

    fn focus(&mut self, window: WindowId) {
        let handle = self.handle(window);
        self.cx
            .update_window(handle, |_, window, _| window.activate_window())
            .expect("open");
        self.settle();
    }

    fn hub<R>(&mut self, window: WindowId, read: impl FnOnce(&HubModel) -> R) -> R {
        self.cx.update(|cx| {
            let hubs = MainProcess::hub_windows(cx);
            let hub = hubs.iter().find(|(id, _)| *id == window).expect("a hub");
            read(hub.1.upgrade().expect("a live hub").read(cx).model())
        })
    }

    /// A message the hub sends main.
    fn send_main(&mut self, window: WindowId, name: HubToMain, data: JsValue) {
        self.cx.update(|cx| {
            let hubs = MainProcess::hub_windows(cx);
            let hub = hubs.iter().find(|(id, _)| *id == window).expect("a hub");
            hub.1.upgrade().expect("a live hub").update(cx, |hub, cx| {
                hub.update(cx, |_, host| {
                    host.send_main_message(NamedMessage::new(name, data))
                });
            });
        });
    }

    /// Counts the `error` messages the hub sends main from now on.
    fn errors(&mut self, window: WindowId) -> Rc<Cell<usize>> {
        let errors: Rc<Cell<usize>> = Rc::default();
        let count = errors.clone();
        self.cx.update(|cx| {
            let hubs = MainProcess::hub_windows(cx);
            let hub = hubs.iter().find(|(id, _)| *id == window).expect("a hub");
            let hub = hub.1.upgrade().expect("a live hub");
            cx.subscribe(&hub, move |_, event, _| {
                if matches!(event, HubEvent::Main(m) if m.name == HubToMain::Error) {
                    count.set(count.get() + 1);
                }
            })
            .detach();
        });
        errors
    }

    fn logs(&mut self, window: WindowId) -> Vec<String> {
        self.hub(window, |hub| {
            hub.historical_paths()
                .map(|p| p.to_string_lossy())
                .collect()
        })
    }

    fn tabs(&mut self, window: WindowId) -> Vec<Option<TabType>> {
        self.hub(window, |hub| {
            hub.tabs().tabs().map(|t| t.tab_type()).collect()
        })
    }

    /// The progress values the loading glow shows from now on.
    fn watch_glow(&mut self, window: WindowId) -> Rc<RefCell<Vec<JsString>>> {
        let seen: Rc<RefCell<Vec<JsString>>> = Rc::default();
        let shown = seen.clone();
        self.cx.update(|cx| {
            let hubs = MainProcess::hub_windows(cx);
            let hub = hubs.iter().find(|(id, _)| *id == window).expect("a hub");
            let hub = hub.1.upgrade().expect("a live hub");
            cx.observe(&hub, move |hub, cx| {
                let loading = hub.read(cx).model().loading();
                if loading.show == Some(true) {
                    shown.borrow_mut().extend(loading.progress.clone());
                }
            })
            .detach();
        });
        seen
    }

    fn glow(&mut self, window: WindowId) -> Option<bool> {
        self.hub(window, |hub| hub.loading().show)
    }

    fn run(
        &mut self,
        replies: &[Reply],
        act: impl FnOnce(&mut AppUnderTest),
    ) -> Vec<(Option<AnyWindowHandle>, Shown)> {
        self.cx
            .update(|cx| cx.global_mut::<Script>().replies = replies.to_vec().into());
        act(self);
        self.settle();
        self.cx.update(|cx| {
            let script = cx.global_mut::<Script>();
            assert!(
                script.replies.is_empty(),
                "never asked for {:?}",
                script.replies
            );
            std::mem::take(&mut script.shown)
        })
    }

    fn click(
        &mut self,
        command: Command,
        replies: &[Reply],
    ) -> Vec<(Option<AnyWindowHandle>, Shown)> {
        self.run(replies, |app| {
            app.cx.update(|cx| {
                dispatch(
                    MenuAction::Command {
                        command,
                        checked: false,
                    },
                    cx,
                )
            })
        })
    }

    fn click_at(&mut self, handle: AnyWindowHandle, x: f32, y: f32) {
        let position = point(px(x), px(y));
        let (button, modifiers) = (MouseButton::Left, Modifiers::default());
        let inputs = [
            PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers,
            }),
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position,
                modifiers,
                click_count: 1,
                first_mouse: false,
            }),
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position,
                modifiers,
                click_count: 1,
            }),
        ];
        self.cx
            .update_window(handle, |_, window, _| window.refresh())
            .expect("open");
        self.settle();
        self.cx
            .update_window(handle, |_, window, cx| {
                for input in inputs {
                    window.dispatch_event(input, cx);
                }
            })
            .expect("open");
        self.settle();
    }

    fn prompt(&mut self) -> Option<AnyWindowHandle> {
        self.cx
            .update(|cx| Windows::of_kind(cx, WindowKind::Export))
    }

    fn write(&self, name: &str, contents: &[u8]) -> PathBuf {
        let path = self.home.join(name);
        std::fs::write(&path, contents).expect("a file");
        path
    }
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn open(path: &Path) -> Reply {
    Reply::Open(vec![path.to_owned()])
}

fn ds_log() -> Vec<u8> {
    let mut out = b"WPILOG\x00\x01".to_vec();
    out.extend(11u32.to_le_bytes());
    out.extend(b"DS Log File");
    out
}

fn buttons(shown: &[(Option<AnyWindowHandle>, Shown)]) -> Vec<&str> {
    let [.., (_, Shown::Message(options))] = shown else {
        panic!("{shown:?}");
    };
    assert_eq!(options.message, "Open this log using the DS layout?");
    options.buttons.iter().map(String::as_str).collect()
}

fn ds_tabs() -> Vec<Option<TabType>> {
    let layout = default_ds_layout(true);
    layout.tabs.tabs.iter().map(|t| Some(t.tab_type)).collect()
}

fn files(path: &Path) -> JsValue {
    ops::object([("files", JsValue::Array(vec![JsValue::string(&text(path))]))])
}

#[test]
#[ignore = "run by routing_in_the_app with its own home directory"]
fn routing_in_a_fresh_home() {
    let Some(mut app) = AppUnderTest::start(&[], &[]) else {
        return;
    };
    let [hub] = app.hubs()[..] else {
        panic!("not one hub");
    };
    app.focus(hub);
    let plain = app.write("plain.csv", b"Timestamp,Key,Value\n0,/A,1\n");
    let ds = app.write("ds.wpilog", &ds_log());

    let shown = app.run(&[], |app| {
        app.send_main(hub, HubToMain::OpenFilesRequest, files(&plain))
    });
    assert!(shown.is_empty());
    assert_eq!(app.logs(hub), [text(&plain)]);
    app.until("the recent document", |app| {
        app.cx
            .update(|cx| cx.global::<Script>().recent_documents == [plain.clone()])
    });
    let written = std::env::temp_dir().join("akit-log-path.txt");
    assert_eq!(std::fs::read_to_string(&written).ok(), Some(text(&plain)));

    let shown = app.click(Command::OpenLogs, &[open(&ds), Reply::Button(2)]);
    assert_eq!(
        buttons(&shown),
        ["Open in New Window", "Open in This Window", "Cancel"]
    );
    assert_eq!(shown[1].0, Some(app.handle(hub)));
    assert_eq!(app.logs(hub), [text(&plain)]);
    assert_eq!(app.hubs(), [hub]);

    app.click(Command::OpenLogs, &[open(&ds), Reply::Button(0)]);
    let [_, ds_hub] = app.hubs()[..] else {
        panic!("{:?}", app.hubs());
    };
    assert_eq!(app.tabs(ds_hub), ds_tabs());
    assert_eq!(app.logs(ds_hub), [text(&ds)]);
    assert_ne!(app.tabs(hub), ds_tabs());

    app.advance(600);
    app.focus(hub);
    let other = app.write("other.wpilog", &ds_log());
    let shown = app.click(Command::OpenLogs, &[open(&other), Reply::Button(0)]);
    assert_eq!(
        buttons(&shown),
        ["Open in DS Window", "Open in This Window", "Cancel"]
    );
    assert_eq!(app.logs(ds_hub), [text(&other)]);
    assert_eq!(app.hubs(), [ds_hub, hub], "the DS window is focused");

    let shown = app.run(&[], |app| {
        app.send_main(ds_hub, HubToMain::OpenFilesRequest, files(&ds))
    });
    assert!(shown.is_empty(), "a DS window takes DS logs unasked");
    assert_eq!(app.logs(ds_hub), [text(&ds)]);

    app.run(&[], |app| {
        app.events.try_send(text(&plain)).expect("an open event");
    });
    let [_, _, opened] = app.hubs()[..] else {
        panic!("{:?}", app.hubs());
    };
    assert_eq!(app.logs(opened), [text(&plain)]);
    app.focus(hub);
    let shown = app.run(&[Reply::Button(1)], |app| {
        app.events.try_send(text(&ds)).expect("an open event");
    });
    assert_eq!(buttons(&shown).len(), 3);
    assert_eq!(app.logs(hub), [text(&ds)]);
    assert_eq!(app.hubs().len(), 3);

    assert_eq!(std::fs::read_to_string(&written).ok(), Some(text(&ds)));
    app.cx.update(|cx| cx.shutdown());
    assert!(!written.exists(), "quitting withdraws the path");
}

#[test]
#[ignore = "run by a_ds_log_argument_asks_at_startup with its own home directory"]
fn startup_in_a_fresh_home() {
    let Some(home) = std::env::var_os(HOME) else {
        return;
    };
    let ds = PathBuf::from(home).join("ds.wpilog");
    std::fs::write(&ds, ds_log()).expect("a log");
    let argv = ["asrs".to_owned(), text(&ds)];
    let mut app = AppUnderTest::start(&argv, &[Reply::Button(0)]).expect("a home");
    let hubs = app.hubs();
    let [hub, ds_hub] = hubs[..] else {
        panic!("{hubs:?}");
    };
    assert!(app.logs(hub).is_empty());
    assert_eq!(app.logs(ds_hub), [text(&ds)]);
    assert_eq!(app.tabs(ds_hub), ds_tabs());
    let shown = app
        .cx
        .update(|cx| std::mem::take(&mut cx.global_mut::<Script>().shown));
    assert_eq!(buttons(&shown).len(), 3);
}

#[test]
#[ignore = "run by exports_in_the_app with its own home directory"]
fn exports_in_a_fresh_home() {
    let Some(mut app) = AppUnderTest::start(&[], &[]) else {
        return;
    };
    let [hub] = app.hubs()[..] else {
        panic!("not one hub");
    };
    app.focus(hub);
    the_prompt_exports(&mut app, hub);
    every_run_writes_upstreams_bytes(&mut app, hub);
    a_write_that_fails_is_reported(&mut app, hub);
    a_hub_asks_before_closing_on_an_export(&mut app, hub);
    a_close_button_asks_and_takes_the_prompt_along(&mut app);
}

struct Run {
    file: PathBuf,
    options: JsValue,
    bytes: Option<Vec<u8>>,
}

fn runs() -> Vec<Run> {
    let records = vectors::load("export/runs").unwrap_or_else(|e| panic!("{e}"));
    let specs = vectors::workspace_root().join("parity/specs/export");
    let run = |record: &vectors::Record| {
        let value = |key: &str| record.value(key).unwrap_or_else(|_| panic!("{key}"));
        Run {
            file: specs.join(record.string("file").expect("file")),
            options: js_json::to_js(&value("options")).unwrap_or_else(|e| panic!("{e}")),
            bytes: match value("bytes") {
                Value::Null => None,
                Value::Bytes(bytes) => Some(bytes),
                other => panic!("{other:?}"),
            },
        }
    };
    records.iter().map(run).collect()
}

fn open_log(app: &mut AppUnderTest, hub: WindowId, path: &Path) {
    let data = files(path);
    app.run(&[], |app| {
        app.send_main(hub, HubToMain::OpenFilesRequest, data)
    });
    app.until("the log to open", |app| {
        app.hub(hub, |hub| {
            !hub.title().to_string_lossy().contains("(Loading)")
                && hub.page().log().field_keys().next().is_some()
        })
    });
}

fn the_prompt_exports(app: &mut AppUnderTest, hub: WindowId) {
    let shown = app.click(Command::ExportData, &[]);
    assert!(shown.is_empty() && app.prompt().is_none(), "{shown:?}");

    let log = vectors::workspace_root().join("parity/specs/hub/logs/match.csv");
    open_log(app, hub, &log);

    app.click(Command::ExportData, &[]);
    let prompt = app.prompt().expect("the export prompt");
    app.click_at(prompt, 300.0 - 58.0, 189.0 - 23.0);
    assert!(app.prompt().is_none(), "exit closes it");
    assert!(!app.hub(hub, HubModel::is_exporting));

    app.click(Command::ExportData, &[]);
    assert!(app.hub(hub, HubModel::is_exporting));
    let prompt = app.prompt().expect("the export prompt");
    let confirm = (300.0 - 23.0, 189.0 - 23.0);
    let shown = app.run(&[Reply::Save(None)], |app| {
        app.click_at(prompt, confirm.0, confirm.1)
    });
    let [(Some(parent), Shown::Save(options))] = &shown[..] else {
        panic!("{shown:?}");
    };
    assert_eq!(*parent, prompt);
    assert_eq!(options.title, "Select export location for robot log");
    assert_eq!(options.filters[0].extensions, ["csv"]);
    let suggested = options.default_path.as_deref().map(text);
    assert_eq!(suggested, Some(text(&log)), "a .csv log's own path");
    assert_eq!(app.prompt(), Some(prompt), "cancelling keeps it open");

    let glow = app.watch_glow(hub);
    let out = app.home.join("prompt.csv");
    app.run(&[Reply::Save(Some(out.clone()))], |app| {
        app.click_at(prompt, confirm.0, confirm.1)
    });
    assert!(app.prompt().is_none());
    app.until("the export", |app| {
        !app.hub(hub, HubModel::is_exporting) && app.glow(hub) == Some(false)
    });
    let glow = glow.borrow();
    assert!(
        glow.contains(&JsString::from("1")),
        "the worker's last progress never showed: {glow:?}"
    );
    let defaults = ops::object([
        ("format", JsValue::string("csv-table")),
        ("samplingMode", JsValue::string("changes")),
    ]);
    let run = runs()
        .into_iter()
        .find(|run| {
            run.file.ends_with("match.csv")
                && ["format", "samplingMode"]
                    .iter()
                    .all(|key| run.options.get(key) == defaults.get(key))
                && run.options.get("prefixes") == Some(&JsValue::string(""))
                && run.options.get("includeGenerated") == Some(&JsValue::Bool(true))
        })
        .expect("the prompt's default options");
    assert!(std::fs::read(&out).ok() == run.bytes, "prompt.csv differs");
}

fn every_run_writes_upstreams_bytes(app: &mut AppUnderTest, hub: WindowId) {
    let errors = app.errors(hub);
    let (mut written, mut rejected) = (0, 0);
    let mut open = None;
    for (i, run) in runs().into_iter().enumerate() {
        if open.as_ref() != Some(&run.file) {
            open_log(app, hub, &run.file);
            open = Some(run.file.clone());
        }
        let out = app.home.join(format!("run-{i}"));
        let data = ops::object([
            ("path", JsValue::string(&text(&out))),
            ("options", run.options.clone()),
        ]);
        app.cx
            .update(|cx| MainProcess::send_to_hub(cx, hub, MainToHub::PrepareExport, data));
        match run.bytes {
            Some(bytes) => {
                app.until("the file", |_| {
                    std::fs::metadata(&out).is_ok_and(|m| m.len() == bytes.len() as u64)
                });
                let got = std::fs::read(&out).expect("the export");
                assert!(got == bytes, "{:?} {:?}", run.file, run.options);
                written += 1;
            }
            None => {
                rejected += 1;
                app.until("the rejection", |_| errors.get() == rejected);
                assert!(!out.exists(), "{:?} {:?}", run.file, run.options);
            }
        }
        let _ = std::fs::remove_file(&out);
    }
    assert_eq!(errors.get(), rejected);
    assert!(written > 60 && rejected > 10, "{written} {rejected}");
}

fn a_write_that_fails_is_reported(app: &mut AppUnderTest, hub: WindowId) {
    let run = runs().remove(0);
    open_log(app, hub, &run.file);
    let out = app.home.join("missing").join("run.csv");
    let data = ops::object([
        ("path", JsValue::string(&text(&out))),
        ("options", run.options),
    ]);
    let shown = app.run(&[Reply::Button(0)], |app| {
        app.cx
            .update(|cx| MainProcess::send_to_hub(cx, hub, MainToHub::PrepareExport, data));
        app.until("the error", |app| {
            app.cx.update(|cx| !cx.global::<Script>().shown.is_empty())
        });
    });
    let [(_, Shown::Message(options))] = &shown[..] else {
        panic!("{shown:?}");
    };
    assert_eq!(options.message, "The export failed");
    assert!(options.detail.contains("ENOENT"), "{}", options.detail);
    assert!(!out.exists());
    assert!(!app.hub(hub, HubModel::is_exporting));
    assert_eq!(app.glow(hub), Some(true), "upstream's never finishes");
    app.cx.update(|cx| {
        MainProcess::send_to_hub(cx, hub, MainToHub::FinishExport, JsValue::Undefined)
    });
    app.settle();
}

fn a_close_button_asks_and_takes_the_prompt_along(app: &mut AppUnderTest) {
    let [hub] = app.hubs()[..] else {
        panic!("{:?}", app.hubs());
    };
    app.click(Command::NewWindow, &[]);
    app.focus(hub);
    let log = vectors::workspace_root().join("parity/specs/hub/logs/match.csv");
    open_log(app, hub, &log);
    app.click(Command::ExportData, &[]);
    let prompt = app.prompt().expect("the export prompt");
    let handle = app.handle(hub);
    let shown = app.run(&[Reply::Button(0)], |app| {
        assert!(!app.cx.simulate_close(handle));
    });
    let [(Some(parent), Shown::Message(options))] = &shown[..] else {
        panic!("{shown:?}");
    };
    assert_eq!(*parent, handle);
    assert_eq!(options.message, "Export in progress");
    assert!(app.hubs().contains(&hub));
    assert_eq!(app.prompt(), Some(prompt));

    app.run(&[Reply::Button(1)], |app| {
        assert!(!app.cx.simulate_close(handle));
    });
    assert!(!app.hubs().contains(&hub));
    assert!(
        app.prompt().is_none(),
        "a closed hub takes its prompt along"
    );

    let [other] = app.hubs()[..] else {
        panic!("{:?}", app.hubs());
    };
    app.click(Command::NewWindow, &[]);
    let handle = app.handle(other);
    let shown = app.run(&[], |app| assert!(app.cx.simulate_close(handle)));
    assert!(shown.is_empty() && !app.hubs().contains(&other));
}

fn a_hub_asks_before_closing_on_an_export(app: &mut AppUnderTest, hub: WindowId) {
    app.click(Command::NewWindow, &[]);
    app.focus(hub);
    app.send_main(hub, HubToMain::SetExporting, JsValue::Bool(true));
    let shown = app.click(Command::CloseWindow, &[Reply::Button(0)]);
    let [(Some(parent), Shown::Message(options))] = &shown[..] else {
        panic!("{shown:?}");
    };
    assert_eq!(*parent, app.handle(hub));
    assert_eq!(options.message, "Export in progress");
    assert_eq!(options.buttons, ["Don't Close", "Close"]);
    assert!(app.hubs().contains(&hub));
    app.click(Command::CloseWindow, &[Reply::Button(1)]);
    assert!(!app.hubs().contains(&hub));
    let [other] = app.hubs()[..] else {
        panic!("{:?}", app.hubs());
    };
    app.focus(other);
    app.send_main(other, HubToMain::SetExporting, JsValue::Bool(true));
    app.send_main(other, HubToMain::SetExporting, JsValue::Bool(false));
    app.click(Command::NewWindow, &[]);
    app.focus(other);
    assert!(app.click(Command::CloseWindow, &[]).is_empty());
    assert!(!app.hubs().contains(&other));
}
