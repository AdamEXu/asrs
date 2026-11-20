use std::io::Write;
use std::time::Duration;

use gpui_kit::{
    App, AppContext, Bounds, Context, InteractiveElement, IntoElement, Render,
    ScrollDelta, ScrollEvent, Styled, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    point, px, rgb, size,
};

struct Probe;

impl Render for Probe {
    fn render(&mut self, window: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let scale = window.scale_factor();
        div()
            .id("root")
            .size_full()
            .bg(rgb(0xffffff))
            .on_scroll(move |ev: &ScrollEvent, _, _| {
                let d = match ev.delta {
                    ScrollDelta::Pixels(p) => {
                        format!("Pixels({:.4},{:.4})", f32::from(p.x), f32::from(p.y))
                    }
                    ScrollDelta::Lines(l) => format!("Lines({:.4},{:.4})", l.x, l.y),
                };
                println!(
                    "gpui {d} phase={:?} pos=({:.0},{:.0}) scale={scale}",
                    ev.touch_phase,
                    f32::from(ev.position.x),
                    f32::from(ev.position.y)
                );
                let _ = std::io::stdout().flush();
            })
    }
}

fn main() {
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);
    gpui_kit::application().run(move |cx: &mut App| {
        let bounds = Bounds::new(point(px(100.0), px(100.0)), size(px(600.0), px(400.0)));
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("x03-wheel".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_, cx| cx.new(|_| Probe),
        )
        .unwrap();
        cx.activate(true);
        println!("gpui window open");
        let _ = std::io::stdout().flush();
        cx.spawn(async move |cx| {
            cx.background_executor()
                .timer(Duration::from_secs(secs))
                .await;
            let _ = cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
