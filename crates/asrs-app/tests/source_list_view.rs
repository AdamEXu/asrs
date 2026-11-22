#![allow(clippy::disallowed_methods)]

//! U09's exit criterion on the real hub window, headless: `dom/source-list/view-drag`'s item
//! reorders, field drop and cross-list move, sent as platform mouse events at the points the
//! fixture's `__slMouse` helper computes from the layout, end with the lists Blink ended with.

use std::sync::Arc;

use asrs_app::hub::Hub;
use asrs_app::views::frame::HubFrame;
use asrs_app::views::hub_host::HubEntityHost;
use asrs_hub::drag::{DragItem, DragPayload};
use asrs_hub::frame::{DragContent, DragData};
use asrs_hub::hub::HubModel;
use asrs_js::string::JsString;
use asrs_model::source_list::layout::{ListLayout, controller_roots};
use asrs_model::source_list::{Rect, SourceList};
use asrs_model::tab_type::TabType;
use asrs_parity::Value;
use asrs_parity::vectors;
use asrs_tabs::line_graph::{LineGraphController, ListId};
use asrs_tabs::list_cx::with_env;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{
    Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput,
    WindowHandle, point, px, size,
};

const VECTOR: &str = "dom/source-list/view-drag";
/// `div.controls-content` in a 1100 x 650 window.
const CONTROLS: Rect = Rect {
    x: 300.0,
    y: 450.0,
    width: 800.0,
    height: 200.0,
};

type Window = WindowHandle<HubFrame<HubEntityHost>>;

/// The left and right lists' keys at each `sourceLists` capture of the fixture, by label. The
/// recorder registers the line graph's lists as left (0), discrete (1), right (2).
fn blink_outcomes() -> Vec<(String, Vec<String>, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    let mut label = String::new();
    let keys = |dump: &Value| -> Vec<String> {
        let state = dump.get("state").expect("dump state");
        let Value::Array(items) = state else {
            panic!("state is not a list")
        };
        items
            .iter()
            .map(|i| {
                i.get("logKey")
                    .and_then(Value::as_str)
                    .expect("logKey")
                    .to_string_lossy()
            })
            .collect()
    };
    for record in vectors::load(VECTOR).unwrap() {
        if let Some(l) = record
            .value("capture")
            .ok()
            .and_then(|v| v.as_str().cloned())
        {
            label = l.to_string_lossy();
            out.push((label.clone(), Vec::new(), Vec::new()));
        }
        let Ok(dump) = record.value("dump") else {
            continue;
        };
        let id = record.value("sl").unwrap().as_f64().unwrap() as usize;
        let entry = out.last_mut().expect("a dump after a capture");
        assert_eq!(entry.0, label);
        match id {
            0 => entry.1 = keys(&dump),
            2 => entry.2 = keys(&dump),
            _ => {}
        }
    }
    out
}

fn entities(
    headless: &mut HeadlessCapture,
    window: Window,
) -> (Entity<Hub>, Entity<HubFrame<HubEntityHost>>) {
    headless
        .context()
        .update_window(window.into(), |root, _, cx| {
            let frame = root
                .downcast::<HubFrame<HubEntityHost>>()
                .expect("hub frame");
            (frame.read(cx).host().hub().clone(), frame)
        })
        .expect("window")
}

fn page<R>(
    headless: &mut HeadlessCapture,
    hub: &Entity<Hub>,
    f: impl FnOnce(&mut HubModel) -> R,
) -> R {
    headless
        .context()
        .update(|cx| hub.update(cx, |hub, cx| hub.update(cx, |model, _| f(model))))
}

fn keys(list: &SourceList) -> Vec<String> {
    list.state()
        .iter()
        .map(|s| s.log_key.to_string_lossy())
        .collect()
}

/// The line graph's left and right lists' keys.
fn lists(headless: &mut HeadlessCapture, hub: &Entity<Hub>) -> (Vec<String>, Vec<String>) {
    page(headless, hub, |model| {
        let tab = model
            .tabs()
            .tabs()
            .find(|t| t.tab_type() == Some(TabType::LineGraph))
            .expect("line graph tab");
        let lists = tab.controller().source_lists();
        (keys(lists[0]), keys(lists[1]))
    })
}

/// `__slMouse`'s point: `dx` right of the root and `dy` below the top of item `anchor` (scaled
/// to its height), or below the last item's bottom for an anchor past the end.
fn point_at(
    headless: &mut HeadlessCapture,
    hub: &Entity<Hub>,
    list: usize,
    anchor: usize,
    dx: f64,
    dy: f64,
) -> (f64, f64) {
    page(headless, hub, |model| {
        let tab = model
            .tabs()
            .tabs()
            .find(|t| t.tab_type() == Some(TabType::LineGraph))
            .expect("line graph tab");
        let source = tab.controller().source_lists()[list];
        let root = controller_roots(TabType::LineGraph, CONTROLS)[list];
        let layout = ListLayout::new(root, source.items(), 0.0);
        let count = source.items().len();
        let y = if anchor < count {
            let item = layout.item_rect(anchor);
            item.y + (dy * item.height / 30.0).round()
        } else {
            let end = layout.item_rect(count - 1);
            end.y + end.height + (anchor - count) as f64 * 30.0 + dy
        };
        (root.rect.x + dx, y)
    })
}

