//! The source lists' Help window's page (`www/sourceListHelp.html`).

use std::rc::Rc;

use asrs_canvas::text::dom::Lu;
use asrs_js::color::hex_rgb;
use asrs_tabs::info::SourceListHelp;
use asrs_tabs::info::source_list_help::{HelpItem, ThemeColors};
use asrs_ui::gpui::{
    App, Context, Div, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyDownEvent,
    ParentElement, Render, Styled, Window, div, rgb,
};
use asrs_ui::icon::{Icon, IconSource};
use asrs_ui::parity::probed;
use asrs_ui::symbols::Symbol;
use asrs_ui::text::Text;
use asrs_ui::theme::Appearance;

use super::layout::{HelpItemLayout, HelpLayout, LuRect, TextBlock, help_title_style};
use super::page::{self, PageScroll, Placer, ScrolledPage};
use crate::views::cell_text::css;

const PARENT_WARNING: u32 = 0xff6600;
const SOURCES_OPACITY: f32 = 0.6;

pub struct SourceListHelpPage {
    help: SourceListHelp,
    /// The layout at a width and scale.
    layout: Option<(Lu, f32, Rc<HelpLayout>)>,
    scroll: PageScroll,
    focus: FocusHandle,
}

impl SourceListHelpPage {
    pub fn new(help: SourceListHelp, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        SourceListHelpPage {
            help,
            layout: None,
            scroll: PageScroll::new(cx),
            focus,
        }
    }

    pub fn help(&self) -> &SourceListHelp {
        &self.help
    }

    fn laid_out(&mut self, window: &Window) -> Rc<HelpLayout> {
        let (width, scale) = (page::device_width(window), window.scale_factor());
        match &self.layout {
            Some((w, s, layout)) if (*w, *s) == (width, scale) => layout.clone(),
            _ => {
                let layout = Rc::new(HelpLayout::new(self.help.items(), width, scale));
                self.layout = Some((width, scale, layout.clone()));
                layout
            }
        }
    }
}

fn color(colors: &ThemeColors, appearance: Appearance) -> u32 {
    let [r, g, b] = hex_rgb(appearance.pick(&colors[0], &colors[1]));
    u32::from_be_bytes([0, r, g, b])
}

/// The type's `<object>`, whose SVG takes `color` through `fill="currentColor"`.
fn icon(item: &HelpItem, appearance: Appearance) -> Option<Icon> {
    let svg = std::str::from_utf8(Symbol::source_list(&item.symbol)?.svg()).ok()?;
    let color = format!("#{:06x}", color(&item.colors, appearance));
    let source = IconSource::Svg(svg.replace("currentColor", &color).into());
    Some(Icon::new(source, ""))
}

struct Painter<'a> {
    placer: Placer,
    appearance: Appearance,
    scale: f32,
    boxes: Vec<Div>,
    cx: &'a mut App,
}

impl Painter<'_> {
    fn text(&mut self, block: &TextBlock, color: impl Fn(usize) -> u32) -> Vec<Div> {
        self.placer.block(block, color, self.cx)
    }

    fn probe(&mut self, id: &'static str, rect: LuRect) {
        self.boxes
            .push(div().child(probed(id, self.placer.place(rect))));
    }

    fn item(&mut self, item: &HelpItem, layout: &HelpItemLayout) {
        let theme = self.appearance.theme();
        self.probe("div.type-header", layout.header);
        if let Some(rule) = layout.rule {
            let rule_color = self.appearance.pick(0xeeeeee, 0x333333);
            self.boxes.push(self.placer.place(rule).bg(rgb(rule_color)));
        }
        if let Some(icon) = icon(item, self.appearance) {
            let [dx, dy] = self.placer.remainder(layout.icon);
            let icon = self.placer.place(layout.icon).child(icon.subpixel(dx, dy));
            self.boxes.push(icon);
        }
        let [_, dy] = self.placer.remainder(layout.title);
        let title = Text::new(item.title.clone(), help_title_style())
            .color(theme.text)
            .content_width(css(layout.title[2], self.scale))
            .subpixel(0.0, dy as f32);
        self.boxes.push(div().child(probed(
            "div.type-title",
            self.placer.place(layout.title).child(title),
        )));
        if let Some((rect, block)) = &layout.warning {
            self.probe("div.parent-warning", *rect);
            let text = self.text(block, |_| PARENT_WARNING);
            self.boxes.extend(text);
        }
        if let Some((rect, rows)) = &layout.table {
            self.probe("table.options", *rect);
            for (row, option) in rows.iter().zip(&item.options) {
                let name = self.text(&row.name, |_| theme.text);
                let appearance = self.appearance;
                let values = self.text(&row.values, |i| {
                    let colors = option.values[i].colors.as_ref();
                    colors.map_or(theme.text, |colors| color(colors, appearance))
                });
                self.boxes.extend(name.into_iter().chain(values));
            }
        }
        self.probe("div.source-types", layout.sources);
        let text = self.text(&layout.sources_text, |_| theme.text);
        self.boxes
            .extend(text.into_iter().map(|line| line.opacity(SOURCES_OPACITY)));
    }
}

impl Focusable for SourceListHelpPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl ScrolledPage for SourceListHelpPage {
    fn scroll(&mut self) -> &mut PageScroll {
        &mut self.scroll
    }
}

impl Render for SourceListHelpPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let layout = self.laid_out(window);
        self.scroll.lay_out(layout.height, window, cx);
        let appearance = Appearance::of(window, cx);
        let mut painter = Painter {
            placer: Placer::new(&self.scroll),
            appearance,
            scale: window.scale_factor(),
            boxes: Vec::new(),
            cx,
        };
        for (item, layout) in self.help.items().iter().zip(&layout.items) {
            painter.item(item, layout);
        }
        let boxes = painter.boxes;
        page::root("source-list-help", cx)
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                if view.scroll.key(&event.keystroke, cx) {
                    cx.notify();
                }
            }))
            .bg(rgb(appearance.theme().background))
            .children(boxes)
    }
}
