use asrs_js::ops;
use asrs_js::value::JsValue;

pub const CHECKBOXES: usize = 3;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BetaWelcome {
    checked: [bool; CHECKBOXES],
    alpha: Option<bool>,
}

impl BetaWelcome {
    pub fn checked(&self) -> [bool; CHECKBOXES] {
        self.checked
    }

    /// `!CONTINUE_BUTTON.disabled`.
    pub fn enabled(&self) -> bool {
        self.checked.iter().all(|checked| *checked)
    }

    /// Whether `.beta-only` shows, which it also does until main's message.
    pub fn shows_beta(&self) -> bool {
        self.alpha != Some(true)
    }

    /// Whether `.alpha-only` shows.
    pub fn shows_alpha(&self) -> bool {
        self.alpha == Some(true)
    }

    /// `messagePort.onmessage` with main's `isAlpha()`.
    pub fn message(&mut self, data: &JsValue) {
        self.alpha = Some(ops::to_boolean(data));
    }

    /// A click on a checkbox, which fires its `change` event.
    pub fn toggle(&mut self, index: usize) {
        self.checked[index] = !self.checked[index];
    }

    /// A click on the button; whether it posts `null` to main.
    pub fn click_continue(&self) -> bool {
        self.enabled()
    }
}
