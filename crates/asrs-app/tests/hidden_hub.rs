#![allow(clippy::disallowed_methods)]

//! The app, headless, while its windows give few frames or none: a hub that is covered,
//! minimized or starved keeps running its frame loops and driving satellites, and runs them once
//! per frame when the window has frames. The rates are Electron 39's with upstream's
//! `webPreferences`, whose hub is not throttled and whose satellites are.

#[path = "support/child.rs"]
mod child;

use std::cell::{Cell, RefCell};
use std::process::Command;
use std::rc::Rc;
use std::time::Duration;

use asrs_app::hub::{Hub, HubEvent};
use asrs_app::main_process::{self, MainProcess, SatelliteConfig};
use asrs_app::satellite::{Satellite, SatelliteEvent};
use asrs_app::windows::{self, Windows};
use asrs_js::ops;
use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage, SatelliteToMain};
use asrs_model::tab_type::TabType;
use asrs_ui::gpui::{AnyWindowHandle, AppContext, Entity, TestAppContext, WindowVisibility};

const HOME: &str = "ASRS_HIDDEN_HUB_HOME";
const UUID: &str = "hidden-hub";
const SECOND: Duration = Duration::from_secs(1);
/// The test display's.
const FRAME: Duration = Duration::from_nanos(16_666_666);

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
                body::$body(&mut Page::open());
            }
        }
    )*};
}

in_the_app! {
    a_satellite_follows_a_hidden_hub, hidden_drives_satellites;
    a_hidden_hub_keeps_its_displays_rate, hidden_rate;
    a_hidden_hub_comes_back_to_its_windows_frames, shown_again;
    a_starved_hub_runs_on_a_timer, starved;
    a_window_with_frames_runs_the_loops_once_each, once_per_frame;
    a_hidden_satellite_saves_once_a_second, hidden_satellite;
}

fn run_in_a_fresh_home(test: &str) {
    let home = std::env::temp_dir().join(format!("asrs-hidden-hub-{test}-{}", std::process::id()));
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

/// One hub with a satellite of its only tab.
struct Page {
    cx: TestAppContext,
    hub: Entity<Hub>,
    window: AnyWindowHandle,
    satellite: Entity<Satellite>,
    satellite_window: AnyWindowHandle,
    updates: Rc<Cell<usize>>,
    saves: Rc<RefCell<[usize; 2]>>,
}

impl Page {
    fn open() -> Page {
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
        let updates = Rc::new(Cell::new(0));
        let saves = Rc::new(RefCell::new([0; 2]));
        let (satellite, satellite_window) = cx.update(|cx| {
            let state = restore_state();
            hub.update(cx, |hub, cx| {
                hub.handle_main_message(NamedMessage::new(MainToHub::RestoreState, state), cx)
            });
            let config = SatelliteConfig::New {
                parent: id,
                uuid: JsValue::string(UUID),
                type_value: JsValue::Number(TabType::Table.number()),
            };
            let opened = MainProcess::open_satellite(cx, config).expect("a satellite");
            let (_, satellite) = MainProcess::satellite_windows(cx).remove(0);
            let (count, saved) = (updates.clone(), saves.clone());
            cx.subscribe(&hub, move |_, event, _| match event {
                HubEvent::SatelliteUpdate(_) => count.set(count.get() + 1),
                HubEvent::Main(m) if m.name == HubToMain::SaveState => saved.borrow_mut()[0] += 1,
                _ => {}
            })
            .detach();
            let saved = saves.clone();
            cx.subscribe(&satellite, move |_, event, _| match event {
                SatelliteEvent::Main(m) if m.name == SatelliteToMain::SaveState => {
                    saved.borrow_mut()[1] += 1;
                }
                _ => {}
            })
            .detach();
            let window = Windows::handle(cx, opened).expect("its window");
            (satellite, window)
        });
        let mut page = Page {
            cx,
            hub,
            window,
            satellite,
            satellite_window,
            updates,
            saves,
        };
        for _ in 0..3 {
            page.advance(FRAME);
            page.frame();
        }
        page
    }

    fn advance(&self, time: Duration) {
        self.cx.background_executor.advance_clock(time);
        self.cx.run_until_parked();
    }

    /// The foreground does one thing for `time`, then what timers came due.
    fn block(&self, time: Duration) {
        self.cx.dispatcher.scheduler().clock().advance(time);
        self.cx.run_until_parked();
    }

    /// Delivers the window's next frame.
    fn frame(&mut self) {
        let window = self.window;
        self.cx
            .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
            .expect("the window is open");
        self.cx.run_until_parked();
    }

    fn visibility(&self, window: AnyWindowHandle, visibility: WindowVisibility) {
        self.cx
            .simulate_window_visibility_change(window, visibility);
        self.cx.run_until_parked();
    }

    fn hide(&self) {
        self.visibility(self.window, WindowVisibility::Hidden);
    }

    fn show(&self) {
        self.visibility(self.window, WindowVisibility::Visible);
    }

    fn rename(&mut self, title: &str) {
        let hub = self.hub.clone();
        let data = ops::object([
            ("index", JsValue::Number(1.0)),
            ("name", JsValue::string(title)),
        ]);
        self.cx.update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.handle_main_message(NamedMessage::new(MainToHub::RenameTab, data), cx)
            })
        });
    }

    fn assert_satellite_shows(&self, title: &str) {
        let satellite = self.satellite.clone();
        let shown = self
            .cx
            .read(|cx| satellite.read(cx).model().title().to_string_lossy());
        assert!(shown.contains(title), "{shown}");
    }

    /// Frames the loops run in `time` without one from the window.
    fn unaided(&self, time: Duration) -> usize {
        let before = self.updates.get();
        self.advance(time);
        self.updates.get() - before
    }

    /// Frames the loops run in a second in which the window gives one every `FRAME`.
    fn aided(&mut self) -> usize {
        let before = self.updates.get();
        for _ in 0..60 {
            self.advance(FRAME);
            self.frame();
        }
        self.updates.get() - before
    }

    fn saves_in(&self, time: Duration) -> [usize; 2] {
        *self.saves.borrow_mut() = [0; 2];
        self.advance(time);
        *self.saves.borrow()
    }
}

