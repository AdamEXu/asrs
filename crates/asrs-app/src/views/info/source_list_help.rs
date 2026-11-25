//! The source lists' Help window's page (`www/sourceListHelp.html`).

use std::borrow::Cow;
use std::rc::Rc;

use asrs_canvas::text::TextBackend;
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
use asrs_ui::selection::colors::SelectionColors;
use asrs_ui::selection::{Caret, Document, Hit, Place, Position};
use asrs_ui::symbols::Symbol;
use asrs_ui::text::Text;
use asrs_ui::theme::Appearance;

use super::layout::{
    HelpItemLayout, HelpLayout, HelpText, LuRect, TextBlock, Typesetter, help_title_style,
};
use super::page::{self, PageScroll, Placer, ScrolledPage, mix};
use super::text::{self, DocumentText, PageText};
use crate::views::cell_text::css;
use crate::views::page_text::Selected;

const PARENT_WARNING: u32 = 0xff6600;
const SOURCES_OPACITY: f32 = 0.6;

pub struct SourceListHelpPage {
    help: SourceListHelp,
    typesetter: Typesetter,
    /// The layout at a width and scale.
    layout: Option<(Lu, f32, Rc<HelpLayout>, Rc<HelpText>)>,
    scroll: PageScroll,
    text: PageText,
    focus: FocusHandle,
}

impl SourceListHelpPage {
    pub fn new(help: SourceListHelp, window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        SourceListHelpPage {
            help,
            typesetter: Typesetter::default(),
            layout: None,
            scroll: PageScroll::new(cx),
            text: PageText::default(),
            focus,
        }
    }

    pub fn help(&self) -> &SourceListHelp {
        &self.help
    }

