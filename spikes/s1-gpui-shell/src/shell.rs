use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{
    AnyWindowHandle, App, AppContext, AsyncApp, Bounds, Context, FontWeight, InteractiveElement,
    IntoElement, KeyBinding, Menu, MenuItem, MouseButton, MouseDownEvent, ParentElement, Pixels,
    Point, Render, ScrollDelta, ScrollEvent, SharedString, Styled, TitlebarOptions, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, actions, anchored,
    deferred, div, point, prelude::FluentBuilder, px, rgb, rgba, size,
};

use crate::mac;

actions!(
    s1,
    [
        About,
        ShowPreferences,
        OpenLogs,
        AddLogs,
        ToggleCustomAssets,
        NewLineGraph,
        NewField2d,
        PreviousTab,
        NextTab,
        ShiftLeft,
        Quit,
        OpenModal,
        OpenFloating
    ]
);

pub const SIDEBAR_W: f32 = 300.0;

pub struct Hub {
    pub popup_at: Option<Point<Pixels>>,
    pub custom_assets: bool,
    pub wheel_log: Rc<RefCell<Vec<String>>>,
    pub action_log: Rc<RefCell<Vec<String>>>,
    pub focus: Option<gpui_kit::FocusHandle>,
}

fn menus(custom_assets: bool) -> Vec<Menu> {
    vec![
        Menu::new("").items([
            MenuItem::action("About AdvantageScope", About),
            MenuItem::separator(),
            MenuItem::action("Settings...", ShowPreferences),
            MenuItem::action("Use Custom Assets Folder", ToggleCustomAssets).checked(custom_assets),
            MenuItem::action("Disabled Example", About).disabled(true),
            MenuItem::separator(),
            MenuItem::action("Quit AdvantageScope", Quit),
        ]),
        Menu::new("File").items([
            MenuItem::action("Open Log(s)...", OpenLogs),
            MenuItem::action("Add New Log(s)...", AddLogs),
        ]),
        Menu::new("Tabs").items([
            MenuItem::submenu(Menu::new("New Tab").items([
                MenuItem::action("\u{1f4c9} Line Graph", NewLineGraph),
                MenuItem::action("\u{1f5fa}\u{fe0f} 2D Field", NewField2d),
            ])),
            MenuItem::separator(),
            MenuItem::action("Previous Tab", PreviousTab),
            MenuItem::action("Next Tab", NextTab),
            MenuItem::action("Shift Left", ShiftLeft),
        ]),
        Menu::new("Window").items([
            MenuItem::action("Open Modal (EditRange)", OpenModal),
            MenuItem::action("Open Always-On-Top (Preferences)", OpenFloating),
        ]),
    ]
}

pub fn install_menus(cx: &mut App, custom_assets: bool) {
    cx.set_menus(menus(custom_assets));
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-,", ShowPreferences, None),
        KeyBinding::new("cmd-o", OpenLogs, None),
        KeyBinding::new("cmd-shift-o", AddLogs, None),
        KeyBinding::new("alt-g", NewLineGraph, None),
        KeyBinding::new("alt-2", NewField2d, None),
        KeyBinding::new("cmd-left", PreviousTab, None),
        KeyBinding::new("cmd-right", NextTab, None),
        KeyBinding::new("cmd-[", ShiftLeft, None),
        KeyBinding::new("cmd-q", Quit, None),
    ]);
}

pub fn hub_options(cx: &App) -> WindowOptions {
    let bounds = Bounds::centered(None, size(px(1100.0), px(650.0)), cx);
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("AdvantageScope".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(13.0), px(13.0))),
        }),
        window_background: WindowBackgroundAppearance::Transparent,
        app_owns_titlebar_drag: true,
        window_min_size: Some(size(px(800.0), px(400.0))),
        ..Default::default()
    }
}

impl Hub {
    fn log(&self, s: &str) {
        self.action_log.borrow_mut().push(s.to_string());
    }
}

