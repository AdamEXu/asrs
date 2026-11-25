use std::sync::Arc;

use asrs_ui::capture::{Capture, HeadlessCapture};
use gpui_kit::{
    AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, WindowHandle, div, px,
    rgb, size,
};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

struct Probe;

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0xffffff))
            .child(
                div()
                    .absolute()
                    .left(px(10.0))
                    .top(px(10.0))
                    .size(px(20.0))
                    .bg(rgb(0xff0000)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(40.0))
                    .top(px(8.0))
                    .text_size(px(16.0))
                    .text_color(rgb(0x000000))
                    .child("Parity 1x 2x"),
            )
    }
}

fn open(capture: &mut HeadlessCapture, scale: f32) -> WindowHandle<Probe> {
    capture
        .open_window(size(px(300.0), px(200.0)), scale, |_, cx| cx.new(|_| Probe))
        .expect("open headless window")
}

fn fresh(scale: f32) -> Capture {
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    let window = open(&mut capture, scale);
    capture.capture(window.into()).expect("capture")
}

fn assert_square(shot: &Capture, scale: u32) {
    let (lo, hi) = (10 * scale, 30 * scale);
    assert_eq!(shot.pixel(lo, lo), RED);
    assert_eq!(shot.pixel(hi - 1, hi - 1), RED);
    assert_eq!(shot.pixel(lo - 1, lo - 1), WHITE);
    assert_eq!(shot.pixel(hi, hi), WHITE);
}

fn has_text(shot: &Capture, scale: u32) -> bool {
    (8 * scale..30 * scale).any(|y| {
        (40 * scale..140 * scale).any(|x| {
            let [r, g, b, _] = shot.pixel(x, y);
            r == g && g == b && r < 128
        })
    })
}

/// Scrollbars follow the system's style while the app runs.
#[cfg(target_os = "macos")]
fn scrollbar_preferences(headless: &mut HeadlessCapture) {
    use asrs_ui::scroll::{Style, set_style, style, use_system_style};

    let cx = headless.context();
    let system = cx.update(|cx| {
        use_system_style(cx);
        style(cx)
    });
    let other = match system {
        Style::Overlay => Style::Classic,
        _ => Style::Overlay,
    };
    cx.update(|cx| set_style(other, false, cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| style(cx)), other);
    asrs_ui::platform::probe::post_scroller_style_change();
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| style(cx)), system);
}

fn main() {
    if asrs_parity::ci::skip_in_ci("headless capture", "a GPU") {
        return;
    }
    if cfg!(any(target_os = "macos", target_os = "linux")) {
        run();
    } else {
        println!("headless capture: skipped, no headless renderer on this platform");
    }
}

fn run() {
    let one = fresh(1.0);
    assert_eq!((one.width, one.height), (300, 200));
    assert_square(&one, 1);
    assert!(has_text(&one, 1));

    let two = fresh(2.0);
    assert_eq!((two.width, two.height), (600, 400));
    assert_square(&two, 2);
    assert!(has_text(&two, 2));

    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    let window = open(&mut capture, 1.0);
    let handle = window.into();
    assert_eq!(capture.capture(handle).expect("capture"), one);
    assert_eq!(capture.capture(handle).expect("capture"), one);

    #[cfg(target_os = "macos")]
    scrollbar_preferences(&mut capture);

    capture.set_scale_factor(handle, 2.0).expect("scale 2");
    assert_eq!(capture.capture(handle).expect("capture"), two);
    capture.set_scale_factor(handle, 1.0).expect("scale 1");
    assert_eq!(capture.capture(handle).expect("capture"), one);

    assert_eq!(fresh(1.0), one);
    assert_eq!(fresh(2.0), two);
    println!("headless capture: 300x200 at 1x and 600x400 at 2x, byte-stable");
}
