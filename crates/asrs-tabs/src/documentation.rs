//! `src/shared/renderers/DocumentationRenderer.ts`: the docs page in an iframe. The view owns
//! the frame; this keeps what upstream reads back from it and the navigation it queues until
//! the frame has loaded.

use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_model::tab_controller::{TabCommand, TabRenderer};

pub const DOCS_URL: &str = "../docs/build/index.html";

#[derive(Clone, Debug, PartialEq)]
pub struct DocumentationRenderer {
    state_queue: Option<JsString>,
    loaded: bool,
    first_render: bool,
    /// `IFRAME.src`, once the first render has set it.
    src: Option<&'static str>,
    /// `IFRAME.contentWindow.location.hash`.
    hash: JsString,
}

impl Default for DocumentationRenderer {
    fn default() -> Self {
        DocumentationRenderer {
            state_queue: None,
            loaded: false,
            first_render: true,
            src: None,
            hash: JsString::new(),
        }
    }
}

impl DocumentationRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn src(&self) -> Option<&'static str> {
        self.src
    }

    pub fn hash(&self) -> &JsString {
        &self.hash
    }

    /// The iframe's `load` event.
    pub fn frame_loaded(&mut self) {
        self.loaded = true;
    }

    /// The frame navigated on its own (a link inside the docs).
    pub fn set_hash(&mut self, hash: JsString) {
        self.hash = hash;
    }
}

impl<Cx: ?Sized> TabRenderer<Cx> for DocumentationRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::String(self.hash.clone())
    }

    fn restore_state(&mut self, _cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        if let JsValue::String(state) = state {
            if self.loaded {
                self.hash = state.clone();
            } else {
                self.state_queue = Some(state.clone());
            }
        }
        Ok(())
    }

    fn aspect_ratio(&self, _cx: &Cx) -> Option<f64> {
        None
    }

    fn render(&mut self, _cx: &mut Cx, _command: &dyn TabCommand) -> Result<(), JsError> {
        if self.first_render {
            self.first_render = false;
            self.src = Some(DOCS_URL);
        }
        if self.loaded
            && let Some(state) = self.state_queue.take()
        {
            self.hash = state;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::*;

    #[test]
    fn a_restore_waits_for_the_frame_to_load() {
        let mut r = DocumentationRenderer::new();
        let cx = &mut ();
        let state = JsValue::string("#/tab-reference/line-graph");
        TabRenderer::<()>::restore_state(&mut r, cx, &state).unwrap();
        assert_eq!(TabRenderer::<()>::save_state(&r, cx), JsValue::string(""));
        let command = Rc::new(JsValue::Null);
        TabRenderer::<()>::render(&mut r, cx, &*command).unwrap();
        assert_eq!(r.src(), Some(DOCS_URL));
        assert_eq!(TabRenderer::<()>::save_state(&r, cx), JsValue::string(""));
        r.frame_loaded();
        TabRenderer::<()>::render(&mut r, cx, &*command).unwrap();
        assert_eq!(TabRenderer::<()>::save_state(&r, cx), state);
        TabRenderer::<()>::restore_state(&mut r, cx, &JsValue::Number(1.0)).unwrap();
        assert_eq!(TabRenderer::<()>::save_state(&r, cx), state);
    }
}