fn popup_menu_element() -> impl IntoElement {
    let row = |label: &'static str, check: bool, disabled: bool, arrow: bool| {
        div()
            .h(px(22.0))
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .text_size(px(13.0))
            .when(disabled, |d| d.text_color(rgba(0x00000055)))
            .when(!disabled, |d| d.hover(|s| s.bg(rgb(0x2a69d4)).text_color(rgb(0xffffff))))
            .child(div().w(px(16.0)).child(if check { "\u{2713}" } else { "" }))
            .child(div().flex_1().child(label))
            .when(arrow, |d| d.child("\u{203a}"))
    };
    div()
        .w(px(200.0))
        .py(px(4.0))
        .bg(rgb(0xf6f6f6))
        .border_1()
        .border_color(rgba(0x0000001f))
        .rounded(px(6.0))
        .shadow_md()
        .child(row("Copy Key", false, false, false))
        .child(row("Loop Visible Range", true, false, false))
        .child(row("(No Unit Metadata)", false, true, false))
        .child(div().h(px(1.0)).my(px(4.0)).bg(rgba(0x0000001a)))
        .child(row("Driver Station", false, false, true))
}

impl Render for Hub {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let title_left = SIDEBAR_W + 20.0;
        let wheel_log = self.wheel_log.clone();
        let scale = window.scale_factor();
        div()
            .id("root")
            .when_some(self.focus.clone(), |d, f| d.track_focus(&f))
            .size_full()
            .relative()
            .font_family(".SystemUIFont")
            .text_color(rgb(0x000000))
            .on_action(cx.listener(|this, _: &ToggleCustomAssets, _, cx| {
                this.custom_assets = !this.custom_assets;
                this.log("ToggleCustomAssets");
                install_menus(cx, this.custom_assets);
            }))
            .on_action(cx.listener(|this, _: &NewLineGraph, _, _| this.log("NewLineGraph")))
            .on_action(cx.listener(|this, _: &OpenModal, window, cx| {
                this.log("OpenModal");
                open_modal(window.window_handle(), cx);
            }))
            .on_action(cx.listener(|this, _: &OpenFloating, _, cx| {
                this.log("OpenFloating");
                open_floating(cx);
            }))
            .on_scroll(move |ev: &ScrollEvent, _, _| {
                let d = match ev.delta {
                    ScrollDelta::Pixels(p) => format!(
                        "Pixels({:.3},{:.3})",
                        f32::from(p.x),
                        f32::from(p.y)
                    ),
                    ScrollDelta::Lines(l) => format!("Lines({:.3},{:.3})", l.x, l.y),
                };
                let px_line = ev.delta.pixel_delta(px(20.0));
                wheel_log.borrow_mut().push(format!(
                    "gpui ScrollEvent delta={d} pixel_delta(line=20px)=({:.3},{:.3}) phase={:?} pos=({:.0},{:.0}) scale={scale}",
                    f32::from(px_line.x),
                    f32::from(px_line.y),
                    ev.touch_phase,
                    f32::from(ev.position.x),
                    f32::from(ev.position.y),
                ));
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.popup_at = Some(ev.position);
                    cx.notify();
                }),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    if this.popup_at.take().is_some() {
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(SIDEBAR_W))
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .bg(rgb(0xffffff)),
            )
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(38.0))
                    .w(px(SIDEBAR_W))
                    .bottom_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .mt(px(5.0))
                            .h(px(20.0))
                            .text_size(px(12.0))
                            .font_weight(FontWeight::BOLD)
                            .opacity(0.5)
                            .flex()
                            .justify_center()
                            .child("Log Fields"),
                    )
                    .children(["AdvantageKit", "DriverStation", "NT:/SmartDashboard"].map(
                        |k| {
                            div()
                                .h(px(30.0))
                                .pl(px(35.0))
                                .pt(px(5.0))
                                .text_size(px(15.0))
                                .child(k)
                        },
                    )),
            )
            .child(
                div()
                    .id("titlebar")
                    .absolute()
                    .left_0()
                    .right_0()
                    .top_0()
                    .h(px(38.0))
                    .on_mouse_down(MouseButton::Left, |ev: &MouseDownEvent, window, _| {
                        if ev.click_count == 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .child(
                        div()
                            .absolute()
                            .left(px(title_left))
                            .right(px(154.0))
                            .top_0()
                            .h(px(38.0))
                            .line_height(px(38.0))
                            .text_size(px(14.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(SharedString::from("example.wpilog - AdvantageScope")),
                    ),
            )
            .when_some(self.popup_at, |d, p| {
                d.child(deferred(anchored().position(p).child(popup_menu_element())))
            })
    }
}

pub fn open_modal(parent: AnyWindowHandle, cx: &mut App) {
    let _ = parent;
    let bounds = Bounds::centered(None, size(px(300.0), px(108.0)), cx);
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Edit Range".into()),
                ..Default::default()
            }),
            kind: WindowKind::Dialog,
            is_resizable: false,
            ..Default::default()
        },
        |_, cx| cx.new(|_| Simple("Edit Range: modal child".into())),
    );
}

