//! Where `licenses.css`, `sourceListHelp.css` and `betaWelcome.css` put their pages' boxes, in
//! device `LayoutUnit`s from the page's top left corner.

use std::collections::HashMap;
use std::rc::Rc;

use asrs_canvas::text::dom::{self, Align, Lu};
use asrs_canvas::text::flow::{self, Flow};
use asrs_canvas::text::{FixedAdvance, TextBackend, platform_backend};
use asrs_tabs::info::beta_welcome::CHECKBOXES;
use asrs_tabs::info::source_list_help::HelpItem;
use asrs_tabs::info::{BetaWelcome, License};
use asrs_ui::input::style::CONTROL_FONT;
use asrs_ui::text::TextStyle;
use asrs_ui::theme::FONT_FAMILY;

use crate::views::cell_text::{border, length};

/// `[x, y, width, height]`.
pub type LuRect = [Lu; 4];

/// A length on Blink's device pixel grid (`SnapSizeToPixel` at the page's origin).
pub fn snapped(length: Lu) -> Lu {
    (length + dom::LU_PER_PX / 2).div_euclid(dom::LU_PER_PX) * dom::LU_PER_PX
}

fn style(font: &str) -> TextStyle {
    let font = format!("{font} {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([("font", font.as_str())]).expect("a font")
}

/// Text nodes of one block, laid out at `origin`.
#[derive(Clone, Debug, PartialEq)]
pub struct TextBlock {
    pub style: TextStyle,
    pub origin: [Lu; 2],
    pub flow: Rc<Flow>,
}

impl TextBlock {
    /// The boxes of the text node `item`, one per line it is on.
    pub fn rects(&self, item: usize) -> Vec<LuRect> {
        let [x, y] = self.origin;
        let lines = self.flow.lines.iter();
        lines
            .flat_map(|line| line.pieces.iter().map(move |piece| (line, piece)))
            .filter(|(_, piece)| piece.item == item)
            .map(|(line, piece)| {
                let top = y + line.box_top;
                [x + piece.x, top, piece.width, self.flow.box_height]
            })
            .collect()
    }
}

/// Measures and lays out the pages' text.
pub struct Typesetter {
    backend: Box<dyn TextBackend>,
    scale: f32,
}

impl Typesetter {
    pub fn new(scale: f32) -> Typesetter {
        Typesetter {
            backend: platform_backend().unwrap_or_else(|| Box::new(FixedAdvance)),
            scale,
        }
    }

    fn px(&self, css: f64) -> Lu {
        length(css, self.scale)
    }

    fn block(
        &mut self,
        style: TextStyle,
        items: &[&str],
        origin: [Lu; 2],
        available: Option<Lu>,
    ) -> TextBlock {
        let backend = self.backend.as_mut();
        let flow = flow::layout(backend, &style, items, false, available, self.scale);
        TextBlock {
            style,
            origin,
            flow: Rc::new(flow),
        }
    }

    fn max_content(&mut self, style: &TextStyle, text: &str) -> Lu {
        let backend = self.backend.as_mut();
        flow::layout(backend, style, &[text], false, None, self.scale).width
    }
}

/// A line box of a `div.license-text` or `div.module-text` that has text.
#[derive(Clone, Debug, PartialEq)]
pub struct ParagraphLine {
    /// The index of the text node among its siblings, `<br>`s not counted.
    pub part: usize,
    pub text: String,
    /// The text box's top, from the block's.
    pub box_top: Lu,
    pub width: Lu,
}

/// Text nodes with `<br>`s between them under `white-space: break-spaces`.
#[derive(Clone, Debug, PartialEq)]
pub struct Paragraphs {
    pub lines: Vec<ParagraphLine>,
    pub height: Lu,
    pub box_height: Lu,
}

impl Paragraphs {
    /// The lines whose boxes overlap `from..to` of the block.
    pub fn lines_between(&self, from: Lu, to: Lu) -> &[ParagraphLine] {
        let bottom = |line: &ParagraphLine| line.box_top + self.box_height;
        let first = self.lines.partition_point(|line| bottom(line) <= from);
        let last = self.lines.partition_point(|line| line.box_top < to);
        &self.lines[first..last.max(first)]
    }

    fn new(typesetter: &mut Typesetter, style: &TextStyle, parts: &[String], width: Lu) -> Self {
        let backend = typesetter.backend.as_mut();
        let scale = typesetter.scale;
        let (ascent, descent, strut) = dom::line_metrics(backend, style, scale);
        let mut out = Paragraphs {
            lines: Vec::new(),
            height: 0,
            box_height: ascent + descent,
        };
        for (part, text) in parts.iter().enumerate() {
            if text.is_empty() {
                // A `<br>` before the block's end starts no line.
                if part + 1 < parts.len() {
                    out.height += strut;
                }
                continue;
            }
            let flow = flow::layout(backend, style, &[text], true, Some(width), scale);
            for line in flow.lines {
                for piece in line.pieces {
                    out.lines.push(ParagraphLine {
                        part,
                        text: piece.text,
                        box_top: out.height + line.box_top,
                        width: piece.width,
                    });
                }
            }
            out.height += flow.height;
        }
        out
    }
}

/// `div.module-text`.
pub fn module_style() -> TextStyle {
    style("bold 30px")
}

/// `div.license-text`.
pub fn license_style() -> TextStyle {
    style("16px")
}

#[derive(Clone, Debug, PartialEq)]
pub struct LicenseBlock {
    /// Whether it is a `div.module-text`.
    pub module: bool,
    pub top: Lu,
    pub text: Rc<Paragraphs>,
}

/// licenses.html: one column of blocks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LicensesLayout {
    pub left: Lu,
    pub width: Lu,
    pub blocks: Vec<LicenseBlock>,
    pub height: Lu,
}

