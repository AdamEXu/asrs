#![allow(clippy::disallowed_methods)]

//! The `tabs/documentation` records, which upstream's renderer left in Blink, on the real page in
//! the system's webview: after every step `saveState` must return the recorded hash. Then the
//! page cannot reach the network or leave the documentation, a page hidden and shown again keeps
//! its scroll position, and a Documentation tab in the real hub window restores a hash, saves it,
//! hides with its tab and survives the window closing.
//!
//! Needs a display, the built documentation (`cargo xtask docs`), the system's webview and, on
//! Linux, `asrs-webview` and an X11 session; skipped under `ASRS_CI=1` and on Windows.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use asrs_app::main_process::{documentation_dir, webview_host};
use asrs_app::views::documentation::{DocumentationPage, DocumentationRoot};
use asrs_app::views::hub_host::HubEntityHost;
use asrs_js::ops;
use asrs_js::value::JsValue;
use asrs_model::messages::{MainToHub, NamedMessage};
use asrs_model::tab_controller::TabRenderer;
use asrs_parity::cve1::Value;
use asrs_parity::vectors;
use asrs_tabs::documentation::{DocumentationFrame, DocumentationRenderer};
use asrs_ui::gpui::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Context, IntoElement, ParentElement,
    Render, Styled, Window, WindowBounds, WindowOptions, div, point, px, size,
};
use asrs_ui::theme::Appearance;

#[path = "../../asrs-cli/src/js_json.rs"]
#[allow(dead_code)]
mod js_json;

const RECORDS: usize = 29;
const STEPS: usize = 212;
const SIZE: [f64; 2] = [1100.0, 650.0];
const TIMEOUT_MS: u64 = 20_000;
const QUIET_MS: u64 = 300;

struct Shown {
    frame: Rc<RefCell<DocumentationFrame>>,
    page: Option<DocumentationPage>,
}

impl Render for Shown {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        window.request_animation_frame();
        if self.frame.borrow().src().is_some() && self.page.is_none() {
            self.page = Some(DocumentationPage::new(&self.frame, false, window, cx));
        }
        let root = div().size_full();
        let Some(page) = &mut self.page else {
            return root;
        };
        page.sync(false, cx);
        root.child(page.element(
            [0.0, 0.0, SIZE[0], SIZE[1]],
            Appearance::Light,
            window.scale_factor(),
            cx,
        ))
    }
}

async fn sleep(cx: &AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn options(origin: f32) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(origin), px(origin)),
            size(px(SIZE[0] as f32), px(SIZE[1] as f32)),
        ))),
        ..Default::default()
    }
}

struct Page {
    renderer: DocumentationRenderer,
    window: AnyWindowHandle,
    view: asrs_ui::gpui::Entity<Shown>,
}

impl Page {
    fn open(cx: &mut AsyncApp) -> Page {
        let renderer = DocumentationRenderer::new();
        let frame = renderer.frame().clone();
        let (window, view) = cx.update(|cx| {
            asrs_ui::open_window(options(60.0), cx, |_, cx| {
                cx.new(|_| Shown { frame, page: None })
            })
            .expect("window")
        });
        Page {
            renderer,
            window,
            view,
        }
    }

    fn close(self, cx: &mut AsyncApp) {
        drop(self.view);
        let _ = self
            .window
            .update(cx, |_, window, _| window.remove_window());
    }

    fn saved(&self) -> JsValue {
        TabRenderer::<()>::save_state(&self.renderer, &())
    }

    fn render(&mut self) {
        let command = Rc::new(JsValue::Null);
        TabRenderer::<()>::render(&mut self.renderer, &mut (), &*command).expect("render");
    }

    fn restore(&mut self, state: &JsValue) {
        TabRenderer::<()>::restore_state(&mut self.renderer, &mut (), state).expect("restore");
    }

