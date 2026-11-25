#![allow(clippy::disallowed_methods)]

//! How often the app, headless, draws its windows while its frame loops run every frame, with
//! nothing changing, a log playing and input: upstream's `requestAnimationFrame` loops run at the
//! display's rate too, and Chromium paints only what they changed.

#[path = "support/child.rs"]
mod child;

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use asrs_app::hub::Hub;
use asrs_app::main_process::{self, MainProcess, SatelliteConfig};
use asrs_app::satellite::Satellite;
use asrs_app::windows::{self, WindowId, Windows};
use asrs_js::ops;
use asrs_js::value::JsValue;
use asrs_model::messages::{MainToHub, NamedMessage};
use asrs_model::selection::Selection;
use asrs_model::tab_type::TabType;
use asrs_ui::gpui::{
    AnyWindowHandle, AppContext, Entity, MouseMoveEvent, PlatformInput, TestAppContext,
    WindowVisibility, point, px,
};

const HOME: &str = "ASRS_IDLE_REDRAW_HOME";
const UUID: &str = "idle-redraw";
const FRAME: Duration = Duration::from_nanos(16_666_666);
const DECODE: Duration = Duration::from_secs(120);

macro_rules! in_the_app {
    ($($test:ident, $body:ident;)*) => {$(
        #[test]
        fn $test() {
            run_in_a_fresh_home(stringify!($body));
        }

        #[test]
        #[ignore = "run by another test, in a fresh home"]
        fn $body() {
            if std::env::var_os(HOME).is_some() {
                body::$body();
            }
        }
    )*};
}

in_the_app! {
    an_idle_page_is_not_drawn, idle;
    a_playing_log_is_drawn_every_frame, playing;
    what_changes_is_drawn, changes;
    a_hidden_hub_draws_its_satellite_only_for_changes, hidden;
}

fn run_in_a_fresh_home(test: &str) {
    let home = std::env::temp_dir().join(format!("asrs-idle-{test}-{}", std::process::id()));
    std::fs::create_dir_all(&home).expect("home");
    let mut child = Command::new(std::env::current_exe().expect("the test binary"));
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
    let status = child::run(&mut child, test);
    let _ = std::fs::remove_dir_all(&home);
    assert!(status.success(), "{test}: {status}");
}

/// The app's hub, with a satellite of its one tab beside Documentation.
struct Page {
    cx: TestAppContext,
    id: WindowId,
    hub: Entity<Hub>,
    window: AnyWindowHandle,
    satellite: Option<(Entity<Satellite>, AnyWindowHandle)>,
}

impl Page {
    fn start() -> Page {
        let cx = TestAppContext::single();
        let (_send, events) = async_channel::unbounded();
        cx.update(|cx| {
            asrs_ui::init(cx);
            windows::init(cx);
            main_process::start(cx, &[], events);
        });
        cx.run_until_parked();
        let (id, hub, window) = cx.update(|cx| {
            let (id, hub) = MainProcess::hub_windows(cx).remove(0);
            let window = Windows::handle(cx, id).expect("the hub's window");
            (id, hub.upgrade().expect("the hub"), window)
        });
        Page {
            cx,
            id,
            hub,
            window,
            satellite: None,
        }
    }

    fn message(&mut self, name: MainToHub, data: JsValue) {
        let hub = self.hub.clone();
        self.cx.update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.handle_main_message(NamedMessage::new(name, data), cx)
            })
        });
        self.cx.run_until_parked();
    }

    fn select(&mut self, kind: TabType) {
        if let Some((_, window)) = self.satellite.take() {
            self.cx
                .update_window(window, |_, window, _| window.remove_window())
                .expect("the window is open");
            self.cx.run_until_parked();
        }
        self.message(MainToHub::RestoreState, restore_state(kind));
        let parent = self.id;
        self.satellite = self.cx.update(|cx| {
            let config = SatelliteConfig::New {
                parent,
                uuid: JsValue::string(UUID),
                type_value: JsValue::Number(kind.number()),
            };
            let opened = MainProcess::open_satellite(cx, config)?;
            let pages = MainProcess::satellite_windows(cx);
            let (_, page) = pages.into_iter().find(|(id, _)| *id == opened)?;
            Some((page, Windows::handle(cx, opened)?))
        });
        assert!(self.satellite.is_some());
        self.cx.run_until_parked();
    }

    fn open_log(&mut self) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let log = root.join("parity/specs/hub/logs/corpus-a-struct-wpilib.wpilog");
        let log = JsValue::string(&log.to_string_lossy());
        let data = ops::object([
            ("files", JsValue::Array(vec![log])),
            ("merge", JsValue::Bool(false)),
        ]);
        self.message(MainToHub::OpenFiles, data);
        loop {
            self.frames(2);
            let waited = self
                .cx
                .read(|cx| self.hub.read(cx).wait_for_threads(DECODE));
            if !waited.expect("a decode thread stopped without finishing") {
                break;
            }
        }
        let fields = self.cx.read(|cx| {
            let log = self.hub.read(cx).model().page().log();
            log.field_keys().count()
        });
        assert!(fields > 0);
    }

    fn selection(&mut self, f: impl FnOnce(&mut dyn Selection)) {
        let hub = self.hub.clone();
        self.cx.update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.update(cx, |model, _| f(model.page_mut().selection_mut()))
            })
        });
        self.cx.run_until_parked();
    }

    fn range(&self) -> [f64; 2] {
        self.cx.read(|cx| {
            let log = self.hub.read(cx).model().page().log();
            log.get_timestamp_range()
        })
    }

    fn frame(&mut self, window: AnyWindowHandle) {
        self.cx
            .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
            .expect("the window is open");
        self.cx.run_until_parked();
    }

    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.cx.background_executor.advance_clock(FRAME);
            self.cx.run_until_parked();
            self.frame(self.window);
            if let Some((_, window)) = &self.satellite {
                self.frame(*window);
            }
        }
    }

    fn draws(&self) -> [u64; 2] {
        self.cx.read(|cx| {
            let satellite = self.satellite.as_ref();
            let satellite = satellite.map_or(0, |(page, _)| page.read(cx).draws());
            [self.hub.read(cx).draws(), satellite]
        })
    }

    /// How often the hub and the satellite are drawn in `count` frames.
    fn drawn_in(&mut self, count: usize) -> [u64; 2] {
        let before = self.draws();
        self.frames(count);
        let after = self.draws();
        [after[0] - before[0], after[1] - before[1]]
    }

    /// Long enough for every transition, and for the loads a tab starts.
    fn settle(&mut self) {
        for _ in 0..20 {
            self.frames(15);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[track_caller]
    fn assert_idle(&mut self, what: &str) {
        self.settle();
        assert_eq!(self.drawn_in(120), [0, 0], "{what}");
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        let event = PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x), px(y)),
            ..MouseMoveEvent::default()
        });
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(event, cx)
            })
            .expect("the window is open");
        self.cx.run_until_parked();
    }
}

