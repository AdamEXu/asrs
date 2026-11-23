// Ported from Chart.js 4.4.0 (MIT, Copyright (c) 2014-2022 Chart.js Contributors). See THIRD_PARTY_NOTICES.md.

//! The tooltip plugin with its default options and the histogram's title callback.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_canvas::Property;
use asrs_canvas::text::TextBackend;
use asrs_js::math::max2;
use asrs_js::string::JsString;

use super::animation::{self, AnimationRef, Config, Easing, Target, Value};
use super::chart::{ActiveElement, Area, BOLD_FONT, DEFAULT_BORDER, FONT, FONT_SIZE, LINE_HEIGHT};
use super::surface::Surface;
use super::ticks::limit;

const PADDING: f64 = 6.0;
const CARET_PADDING: f64 = 2.0;
const CARET_SIZE: f64 = 5.0;
const CORNER_RADIUS: f64 = 6.0;
const TITLE_SPACING: f64 = 2.0;
const TITLE_MARGIN_BOTTOM: f64 = 6.0;
const BODY_SPACING: f64 = 2.0;
const BOX_SIZE: f64 = FONT_SIZE;
const BOX_PADDING: f64 = 0.0;

const MOVE: Config = Config {
    duration: 400.0,
    easing: Easing::EaseOutQuart,
};
const FADE: Config = Config {
    duration: 200.0,
    easing: Easing::Linear,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prop {
    Opacity,
    X,
    Y,
    Width,
    Height,
    CaretX,
    CaretY,
}

const PROPS: usize = 7;

/// The animated properties; `None` is `undefined`.
pub struct Props([Option<f64>; PROPS]);

impl Props {
    pub fn get(&self, prop: Prop) -> Option<f64> {
        self.0[prop as usize]
    }

    pub fn set(&mut self, prop: Prop, value: f64) {
        self.0[prop as usize] = Some(value);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum XAlign {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum YAlign {
    Top,
    Center,
    Bottom,
}

/// One `bodyItem` with its colour box.
pub struct Item {
    pub title: JsString,
    pub label: JsString,
    pub background: String,
}

pub struct Tooltip {
    props: Rc<RefCell<Props>>,
    animations: [Option<AnimationRef>; PROPS],
    pub active: Vec<ActiveElement>,
    title: JsString,
    body: Vec<(JsString, String)>,
    x_align: XAlign,
    y_align: YAlign,
}

pub struct Bounds<'a> {
    pub width: f64,
    pub height: f64,
    pub area: &'a Area,
}

struct Placement {
    position: (f64, f64),
    size: (f64, f64),
    point: (f64, f64),
    x_align: XAlign,
    y_align: YAlign,
}

/// The `average` positioner, or `None` for its `false`.
pub fn average(items: &[ActiveElement]) -> Option<(f64, f64)> {
    if items.is_empty() {
        return None;
    }
    let (mut x, mut y, mut count) = (0.0, 0.0, 0.0);
    for item in items {
        let bar = item.element.borrow();
        if bar.x.is_finite() && bar.y.is_finite() {
            x += bar.x;
            y += bar.y;
            count += 1.0;
        }
    }
    Some((x / count, y / count))
}

impl Tooltip {
    pub fn new() -> Tooltip {
        let mut props = Props([None; PROPS]);
        props.set(Prop::Opacity, 0.0);
        Tooltip {
            props: Rc::new(RefCell::new(props)),
            animations: [const { None }; PROPS],
            active: Vec::new(),
            title: JsString::new(),
            body: Vec::new(),
            x_align: XAlign::Center,
            y_align: YAlign::Center,
        }
    }

    fn get(&self, prop: Prop) -> f64 {
        self.props.borrow().get(prop).unwrap_or(f64::NAN)
    }

    pub fn position_changed(&self, active: &[ActiveElement]) -> bool {
        average(active).is_some_and(|(x, y)| {
            let props = self.props.borrow();
            props.get(Prop::CaretX) != Some(x) || props.get(Prop::CaretY) != Some(y)
        })
    }

    /// `_resolveAnimations().update(this, values)`.
    fn animate(&mut self, values: &[(Prop, f64)], now: f64) -> Vec<AnimationRef> {
        let mut created = Vec::new();
        for &(prop, value) in values.iter().rev() {
            let config = if prop == Prop::Opacity { FADE } else { MOVE };
            let props = &self.props;
            animation::animate(
                &mut self.animations[prop as usize],
                config,
                || Target::Tooltip(props.clone(), prop),
                Value::Number(value),
                now,
                &mut created,
            );
        }
        created
    }

    /// `update` with no active element.
    pub fn hide(&mut self, now: f64) -> Vec<AnimationRef> {
        if self.get(Prop::Opacity) == 0.0 {
            return Vec::new();
        }
        self.animate(&[(Prop::Opacity, 0.0)], now)
    }

    /// `update` with the items of the active elements.
    pub fn show(
        &mut self,
        position: (f64, f64),
        items: Vec<Item>,
        bounds: &Bounds<'_>,
        ctx: &mut Surface,
        text: &mut dyn TextBackend,
        now: f64,
    ) -> Vec<AnimationRef> {
        self.title = items[0].title.clone();
        self.body = items.into_iter().map(|i| (i.label, i.background)).collect();
        let placed = self.place(position, bounds, ctx, text);
        self.x_align = placed.x_align;
        self.y_align = placed.y_align;
        self.animate(
            &[
                (Prop::Opacity, 1.0),
                (Prop::X, placed.point.0),
                (Prop::Y, placed.point.1),
                (Prop::Width, placed.size.0),
                (Prop::Height, placed.size.1),
                (Prop::CaretX, placed.position.0),
                (Prop::CaretY, placed.position.1),
            ],
            now,
        )
    }

    /// `getTooltipSize`.
    fn size(&self, ctx: &mut Surface, text: &mut dyn TextBackend) -> (f64, f64) {
        let lines = self.body.len() as f64;
        let mut height = PADDING + PADDING;
        let mut width = 0.0;
        height += LINE_HEIGHT + TITLE_MARGIN_BOTTOM;
        height += lines * max2(BOX_SIZE, LINE_HEIGHT) + (lines - 1.0) * BODY_SPACING;
        ctx.save();
        ctx.font(BOLD_FONT);
        width = max2(width, ctx.measure(text, &self.title));
        ctx.font(FONT);
        let box_width = BOX_SIZE + 2.0 + BOX_PADDING;
        for (line, _) in &self.body {
            width = max2(width, ctx.measure(text, line) + box_width);
        }
        ctx.font(BOLD_FONT);
        ctx.restore();
        width += PADDING + PADDING;
        (width, height)
    }

    /// `getTooltipSize`, `determineAlignment` and `getBackgroundPoint`.
    fn place(
        &self,
        (x, y): (f64, f64),
        bounds: &Bounds<'_>,
        ctx: &mut Surface,
        text: &mut dyn TextBackend,
    ) -> Placement {
        let (width, height) = self.size(ctx, text);
        let caret = CARET_SIZE + CARET_PADDING;
        let y_align = if y < height / 2.0 {
            YAlign::Top
        } else if y > bounds.height - height / 2.0 {
            YAlign::Bottom
        } else {
            YAlign::Center
        };
        let mut x_align = XAlign::Center;
        if y_align == YAlign::Center {
            x_align = if x <= (bounds.area.left + bounds.area.right) / 2.0 {
                XAlign::Left
            } else {
                XAlign::Right
            };
        } else if x <= width / 2.0 {
            x_align = XAlign::Left;
        } else if x >= bounds.width - width / 2.0 {
            x_align = XAlign::Right;
        }
        if (x_align == XAlign::Left && x + width + caret > bounds.width)
            || (x_align == XAlign::Right && x - width - caret < 0.0)
        {
            x_align = XAlign::Center;
        }
        let mut px = match x_align {
            XAlign::Left => x,
            XAlign::Center => x - width / 2.0,
            XAlign::Right => x - width,
        };
        let py = match y_align {
            YAlign::Top => y + caret,
            YAlign::Bottom => y - (height + caret),
            YAlign::Center => y - height / 2.0,
        };
        if y_align == YAlign::Center {
            match x_align {
                XAlign::Left => px += caret,
                XAlign::Right => px -= caret,
                XAlign::Center => {}
            }
        } else {
            match x_align {
                XAlign::Left => px -= CORNER_RADIUS + CARET_SIZE,
                XAlign::Right => px += CORNER_RADIUS + CARET_SIZE,
                XAlign::Center => {}
            }
        }
        Placement {
            position: (x, y),
            size: (width, height),
            point: (
                limit(px, 0.0, bounds.width - width),
                limit(py, 0.0, bounds.height - height),
            ),
            x_align,
            y_align,
        }
    }

    fn animation_to(&self, prop: Prop) -> Option<f64> {
        match self.animations[prop as usize].as_ref()?.borrow().to() {
            Value::Number(v) => Some(*v),
            Value::Color(_) => None,
        }
    }

    /// `draw`, with the animations `_updateAnimationTarget` starts.
    pub fn draw(
        &mut self,
        bounds: &Bounds<'_>,
        ctx: &mut Surface,
        text: &mut dyn TextBackend,
        now: f64,
    ) -> Vec<AnimationRef> {
        let opacity = self.get(Prop::Opacity);
        if opacity == 0.0 {
            return Vec::new();
        }
        let mut created = Vec::new();
        if self.animations[Prop::X as usize].is_some()
            && let Some(position) = average(&self.active)
        {
            let placed = self.place(position, bounds, ctx, text);
            if self.animation_to(Prop::X) != Some(placed.point.0)
                || self.animation_to(Prop::Y) != Some(placed.point.1)
            {
                self.x_align = placed.x_align;
                self.y_align = placed.y_align;
                let mut props = self.props.borrow_mut();
                props.set(Prop::Width, placed.size.0);
                props.set(Prop::Height, placed.size.1);
                props.set(Prop::CaretX, placed.position.0);
                props.set(Prop::CaretY, placed.position.1);
                drop(props);
                created =
                    self.animate(&[(Prop::X, placed.point.0), (Prop::Y, placed.point.1)], now);
            }
        }
        let opacity = if opacity < 1e-3 && opacity > -1e-3 {
            0.0
        } else {
            opacity
        };
        ctx.save();
        ctx.set(Property::GlobalAlpha(opacity));
        self.draw_background(ctx);
        let mut y = self.get(Prop::Y) + PADDING;
        let x = self.get(Prop::X) + PADDING;
        ctx.text_align("left");
        ctx.text_baseline("middle");
        ctx.fill_style("#fff");
        ctx.font(BOLD_FONT);
        ctx.fill_text(self.title.clone(), x, y + LINE_HEIGHT / 2.0);
        y += LINE_HEIGHT + TITLE_SPACING;
        y += TITLE_MARGIN_BOTTOM - TITLE_SPACING;
        self.draw_body(x, y, ctx);
        ctx.restore();
        created
    }

    fn draw_body(&self, x: f64, mut y: f64, ctx: &mut Surface) {
        ctx.text_align("left");
        ctx.text_baseline("middle");
        ctx.font(FONT);
        ctx.fill_style("#fff");
        for (line, background) in &self.body {
            ctx.fill_style("#fff");
            let offset = if BOX_SIZE < LINE_HEIGHT {
                (LINE_HEIGHT - BOX_SIZE) / 2.0
            } else {
                0.0
            };
            let box_y = y + offset;
            ctx.line_width(1.0);
            ctx.stroke_style(DEFAULT_BORDER);
            ctx.line_dash();
            ctx.set(Property::LineDashOffset(Some(0.0)));
            ctx.fill_style("#fff");
            ctx.fill_rect(x, box_y, BOX_SIZE, BOX_SIZE);
            ctx.stroke_rect(x, box_y, BOX_SIZE, BOX_SIZE);
            ctx.fill_style(background);
            ctx.fill_rect(x + 1.0, box_y + 1.0, BOX_SIZE - 2.0, BOX_SIZE - 2.0);
            ctx.fill_style("#fff");
            let line_height = max2(LINE_HEIGHT, BOX_SIZE);
            ctx.fill_text(
                line.clone(),
                x + (BOX_SIZE + 2.0 + BOX_PADDING),
                y + line_height / 2.0,
            );
            y += line_height + BODY_SPACING;
        }
    }

    fn draw_caret(&self, x: f64, y: f64, width: f64, height: f64, ctx: &mut Surface) {
        let (x1, x2, x3, y1, y2, y3);
        if self.y_align == YAlign::Center {
            y2 = y + height / 2.0;
            if self.x_align == XAlign::Left {
                x1 = x;
                x2 = x1 - CARET_SIZE;
                y1 = y2 + CARET_SIZE;
                y3 = y2 - CARET_SIZE;
            } else {
                x1 = x + width;
                x2 = x1 + CARET_SIZE;
                y1 = y2 - CARET_SIZE;
                y3 = y2 + CARET_SIZE;
            }
            x3 = x1;
        } else {
            x2 = match self.x_align {
                XAlign::Left => x + CORNER_RADIUS + CARET_SIZE,
                XAlign::Right => x + width - CORNER_RADIUS - CARET_SIZE,
                XAlign::Center => self.get(Prop::CaretX),
            };
            if self.y_align == YAlign::Top {
                y1 = y;
                y2 = y1 - CARET_SIZE;
                x1 = x2 - CARET_SIZE;
                x3 = x2 + CARET_SIZE;
            } else {
                y1 = y + height;
                y2 = y1 + CARET_SIZE;
                x1 = x2 + CARET_SIZE;
                x3 = x2 - CARET_SIZE;
            }
            y3 = y1;
        }
        ctx.line_to(x1, y1);
        ctx.line_to(x2, y2);
        ctx.line_to(x3, y3);
    }

    fn draw_background(&self, ctx: &mut Surface) {
        let (x, y) = (self.get(Prop::X), self.get(Prop::Y));
        let (width, height) = (self.get(Prop::Width), self.get(Prop::Height));
        let r = CORNER_RADIUS;
        ctx.fill_style("rgba(0,0,0,0.8)");
        ctx.stroke_style("rgba(0,0,0,0)");
        ctx.line_width(0.0);
        ctx.begin_path();
        ctx.move_to(x + r, y);
        if self.y_align == YAlign::Top {
            self.draw_caret(x, y, width, height, ctx);
        }
        ctx.line_to(x + width - r, y);
        ctx.quadratic_curve_to(x + width, y, x + width, y + r);
        if self.y_align == YAlign::Center && self.x_align == XAlign::Right {
            self.draw_caret(x, y, width, height, ctx);
        }
        ctx.line_to(x + width, y + height - r);
        ctx.quadratic_curve_to(x + width, y + height, x + width - r, y + height);
        if self.y_align == YAlign::Bottom {
            self.draw_caret(x, y, width, height, ctx);
        }
        ctx.line_to(x + r, y + height);
        ctx.quadratic_curve_to(x, y + height, x, y + height - r);
        if self.y_align == YAlign::Center && self.x_align == XAlign::Left {
            self.draw_caret(x, y, width, height, ctx);
        }
        ctx.line_to(x, y + r);
        ctx.quadratic_curve_to(x, y, x + r, y);
        ctx.close_path();
        ctx.fill();
    }
}