impl LicensesLayout {
    pub fn new(licenses: &[License], width: Lu, scale: f32) -> LicensesLayout {
        let mut typesetter = Typesetter::new(scale);
        let (margin, padding) = (typesetter.px(8.0), typesetter.px(10.0));
        let below = typesetter.px(25.0);
        let inset = margin + padding;
        let mut out = LicensesLayout {
            left: inset,
            width: (width - 2 * inset).max(0),
            blocks: Vec::with_capacity(2 * licenses.len()),
            height: inset,
        };
        let styles = [license_style(), module_style()];
        let mut known: HashMap<(bool, Vec<String>), Rc<Paragraphs>> = HashMap::new();
        for license in licenses {
            for (module, text) in [(true, &license.module), (false, &license.parts)] {
                let parts: Vec<String> = text.iter().map(|t| t.to_string_lossy()).collect();
                let text = known
                    .entry((module, parts))
                    .or_insert_with_key(|(_, parts)| {
                        let style = &styles[usize::from(module)];
                        Rc::new(Paragraphs::new(&mut typesetter, style, parts, out.width))
                    });
                out.blocks.push(LicenseBlock {
                    module,
                    top: out.height,
                    text: text.clone(),
                });
                out.height += text.height + below;
            }
        }
        out.height += inset;
        out
    }

    /// The blocks that overlap `from..to`.
    pub fn blocks_between(&self, from: Lu, to: Lu) -> &[LicenseBlock] {
        let first = self
            .blocks
            .partition_point(|b| b.top + b.text.height <= from);
        let last = self.blocks.partition_point(|b| b.top < to).max(first);
        &self.blocks[first..last]
    }
}

pub fn help_title_style() -> TextStyle {
    let font = format!("800 20px {}", FONT_FAMILY.join(", "));
    TextStyle::from_css([
        ("font", font.as_str()),
        ("line-height", "40px"),
        ("white-space", "nowrap"),
        ("text-overflow", "ellipsis"),
    ])
    .expect("help title style")
}

#[derive(Clone, Debug, PartialEq)]
pub struct HelpRow {
    pub row: LuRect,
    /// `td:first-child` and its text.
    pub name_cell: LuRect,
    pub name: TextBlock,
    /// `td:last-child` and its text nodes.
    pub value_cell: LuRect,
    pub values: TextBlock,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HelpItemLayout {
    /// `div.type-header`'s border box.
    pub header: LuRect,
    /// Its `border-top`, if it has one.
    pub rule: Option<LuRect>,
    /// `div.type-icon-container`.
    pub icon: LuRect,
    /// `div.type-title`.
    pub title: LuRect,
    /// `div.parent-warning` and its text.
    pub warning: Option<(LuRect, TextBlock)>,
    /// `table.options` and its rows.
    pub table: Option<(LuRect, Vec<HelpRow>)>,
    /// `div.source-types` and its text.
    pub sources: LuRect,
    pub sources_text: TextBlock,
}

/// sourceListHelp.html.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HelpLayout {
    pub items: Vec<HelpItemLayout>,
    pub height: Lu,
}

impl HelpLayout {
    pub fn new(items: &[HelpItem], width: Lu, scale: f32) -> HelpLayout {
        let mut typesetter = Typesetter::new(scale);
        let mut out = HelpLayout::default();
        for item in items {
            let first = out.items.is_empty();
            let layout = help_item(&mut typesetter, item, width, out.height, first);
            out.height = layout.sources[1] + layout.sources[3];
            out.items.push(layout);
        }
        out
    }
}

