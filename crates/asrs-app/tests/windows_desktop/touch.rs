//! Touch and pen in the hub against Chromium's, by `InjectSyntheticPointerInput`: a drag over a
//! scroller pans it and flings, selecting no text, a held press opens the context menu as it
//! lifts, a tap clicks, and drags and pinches over the timeline and line graph still move them,
//! as drags of the sidebar's handle still resize it.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use asrs_app::hub::{Hub, HubEvent};
use asrs_app::views::frame::{FrameHost, HubFrame};
use asrs_app::views::hub_host::HubEntityHost;
use asrs_app::windows;
use asrs_hub::frame::FrameElement;
use asrs_js::value::JsValue;
use asrs_model::messages::{HubToMain, MainToHub, NamedMessage};
use asrs_model::selection::Selection;
use asrs_model::tab_type::TabType;
use asrs_parity::json::Json;
use asrs_parity::vectors;
use asrs_tabs::table::TableRenderer;
use asrs_ui::gpui::{App, AppContext, AsyncApp, Entity, px, size};
use asrs_ui::platform::probe::{self, Contact, ContactPhase, PointerKind, SyntheticPointer};

use crate::desktop::{Electron, Report, fresh_boot, handle_of, sleep, with_window};

const FRAME_MS: u64 = 16;

const PAGE: &str = r#"<body style="margin:0;font:14px sans-serif;user-select:none">
<div id="scroll" style="position:absolute;left:20px;top:20px;width:200px;height:360px;overflow:auto"></div>
<div id="table" style="position:absolute;left:260px;top:20px;width:300px;height:360px;overflow:auto;user-select:text"></div>
<div id="tab" style="position:absolute;left:600px;top:20px;width:80px;height:30px"></div>
<script>
for (const id of ["scroll", "table"]) {
  const box = document.getElementById(id);
  for (let i = 0; i < 300; i++) {
    const row = document.createElement("div");
    row.style.height = "30px";
    row.textContent = id + " row " + i;
    box.appendChild(row);
  }
}
const tab = document.getElementById("tab");
const menus = [];
let ups = 0, clicks = 0;
tab.addEventListener("pointerup", () => ups++);
tab.addEventListener("contextmenu", () => menus.push(ups));
tab.addEventListener("click", () => clicks++);
window.state = () => [scroll.scrollTop, table.scrollTop, String(getSelection()).length,
  menus.length, menus.length ? menus.at(-1) : -1, ups, clicks];
</script>"#;

/// Where the page puts its boxes, in its client px.
const SCROLL: [f64; 2] = [120.0, 300.0];
const TABLE: [f64; 2] = [410.0, 300.0];
const TAB: [f64; 2] = [640.0, 35.0];

type Point = [f64; 2];

fn plus(a: Point, b: Point) -> Point {
    [a[0] + b[0], a[1] + b[1]]
}

fn contact(id: u32, [x, y]: Point, phase: ContactPhase) -> Contact {
    Contact { id, x, y, phase }
}

/// A contact pressed at `from`, moved by `by` in `steps` frames, held still `hold` ms and lifted.
async fn stroke(
    cx: &mut AsyncApp,
    pointer: &SyntheticPointer,
    kind: PointerKind,
    from: Point,
    by: Point,
    steps: u32,
    hold: u64,
) {
    let pen = kind == PointerKind::Pen;
    let at = |step: u32| {
        let t = if steps == 0 {
            0.0
        } else {
            f64::from(step) / f64::from(steps)
        };
        plus(from, [by[0] * t, by[1] * t])
    };
    for _ in 0..if pen { 5 } else { 0 } {
        pointer.inject(&[contact(0, from, ContactPhase::Hover)]);
        sleep(cx, FRAME_MS).await;
    }
    pointer.inject(&[contact(0, from, ContactPhase::Down)]);
    sleep(cx, FRAME_MS).await;
    for step in 1..=steps {
        pointer.inject(&[contact(0, at(step), ContactPhase::Move)]);
        sleep(cx, FRAME_MS).await;
    }
    for _ in 0..hold / FRAME_MS {
        pointer.inject(&[contact(0, at(steps), ContactPhase::Move)]);
        sleep(cx, FRAME_MS).await;
    }
    pointer.inject(&[contact(0, at(steps), ContactPhase::Up)]);
    if pen {
        sleep(cx, FRAME_MS).await;
        pointer.inject(&[contact(0, at(steps), ContactPhase::Hover)]);
    }
}