fn restore_state(kind: TabType) -> JsValue {
    let tab = |kind: TabType, uuid: &str| {
        ops::object([
            ("type", JsValue::Number(kind.number())),
            ("title", JsValue::string("")),
            ("controller", JsValue::Null),
            ("controllerUUID", JsValue::string(uuid)),
            ("renderer", JsValue::Null),
            ("controlsHeight", JsValue::Number(200.0)),
        ])
    };
    let tabs = vec![tab(TabType::Documentation, "documentation"), tab(kind, UUID)];
    let sidebar = ops::object([
        ("width", JsValue::Number(300.0)),
        ("expanded", JsValue::Array(Vec::new())),
    ]);
    let tabs = ops::object([
        ("selected", JsValue::Number(1.0)),
        ("tabs", JsValue::Array(tabs)),
    ]);
    ops::object([("sidebar", sidebar), ("tabs", tabs)])
}

mod body {
    use super::*;

    pub fn idle() {
        let mut page = Page::start();
        page.assert_idle("the app as it starts");
        for kind in TabType::ALL {
            page.select(kind);
            page.assert_idle(&format!("{kind:?} without a log"));
        }
        page.open_log();
        for kind in TabType::ALL {
            page.select(kind);
            page.assert_idle(&format!("{kind:?} with a log"));
            let [start, end] = page.range();
            page.selection(|selection| selection.set_selected_time((start + end) / 2.0));
            page.assert_idle(&format!("{kind:?} with a paused log"));
            page.selection(|selection| selection.go_idle());
        }
    }

    pub fn playing() {
        let mut page = Page::start();
        page.open_log();
        for kind in TabType::ALL {
            page.select(kind);
            page.settle();
            let [start, _] = page.range();
            page.selection(|selection| {
                selection.set_selected_time(start);
                selection.set_playback_speed(1e-3);
                selection.play();
            });
            assert_eq!(page.drawn_in(60), [60, 60], "{kind:?}");
            page.selection(|selection| selection.pause());
            page.assert_idle(&format!("{kind:?} paused"));
        }
    }

    pub fn changes() {
        let mut page = Page::start();
        page.open_log();
        page.select(TabType::LineGraph);
        page.assert_idle("before");

        let rename = ops::object([
            ("index", JsValue::Number(1.0)),
            ("name", JsValue::string("Renamed")),
        ]);
        page.message(MainToHub::RenameTab, rename);
        let [hub, satellite] = page.drawn_in(30);
        assert!((1..10).contains(&hub), "a message: {hub}");
        assert!((1..10).contains(&satellite), "a message: {satellite}");
        page.assert_idle("after a message");

        let hover = |page: &Page| {
            page.cx.read(|cx| {
                let page = page.hub.read(cx).model().page();
                page.selection().get_hovered_time()
            })
        };
        let mut shown = Vec::new();
        for x in [600.0, 700.0] {
            page.mouse_move(x, 200.0);
            let [hub, satellite] = page.drawn_in(30);
            assert!((1..15).contains(&hub), "the pointer: {hub}");
            assert!((1..15).contains(&satellite), "the pointer: {satellite}");
            shown.push(hover(&page).expect("a hovered time"));
        }
        assert_ne!(shown[0], shown[1]);
        page.assert_idle("after the pointer");
    }

    pub fn hidden() {
        let mut page = Page::start();
        page.open_log();
        page.select(TabType::LineGraph);
        page.assert_idle("before");
        let window = page.window;
        page.cx
            .simulate_window_visibility_change(window, WindowVisibility::Hidden);
        page.settle();
        assert_eq!(page.drawn_in(120)[1], 0, "hidden");
        let [start, _] = page.range();
        page.selection(|selection| {
            selection.set_selected_time(start);
            selection.set_playback_speed(1e-3);
            selection.play();
        });
        let [_, satellite] = page.drawn_in(60);
        assert!((59..=61).contains(&satellite), "hidden and playing: {satellite}");
        page.selection(|selection| selection.pause());
        page.settle();
        assert_eq!(page.drawn_in(120)[1], 0, "hidden and paused");
        page.cx
            .simulate_window_visibility_change(window, WindowVisibility::Visible);
        assert!(page.drawn_in(30)[0] > 0, "shown again");
        page.assert_idle("shown again");
    }
}