fn help_item(
    typesetter: &mut Typesetter,
    item: &HelpItem,
    width: Lu,
    top: Lu,
    first: bool,
) -> HelpItemLayout {
    let header_height = typesetter.px(40.0);
    let left = typesetter.px(42.0);
    let below = typesetter.px(8.0);
    let content = width - typesetter.px(47.0);
    let rule = if first { 0 } else { border(typesetter.scale) };
    let mut y = top + rule + header_height;

    let warning = item.parent_warning.as_ref().map(|text| {
        let block = typesetter.block(style("16px"), &[text], [left, y], Some(content));
        let rect = [left, y, content, block.flow.height + below];
        y += rect[3];
        (rect, block)
    });
    let table = (!item.options.is_empty()).then(|| {
        let (rect, rows) = help_table(typesetter, item, [left, y], content);
        y += rect[3];
        (rect, rows)
    });
    let text = typesetter.block(
        style("italic 12px"),
        &[&item.sources],
        [left, y],
        Some(content),
    );
    HelpItemLayout {
        header: [0, top, width, rule + header_height],
        rule: (!first).then_some([0, top, width, rule]),
        icon: [0, top + rule, header_height, header_height],
        title: [left, top + rule, width - left, header_height],
        warning,
        table,
        sources: [left, y, content, text.flow.height + below],
        sources_text: text,
    }
}

impl HelpItemLayout {
    /// The line boxes `Range.getClientRects()` gives for `div.type-title`'s text: the whole
    /// text's, and where an ellipsis cuts it, what is left.
    pub fn title_rects(&self, title: &str, scale: f32) -> Vec<LuRect> {
        let [x, y, w, _] = self.title;
        let mut typesetter = Typesetter::new(scale);
        let style = help_title_style();
        let layout = dom::layout(typesetter.backend.as_mut(), &style, title, Some(w), scale);
        let height = layout.ascent + layout.descent;
        let mut out = Vec::new();
        for line in &layout.lines {
            let top = y + line.baseline - layout.ascent;
            let rect = |width| [x, top, width, height];
            if line.cut_at.is_some() {
                out.push(rect(line.width));
            }
            let kept = line
                .fragments
                .iter()
                .filter(|f| line.cut_at.is_none() || f.text != dom::ELLIPSIS);
            out.push(rect(kept.map(|f| f.width).sum()));
        }
        out
    }
}

/// The table's columns: the names take their widest, the values the rest.
fn help_table(
    typesetter: &mut Typesetter,
    item: &HelpItem,
    [left, top]: [Lu; 2],
    width: Lu,
) -> (LuRect, Vec<HelpRow>) {
    let padding = typesetter.px(1.0);
    let (below, before) = (typesetter.px(8.0), typesetter.px(5.0));
    let mut name_style = style("600 16px");
    name_style.align = Align::Right;
    let mut value_style = style("16px");
    value_style.break_word = true;
    let names = item.options.iter();
    let name_width = names
        .map(|option| typesetter.max_content(&name_style, &option.name))
        .max()
        .unwrap_or(0);
    let name_column = name_width + padding;
    let value_left = left + name_column;
    let value_width = width - name_column;
    let mut y = top;
    let rows = item
        .options
        .iter()
        .map(|option| {
            let content = y + padding;
            let name = typesetter.block(
                name_style,
                &[&option.name],
                [left, content],
                Some(name_width),
            );
            let values: Vec<&str> = option.values.iter().map(|v| v.text.as_str()).collect();
            let values = typesetter.block(
                value_style,
                &values,
                [value_left + before, content],
                Some(value_width - before),
            );
            let height = padding + name.flow.height.max(values.flow.height) + below;
            let row = HelpRow {
                row: [left, y, width, height],
                name_cell: [left, y, name_column, height],
                name,
                value_cell: [value_left, y, value_width, height],
                values,
            };
            y += height;
            row
        })
        .collect();
    ([left, top, width, y - top], rows)
}

pub const BETA_CHECKS: [&str; CHECKBOXES] = [
    "I understand that this is an unstable release, and I will encounter issues not present in stable releases.",
    "I agree to report any issues I encounter to the developers, which will help to improve future releases.",
    "I agree to provide feedback to the developers if asked about my experience using this release.",
];

/// The `p` under the table: the `span.purple` is the second of its text nodes.
pub const BETA_ICON_TEXT: [&str; 3] = [
    "At any time, click the ",
    "purple icon",
    " in the title bar to report an issue, request a feature, or ask a question.",
];

pub fn beta_intro(app_name: &str) -> String {
    format!(
        "Thanks for helping test and improve upcoming features in {app_name}! Please check the boxes below to acknowledge each statement before continuing."
    )
}

pub fn beta_button_style() -> TextStyle {
    let mut font = CONTROL_FONT;
    font.size = 25.0;
    TextStyle::new(font)
}

#[derive(Clone, Debug, PartialEq)]
pub struct BetaRow {
    pub row: LuRect,
    pub cells: [LuRect; 2],
    pub checkbox: LuRect,
    pub text: TextBlock,
}

