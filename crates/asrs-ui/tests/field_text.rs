//! Focused inputs against the `focus/text` vector: Chromium's pixels of the value, its selection
//! and the placeholder in an active and an inactive window, and where presses put the caret.

#![allow(clippy::disallowed_methods)]

use std::sync::Arc;

use asrs_parity::json::Json;
use asrs_parity::pixel::{self, Rgb};
use asrs_parity::vectors::{self, Record};
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::input::{FieldKind, FieldStyle, PlainText, TextField};
use asrs_ui::parity::{FocusedInput, Recorder};
use asrs_ui::selection::colors::ForcedActive;
use asrs_ui::text::snap_remainder;
use asrs_ui::theme::Appearance;
use gpui_kit::{
    App, AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement, Keystroke,
    MouseButton, MouseDownEvent, MouseUpEvent, ParentElement, PlatformInput, Render, Styled,
    Window, div, point, px, rgb, size,
};

/// The native frame's corners and a glyph a selection ends in differ by this much.
const TOLERANCE: u64 = 3;

struct Page {
    field: Entity<TextField<PlainText>>,
    origin: [f32; 2],
    background: u32,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let [x, y] = self.origin;
        div().id("page").size_full().bg(rgb(self.background)).child(
            div()
                .absolute()
                .left(px(x))
                .top(px(y))
                .child(self.field.clone()),
        )
    }
}

fn text(record: &Record, key: &str) -> String {
    let value = record.field(key).and_then(Json::as_str);
    value.expect(key).to_string_lossy()
}

fn numbers(value: &Json) -> Vec<f64> {
    let list = value.as_array().expect("a list");
    list.iter().map(|n| n.as_f64().expect("a number")).collect()
}

fn pixels(record: &Record) -> Rgb {
    let rows = record.field("rows").and_then(Json::as_array).expect("rows");
    let mut data = Vec::new();
    for row in rows {
        for run in row.as_array().expect("runs") {
            let [count, color] = run.as_array().expect("a run") else {
                panic!("{run:?}")
            };
            let color = color.as_str().expect("a colour").to_string_lossy();
            let color = u32::from_str_radix(&color, 16).expect("hex");
            let color = [color >> 16, color >> 8, color].map(|c| c as u8);
            data.extend(std::iter::repeat_n(
                color,
                count.as_f64().expect("a count") as usize,
            ));
        }
    }
    Rgb {
        width: (data.len() / rows.len()) as u32,
        height: rows.len() as u32,
        data,
    }
}

fn style(control: &str, appearance: Appearance) -> FieldStyle {
    match control {
        "setting_block" => FieldStyle::setting_block(),
        "field_value" => FieldStyle::field_value(),
        "native" | "table_jump" => FieldStyle::native(),
        "sidebar_search" => FieldStyle::sidebar_search(appearance),
        "console_filter" => FieldStyle::console_filter(),
        other => panic!("no style for {other}"),
    }
}

fn placeholder(control: &str, kind: FieldKind) -> &'static str {
    match control {
        "field_value" => "12.345",
        "native" if kind == FieldKind::Text => "/U/logs",
        "sidebar_search" => "Search",
        "console_filter" => "Filter",
        "table_jump" => "0.000",
        _ => "",
    }
}

fn type_key(key: &'static str, times: usize, window: &mut Window, cx: &mut App) {
    for _ in 0..times {
        window.dispatch_keystroke(Keystroke::parse(key).expect("a key"), cx);
    }
}

