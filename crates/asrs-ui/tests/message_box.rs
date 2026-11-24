#![cfg(not(target_os = "macos"))]

//! Message boxes where GPUI's prompts show them.

use std::cell::Cell;
use std::rc::Rc;

use asrs_ui::gpui::TestAppContext;
use asrs_ui::platform::dialog::{MessageBoxOptions, MessageBoxResult, message_box};

fn answer(options: MessageBoxOptions, button: &str) -> MessageBoxResult {
    let mut cx = TestAppContext::single();
    let result = Rc::new(Cell::new(None));
    let done = result.clone();
    let window = cx.add_empty_window();
    window.update(|window, cx| {
        message_box(Some(window), cx, options, move |result, _| {
            done.set(Some(result));
        });
    });
    window.simulate_prompt_answer(button);
    window.run_until_parked();
    result.get().expect("an answer")
}

fn options(checkbox_label: &str, buttons: &[&str], default_id: Option<usize>) -> MessageBoxOptions {
    MessageBoxOptions {
        message: "Message".to_owned(),
        buttons: buttons.iter().map(|b| (*b).to_owned()).collect(),
        checkbox_label: checkbox_label.to_owned(),
        default_id,
        ..MessageBoxOptions::default()
    }
}

#[test]
fn a_checkbox_is_a_last_button() {
    let result = |response, checkbox_checked| MessageBoxResult {
        response,
        checkbox_checked,
    };
    let label = "Don't Show Again";
    let ok: &[&str] = &["OK"];
    let two: &[&str] = &["Open", "Skip"];
    for (buttons, default_id, button, expected) in [
        (ok, None, "OK", result(0, false)),
        (ok, None, label, result(0, true)),
        (two, Some(1), "Open", result(0, false)),
        (two, Some(1), label, result(1, true)),
    ] {
        assert_eq!(
            answer(options(label, buttons, default_id), button),
            expected,
            "{buttons:?} {button}"
        );
    }
    assert_eq!(answer(options("", two, None), "Skip"), result(1, false));
}
