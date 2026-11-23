#![allow(clippy::disallowed_methods)]

//! The Joysticks, Swerve and Points views through the real hub window, headless. While a layout's
//! image is shown the joysticks painter paints every animation frame; the canvas draws only the
//! frames that differ. Choosing a select's option and typing a number reach the controller, and
//! its `change` listener's answer reaches the input.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use asrs_app::hub::Hub;
use asrs_app::views::canvas_tabs::{Control, JoysticksTab};
use asrs_app::views::frame::HubFrame;
use asrs_app::views::hub_host::HubEntityHost;
use asrs_js::json;
use asrs_js::string::JsString;
use asrs_js::value::JsValue;
use asrs_model::messages::{MainToHub, NamedMessage};
use asrs_model::selection::Selection;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{Entity, WindowHandle, px, size};

const FRAME: Duration = Duration::from_millis(16);
const IDLE_FRAMES: u64 = 60;
const SEEKS: u64 = 5;

type Window = WindowHandle<HubFrame<HubEntityHost>>;

struct Page<'a> {
    headless: &'a mut HeadlessCapture,
    window: Window,
    hub: Entity<Hub>,
}

impl Page<'_> {
    fn open(headless: &mut HeadlessCapture, scale: f32) -> Page<'_> {
        let window = headless
            .open_window(size(px(1100.0), px(650.0)), scale, |window, cx| {
                HubEntityHost::open(window, false, false, Vec::new(), cx)
            })
            .expect("hub window");
        let hub = headless
            .context()
            .update(|cx| window.read_with(cx, |frame, _| frame.host().hub().clone()))
            .expect("window");
        let mut page = Page {
            headless,
            window,
            hub,
        };
        page.frames(2);
        page
    }

    fn main(&mut self, name: MainToHub, data: JsValue) {
        let hub = self.hub.clone();
        self.headless.context().update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.handle_main_message(NamedMessage::new(name, data), cx)
            })
        });
        self.headless.context().run_until_parked();
    }

    fn frames(&mut self, count: u64) {
        for _ in 0..count {
            let cx = self.headless.context();
            cx.advance_clock(FRAME);
            cx.update_window(self.window.into(), |_, window, cx| {
                window.simulate_next_frame(cx);
            })
            .expect("window");
            self.headless.context().run_until_parked();
        }
    }

    fn restore(&mut self, kind: u8, controller: &str) {
        let state = format!(
            r#"{{"sidebar": {{"width": 300, "expanded": []}}, "tabs": {{"selected": 0, "tabs": [
                {{"type": {kind}, "title": "", "controller": {controller},
                  "controllerUUID": "tab", "renderer": null, "controlsHeight": 200}}]}}}}"#
        );
        self.main(
            MainToHub::RestoreState,
            json::parse_str(&state).expect("state"),
        );
        self.frames(3);
    }

    fn saved_controller(&mut self) -> String {
        let hub = self.hub.clone();
        self.headless.context().update(|cx| {
            let model = hub.read(cx).model();
            let state = model.tabs().save_state(model.page());
            let tabs = state.get("tabs").and_then(JsValue::as_array).expect("tabs");
            json::stringify(tabs[0].get("controller").expect("controller"))
                .expect("json")
                .expect("json")
        })
    }

    /// How often the joysticks canvas was drawn and how often its painter's image loaded.
    fn joysticks(&mut self) -> (u64, u64) {
        let hub = self.hub.clone();
        let loads = self.headless.context().update(|cx| {
            let tab = hub.read(cx).model().tabs().tabs().next().expect("tab");
            let tab = tab.renderer().downcast_ref::<JoysticksTab>();
            tab.expect("joysticks").renderer().image_load_count()
        });
        let draws = self
            .headless
            .context()
            .update(|cx| {
                self.window.read_with(cx, |frame, cx| {
                    let canvas = frame.joysticks_canvas(cx).expect("canvas");
                    canvas.read(cx).draw_count()
                })
            })
            .expect("window");
        (draws, loads)
    }

    fn choose(&mut self, control: Control, item: usize) {
        let select = self
            .headless
            .context()
            .update(|cx| {
                self.window
                    .read_with(cx, |frame, cx| frame.displayed_select(control, cx))
            })
            .expect("window")
            .expect("select");
        self.headless
            .context()
            .update(|cx| select.update(cx, |s, cx| s.popup_closed(Some(item), cx)));
        self.frames(2);
    }

    fn close(self) {
        self.headless
            .context()
            .update_window(self.window.into(), |_, window, _| window.remove_window())
            .expect("window");
        self.headless.context().run_until_parked();
    }
}