pub fn open_floating(cx: &mut App) {
    let bounds = Bounds::centered(None, size(px(400.0), px(473.0)), cx);
    let _ = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Preferences".into()),
                ..Default::default()
            }),
            kind: WindowKind::Floating,
            is_resizable: false,
            ..Default::default()
        },
        |_, cx| cx.new(|_| Simple("Preferences: always on top".into())),
    );
}

pub struct Simple(SharedString);

impl Render for Simple {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0xffffff))
            .p(px(10.0))
            .text_size(px(13.0))
            .child(self.0.clone())
    }
}

pub fn run_interactive() {
    gpui_kit::application().run(|cx: &mut App| {
        gpui_kit::init(cx);
        bind_keys(cx);
        install_menus(cx, false);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let opts = hub_options(cx);
        let handle = cx
            .open_window(opts, |window, cx| {
                mac::install_sidebar_vibrancy(window);
                cx.new(|_| Hub {
                    popup_at: None,
                    custom_assets: false,
                    wheel_log: Default::default(),
                    action_log: Default::default(),
                    focus: None,
                })
            })
            .unwrap();
        let _ = handle;
        cx.activate(true);
    });
}

fn save(img: &image::RgbaImage, dir: &std::path::Path, name: &str) -> String {
    let p = dir.join(name);
    img.save(&p).unwrap();
    p.display().to_string()
}

async fn sleep(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor().timer(Duration::from_millis(ms)).await;
}

