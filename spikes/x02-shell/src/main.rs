#[cfg(windows)]
mod win;

#[cfg(windows)]
mod probe {
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use gpui_kit::{
        App, AppContext, AsyncApp, Bounds, Context, IntoElement, ParentElement, Render,
        ScrollDelta, ScrollEvent, Styled, TitlebarOptions, Window, WindowBackgroundAppearance,
        WindowBounds, WindowKind, WindowOptions, div, prelude::*, px, rgb, size,
    };

    use crate::win;

    type Log = Rc<RefCell<Vec<String>>>;

    struct Hub {
        wheel: Log,
    }

    impl Render for Hub {
        fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
            let wheel = self.wheel.clone();
            div()
                .size_full()
                .flex()
                .on_scroll(move |ev: &ScrollEvent, _, _| {
                    let d = match ev.delta {
                        ScrollDelta::Pixels(p) => {
                            format!("Pixels({:.3},{:.3})", f32::from(p.x), f32::from(p.y))
                        }
                        ScrollDelta::Lines(l) => format!("Lines({:.4},{:.4})", l.x, l.y),
                    };
                    wheel
                        .borrow_mut()
                        .push(format!("gpui ScrollEvent {d} phase={:?}", ev.touch_phase));
                })
                .child(div().w(px(300.0)).h_full())
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .bg(rgb(0xf0f0f0))
                        .child(div().m(px(40.0)).size(px(200.0)).bg(rgb(0x2b66a2))),
                )
        }
    }

    struct Plain;

    impl Render for Plain {
        fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
            div().size_full().bg(rgb(0xffffff))
        }
    }

    async fn sleep(cx: &mut AsyncApp, ms: u64) {
        cx.background_executor()
            .timer(Duration::from_millis(ms))
            .await;
    }

    fn say(lines: &Log, s: String) {
        println!("{s}");
        lines.borrow_mut().push(s);
    }

    pub fn run(out: PathBuf, wheel_deltas: Vec<String>) {
        std::fs::create_dir_all(&out).unwrap();
        gpui_kit::application().run(move |cx: &mut App| {
            gpui_kit::init(cx);
            let wheel: Log = Default::default();
            let report: Log = Default::default();
            let bounds = Bounds::centered(None, size(px(1100.0), px(650.0)), cx);
            let hub = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("x02 hub".into()),
                            appears_transparent: true,
                            traffic_light_position: None,
                        }),
                        window_background: WindowBackgroundAppearance::Transparent,
                        app_owns_titlebar_drag: true,
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| Hub {
                            wheel: wheel.clone(),
                        })
                    },
                )
                .unwrap();
            cx.activate(true);
            let any: gpui_kit::AnyWindowHandle = hub.into();
            cx.spawn(async move |cx: &mut AsyncApp| {
                sleep(cx, 2000).await;
                let mut hwnd = None;
                let _ = any.update(cx, |_, window, _| {
                    hwnd = win::hwnd(window);
                    say(&report, format!(
                        "hub opened: gpui bounds={:?} viewport={:?} scale={} client={:?} frame={:?}",
                        window.bounds(),
                        window.viewport_size(),
                        window.scale_factor(),
                        hwnd.map(win::client_rect),
                        hwnd.map(win::window_rect)
                    ));
                    window.resize(size(px(1100.0), px(650.0)));
                });
                let hwnd = hwnd.expect("hub hwnd");
                sleep(cx, 500).await;
                let _ = any.update(cx, |_, window, _| {
                    say(&report, format!(
                        "after resize: viewport={:?} client={:?}",
                        window.viewport_size(),
                        win::client_rect(hwnd)
                    ));
                    for attempt in 0..2 {
                        let t = Instant::now();
                        match window.render_to_image() {
                            Ok(img) => {
                                let el = t.elapsed();
                                let path = out.join(format!("render_to_image_{attempt}.png"));
                                img.save(&path).unwrap();
                                say(&report, format!(
                                    "render_to_image #{attempt}: {}x{} in {el:?}, px(100,300)={:?} px(700,300)={:?} px(400,100)={:?}",
                                    img.width(),
                                    img.height(),
                                    img.get_pixel(100, 300).0,
                                    img.get_pixel(700, 300).0,
                                    img.get_pixel(400, 100).0,
                                ));
                            }
                            Err(e) => say(&report, format!("render_to_image failed: {e:#}")),
                        }
                    }
                });
                let [x, y, w, h] = win::client_rect(hwnd);
                let screen = win::capture_screen(x, y, w, h);
                screen.save(out.join("screen_before_acrylic.png")).unwrap();
                if let Ok(rti) = image::open(out.join("render_to_image_0.png")) {
                    let rti = rti.to_rgba8();
                    let (mut opaque, mut exact) = (0, 0);
                    for (px, py, p) in rti.enumerate_pixels() {
                        if p[3] == 255 && px < screen.width() && py < screen.height() {
                            opaque += 1;
                            let s = screen.get_pixel(px, py);
                            if s[0] == p[0] && s[1] == p[1] && s[2] == p[2] {
                                exact += 1;
                            }
                        }
                    }
                    say(&report, format!("render_to_image vs screen BitBlt: {exact}/{opaque} opaque pixels exact"));
                }
                say(&report, format!("sidebar on screen before acrylic: {:?}", screen.get_pixel(100, 300).0));
                say(&report, format!(
                    "acrylic: DwmSetWindowAttribute(SYSTEMBACKDROP_TYPE, TRANSIENTWINDOW) -> {}, readback {}",
                    win::set_acrylic(hwnd),
                    win::backdrop(hwnd)
                ));
                let _ = any.update(cx, |_, window, _| window.refresh());
                sleep(cx, 1500).await;
                let screen = win::capture_screen(x, y, w, h);
                screen.save(out.join("screen_after_acrylic.png")).unwrap();
                say(&report, format!("sidebar on screen after acrylic: {:?}", screen.get_pixel(100, 300).0));

                let float_hwnd: Rc<RefCell<Option<windows::Win32::Foundation::HWND>>> = Default::default();
                let popup_hwnd: Rc<RefCell<Option<windows::Win32::Foundation::HWND>>> = Default::default();
                let (fh, ph) = (float_hwnd.clone(), popup_hwnd.clone());
                let _ = cx.update(|cx| {
                    for (kind, slot, title) in [
                        (WindowKind::Floating, fh, "x02 floating"),
                        (WindowKind::PopUp, ph, "x02 popup"),
                    ] {
                        let b = Bounds::centered(None, size(px(300.0), px(200.0)), cx);
                        let _ = cx.open_window(
                            WindowOptions {
                                window_bounds: Some(WindowBounds::Windowed(b)),
                                titlebar: Some(TitlebarOptions {
                                    title: Some(title.into()),
                                    ..Default::default()
                                }),
                                kind,
                                ..Default::default()
                            },
                            |window, cx| {
                                *slot.borrow_mut() = win::hwnd(window);
                                cx.new(|_| Plain)
                            },
                        );
                    }
                });
                sleep(cx, 800).await;
                if let Some(f) = *float_hwnd.borrow() {
                    let before = win::is_topmost(f);
                    let ok = win::set_topmost(f);
                    say(&report, format!(
                        "WindowKind::Floating: WS_EX_TOPMOST={before}; SetWindowPos(HWND_TOPMOST) ok={ok}, now {}",
                        win::is_topmost(f)
                    ));
                }
                if let Some(p) = *popup_hwnd.borrow() {
                    say(&report, format!("WindowKind::PopUp: WS_EX_TOPMOST={}", win::is_topmost(p)));
                }

                let (lines, chars) = win::wheel_settings();
                say(&report, format!("SPI_GETWHEELSCROLLLINES={lines} SPI_GETWHEELSCROLLCHARS={chars}"));
                let _ = any.update(cx, |_, window, _| window.activate_window());
                sleep(cx, 500).await;
                let (cxp, cyp) = (x + 700, y + 400);
                let deltas = wheel_deltas.clone();
                std::thread::spawn(move || win::send_wheel(cxp, cyp, &deltas));
                sleep(cx, 400 * wheel_deltas.len() as u64 + 1500).await;
                for line in wheel.borrow().iter() {
                    say(&report, line.clone());
                }
                std::fs::write(out.join("shell.txt"), report.borrow().join("\n") + "\n").unwrap();
                let _ = cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let deltas = || -> Vec<String> {
        vec!["120", "-120", "360", "40", "-40", "h120", "h-120"]
            .into_iter()
            .map(String::from)
            .collect()
    };
    match args.first().map(String::as_str) {
        #[cfg(windows)]
        Some("shell") => probe::run(
            args.get(1).map_or_else(|| "x02-shell-out".into(), Into::into),
            deltas(),
        ),
        #[cfg(windows)]
        Some("send-wheel") => {
            let x = args[1].parse().unwrap();
            let y = args[2].parse().unwrap();
            let d = if args.len() > 3 { args[3..].to_vec() } else { deltas() };
            win::send_wheel(x, y, &d);
        }
        #[cfg(windows)]
        Some("winfo") => {
            for line in win::windows_matching(args.get(1).map_or("", String::as_str)) {
                println!("{line}");
            }
        }
        #[cfg(windows)]
        Some("screen") => {
            let n: Vec<i32> = args[2..6].iter().map(|v| v.parse().unwrap()).collect();
            win::capture_screen(n[0], n[1], n[2], n[3]).save(&args[1]).unwrap();
        }
        _ => {
            eprintln!("usage: x02-shell shell <out dir> | send-wheel x y [deltas] | winfo <title> | screen <png> x y w h");
            std::process::exit(2);
        }
    }
}