fn send(headless: &mut HeadlessCapture, window: Window, input: PlatformInput) {
    headless
        .context()
        .update_window(window.into(), |_, window, cx| {
            window.dispatch_event(input, cx);
        })
        .expect("window");
    headless.capture(window.into()).expect("render");
}

fn down(headless: &mut HeadlessCapture, window: Window, (x, y): (f64, f64)) {
    send(
        headless,
        window,
        PlatformInput::MouseDown(MouseDownEvent {
            button: MouseButton::Left,
            position: point(px(x as f32), px(y as f32)),
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }),
    );
}

fn moved(headless: &mut HeadlessCapture, window: Window, (x, y): (f64, f64), pressed: bool) {
    send(
        headless,
        window,
        PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x as f32), px(y as f32)),
            pressed_button: pressed.then_some(MouseButton::Left),
            modifiers: Modifiers::default(),
        }),
    );
}

fn up(headless: &mut HeadlessCapture, window: Window, (x, y): (f64, f64)) {
    send(
        headless,
        window,
        PlatformInput::MouseUp(MouseUpEvent {
            button: MouseButton::Left,
            position: point(px(x as f32), px(y as f32)),
            modifiers: Modifiers::default(),
            click_count: 1,
        }),
    );
}

/// The fixture's item drag: press 5 px into item `from`, move 6 px down inside the list, then
/// twice to `dy` into item `anchor` of the list `to`, and release.
#[allow(clippy::too_many_arguments)]
fn item_drag(
    headless: &mut HeadlessCapture,
    window: Window,
    hub: &Entity<Hub>,
    list: usize,
    from: usize,
    to: usize,
    anchor: usize,
    dy: f64,
) {
    let press = point_at(headless, hub, list, from, 20.0, 5.0);
    down(headless, window, press);
    let start = point_at(headless, hub, list, from, 20.0, 11.0);
    moved(headless, window, start, true);
    let target = point_at(headless, hub, to, anchor, 20.0, dy);
    moved(headless, window, target, true);
    moved(headless, window, target, true);
    up(headless, window, target);
}

fn run(headless: &mut HeadlessCapture, scale: f32, blink: &[(String, Vec<String>, Vec<String>)]) {
    let window = headless
        .open_window(size(px(1100.0), px(650.0)), scale, |window, cx| {
            HubEntityHost::open(window, false, false, Vec::new(), cx)
        })
        .expect("hub window");
    let (hub, frame) = entities(headless, window);
    page(headless, &hub, |model| {
        {
            let mut log = model.page().shared_log().borrow_mut();
            for (i, key) in ["/A", "/B", "/C", "/D", "/E"].into_iter().enumerate() {
                log.put_number(&JsString::from(key), 0.0, i as f64);
            }
        }
        let (tabs, page) = model.tabs_mut();
        let controller = tabs
            .selected_controller_as::<LineGraphController>(TabType::LineGraph)
            .expect("selected tab")
            .expect("a line graph");
        for (id, key) in [
            (ListId::Left, "/A"),
            (ListId::Left, "/B"),
            (ListId::Left, "/C"),
            (ListId::Right, "/E"),
        ] {
            with_env(page, |env| {
                controller.with_list(id, |list, others| {
                    list.add_field(env, &others, &JsString::from(key), None)
                })
            })
            .expect("addField");
        }
    });
    headless.capture(window.into()).expect("render");
    let mut step = 0;
    let mut check = |headless: &mut HeadlessCapture, label: &str| {
        let (left, right) = lists(headless, &hub);
        let (want, blink_left, blink_right) = &blink[step];
        assert_eq!(want, label, "capture order");
        assert_eq!(
            (&left, &right),
            (blink_left, blink_right),
            "{scale}x {label}: left and right lists"
        );
        println!("{scale}x {label}: {left:?} {right:?}");
        step += 1;
    };
    check(headless, "added");
    item_drag(headless, window, &hub, 0, 0, 0, 2, 25.0);
    check(headless, "reordered");
    item_drag(headless, window, &hub, 0, 2, 0, 0, 4.0);
    check(headless, "reordered-up");
    headless.context().update(|cx| {
        frame.update(cx, |frame, _| {
            frame.shell_mut().start_drag(
                10.0,
                10.0,
                0.0,
                0.0,
                DragContent::Field(DragItem {
                    text: JsString::from("D"),
                    bold: false,
                }),
                DragData::Fields(DragPayload {
                    fields: vec![JsString::from("/D")],
                    children: Vec::new(),
                }),
            )
        })
    });
    let target = point_at(headless, &hub, 0, 1, 20.0, 2.0);
    moved(headless, window, target, false);
    moved(headless, window, target, false);
    up(headless, window, target);
    check(headless, "dropped");
    item_drag(headless, window, &hub, 0, 1, 1, 0, 3.0);
    check(headless, "moved-right");
    assert_eq!(step, blink.len(), "every capture compared");
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("source_list_view: skipped, no headless renderer on this platform");
        return;
    }
    let blink = blink_outcomes();
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless app");
    for scale in [1.0, 2.0] {
        run(&mut headless, scale, &blink);
    }
    println!("source_list_view: ok");
}
