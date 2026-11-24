//! The Licenses window's page (`www/licenses.html`).

use std::borrow::Cow;
use std::rc::Rc;

use asrs_canvas::text::TextBackend;
use asrs_canvas::text::dom::Lu;
use asrs_tabs::info::License;
use asrs_ui::gpui::{
    App, Context, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, Styled, Window, rgb,
};
use asrs_ui::selection::{Document, Hit, Place};
use asrs_ui::theme::Appearance;

use super::layout::{LicenseBlock, LicensesLayout, Typesetter, license_style, module_style};
use super::page::{self, PageScroll, Placer, ScrolledPage};
use super::text::{self, DocumentText, PageText};

pub struct LicensesPage {
    licenses: Vec<License>,
    typesetter: Typesetter,
    /// The layout at a width and scale.
    layout: Option<(Lu, f32, Rc<LicensesLayout>)>,
    scroll: PageScroll,
    text: PageText,
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
            text: PageText::default(),
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

/// The document's blocks.
struct LicensesText(Rc<LicensesLayout>);

impl Document for LicensesText {
    fn len(&self) -> usize {
        self.0.blocks.len()
    }

    fn text(&self, block: usize) -> Cow<'_, str> {
        Cow::Borrowed(&self.0.blocks[block].text.text)
    }

    fn place(&self, _: usize) -> Place {
        Place {
            newline_before: true,
            newline_after: true,
            gap: 1,
            ..Place::default()
        }
    }
}

impl DocumentText for LicensesText {
    fn document(&self) -> &dyn Document {
        self
    }

    fn hit(&self, at: [Lu; 2], backend: &mut dyn TextBackend, scale: f32) -> Hit {
        let blocks = &self.0.blocks;
        let index = blocks.partition_point(|b| b.top + b.text.height <= at[1]);
        let index = index.min(blocks.len().saturating_sub(1));
        let Some(LicenseBlock { module, top, text }) = blocks.get(index) else {
            return text::unselectable(0);
        };
        let style = if *module {
            module_style()
        } else {
            license_style()
        };
        let block = text.block(&style, 0, text.height);
        let origin = [self.0.left, *top];
        text::caret_in((index, &block), &text.text, at, origin, backend, scale)
    }
}

impl ScrolledPage for LicensesPage {
    fn scroll(&mut self) -> &mut PageScroll {
        &mut self.scroll
    }

    fn text(&mut self) -> &mut PageText {
        &mut self.text
    }

    fn document_text(&self, _: &Window) -> Option<Box<dyn DocumentText>> {
        let (_, _, layout) = self.layout.as_ref()?;
        Some(Box::new(LicensesText(layout.clone())))
    }
}

impl Render for LicensesPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let layout = self.laid_out(window);
        self.scroll.lay_out(layout.height, window, cx);
        text::autoscroll(self, window, cx);
        let doc = LicensesText(layout.clone());
        let selected = self.text.selected(&doc, window, cx);
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
        let mut root = page::root("licenses", self, cx)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                if view.scroll.key(&event.keystroke, cx) {
                    cx.notify();
                }
            }))
            .bg(rgb(theme.background));
        let first = if recording {
            0
        } else {
            let above = |b: &LicenseBlock| b.top + b.text.height <= from;
            layout.blocks.partition_point(above)
        };
        for (i, LicenseBlock { module, top, text }) in shown.iter().enumerate() {
            if let Some(selected) = &selected {
                let index = first + i;
                let style = &styles[usize::from(*module)];
                let block = text.block(style, from - top, to - top);
                let origin = [layout.left, *top];
                let scale = window.scale_factor();
                let rects =
                    (self.text).rects(&selected.range, (index, &block), &text.text, origin, scale);
                root = root.children(placer.selection(rects, selected.colors.rgba()));
            }
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
