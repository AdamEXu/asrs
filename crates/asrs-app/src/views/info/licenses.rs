//! The Licenses window's page (`www/licenses.html`).

use std::rc::Rc;

use asrs_canvas::text::dom::Lu;
use asrs_tabs::info::License;
use asrs_ui::gpui::{
    App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, Styled, Window, rgb,
};
use asrs_ui::theme::Appearance;

use super::layout::{LicenseBlock, LicensesLayout, Typesetter, license_style, module_style};
use super::page::{self, PageScroll, Placer, ScrolledPage};

pub struct LicensesPage {
    licenses: Vec<License>,
    typesetter: Typesetter,
    /// The layout at a width and scale.
    layout: Option<(Lu, f32, Rc<LicensesLayout>)>,
    scroll: PageScroll,
    focus: FocusHandle,
}

impl LicensesPage {
    pub fn new(licenses: Vec<License>, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        LicensesPage {
            licenses,
            typesetter: Typesetter::default(),
            layout: None,
            scroll: PageScroll::new(cx),
            focus,
        }
    }

    pub fn licenses(&self) -> &[License] {
        &self.licenses
    }

    fn laid_out(&mut self, window: &Window) -> Rc<LicensesLayout> {
        let (width, scale) = (page::device_width(window), window.scale_factor());
        match &self.layout {
            Some((w, s, layout)) if (*w, *s) == (width, scale) => layout.clone(),
            _ => {
                let t = &mut self.typesetter;
                let layout = Rc::new(LicensesLayout::new(t, &self.licenses, width, scale));
                self.layout = Some((width, scale, layout.clone()));
                layout
            }
        }
    }
}

impl Focusable for LicensesPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ScrolledPage for LicensesPage {
    fn scroll(&mut self) -> &mut PageScroll {
        &mut self.scroll
    }
}

impl Render for LicensesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let layout = self.laid_out(window);
        self.scroll.lay_out(layout.height, window, cx);
        let placer = Placer::new(&self.scroll);
        let theme = Appearance::of(window, cx).theme();
        let styles = [license_style(), module_style()];
        let (from, to) = placer.visible(window);
        let recording = page::recording(cx);
        let shown = if recording {
            &layout.blocks[..]
        } else {
            layout.blocks_between(from, to)
        };
        let mut root = page::root("licenses", cx)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                if view.scroll.key(&event.keystroke, cx) {
                    cx.notify();
                }
            }))
            .bg(rgb(theme.background));
        for LicenseBlock { module, top, text } in shown {
            let height = text.box_height;
            let lines = if recording {
                &text.lines[..]
            } else {
                text.lines_between(from - top, to - top)
            };
            for line in lines {
                let bounds = [layout.left, top + line.box_top, line.width, height];
                placer.record(cx, &line.text, bounds);
                if placer.shows(bounds[1], height, window) {
                    let style = &styles[usize::from(*module)];
                    root = root.child(placer.text(&line.text, style, theme.text, bounds));
                }
            }
        }
        root
    }
}