/// Two fingers on either side of `center`, moved from `from` to `to` px apart in `steps` frames.
async fn pinch(cx: &mut AsyncApp, touch: &SyntheticPointer, center: Point, from: f64, to: f64) {
    const STEPS: u32 = 30;
    let fingers = |apart: f64, phase| {
        [
            contact(0, plus(center, [-apart / 2.0, 0.0]), phase),
            contact(1, plus(center, [apart / 2.0, 0.0]), phase),
        ]
    };
    touch.inject(&fingers(from, ContactPhase::Down));
    sleep(cx, FRAME_MS).await;
    for step in 1..=STEPS {
        let apart = from + (to - from) * f64::from(step) / f64::from(STEPS);
        touch.inject(&fingers(apart, ContactPhase::Move));
        sleep(cx, FRAME_MS).await;
    }
    touch.inject(&fingers(to, ContactPhase::Up));
}

/// What Chromium did with the same strokes.
struct Chromium {
    drag: f64,
    table: f64,
    coast: f64,
    selected: f64,
    menu_on_lift: bool,
    clicks: f64,
    pen: f64,
    pen_selected: f64,
}

async fn chromium(
    cx: &mut AsyncApp,
    electron: &Electron,
    touch: &SyntheticPointer,
    pen: &SyntheticPointer,
) -> Chromium {
    let request = Json::object([("op", Json::string("touch")), ("page", Json::string(PAGE))]);
    let opened = electron.numbers(cx, &request.to_compact(), "touch ").await;
    let origin = [opened[1], opened[2]];
    let at = |p: Point| plus(origin, p);
    let state = r#"{"op":"touchstate"}"#;
    let touched = PointerKind::Touch;
    stroke(cx, touch, touched, at(SCROLL), [0.0, -150.0], 30, 300).await;
    sleep(cx, 800).await;
    let dragged = electron.numbers(cx, state, "touchstate ").await;
    stroke(cx, touch, touched, at(TABLE), [0.0, -120.0], 30, 300).await;
    sleep(cx, 800).await;
    let table = electron.numbers(cx, state, "touchstate ").await;
    stroke(cx, touch, touched, at(TABLE), [0.0, -200.0], 6, 0).await;
    sleep(cx, 2000).await;
    let flung = electron.numbers(cx, state, "touchstate ").await;
    stroke(cx, touch, touched, at(TAB), [0.0, 0.0], 0, 800).await;
    sleep(cx, 600).await;
    let held = electron.numbers(cx, state, "touchstate ").await;
    stroke(cx, touch, touched, at(TAB), [0.0, 0.0], 0, 0).await;
    sleep(cx, 600).await;
    let tapped = electron.numbers(cx, state, "touchstate ").await;
    stroke(cx, pen, PointerKind::Pen, at(TABLE), [0.0, -120.0], 30, 300).await;
    sleep(cx, 800).await;
    let penned = electron.numbers(cx, state, "touchstate ").await;
    electron.request(cx, r#"{"op":"close"}"#, "closed").await;
    Chromium {
        drag: dragged[0],
        table: table[1],
        coast: flung[1] - table[1] - 200.0,
        selected: flung[2],
        menu_on_lift: held[3] == 1.0 && held[4] == held[5],
        clicks: tapped[6] - held[6],
        pen: penned[1] - flung[1],
        pen_selected: penned[2],
    }
}

struct OurHub {
    frame: Entity<HubFrame<HubEntityHost>>,
    hub: Entity<Hub>,
    client: Point,
    sent: Rc<RefCell<Vec<HubToMain>>>,
}

fn main_message(cx: &mut AsyncApp, hub: &Entity<Hub>, name: MainToHub, data: JsValue) {
    cx.update(|cx| {
        hub.update(cx, |hub, cx| {
            hub.handle_main_message(NamedMessage::new(name, data), cx)
        })
    });
}

fn json(text: &str) -> JsValue {
    asrs_js::json::parse_str(text).expect("json")
}

const STATE: &str = r#"{"sidebar":{"width":300,"expanded":["/Drive","/Odometry","/Odometry/Robot",
"/Odometry/Robot/translation","/Odometry/Robot/rotation","/Odometry/Robot3d",
"/Odometry/Robot3d/translation","/Odometry/Robot3d/rotation","/Odometry/Robot3d/rotation/q",
"/Drive/ModuleSpeeds","/DriverStation"]},
"tabs":{"selected":2,"tabs":[
{"type":0,"title":"","controller":null,"controllerUUID":"documentation0000000000000000000","renderer":"","controlsHeight":0},
{"type":1,"title":"Graph","controller":{"leftSources":[],"rightSources":[],"discreteSources":[],"leftLockedRange":null,"rightLockedRange":null,"leftUnitConversion":{"autoTarget":null,"preset":null},"rightUnitConversion":{"autoTarget":null,"preset":null},"leftFilter":0,"rightFilter":0,"showRobotMode":false},"controllerUUID":"linegraph00000000000000000000000","renderer":null,"controlsHeight":200},
{"type":4,"title":"Table","controller":["/Drive/LeftVelocity","/Drive/RightVelocity","/Drive/Mode"],"controllerUUID":"table000000000000000000000000000","renderer":null,"controlsHeight":0}]}}"#;

