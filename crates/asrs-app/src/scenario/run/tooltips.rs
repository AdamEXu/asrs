//! The tooltips at a capture, in the format `oracle/src/app/tooltips.ts` records upstream's.

use asrs_js::ops::object;
use asrs_js::value::JsValue;
use asrs_ui::gpui::{MouseExitEvent, MouseMoveEvent, PlatformInput, point, px};
use asrs_ui::tooltip;

use super::{Variant, capture};
use crate::scenario::Page;

const GRID_STEP: f64 = 24.0;
const EDGE: f64 = 3.0;
const SETTLE_FRAMES: usize = 20;

fn strings(texts: &[String]) -> JsValue {
    JsValue::Array(texts.iter().map(|t| JsValue::string(t)).collect())
}

fn numbers(values: &[f64]) -> JsValue {
    JsValue::Array(values.iter().map(|v| JsValue::Number(*v)).collect())
}

impl Variant<'_> {
    fn hover(&mut self, x: f64, y: f64) -> Result<Vec<String>, String> {
        self.dispatch(PlatformInput::MouseMove(MouseMoveEvent {
            position: point(px(x as f32), px(y as f32)),
            pressed_button: self.pressed,
            modifiers: Default::default(),
        }))?;
        self.draw()?;
        self.draw()?;
        self.given()
    }

    fn draw(&mut self) -> Result<(), String> {
        self.tick()?;
        let window = self.active();
        (self.headless.capture(window))
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn given(&mut self) -> Result<Vec<String>, String> {
        let window = self.active();
        self.headless
            .context()
            .update_window(window, |_, window, cx| tooltip::take_given(window, cx))
            .map_err(|e| e.to_string())
    }

    pub(super) fn capture_tooltips(&mut self, name: &str) -> Result<(), String> {
        if self.pressed.is_some() {
            return Ok(());
        }
        if self.scenario.page == Page::App {
            self.wait_idle()?;
        }
        self.hover(0.0, 0.0)?;
        for _ in 0..SETTLE_FRAMES {
            self.tick()?;
        }
        self.draw()?;
        self.given()?;
        let window = self.active();
        let (titles, texts, grid, bare) = self
            .headless
            .context()
            .update_window(window, |_, window, cx| {
                let titles = tooltip::titles(window, cx);
                let size = window.viewport_size();
                let mut texts = Vec::new();
                let mut grid = Vec::new();
                let mut bare = None;
                let mut at = |x: f64, y: f64| {
                    tooltip::title_at(point(px(x as f32), px(y as f32)), window, cx)
                };
                let mut y = GRID_STEP / 2.0;
                while y < f64::from(f32::from(size.height)) {
                    let mut row = String::new();
                    let mut x = GRID_STEP / 2.0;
                    while x < f64::from(f32::from(size.width)) {
                        let title = at(x, y);
                        let offsets = [-EDGE, 0.0, EDGE];
                        let same = offsets
                            .iter()
                            .flat_map(|dx| offsets.map(|dy| (x + dx, y + dy)))
                            .all(|(x, y)| at(x, y) == title);
                        row.push(match title {
                            _ if !same => '?',
                            None => {
                                bare.get_or_insert((x, y));
                                '.'
                            }
                            Some(title) => {
                                let i = texts.iter().position(|t| *t == title);
                                let i = i.unwrap_or_else(|| {
                                    texts.push(title);
                                    texts.len() - 1
                                });
                                char::from_u32('A' as u32 + i as u32).expect("a letter")
                            }
                        });
                        x += GRID_STEP;
                    }
                    grid.push(row);
                    y += GRID_STEP;
                }
                (titles, texts, grid, bare)
            })
            .map_err(|e| e.to_string())?;
        let mut records = Vec::new();
        let mut first = None;
        for (title, bounds) in &titles {
            let [x, y, w, h] = asrs_ui::parity::rect_of(*bounds);
            let middle = [(x + w / 2.0).round(), (y + h / 2.0).round()];
            first.get_or_insert(middle);
            if let Some((x, y)) = bare {
                self.hover(x, y)?;
            }
            let shown = self.hover(middle[0], middle[1])?;
            let corner = self.hover(x.ceil() + 2.0, y.ceil() + 2.0)?;
            records.push(object([
                ("title", JsValue::string(title)),
                ("rect", numbers(&[x, y, w, h])),
                ("point", numbers(&middle)),
                ("shown", strings(&shown)),
                ("corner", strings(&corner)),
            ]));
        }
        let mut left = None;
        if let Some([x, y]) = first {
            self.hover(x, y)?;
            self.dispatch(PlatformInput::MouseExited(MouseExitEvent::default()))?;
            left = Some(strings(&self.given()?));
        }
        let (x, y) = self.pointer;
        self.hover(x, y)?;
        let mut fields = vec![("titles", JsValue::Array(records))];
        fields.extend(left.map(|left| ("left", left)));
        fields.push(("gridStep", JsValue::Number(GRID_STEP)));
        fields.push(("texts", strings(&texts)));
        fields.push(("grid", strings(&grid)));
        capture::write(
            &self
                .dir
                .join("captures")
                .join(format!("{name}.tooltips.json")),
            &capture::json(&object(fields))?,
        )
    }
}