    fn with_page<R>(
        &self,
        cx: &mut AsyncApp,
        f: impl FnOnce(&asrs_ui::webview::Page, &mut App) -> R,
    ) -> R {
        cx.update(|cx| {
            self.view.update(cx, |shown, cx| {
                let page = shown.page.as_ref().and_then(DocumentationPage::page);
                f(page.expect("an open page"), cx)
            })
        })
    }

    async fn query(&self, cx: &mut AsyncApp, script: &str) -> String {
        let (tx, rx) = mpsc::channel();
        self.with_page(cx, |page, cx| {
            page.query(
                script,
                move |json| {
                    let _ = tx.send(json);
                },
                cx,
            )
        });
        for _ in 0..TIMEOUT_MS / 10 {
            if let Ok(json) = rx.try_recv() {
                return json;
            }
            sleep(cx, 10).await;
        }
        panic!("no result of {script}");
    }

    async fn loaded(&self, cx: &AsyncApp) {
        for _ in 0..TIMEOUT_MS / 10 {
            if self.renderer.frame().borrow().loaded() {
                return;
            }
            sleep(cx, 10).await;
        }
        panic!("the page did not load");
    }

    async fn settle(&self, cx: &mut AsyncApp) {
        for _ in 0..TIMEOUT_MS / QUIET_MS {
            let before = self.saved();
            sleep(cx, QUIET_MS).await;
            let shown = self
                .query(cx, "document.querySelector('main') !== null")
                .await;
            if shown == "true" && self.saved() == before {
                return;
            }
        }
        panic!("the page did not settle");
    }
}

#[derive(Default)]
struct Report {
    compared: usize,
    failed: Vec<String>,
}

impl Report {
    fn equal(&mut self, name: &str, actual: &JsValue, expected: &JsValue) {
        self.compared += 1;
        if actual != expected {
            println!("FAIL {name}: {actual:?}, expected {expected:?}");
            self.failed.push(name.to_owned());
        }
    }
}

fn js(value: &Value) -> JsValue {
    js_json::to_js(value).unwrap_or_else(|e| panic!("{e}: {value:?}"))
}

async fn replay(cx: &mut AsyncApp, report: &mut Report) {
    let records = vectors::load("tabs/documentation").expect("vectors");
    assert_eq!(records.len(), RECORDS);
    for record in &records {
        let case = Value::from_json(&record.json).expect("a case");
        let name = case.get("name").and_then(Value::as_str).expect("name");
        let mut page = Page::open(cx);
        let steps = case.get("steps").and_then(Value::as_array).expect("steps");
        for (i, step) in steps.iter().enumerate() {
            let op = step.get("op").and_then(Value::as_str).expect("op");
            let op = op.to_string_lossy();
            match op.as_str() {
                "save" => {}
                "restore" => page.restore(&js(step.get("state").expect("state"))),
                "render" => page.render(),
                "load" => {
                    page.loaded(cx).await;
                    page.settle(cx).await;
                }
                "settle" => page.settle(cx).await,
                "click" => {
                    let selector = step.get("selector").and_then(Value::as_str);
                    let selector = selector.expect("selector").to_string_lossy();
                    let script =
                        format!("document.querySelector('{selector}').click(); location.hash");
                    page.query(cx, &script).await;
                    page.settle(cx).await;
                }
                other => panic!("{other} step"),
            }
            let expected = js(step.get("saved").expect("saved"));
            report.equal(&format!("{name} step {i} ({op})"), &page.saved(), &expected);
        }
        page.close(cx);
    }
    assert_eq!(report.compared, STEPS);
}