fn main() {
    if asrs_parity::ci::skip_in_ci("field text", "a GPU") {
        return;
    }
    if !cfg!(target_os = "macos") {
        println!("field text: skipped, recorded on macOS only");
        return;
    }
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    capture.context().update(|cx| {
        asrs_ui::init(cx);
        cx.set_global(Recorder::default());
    });
    let records =
        vectors::load("focus/text/macos-aarch64").unwrap_or_else(|e| panic!("focus/text: {e}"));
    let (mut compared, mut presses, mut failures) = (0, 0, Vec::new());
    let mut worst = pixel::Stats::default();
    for record in &records {
        let control = text(record, "control");
        let kind = match text(record, "type").as_str() {
            "number" => FieldKind::Number,
            "search" => FieldKind::Search,
            _ => FieldKind::Text,
        };
        let dark = text(record, "theme") == "dark";
        let appearance = match dark {
            true => Appearance::Dark,
            false => Appearance::Light,
        };
        let scale = record.field("scale").and_then(Json::as_f64).expect("scale") as f32;
        let value = text(record, "value");
        let press = record.field("press").map(numbers);
        let selection = record.field("selection").unwrap_or(&Json::Null);
        let active = record.field("active") == Some(&Json::Bool(true));
        let rect = numbers(record.field("rect").expect("rect"));
        let [x, y, w, h] = [0, 1, 2, 3].map(|i| rect[i] as f32);
        let background = if dark { 0x202020 } else { 0xffffff };

        let force = |capture: &mut HeadlessCapture, active| {
            let cx = capture.context();
            cx.update(|cx| cx.set_global(ForcedActive(active)));
        };
        force(&mut capture, true);
        let window = capture
            .open_window(size(px(500.0), px(100.0)), scale, |window, cx| {
                let field = cx.new(|cx| {
                    let style = style(&control, appearance);
                    let model = PlainText::new(&value);
                    let mut field = TextField::new(model, kind, style, window, cx);
                    field.set_size(w, h);
                    field.set_subpixel(snap_remainder(x, scale), snap_remainder(y, scale));
                    field.set_placeholder(placeholder(&control, kind));
                    field
                });
                cx.new(|_| Page {
                    field,
                    origin: [x, y].map(|v| v - snap_remainder(v, scale)),
                    background,
                })
            })
            .expect("a window");
        let close = |capture: &mut HeadlessCapture| {
            let closed = window.update(capture.context(), |_, window, _| window.remove_window());
            closed.expect("open");
            capture.context().update(|cx| {
                if cx.has_global::<FocusedInput>() {
                    cx.remove_global::<FocusedInput>();
                }
            });
        };
        if let Some(press) = press {
            let position = point(px(x + press[0] as f32), px(y + press[1] as f32));
            capture.capture(window.into()).expect("capture");
            let cx = capture.context();
            cx.update_window(window.into(), |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseDown(MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        click_count: 1,
                        ..MouseDownEvent::default()
                    }),
                    cx,
                );
                window.dispatch_event(
                    PlatformInput::MouseUp(MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        click_count: 1,
                        ..MouseUpEvent::default()
                    }),
                    cx,
                );
            })
            .expect("open");
            capture.capture(window.into()).expect("capture");
            let got = capture
                .context()
                .update(|cx| cx.try_global::<FocusedInput>().cloned())
                .and_then(|input| input.selection.map(|s| s.map(|n| n as f64).to_vec()));
            let focused = record.field("focused") == Some(&Json::Bool(true));
            let want = focused.then(|| numbers(record.field("caret").expect("a caret")));
            if got != want {
                failures.push(format!(
                    "{control} {scale}x pressed at {press:?}: {got:?}, not {want:?}"
                ));
            }
            presses += 1;
            close(&mut capture);
            continue;
        }
        window
            .update(capture.context(), |page, window, cx| {
                page.field.update(cx, |field, cx| {
                    match selection.as_str().map(|s| s.to_string_lossy()).as_deref() {
                        Some("all") => field.select(window, cx),
                        _ => window.focus(&field.focus_handle(cx), cx),
                    }
                });
            })
            .expect("open");
        capture.capture(window.into()).expect("capture");
        capture
            .context()
            .update_window(window.into(), |_, window, cx| {
                if let Some(range) = selection.as_array() {
                    let [start, end] = [0, 1].map(|i| range[i].as_f64().expect("an end") as usize);
                    type_key("home", 1, window, cx);
                    type_key("right", start, window, cx);
                    type_key("shift-right", end - start, window, cx);
                } else if selection
                    .as_str()
                    .is_some_and(|s| s.to_string_lossy() == "typed")
                {
                    type_key("end", 1, window, cx);
                }
            })
            .expect("open");
        capture.capture(window.into()).expect("capture");
        force(&mut capture, active);
        let shot = capture.capture(window.into()).expect("capture");

        let want = pixels(record);
        let [left, top] = [x, y].map(|v| (v * scale).round() as u32);
        let data = (0..want.height)
            .flat_map(|row| (0..want.width).map(move |column| (left + column, top + row)))
            .map(|(x, y)| {
                let [r, g, b, _] = shot.pixel(x, y);
                [r, g, b]
            })
            .collect();
        let got = Rgb {
            data,
            ..want.clone()
        };
        let stats = pixel::compare(&want, &got, [255; 3]).expect("sizes").stats;
        let name = format!(
            "{control} {kind:?} {appearance:?} {scale}x {value:?} {selection:?} active {active}"
        );
        if let Ok(dir) = std::env::var("ASRS_FIELD_TEXT_OUT") {
            for (side, image) in [("want", &want), ("got", &got)] {
                let data = image.data.iter().flat_map(|&[r, g, b]| [r, g, b, 255]);
                let image = pixel::Rgba {
                    width: image.width,
                    height: image.height,
                    data: data.collect(),
                };
                let path = format!("{dir}/{compared:03}-{side}.png");
                pixel::write_png(path.as_ref(), &image).expect("a png");
            }
            println!("{compared:03} {name}: {stats:?}");
        }
        if stats.mismatch > TOLERANCE || stats.max_blob > 1 {
            failures.push(format!("{name}: {stats:?}"));
        }
        worst.max_channel = worst.max_channel.max(stats.max_channel);
        worst.exact_mismatch = worst.exact_mismatch.max(stats.exact_mismatch);
        compared += 1;
        close(&mut capture);
    }
    assert!(compared >= 120, "{compared} records");
    assert!(presses >= 40, "{presses} presses");
    assert!(
        failures.is_empty(),
        "{} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
    println!(
        "field text: {presses} presses, {compared} fields, at most {} px and {} levels off",
        worst.exact_mismatch, worst.max_channel
    );
}