async fn open_hub(cx: &mut AsyncApp, user_data: &Path) -> (windows::WindowId, OurHub) {
    let boot = fresh_boot(user_data);
    let opened = cx.update(|cx| {
        windows::open_hub(cx, None, |window, cx| {
            HubEntityHost::open_with(window, false, boot, None, cx)
        })
    });
    let (id, frame) = opened.expect("hub");
    let handle = handle_of(cx, id);
    with_window(cx, handle, |w| w.resize(size(px(1100.0), px(400.0))));
    let hub = cx.update(|cx| frame.read(cx).host().hub().clone());
    let sent = Rc::new(RefCell::new(Vec::new()));
    let log = sent.clone();
    cx.update(|cx| {
        cx.subscribe(&hub, move |_, event: &HubEvent, _| {
            if let HubEvent::Main(message) = event {
                log.borrow_mut().push(message.name);
            }
        })
        .detach()
    });
    let log = vectors::workspace_root().join("parity/scenarios/inputs/drive.wpilog");
    let files = format!(
        r#"{{"files":[{}],"merge":false}}"#,
        Json::string(log.to_str().expect("path")).to_compact()
    );
    main_message(cx, &hub, MainToHub::OpenFiles, json(&files));
    sleep(cx, 5000).await;
    main_message(cx, &hub, MainToHub::RestoreState, json(&STATE.replace('\n', "")));
    sleep(cx, 3000).await;
    with_window(cx, handle, |w| w.activate_window());
    sleep(cx, 800).await;
    let number = with_window(cx, handle, |w| probe::window_number(w)).expect("window id");
    let [_, client] = probe::rects(number).expect("rects");
    let client = [f64::from(client[0]), f64::from(client[1])];
    (
        id,
        OurHub {
            frame,
            hub,
            client,
            sent,
        },
    )
}

impl OurHub {
    fn rect(&self, cx: &mut AsyncApp, element: FrameElement) -> [f64; 4] {
        let layout = cx.update(|cx| self.frame.read(cx).layout(cx, 1100.0, 400.0, 1.0));
        let [x, y, w, h] = layout.rect(element).expect("the element is shown");
        [x + self.client[0], y + self.client[1], w, h]
    }

    fn tab(&self, cx: &mut AsyncApp, index: usize) -> Point {
        let rect = cx.update(|cx| {
            let frame = self.frame.read(cx);
            frame.host().geometry().tab_bar().borrow().title_rect(index)
        });
        plus(self.client, [rect.x + rect.width / 2.0, rect.y + rect.height / 2.0])
    }

    fn sidebar_top(&self, cx: &mut AsyncApp) -> f64 {
        cx.update(|cx| self.frame.read(cx).host().sidebar().read(cx).scroll_top())
    }

    fn sidebar_range(&self, cx: &mut AsyncApp) -> f64 {
        cx.update(|cx| self.frame.read(cx).host().sidebar().read(cx).scroll_range()[1][1])
    }