async fn confined(cx: &mut AsyncApp, report: &mut Report) {
    let mut page = Page::open(cx);
    page.render();
    page.loaded(cx).await;
    page.settle(cx).await;
    page.query(
        cx,
        "window.blocked = 0; \
         addEventListener('securitypolicyviolation', () => window.blocked++); \
         fetch('https://example.com/').catch(() => {}); \
         new Image().src = 'https://example.com/a.png'; \
         location.href = 'https://example.com/'; \
         open('file:///etc/passwd'); 1",
    )
    .await;
    sleep(cx, 1000).await;
    for (name, script) in [
        ("remote requests are refused", "window.blocked === 2"),
        (
            "the page stays the documentation",
            "location.protocol === 'asrs-page:' && document.querySelector('main') !== null",
        ),
    ] {
        let held = page.query(cx, script).await;
        report.equal(name, &JsValue::string(&held), &JsValue::string("true"));
    }
    page.close(cx);
}

/// With a host process that ends at once on Linux, and no documentation elsewhere.
async fn unavailable(cx: &mut AsyncApp, report: &mut Report) {
    let set = |cx: &mut AsyncApp, build: PathBuf, host: PathBuf| {
        cx.update(|cx| {
            cx.set_global(DocumentationRoot {
                build,
                user_data: std::env::temp_dir(),
                host,
            });
        });
    };
    let build = if cfg!(target_os = "linux") {
        documentation_dir()
    } else {
        documentation_dir().join("none")
    };
    set(cx, build, PathBuf::from("/bin/false"));
    let mut page = Page::open(cx);
    page.restore(&JsValue::string("#/a b"));
    page.render();
    page.loaded(cx).await;
    page.render();
    let state = JsValue::string("#/a%20b");
    report.equal("a queued hash without a page", &page.saved(), &state);
    let gone = cx.update(|cx| {
        let shown = page.view.read(cx).page.as_ref();
        shown.is_some_and(|page| page.page().is_none())
    });
    report.equal(
        "no page is left open",
        &JsValue::Bool(gone),
        &JsValue::Bool(true),
    );
    page.close(cx);
    set(cx, documentation_dir(), webview_host());
}

async fn scroll(cx: &mut AsyncApp, report: &mut Report) {
    let mut page = Page::open(cx);
    page.render();
    page.loaded(cx).await;
    page.render();
    page.restore(&JsValue::string("#/tab-reference/console"));
    page.settle(cx).await;
    page.query(cx, "scrollTo(0, 240); scrollY").await;
    page.with_page(cx, |page, cx| page.set_visible(false, cx));
    sleep(cx, 500).await;
    page.with_page(cx, |page, cx| page.set_visible(true, cx));
    sleep(cx, 500).await;
    let top = page.query(cx, "scrollY").await;
    report.equal(
        "scroll position after hiding",
        &JsValue::string(&top),
        &JsValue::string("240"),
    );
    let mode = "getComputedStyle(document.documentElement).colorScheme + ' ' + \
                document.documentElement.getAttribute('data-theme')";
    for (dark, name) in [(true, "dark"), (false, "light")] {
        page.with_page(cx, |page, cx| page.set_dark(dark, cx));
        sleep(cx, 500).await;
        let theme = page.query(cx, mode).await;
        report.equal(
            &format!("{name} theme"),
            &JsValue::Bool(theme.contains(name)),
            &JsValue::Bool(true),
        );
    }
    page.close(cx);
}

fn tabs(selected: f64, hash: &str) -> JsValue {
    let tab = |kind: f64, uuid: &str, renderer: JsValue| {
        ops::object([
            ("type", JsValue::Number(kind)),
            ("title", JsValue::string("")),
            ("controller", JsValue::Null),
            ("controllerUUID", JsValue::string(uuid)),
            ("renderer", renderer),
            ("controlsHeight", JsValue::Number(0.0)),
        ])
    };
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
                ("selected", JsValue::Number(selected)),
                (
                    "tabs",
                    JsValue::Array(vec![
                        tab(
                            0.0,
                            "documentation0000000000000000000",
                            JsValue::string(hash),
                        ),
                        tab(12.0, "metadata000000000000000000000000", JsValue::Null),
                    ]),
                ),
            ]),
        ),
    ])
}