/// betaWelcome.html.
#[derive(Clone, Debug, PartialEq)]
pub struct BetaLayout {
    pub title: (LuRect, TextBlock),
    /// The `.beta-only` and `.alpha-only` headings that show.
    pub headings: Vec<(LuRect, TextBlock)>,
    pub intro: (LuRect, TextBlock),
    pub table: LuRect,
    pub rows: Vec<BetaRow>,
    pub outro: (LuRect, TextBlock),
    pub button: LuRect,
    pub button_text: TextBlock,
    /// The document's height.
    pub height: Lu,
}

impl BetaLayout {
    pub fn new(page: &BetaWelcome, app_name: &str, width: Lu, scale: f32) -> BetaLayout {
        let mut typesetter = Typesetter::new(scale);
        let t = &mut typesetter;
        let inset = t.px(8.0) + t.px(15.0);
        let content = width - 2 * inset;
        let heading_margin = t.px(15.0);
        let heading_left = inset + heading_margin;
        let heading_width = content - 2 * heading_margin;
        let paragraph_margin = t.px(14.0);
        let mut y = heading_margin;

        let centred = |font: &str| TextStyle {
            align: Align::Center,
            ..style(font)
        };
        let block = |t: &mut Typesetter, font: &str, items: &[&str], x, width, y: &mut Lu| {
            let text = t.block(centred(font), items, [x, *y], Some(width));
            let rect = [x, *y, width, text.flow.height];
            *y += rect[3];
            (rect, text)
        };
        let title = block(
            t,
            "bold 40px",
            &["Welcome \u{1f44b}"],
            heading_left,
            heading_width,
            &mut y,
        );
        let mut headings = Vec::new();
        for (shown, text) in [
            (page.shows_beta(), "THIS IS A BETA RELEASE"),
            (page.shows_alpha(), "THIS IS AN ALPHA RELEASE"),
        ] {
            if shown {
                y += heading_margin;
                headings.push(block(
                    t,
                    "bold 25px",
                    &[text],
                    heading_left,
                    heading_width,
                    &mut y,
                ));
            }
        }
        y += heading_margin.max(paragraph_margin);
        let intro = block(t, "14px", &[&beta_intro(app_name)], inset, content, &mut y);
        y += paragraph_margin;
        let table_top = y;
        let rows = beta_rows(t, inset, &mut y, content);
        y += t.px(2.0);
        let table = [inset, table_top, content, y - table_top];
        y += paragraph_margin;
        let outro = block(t, "14px", &BETA_ICON_TEXT, inset, content, &mut y);
        y += paragraph_margin;
        let button_margin = t.px(10.0);
        let button_width = content - button_margin;
        let button = [inset + button_margin / 2, y, button_width, t.px(50.0)];
        let mut button_style = beta_button_style();
        button_style.align = Align::Center;
        let mut button_text = t.block(
            button_style,
            &["Continue"],
            [button[0], y],
            Some(button_width),
        );
        button_text.origin[1] += (button[3] - button_text.flow.height) / 2;
        y += button[3] + button_margin;
        BetaLayout {
            title,
            headings,
            intro,
            table,
            rows,
            outro,
            button,
            button_text,
            height: y + t.px(8.0),
        }
    }
}

fn beta_rows(t: &mut Typesetter, left: Lu, y: &mut Lu, width: Lu) -> Vec<BetaRow> {
    let spacing = t.px(2.0);
    let (padding, before, after) = (t.px(10.0), t.px(5.0), t.px(1.0));
    let (box_size, box_left, box_margin) = (t.px(25.0), t.px(4.0), t.px(3.0));
    let check_column = before + box_left + box_size + box_margin + after;
    let row_left = left + spacing;
    let row_width = width - 2 * spacing;
    let text_column = row_width - check_column - spacing;
    let font = style("italic 14px");
    let (_, descent, strut) = dom::line_metrics(t.backend.as_mut(), &font, t.scale);
    let above = (box_size + box_margin).max(strut - descent);
    let check_line = above + box_margin.max(descent);
    BETA_CHECKS
        .iter()
        .map(|text| {
            *y += spacing;
            let top = *y;
            let text_left = row_left + check_column + spacing;
            let available = text_column - before - after;
            let mut block = t.block(font, &[text], [text_left + before, 0], Some(available));
            let height = 2 * padding + block.flow.height.max(check_line);
            let centre = |content: Lu| top + padding + (height - 2 * padding - content) / 2;
            block.origin[1] = centre(block.flow.height);
            *y += height;
            BetaRow {
                row: [row_left, top, row_width, height],
                cells: [
                    [row_left, top, check_column, height],
                    [text_left, top, text_column, height],
                ],
                checkbox: [
                    row_left + before + box_left,
                    centre(check_line) + above - box_size,
                    box_size,
                    box_size,
                ],
                text: block,
            }
        })
        .collect()
}