    fn laid_out(&mut self, window: &Window) -> (Rc<HelpLayout>, Rc<HelpText>) {
        let (width, scale) = (self.scroll.width(window), window.scale_factor());
        match &self.layout {
            Some((w, s, layout, text)) if (*w, *s) == (width, scale) => {
                (layout.clone(), text.clone())
            }
            _ => {
                let t = &mut self.typesetter;
                let items = self.help.items();
                let layout = Rc::new(HelpLayout::new(t, items, width, scale));
                let text = Rc::new(HelpText::new(t, items, &layout, scale));
                self.layout = Some((width, scale, layout.clone(), text.clone()));
                (layout, text)
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

struct HelpDocument(Rc<HelpText>);

impl Document for HelpDocument {
    fn len(&self) -> usize {
        self.0.blocks.len()
    }

    fn text(&self, block: usize) -> Cow<'_, str> {
        Cow::Borrowed(&self.0.blocks[block].text)
    }

    fn place(&self, block: usize) -> Place {
        self.0.blocks[block].place
    }
}

impl DocumentText for HelpDocument {
    fn document(&self) -> &dyn Document {
        self
    }

    fn hit(&self, at @ [x, y]: [Lu; 2], backend: &mut dyn TextBackend, scale: f32) -> Hit {
        let bands = &self.0.bands;
        let band = bands.partition_point(|band| band.bottom <= y);
        let Some(band) = bands.get(band).or(bands.last()) else {
            return text::unselectable(0);
        };
        if band.icon.is_some_and(|right| x < right) {
            return text::unselectable(band.parts[0].1);
        }
        if let Some(([left, _, width, _], cells)) = &band.table
            && !(*left..=left + width).contains(&x)
        {
            let block = cells[usize::from(x - left > width / 2)];
            let caret = Caret::new(Position { block, offset: 0 });
            return Hit {
                caret: Some(caret),
                selectable: true,
                text: false,
                link: None,
            };
        }
        let part = band.parts.iter().find(|(right, _)| x < *right);
        let index = part.unwrap_or(&band.parts[0]).1;
        let block = &self.0.blocks[index];
        text::caret_in(
            (index, &block.block),
            &block.text,
            at,
            block.origin,
            backend,
            scale,
        )
    }
}

struct Painter<'a> {
    placer: Placer,
    appearance: Appearance,
    scale: f32,
    boxes: Vec<Div>,
    /// The boxes the selection paints behind each block.
    selection: Vec<Vec<LuRect>>,
    selected: Option<SelectionColors>,
    /// The block painted next.
    next: usize,
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

    /// The selection's boxes in the next block.
    fn behind(&mut self) -> Vec<LuRect> {
        self.next += 1;
        std::mem::take(&mut self.selection[self.next - 1])
    }

    /// The next block's selection and text.
    fn block(&mut self, block: &TextBlock, color: impl Fn(usize) -> u32) {
        let rects = self.behind();
        let mut boxes = Vec::new();
        if let Some(colors) = self.selected {
            boxes = self.placer.selection(&rects, colors.rgba());
        }
        boxes.extend(self.text(block, color));
        if let Some(color) = self.selected.and_then(|colors| colors.text) {
            let again = || self.placer.plain_block(block, color);
            boxes.extend(self.placer.recoloured(&rects, again));
        }
        self.boxes.extend(boxes);
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
        let rects = self.behind();
        let [_, dy] = self.placer.remainder(layout.title);
        let title = |color| {
            Text::new(item.title.clone(), help_title_style())
                .color(color)
                .content_width(css(layout.title[2], self.scale))
                .subpixel(0.0, dy as f32)
        };
        let shown = self.placer.place(layout.title).child(title(theme.text));
        let mut boxes = vec![div().child(probed("div.type-title", shown))];
        if let Some(colors) = self.selected {
            boxes.splice(0..0, self.placer.selection(&rects, colors.rgba()));
        }
        if let Some(color) = self.selected.and_then(|colors| colors.text) {
            let again = || {
                let title = title(color).unrecorded();
                vec![self.placer.place(layout.title).child(title)]
            };
            boxes.extend(self.placer.recoloured(&rects, again));
        }
        self.boxes.extend(boxes);
        if let Some((rect, block)) = &layout.warning {
            self.probe("div.parent-warning", *rect);
            self.block(block, |_| PARENT_WARNING);
        }
        if let Some((rect, rows)) = &layout.table {
            self.probe("table.options", *rect);
            self.next += 1;
            for (row, option) in rows.iter().zip(&item.options) {
                self.block(&row.name, |_| theme.text);
                let appearance = self.appearance;
                self.block(&row.values, |i| {
                    let colors = option.values[i].colors.as_ref();
                    colors.map_or(theme.text, |colors| color(colors, appearance))
                });
            }
        }
        if layout.table.is_some() {
            self.next += 1;
        }
        self.probe("div.source-types", layout.sources);
        let first = self.boxes.len();
        if self.selection[self.next].is_empty() {
            self.block(&layout.sources_text, |_| theme.text);
            for line in self.boxes.split_off(first) {
                let line = asrs_ui::text::faded(SOURCES_OPACITY, line);
                self.boxes.push(div().child(line));
            }
            return;
        }
        // The opacity is the group's: what is behind the text does not show through it.
        let faint = |color| mix(color, theme.background, SOURCES_OPACITY.into());
        let colors = self.selected;
        self.selected = colors.map(|colors| SelectionColors {
            background: faint(colors.background >> 8) << 8 | colors.background & 0xff,
            text: colors.text.map(faint),
        });
        self.block(&layout.sources_text, |_| faint(theme.text));
        self.selected = colors;
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

    fn text(&mut self) -> &mut PageText {
        &mut self.text
    }

    fn document_text(&self, _: &Window) -> Option<Box<dyn DocumentText>> {
        let (.., text) = self.layout.as_ref()?;
        Some(Box::new(HelpDocument(text.clone())))
    }
}

impl Render for SourceListHelpPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let (mut layout, mut text) = self.laid_out(window);
        if self.scroll.reserve(layout.height, window, cx) {
            (layout, text) = self.laid_out(window);
        }
        self.scroll.lay_out(layout.height, window, cx);
        text::autoscroll(self, window, cx);
        let appearance = Appearance::of(window, cx);
        let scale = window.scale_factor();
        let doc = HelpDocument(text.clone());
        let selected = self.text.selected(&doc, window, cx);
        let placer = Placer::new(&self.scroll);
        let selection = text.blocks.iter().enumerate().map(|(i, block)| {
            selected.as_ref().map_or_else(Vec::new, |s| {
                let at = (i, &block.block);
                self.text
                    .rects(&s.range, at, &block.text, block.origin, scale)
            })
        });
        let mut painter = Painter {
            placer,
            appearance,
            scale,
            boxes: Vec::new(),
            selection: selection.collect(),
            selected: selected.as_ref().map(|s| s.colors),
            next: 0,
            cx,
        };
        for (item, layout) in self.help.items().iter().zip(&layout.items) {
            painter.item(item, layout);
        }
        let mut boxes = painter.boxes;
        // A selected image is tinted.
        if let Some(Selected { range, colors }) = &selected {
            let whole = self.text.whole(&doc);
            let covered = |(block, _): &&(usize, LuRect)| {
                whole || (range.start.block < *block && *block <= range.end.block)
            };
            let icons = text.icons.iter().filter(covered).map(|(_, rect)| *rect);
            boxes.extend(placer.selection(&icons.collect::<Vec<_>>(), colors.rgba()));
        }
        page::root("source-list-help", self, cx)
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