async fn hub_tab(cx: &mut AsyncApp, report: &mut Report) {
    let (window, frame) = cx.update(|cx| {
        asrs_ui::open_window(options(90.0), cx, |window, cx| {
            HubEntityHost::open(window, false, false, Vec::new(), cx)
        })
        .expect("hub window")
    });
    let hub = cx.update(|cx| frame.read(cx).host().hub().clone());
    let restore = |cx: &mut AsyncApp, selected: f64, hash: &str| {
        let message = NamedMessage::new(MainToHub::RestoreState, tabs(selected, hash));
        cx.update(|cx| hub.update(cx, |hub, cx| hub.handle_main_message(message, cx)));
    };
    let saved = |cx: &mut AsyncApp| {
        cx.update(|cx| {
            let hub = hub.read(cx).model();
            let tab = hub.tabs().tabs().next().expect("the documentation");
            let renderer = tab.renderer().downcast_ref::<DocumentationRenderer>();
            let frame = renderer.expect("its renderer").frame().borrow();
            let state = JsValue::String(frame.hash().clone());
            let whole = hub.save_state();
            let path = ["tabs", "tabs"]
                .iter()
                .fold(whole, |value, key| ops::get(&value, key).expect(key));
            let JsValue::Array(list) = &path else {
                panic!("no tab list");
            };
            let in_layout = ops::get(&list[0], "renderer").expect("renderer state");
            assert_eq!(in_layout, state);
            in_layout
        })
    };
    restore(cx, 1.0, "#/tab-reference/line-graph");
    sleep(cx, 1500).await;
    report.equal("a tab never shown", &saved(cx), &JsValue::string(""));
    for (name, hash) in [
        ("a tab shown", "#/tab-reference/line-graph"),
        ("a layout restored over it", "#/whats-new"),
    ] {
        restore(cx, 0.0, hash);
        let expected = JsValue::string(hash);
        for _ in 0..TIMEOUT_MS / 50 {
            if saved(cx) == expected {
                break;
            }
            sleep(cx, 50).await;
        }
        sleep(cx, QUIET_MS).await;
        report.equal(name, &saved(cx), &expected);
    }
    let _ = window.update(cx, |_, window, _| window.remove_window());
    drop((frame, hub));
    sleep(cx, 500).await;
}

fn main() {
    let needs = "a display and the built documentation";
    if asrs_parity::ci::skip_in_ci("documentation window", needs) {
        return;
    }
    if cfg!(windows) {
        println!("documentation window: skipped on this platform");
        return;
    }
    let set = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if cfg!(target_os = "linux") && (set("WAYLAND_DISPLAY") || !set("DISPLAY")) {
        println!("documentation window: skipped, needs an X11 session (unset WAYLAND_DISPLAY)");
        return;
    }
    let build = documentation_dir();
    assert!(
        build.join("index.html").is_file(),
        "run `cargo xtask docs` first"
    );
    asrs_ui::gpui::application().run(move |cx: &mut App| {
        asrs_ui::init(cx);
        cx.activate(true);
        cx.set_global(DocumentationRoot {
            build,
            user_data: std::env::temp_dir(),
            host: webview_host(),
        });
        cx.spawn(async move |cx| {
            let mut report = Report::default();
            let keep = cx.update(|cx| {
                asrs_ui::open_window(options(30.0), cx, |_, cx| {
                    cx.new(|_| Shown {
                        frame: Rc::default(),
                        page: None,
                    })
                })
                .expect("window")
            });
            replay(cx, &mut report).await;
            confined(cx, &mut report).await;
            unavailable(cx, &mut report).await;
            scroll(cx, &mut report).await;
            hub_tab(cx, &mut report).await;
            drop(keep);
            println!(
                "documentation window: {} compared, {} failed {:?}",
                report.compared,
                report.failed.len(),
                report.failed
            );
            std::process::exit(i32::from(!report.failed.is_empty()));
        })
        .detach();
    });
}