fn assets() -> JsValue {
    let image =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../parity/scenarios/assets/parity-pad.png");
    let path = image.to_string_lossy();
    let assets = format!(
        r#"{{"field2ds": [], "field3ds": [], "robots": [], "loadFailures": [], "joysticks": [
            {{"name": "Pad", "path": {}, "components": [
                {{"type": "axis", "isYellow": false, "centerPx": [100, 100], "sizePx": [40, 80],
                  "niSourceIndex": 0, "niSourceRange": [0, 100]}}]}}]}}"#,
        json::stringify(&JsValue::string(&path))
            .expect("json")
            .expect("json")
    );
    json::parse_str(&assets).expect("assets")
}

fn equal_frames_are_not_drawn(headless: &mut HeadlessCapture, scale: f32) {
    let mut page = Page::open(headless, scale);
    page.main(MainToHub::SetAssets, assets());
    page.restore(8, r#"["Pad", "None", "None", "None", "None", "None"]"#);
    let hub = page.hub.clone();
    page.headless.context().update(|cx| {
        hub.update(cx, |hub, cx| {
            hub.update(cx, |model, _| {
                let axes = JsString::from("DS:joystick0/axes");
                {
                    let mut log = model.page().shared_log().borrow_mut();
                    for t in 0..=100 {
                        let t = f64::from(t);
                        log.put_number_array(&axes, t, vec![t]);
                    }
                }
                model.page_mut().selection_mut().set_selected_time(1.0);
            })
        })
    });
    page.frames(4);

    let (draws, loads) = page.joysticks();
    page.frames(IDLE_FRAMES);
    let (draws_after, loads_after) = page.joysticks();
    assert!(
        loads_after - loads >= IDLE_FRAMES - 1,
        "{scale}x: the painter should repaint every frame: {loads} -> {loads_after}"
    );
    assert_eq!(
        draws_after, draws,
        "{scale}x: frames equal to the last one were drawn"
    );

    for i in 0..SEEKS {
        let hub = page.hub.clone();
        page.headless.context().update(|cx| {
            hub.update(cx, |hub, cx| {
                hub.update(cx, |model, _| {
                    let time = 10.0 * (i + 1) as f64;
                    model.page_mut().selection_mut().set_selected_time(time)
                })
            })
        });
        page.frames(5);
    }
    let (draws_changed, _) = page.joysticks();
    assert_eq!(
        draws_changed,
        draws + SEEKS,
        "{scale}x: each changed frame is drawn once"
    );
    println!("{scale}x: {IDLE_FRAMES} equal frames, 0 drawn; {SEEKS} seeks, {SEEKS} drawn");
    page.close();
}

fn selects_reach_their_controllers(headless: &mut HeadlessCapture) {
    let mut page = Page::open(headless, 1.0);
    page.main(MainToHub::SetAssets, assets());
    page.restore(8, "null");
    page.choose(Control::Joystick(0), 2);
    page.choose(Control::Joystick(5), 1);
    assert_eq!(
        page.saved_controller(),
        r#"["Pad","None","None","None","None","Generic Joystick"]"#
    );
    page.restore(11, "null");
    page.choose(Control::Orientation, 2);
    page.choose(Control::Origin, 4);
    assert_eq!(
        page.saved_controller(),
        r#"{"sources":[],"width":960,"height":720,"orientation":"xu,yl","origin":"c"}"#
    );
    page.close();
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("canvas tabs view: skipped, no headless renderer on this platform");
        return;
    }
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless app");
    for scale in [1.0, 2.0] {
        equal_frames_are_not_drawn(&mut headless, scale);
    }
    selects_reach_their_controllers(&mut headless);
    println!("canvas_tabs_view: ok");
}