    fn table_top(&self, cx: &mut AsyncApp) -> f64 {
        cx.update(|cx| {
            let hub = self.hub.read(cx).model();
            let table = hub.tabs().tabs().find_map(|tab| {
                let shown = !tab.content_hidden() && tab.tab_type() == Some(TabType::Table);
                shown.then(|| tab.renderer().downcast_ref::<TableRenderer>())?
            });
            table.map_or(f64::NAN, |t| t.dom().borrow_mut().layout.wheel_offset()[1])
        })
    }

    fn selected_text(&self, cx: &mut AsyncApp) -> String {
        cx.update(|cx| self.frame.update(cx, |frame, cx| frame.selected_text(cx)))
            .unwrap_or_default()
    }

    fn selected_tab(&self, cx: &mut AsyncApp) -> Option<usize> {
        cx.update(|cx| self.hub.read(cx).model().tabs().selected_index())
    }

    fn range(&self, cx: &mut AsyncApp) -> [f64; 2] {
        cx.update(|cx| {
            self.hub.update(cx, |hub, cx| {
                hub.update(cx, |model, _| model.page_mut().selection_mut().get_timeline_range())
            })
        })
    }

    fn asked_to_rename(&self) -> usize {
        let sent = self.sent.borrow();
        sent.iter().filter(|n| **n == HubToMain::AskRenameTab).count()
    }
}

fn center([x, y, w, h]: [f64; 4]) -> Point {
    [x + w / 2.0, y + h / 2.0]
}

fn near(ours: f64, theirs: f64, tolerance: f64) -> bool {
    (ours - theirs).abs() <= tolerance
}

