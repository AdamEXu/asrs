#![allow(clippy::disallowed_methods)]

use asrs_ui::gpui::{
    App, AppContext, Bounds, Context, IntoElement, Render, Styled, Window, WindowBounds,
    WindowOptions, application, div, px, size,
};

struct EmptyHub;

impl Render for EmptyHub {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn main() {
    application().run(|cx: &mut App| {
        asrs_ui::init(cx);
        let bounds = Bounds::centered(None, size(px(1100.0), px(650.0)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            ..Default::default()
        };
        asrs_ui::open_window(options, cx, |_, cx| cx.new(|_| EmptyHub))
            .expect("failed to open the main window");
        cx.activate(true);
    });
}
