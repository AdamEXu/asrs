//! `:focus` on the fields gate 5 masks, whose window captures cannot show it: the `focus/fields`
//! vector's backgrounds of `div.field-value input`, and the search field's cancel button, which the
//! `focus-search` scenario's upstream captures have in the active window and not in the inactive
//! one.

#![allow(clippy::disallowed_methods)]

use std::sync::Arc;

use asrs_parity::json::Json;
use asrs_parity::vectors;
use asrs_ui::capture::HeadlessCapture;
use asrs_ui::input::{FieldKind, FieldStyle, PlainText, TextField};
use asrs_ui::selection::colors::ForcedActive;
use asrs_ui::theme::Appearance;
use gpui_kit::{
    AppContext, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    ParentElement, Render, Styled, Window, div, px, rgb, size,
};

const HEIGHT: f32 = 29.0;

struct Page {
    body: FocusHandle,
    value: Entity<TextField<PlainText>>,
    search: Entity<TextField<PlainText>>,
}

impl Render for Page {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        let place = |left: f32| div().absolute().top_0().left(px(left));
        div()
            .size_full()
            .track_focus(&self.body)
            .bg(rgb(0xffffff))
            .child(place(0.0).child(self.value.clone()))
            .child(place(60.0).child(self.search.clone()))
    }
}

fn main() {
    if asrs_parity::ci::skip_in_ci("focus fields", "a GPU") {
        return;
    }
    if !cfg!(any(target_os = "macos", target_os = "linux")) {
        println!("focus fields: skipped, no headless renderer on this platform");
        return;
    }
    let mut capture = HeadlessCapture::new(Arc::new(())).expect("headless context");
    capture.context().update(asrs_ui::init);
    let field =
        |kind, style, width: f32, value, window: &mut Window, cx: &mut Context<'_, Page>| {
            cx.new(|cx| {
                let mut field = TextField::new(PlainText::new(value), kind, style, window, cx);
                field.set_size(width, HEIGHT);
                field
            })
        };
    let window = capture
        .open_window(size(px(300.0), px(40.0)), 1.0, |window, cx| {
            cx.new(|cx| {
                let value = FieldStyle::field_value();
                let search = FieldStyle::sidebar_search(Appearance::Light);
                Page {
                    body: cx.focus_handle(),
                    value: field(FieldKind::Number, value, 50.0, "", window, cx),
                    search: field(FieldKind::Search, search, 200.0, "zq", window, cx),
                }
            })
        })
        .expect("a window");
    let mut shot = |active: bool, focused: Option<&str>| {
        capture
            .context()
            .update(|cx| cx.set_global(ForcedActive(active)));
        window
            .update(capture.context(), |page, window, cx| {
                let handle = match focused {
                    Some("v") => page.value.focus_handle(cx),
                    Some("q") => page.search.focus_handle(cx),
                    _ => page.body.clone(),
                };
                window.focus(&handle, cx);
                window.refresh();
            })
            .expect("open");
        capture.capture(window.into()).expect("capture")
    };

    let id = match cfg!(target_os = "macos") {
        true => "focus/fields/macos-aarch64",
        false => "focus/fields/linux-x86_64",
    };
    let records = vectors::load(id).unwrap_or_else(|e| panic!("{id}: {e}"));
    let mut compared = 0;
    let mut sequence = None;
    let mut active = false;
    for record in &records {
        let Some(op) = record.field("op").and_then(Json::as_array) else {
            continue;
        };
        let number = record.field("sequence").and_then(Json::as_f64);
        if number != sequence {
            sequence = number;
            active = false;
        }
        if op[0]
            .as_str()
            .is_some_and(|s| s.to_string_lossy() == "active")
        {
            active = op[1] == Json::Bool(true);
        }
        let focused = record.field("active").and_then(Json::as_str);
        let focused = focused.map(|s| s.to_string_lossy());
        let styles = record.field("styles").and_then(Json::as_array);
        let want = styles.expect("styles")[0].as_str().expect("a colour");
        let [r, g, b, _] = shot(active, focused.as_deref()).pixel(25, 4);
        let got = format!("rgb({r}, {g}, {b})");
        assert_eq!(
            got,
            want.to_string_lossy(),
            "active {active}, focus {focused:?}"
        );
        compared += 1;
    }
    assert!(compared > 30, "{compared} records");

    let cancel = |shot: asrs_ui::capture::Capture| {
        (240..258).any(|x| (4..25).any(|y| shot.pixel(x, y)[0] < 128))
    };
    assert!(cancel(shot(true, Some("q"))), "active");
    assert!(!cancel(shot(false, Some("q"))), "inactive");
    assert!(!cancel(shot(true, None)), "not focused");
    println!("focus fields: {compared} backgrounds and the cancel button");
}
