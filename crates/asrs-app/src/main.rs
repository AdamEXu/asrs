#![allow(clippy::disallowed_methods)]

use asrs_app::windows;
use asrs_ui::gpui::{
    App, AppContext, Context, IntoElement, Render, Styled, Window, application, div,
};

struct EmptyHub;

impl Render for EmptyHub {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn main() {
    if let Err(e) = asrs_js::math::libm::load() {
        eprintln!("asrs: {e}");
        std::process::exit(1);
    }
    application().run(|cx: &mut App| {
        asrs_ui::init(cx);
        windows::init(cx);
        windows::open_hub(cx, None, |_, cx| cx.new(|_| EmptyHub))
            .expect("failed to open the main window");
        cx.activate(true);
    });
}
