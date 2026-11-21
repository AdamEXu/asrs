#![allow(clippy::disallowed_methods)]

use std::sync::Arc;

use asrs_ui::capture::HeadlessCapture;
use asrs_ui::text::probe::{Probe, Record};
use asrs_ui::text::{Align, FontSpec, Text, TextStyle, WhiteSpace};
use gpui_kit::{
    AppContext, Context, IntoElement, ParentElement, Render, Styled, Window, div, px, rgb, size,
};

const SHORT: [&str; 6] = [
    "Drag a field here",
    "Open documentation",
    "No data for this tab",
    "Field Relative Rotation",
    "Values are read-only",
    "Teleoperated 0123456789",
];
const LONG: &str = "Drag a field here to begin, or open the documentation for this tab type.";
const NARROW: f32 = 200.0;
const STRETCHED: f32 = 300.0;

struct Boxes;

impl Render for Boxes {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let mut wrapping = TextStyle::new(FontSpec::parse("14px system-ui").unwrap());
        wrapping.white_space = WhiteSpace::Normal;
        let mut centered = TextStyle::new(FontSpec::parse("12px system-ui").unwrap());
        centered.align = Align::Center;
        let mut root = div().size_full().flex().flex_col().bg(rgb(0xffffff));
        for (i, s) in SHORT.iter().enumerate() {
            root = root.child(
                div()
                    .flex()
                    .w(px(1000.0))
                    .child(Text::new(*s, wrapping).probe(format!("short{i}"))),
            );
        }
        root.child(
            div()
                .flex()
                .w(px(NARROW))
                .child(Text::new(LONG, wrapping).probe("long")),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .w(px(STRETCHED))
                .child(Text::new("Center me", centered).probe("centered")),
        )
    }
}

fn records(scale: f32) -> Vec<Record> {
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    capture
        .context()
        .update(|cx| cx.set_global(Probe::default()));
    let window = capture
        .open_window(size(px(1100.0), px(400.0)), scale, |_, cx| {
            cx.new(|_| Boxes)
        })
        .expect("open window");
    capture
        .context()
        .update(|cx| cx.global_mut::<Probe>().0.clear());
    capture.capture(window.into()).expect("capture");
    capture
        .context()
        .update(|cx| std::mem::take(&mut cx.global_mut::<Probe>().0))
}

fn main() {
    if asrs_parity::ci::skip_in_ci("text box", "a GPU") {
        return;
    }
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("text box: skipped, no headless renderer on this platform");
        return;
    }
    for scale in [1.0, 2.0] {
        let records = records(scale);
        let get = |id: &str| {
            records
                .iter()
                .find(|r| r.id.as_ref() == id)
                .unwrap_or_else(|| panic!("no {id} at {scale}x"))
        };
        for i in 0..SHORT.len() {
            let r = get(&format!("short{i}"));
            assert_eq!(r.fragments.len(), 1, "{scale}x {:?}", r.fragments);
        }
        let long = get("long");
        assert_eq!(long.bounds[2], NARROW, "{scale}x");
        assert!(long.fragments.len() > 2, "{scale}x {:?}", long.fragments);
        for (t, rect) in &long.fragments {
            assert!(
                rect[0] + rect[2] <= f64::from(NARROW),
                "{scale}x {t:?} {rect:?}"
            );
        }
        let centered = get("centered");
        assert_eq!(centered.bounds[2], STRETCHED, "{scale}x");
        let [x, _, w, _] = centered.fragments[0].1;
        let gap = f64::from(STRETCHED) - w;
        assert!(
            (x - gap / 2.0).abs() <= 1.0 / 64.0,
            "{scale}x x {x} in gap {gap}"
        );
    }
    println!("text box: shrink-to-fit, wrapped and stretched boxes lay out at their own width");
}
