//! U07's exit criteria on the real hub window, headless: the sidebar lays out only the rows on
//! screen, and the sidebar's active fields, which the model computes from the view's layout
//! reads, are exactly the value fields of those rows, before and after scrolling, at 1x and 2x.

use std::collections::BTreeSet;
use std::sync::Arc;

use asrs_app::hub::Hub;
use asrs_app::views::frame::HubFrame;
use asrs_app::views::hub_host::HubEntityHost;
use asrs_app::views::sidebar::SidebarView;
use asrs_hub::hub::HubModel;
use asrs_js::string::JsString;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::gpui::{Entity, WindowHandle, px, size};

const GROUPS: usize = 4;
const FIELDS: usize = 60;
const ROW_HEIGHT: f64 = 30.0;

fn entities(
    headless: &mut HeadlessCapture,
    window: WindowHandle<HubFrame<HubEntityHost>>,
) -> (Entity<Hub>, Entity<SidebarView>) {
    headless
        .context()
        .update_window(window.into(), |root, _, cx| {
            let frame = root
                .downcast::<HubFrame<HubEntityHost>>()
                .expect("hub frame");
            let host = frame.read(cx).host();
            (host.hub().clone(), host.sidebar().clone())
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

fn check(headless: &mut HeadlessCapture, window: WindowHandle<HubFrame<HubEntityHost>>, at: &str) {
    headless.capture(window.into()).expect("render");
    let (hub, sidebar) = entities(headless, window);
    let rendered = headless
        .context()
        .update(|cx| sidebar.read(cx).rendered_rows());
    let displayed = page(headless, &hub, |model| model.sidebar().displayed_rows());
    let active = page(headless, &hub, |model| {
        model.with_sidebar(|s, env| s.get_active_fields(env))
    });
    let visible: BTreeSet<JsString> = displayed[rendered.clone()]
        .iter()
        .filter_map(|row| row.full_key.clone())
        .collect();
    let active: BTreeSet<JsString> = active.into_iter().collect();
    assert_eq!(
        active, visible,
        "{at}: active fields differ from the rendered rows {rendered:?}"
    );
    let window_rows = (650.0 / ROW_HEIGHT).ceil() as usize + 2;
    assert!(
        rendered.len() <= window_rows && displayed.len() > 3 * window_rows,
        "{at}: laid out {} of {} rows",
        rendered.len(),
        displayed.len()
    );
    println!(
        "{at}: rows {rendered:?} of {}, {} active",
        displayed.len(),
        active.len()
    );
}

fn run(headless: &mut HeadlessCapture, scale: f32) {
    let window = headless
        .open_window(size(px(1100.0), px(650.0)), scale, |window, cx| {
            HubEntityHost::open(window, false, false, Vec::new(), cx)
        })
        .expect("hub window");
    let (hub, _) = entities(headless, window);
    page(headless, &hub, |model| {
        {
            let mut log = model.page().shared_log().borrow_mut();
            for g in 0..GROUPS {
                for f in 0..FIELDS {
                    let key = JsString::from(format!("/Group{g}/Field{f:02}").as_str());
                    log.put_number(&key, 0.0, (g * FIELDS + f) as f64).unwrap();
                }
            }
        }
        model
            .with_sidebar(|s, env| s.refresh(env, false))
            .expect("refresh");
        model.with_sidebar(|s, env| {
            for root in (0..GROUPS).rev() {
                let title = JsString::from(format!("Group{root}").as_str());
                let index = s
                    .rows()
                    .iter()
                    .position(|r| r.title == title)
                    .expect("group row");
                s.icon_click(env, index, 0, 0.0);
            }
        });
    });
    let at = format!("{scale}x");
    check(headless, window, &format!("{at} top"));
    for scroll in [500.0, 3333.0, 1e9] {
        page(headless, &hub, |model| {
            model.with_sidebar(|_, env| env.geometry.set_scroll_top(scroll))
        });
        headless.capture(window.into()).expect("render");
        check(headless, window, &format!("{at} scrolled to {scroll}"));
    }
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("sidebar_view: skipped, no headless renderer on this platform");
        return;
    }
    let mut headless = HeadlessCapture::new(Arc::new(())).expect("headless app");
    for scale in [1.0, 2.0] {
        run(&mut headless, scale);
    }
    println!("sidebar_view: ok");
}