pub fn run_auto(out: PathBuf) {
    std::fs::create_dir_all(&out).unwrap();
    gpui_kit::application().run(move |cx: &mut App| {
        gpui_kit::init(cx);
        bind_keys(cx);
        install_menus(cx, false);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let opts = hub_options(cx);
        let mut vibrancy = false;
        let handle = cx
            .open_window(opts, |window, cx| {
                vibrancy = mac::install_sidebar_vibrancy(window);
                cx.new(|_| Hub {
                    popup_at: None,
                    custom_assets: false,
                    wheel_log: Default::default(),
                    action_log: Default::default(),
                    focus: None,
                })
            })
            .unwrap();
        cx.activate(true);
        let _ = handle.update(cx, |hub, window, cx| {
            let f = cx.focus_handle();
            window.focus(&f, cx);
            hub.focus = Some(f);
        });
        println!("vibrancy view installed: {vibrancy}");
        let hub = handle.clone();
        let any: AnyWindowHandle = handle.into();
        cx.spawn(async move |cx: &mut AsyncApp| {
            sleep(cx, 1500).await;

            let _ = any.update(cx, |_, window, _| {
                println!("hub window: {}", mac::window_facts(window));
                println!(
                    "gpui bounds={:?} viewport={:?}",
                    window.bounds(),
                    window.viewport_size()
                );
                window.resize(size(px(1100.0), px(650.0)));
                println!("scale_factor={} appearance={:?}", window.scale_factor(), window.appearance());
                let t = Instant::now();
                match window.render_to_image() {
                    Ok(img) => {
                        let el = t.elapsed();
                        let side = img.get_pixel(100, 300).0;
                        let viewer = img.get_pixel(1500, 600).0;
                        println!(
                            "render_to_image: {}x{} in {:?}; sidebar px(100,300)={side:?} viewer px(1500,600)={viewer:?} -> {}",
                            img.width(),
                            img.height(),
                            el,
                            save(&img, &out, "shell_render_to_image.png")
                        );
                        let t2 = Instant::now();
                        let again = window.render_to_image().unwrap();
                        println!(
                            "render_to_image second call {:?}, identical={}",
                            t2.elapsed(),
                            again.as_raw() == img.as_raw()
                        );
                    }
                    Err(e) => println!("render_to_image failed: {e:#}"),
                }
                let t = Instant::now();
                match mac::cg_capture_window(window) {
                    Ok(img) => println!(
                        "CGWindowListCreateImage: {}x{} in {:?}; px(100,300)={:?} px(1500,600)={:?} -> {}",
                        img.width(),
                        img.height(),
                        t.elapsed(),
                        img.get_pixel(100.min(img.width() - 1), 300.min(img.height() - 1)).0,
                        img.get_pixel(1500.min(img.width() - 1), 600.min(img.height() - 1)).0,
                        save(&img, &out, "shell_cgwindow.png")
                    ),
                    Err(e) => println!("CGWindowListCreateImage failed: {e:#}"),
                }
            });

            let _ = cx.update(|_| println!("--- main menu (before toggle)\n{}", mac::dump_main_menu()));
            let _ = any.update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(ToggleCustomAssets), cx);
            });
            sleep(cx, 200).await;
            let _ = cx.update(|cx| {
                let _ = cx;
                println!("--- main menu (after toggle)\n{}", mac::dump_main_menu())
            });

            let _ = hub.update(cx, |hub, _, cx| {
                hub.popup_at = Some(point(px(500.0), px(200.0)));
                cx.notify();
            });
            sleep(cx, 300).await;
            let _ = any.update(cx, |_, window, _| {
                if let Ok(img) = window.render_to_image() {
                    println!("gpui popup capture -> {}", save(&img, &out, "popup_render_to_image.png"));
                }
            });
            let _ = hub.update(cx, |hub, _, cx| {
                hub.popup_at = None;
                cx.notify();
            });

            let view = any.update(cx, |_, window, _| mac::ns_view(window)).ok().flatten();
            if let Some(view) = view {
                let report = mac::native_popup_probe(&view, point(px(500.0), px(200.0)));
                println!("--- native NSMenu popup probe\n{report}");
            }

            let _ = any.update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(OpenModal), cx);
            });
            sleep(cx, 1200).await;
            let windows = cx.update(|cx| cx.windows());
            for w in &windows {
                let _ = w.update(cx, |_, window, _| {
                    println!("window: {}", mac::window_facts(window));
                });
            }
            if let Some(dialog) = windows.iter().find(|w| w.window_id() != any.window_id()) {
                let _ = dialog.update(cx, |_, window, _| {
                    if let Ok(img) = window.render_to_image() {
                        println!("dialog capture {}x{} -> {}", img.width(), img.height(), save(&img, &out, "dialog.png"));
                    }
                    window.remove_window();
                });
            }
            sleep(cx, 800).await;

            let _ = any.update(cx, |_, window, cx| {
                window.dispatch_action(Box::new(OpenFloating), cx);
            });
            sleep(cx, 800).await;
            let windows = cx.update(|cx| cx.windows());
            for w in &windows {
                let _ = w.update(cx, |_, window, _| {
                    println!("window: {}", mac::window_facts(window));
                });
            }

            for (pixel, value, continuous) in [
                (false, 1, false),
                (false, 3, false),
                (false, -1, false),
                (true, 10, true),
                (true, 1, true),
                (true, 37, true),
            ] {
                let nswin = any.update(cx, |_, window, _| mac::ns_window(window)).ok().flatten();
                let Some(nswin) = nswin else { break };
                if let Some((desc, ev)) = mac::make_scroll(&nswin, pixel, value, continuous) {
                    println!("{desc}");
                    nswin.sendEvent(&ev);
                    sleep(cx, 100).await;
                    let lines: Vec<String> = hub
                        .update(cx, |hub, _, _| hub.wheel_log.borrow_mut().drain(..).collect())
                        .unwrap_or_default();
                    if lines.is_empty() {
                        println!("  -> no GPUI scroll event");
                    }
                    for l in lines {
                        println!("  -> {l}");
                    }
                }
            }

            let _ = hub.update(cx, |hub, _, _| {
                println!("actions dispatched: {:?}", hub.action_log.borrow());
            });
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