fn restore_state() -> JsValue {
    let tab = |kind: TabType, uuid: &str, controller: JsValue| {
        ops::object([
            ("type", JsValue::Number(kind.number())),
            ("title", JsValue::string("")),
            ("controller", controller),
            ("controllerUUID", JsValue::string(uuid)),
            ("renderer", JsValue::Null),
            ("controlsHeight", JsValue::Number(0.0)),
        ])
    };
    let tabs = vec![
        tab(TabType::Documentation, "documentation", JsValue::Null),
        tab(TabType::Table, UUID, JsValue::Array(Vec::new())),
    ];
    ops::object([
        (
            "sidebar",
            ops::object([
                ("width", JsValue::Number(300.0)),
                ("expanded", JsValue::Array(Vec::new())),
            ]),
        ),
        (
            "tabs",
            ops::object([
                ("selected", JsValue::Number(1.0)),
                ("tabs", JsValue::Array(tabs)),
            ]),
        ),
    ])
}

mod body {
    use super::*;

    pub fn hidden_drives_satellites(page: &mut Page) {
        page.hide();
        page.rename("First");
        page.advance(FRAME * 2);
        page.assert_satellite_shows("First");
        page.advance(SECOND * 5);
        page.rename("Second");
        page.advance(FRAME * 2);
        page.assert_satellite_shows("Second");
    }

    pub fn hidden_rate(page: &mut Page) {
        page.hide();
        for _ in 0..3 {
            assert_eq!(page.unaided(SECOND), 60);
        }
        let window = page.window;
        let display = page
            .cx
            .update_window(window, |_, window, cx| window.display(cx).map(|d| d.id()))
            .expect("the window is open")
            .expect("a display");
        let interval = Some(SECOND / 120);
        page.cx
            .simulate_display_refresh_interval_change(display, interval);
        page.cx.run_until_parked();
        page.advance(FRAME);
        for _ in 0..3 {
            assert_eq!(page.unaided(SECOND), 120);
        }
    }

    pub fn shown_again(page: &mut Page) {
        for _ in 0..3 {
            page.hide();
            assert_eq!(page.unaided(SECOND), 60);
            page.show();
            assert_eq!(page.unaided(FRAME * 3), 0);
            assert_eq!(page.aided(), 60);
        }
    }

    pub fn starved(page: &mut Page) {
        let before = page.updates.get();
        page.advance(SECOND);
        let caught_up = page.updates.get() - before;
        assert!((40..60).contains(&caught_up), "{caught_up}");
        assert_eq!(page.unaided(SECOND), 60);

        page.frame();
        let resumed = page.aided();
        assert!((60..=61).contains(&resumed), "{resumed}");
        assert_eq!(page.aided(), 60);
        assert_eq!(page.unaided(FRAME * 3), 0);

        page.advance(SECOND);
        assert_eq!(page.unaided(SECOND), 60);
        let before = page.updates.get();
        page.frame();
        page.frame();
        assert!(page.updates.get() - before <= 2);
    }

    pub fn once_per_frame(page: &mut Page) {
        for _ in 0..3 {
            assert_eq!(page.aided(), 60);
        }
        let before = page.updates.get();
        for _ in 0..20 {
            page.block(Duration::from_millis(150));
            page.frame();
        }
        assert_eq!(page.updates.get() - before, 20, "a busy foreground");
        page.hide();
        page.frame();
        assert_eq!(page.unaided(SECOND), 60, "a stray frame while hidden");
    }

    pub fn hidden_satellite(page: &mut Page) {
        assert_eq!(page.saves_in(SECOND * 4), [16, 16]);
        page.visibility(page.satellite_window, WindowVisibility::Hidden);
        page.hide();
        assert_eq!(page.saves_in(SECOND * 4), [16, 4]);
        page.visibility(page.satellite_window, WindowVisibility::Visible);
        assert_eq!(page.saves_in(SECOND * 4), [16, 16]);
    }
}
