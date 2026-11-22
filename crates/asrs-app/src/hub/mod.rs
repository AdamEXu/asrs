use asrs_ui::gpui::{Context, IntoElement, Render, Styled, Window, div};

/// The hub window's root view until the hub frame (U06) replaces it.
pub struct EmptyHub;

impl Render for EmptyHub {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().size_full()
    }
}
