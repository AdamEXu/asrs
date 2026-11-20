use std::sync::Arc;

use asrs_ui::capture::{Capture, HeadlessCapture};
use gpui_kit::{
    AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, div, px, rgb, size,
};

const WHITE: [u8; 4] = [255, 255, 255, 255];
const RED: [u8; 4] = [255, 0, 0, 255];

struct Boxes(f32);

impl Render for Boxes {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0xffffff)).child(
            div()
                .absolute()
                .left(px(self.0))
                .top(px(self.0 + 10.0))
                .size(px(10.0))
                .bg(rgb(0xff0000)),
        )
    }
}

fn shot(offset: f32, scale: f32) -> Capture {
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    let window = capture
        .open_window(size(px(60.0), px(60.0)), scale, |_, cx| {
            cx.new(|_| Boxes(offset))
        })
        .expect("open headless window");
    capture.capture(window.into()).expect("capture")
}

/// The red box must cover device pixels `[lo, hi)` on both axes (y shifted by 10 css px).
fn assert_span(shot: &Capture, scale: u32, lo: u32, hi: u32) {
    let dy = 10 * scale;
    assert_eq!(shot.pixel(lo - 1, lo + dy), WHITE, "left of {lo}");
    assert_eq!(shot.pixel(lo, lo + dy), RED, "at {lo}");
    assert_eq!(shot.pixel(hi - 1, hi - 1 + dy), RED, "at {}", hi - 1);
    assert_eq!(shot.pixel(hi, hi - 1 + dy), WHITE, "right of {hi}");
    assert_eq!(shot.pixel(lo, lo + dy - 1), WHITE, "above {lo}");
    assert_eq!(shot.pixel(hi - 1, hi + dy), WHITE, "below {hi}");
}

fn main() {
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("pixel snap: skipped, no headless renderer on this platform");
        return;
    }
    assert_span(&shot(10.5, 1.0), 1, 11, 21);
    assert_span(&shot(10.25, 2.0), 2, 21, 41);
    assert_span(&shot(10.4, 1.0), 1, 10, 20);
    println!("pixel snap: layout edges round half up, as Chromium snaps them");
}