pub async fn run(cx: &mut AsyncApp, electron: &Electron, report: &mut Report) {
    let touch = SyntheticPointer::new(PointerKind::Touch).expect("a synthetic touch screen");
    let pen = SyntheticPointer::new(PointerKind::Pen).expect("a synthetic pen");
    let theirs = chromium(cx, electron, &touch, &pen).await;
    report.check(
        "Chromium: a touch drag scrolls by the finger's travel, then flings on",
        near(theirs.drag, 150.0, 2.0) && theirs.coast > 20.0,
        format!("dragged {}, coasted {}", theirs.drag, theirs.coast),
    );
    report.check(
        "Chromium: a touch or pen drag over text scrolls it and selects none",
        theirs.table > 90.0 && theirs.pen > 90.0 && theirs.selected + theirs.pen_selected == 0.0,
        format!(
            "touch {} pen {}, selected {} and {}",
            theirs.table, theirs.pen, theirs.selected, theirs.pen_selected
        ),
    );
    report.check(
        "Chromium: a held press opens the context menu as it lifts; a tap clicks",
        theirs.menu_on_lift && theirs.clicks == 1.0,
        format!("menu on lift {}, clicks {}", theirs.menu_on_lift, theirs.clicks),
    );

    let user_data = std::env::temp_dir().join(format!("asrs-touch-{}", std::process::id()));
    let (id, hub) = open_hub(cx, &user_data).await;
    let touched = PointerKind::Touch;

    let side_bar = hub.rect(cx, FrameElement::SideBar);
    let from = [side_bar[0] + side_bar[2] - 12.0, side_bar[1] + side_bar[3] - 40.0];
    let before = hub.sidebar_top(cx);
    stroke(cx, &touch, touched, from, [0.0, -150.0], 30, 300).await;
    sleep(cx, 800).await;
    let dragged = hub.sidebar_top(cx) - before;
    report.check(
        "touch: a drag scrolls the sidebar as far as Chromium's",
        near(dragged, theirs.drag, 3.0),
        format!(
            "{dragged} of {} at most, Chromium {}",
            hub.sidebar_range(cx),
            theirs.drag
        ),
    );

    let table = hub.rect(cx, FrameElement::RendererContent);
    let from = [table[0] + table[2] / 2.0, table[1] + table[3] - 60.0];
    let before = hub.table_top(cx);
    stroke(cx, &touch, touched, from, [0.0, -120.0], 30, 300).await;
    sleep(cx, 800).await;
    let scrolled = hub.table_top(cx) - before;
    let selected = hub.selected_text(cx);
    report.check(
        "touch: a drag scrolls the table as Chromium's and selects no text",
        near(scrolled, theirs.table, 20.0) && selected.is_empty(),
        format!(
            "{scrolled}, Chromium {}; selected {selected:?}",
            theirs.table
        ),
    );

    let before = hub.table_top(cx);
    stroke(cx, &touch, touched, from, [0.0, -200.0], 6, 0).await;
    sleep(cx, 2000).await;
    let coast = hub.table_top(cx) - before - 200.0;
    report.check(
        "touch: a flick flings the table on about as far as Chromium",
        coast > 20.0 && coast > theirs.coast / 2.0 && coast < theirs.coast * 2.0,
        format!("{coast}, Chromium {}", theirs.coast),
    );

    let before = hub.table_top(cx);
    stroke(cx, &pen, PointerKind::Pen, from, [0.0, -120.0], 30, 300).await;
    sleep(cx, 800).await;
    let scrolled = hub.table_top(cx) - before;
    let selected = hub.selected_text(cx);
    report.check(
        "pen: a drag scrolls the table as Chromium's and selects no text",
        near(scrolled, theirs.pen, 20.0) && selected.is_empty(),
        format!("{scrolled}, Chromium {}; selected {selected:?}", theirs.pen),
    );

    let graph = hub.tab(cx, 1);
    let asked = hub.asked_to_rename();
    touch.inject(&[contact(0, graph, ContactPhase::Down)]);
    for _ in 0..800 / FRAME_MS {
        sleep(cx, FRAME_MS).await;
        touch.inject(&[contact(0, graph, ContactPhase::Move)]);
    }
    sleep(cx, 200).await;
    let held = hub.asked_to_rename() - asked;
    touch.inject(&[contact(0, graph, ContactPhase::Up)]);
    sleep(cx, 800).await;
    let lifted = hub.asked_to_rename() - asked;
    report.check(
        "touch: holding a tab opens its context menu as it lifts",
        held == 0 && lifted == 1 && hub.selected_tab(cx) == Some(2),
        format!(
            "asked to rename {held} times while held, {lifted} after; tab {:?} selected",
            hub.selected_tab(cx)
        ),
    );

    stroke(cx, &touch, touched, graph, [0.0, 0.0], 0, 0).await;
    sleep(cx, 1500).await;
    report.check(
        "touch: a tap selects a tab",
        hub.selected_tab(cx) == Some(1),
        format!("{:?}", hub.selected_tab(cx)),
    );

    let timeline = hub.rect(cx, FrameElement::Timeline);
    let before = hub.range(cx);
    stroke(cx, &touch, touched, center(timeline), [-150.0, 0.0], 30, 300).await;
    sleep(cx, 1000).await;
    let after = hub.range(cx);
    report.check(
        "touch: a sideways drag over the timeline pans it later",
        after[0] > before[0] && near(after[1] - after[0], before[1] - before[0], 1e-6),
        format!("{before:?} to {after:?}"),
    );

    let graph = hub.rect(cx, FrameElement::RendererContent);
    let middle = center(graph);
    let before = hub.range(cx);
    stroke(cx, &touch, touched, middle, [-150.0, 0.0], 30, 300).await;
    sleep(cx, 1000).await;
    let after = hub.range(cx);
    report.check(
        "touch: a sideways drag over the line graph pans it later",
        after[0] > before[0] && near(after[1] - after[0], before[1] - before[0], 1e-6),
        format!("{before:?} to {after:?}"),
    );

    let before = hub.range(cx);
    pinch(cx, &touch, middle, 60.0, 240.0).await;
    sleep(cx, 1000).await;
    let after = hub.range(cx);
    report.check(
        "touch: spreading two fingers over the line graph zooms in",
        after[1] - after[0] < (before[1] - before[0]) * 0.9,
        format!("{before:?} to {after:?}"),
    );

    let handle = hub.rect(cx, FrameElement::SideBarHandle);
    let width = side_bar[2];
    stroke(cx, &touch, touched, center(handle), [60.0, 0.0], 20, 300).await;
    sleep(cx, 800).await;
    let resized = hub.rect(cx, FrameElement::SideBar)[2] - width;
    report.check(
        "touch: dragging the sidebar's handle still resizes it",
        near(resized, 60.0, 4.0),
        format!("{resized}"),
    );

    cx.update(|cx: &mut App| windows::close(cx, id));
    sleep(cx, 400).await;
    let _ = std::fs::remove_dir_all(&user_data);
}
