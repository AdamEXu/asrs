//! `src/shared/renderers/DocumentationRenderer.ts`: the docs page in an iframe. The view owns
//! the frame; this keeps what upstream reads back from it and the navigations it queues until
//! the frame has loaded. The page's scroll position is left to the frame, which keeps it while
//! hidden where an iframe without a box loses it.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_model::tab_controller::{TabCommand, TabRenderer};

pub const DOCS_URL: &str = "../docs/build/index.html";

/// `location.hash` after `location.hash = value`: the URL Standard's fragment state.
pub fn assigned_hash(value: &JsString) -> JsString {
    let text = value.to_string_lossy();
    let input = text.strip_prefix('#').unwrap_or(&text);
    let mut hash = String::from("#");
    for c in input.chars() {
        match c {
            '\t' | '\n' | '\r' => {}
            ' ' | '"' | '<' | '>' | '`' => hash.push_str(&format!("%{:02X}", c as u32)),
            '\u{21}'..='\u{7e}' => hash.push(c),
            _ => {
                for byte in c.encode_utf8(&mut [0; 4]).bytes() {
                    hash.push_str(&format!("%{byte:02X}"));
                }
            }
        }
    }
    if hash.len() == 1 {
        hash.clear();
    }
    JsString::from(hash)
}

/// `IFRAME` and its `contentWindow.location`.
#[derive(Debug, Default)]
pub struct DocumentationFrame {
    src: Option<&'static str>,
    loaded: bool,
    hash: JsString,
    assigned: u64,
    pending: Option<JsString>,
}

/// A `location.hash` assignment the view passes on to the page. The page reports `id` back with
/// each hash it reports from then on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HashAssignment {
    pub id: u64,
    pub hash: String,
}

impl DocumentationFrame {
    /// `IFRAME.src`, once the first render has set it.
    pub fn src(&self) -> Option<&'static str> {
        self.src
    }

    pub fn loaded(&self) -> bool {
        self.loaded
    }

    pub fn hash(&self) -> &JsString {
        &self.hash
    }

    /// The iframe's `load` event, with the page's hash at that time.
    pub fn load(&mut self, hash: JsString) {
        self.loaded = true;
        self.hash = hash;
        self.assigned = 0;
        self.pending = None;
    }

    /// The page changed its own hash after it saw assignment `id` (0 before any). Reports sent
    /// before the page saw the latest assignment are out of date.
    pub fn navigated(&mut self, id: u64, hash: JsString) {
        if id == self.assigned {
            self.hash = hash;
        }
    }

    pub fn take_assignment(&mut self) -> Option<HashAssignment> {
        let hash = self.pending.take()?;
        Some(HashAssignment {
            id: self.assigned,
            hash: hash.to_string_lossy(),
        })
    }

    fn assign(&mut self, state: &JsString) {
        let hash = assigned_hash(state);
        self.hash = hash.clone();
        self.assigned += 1;
        self.pending = Some(hash);
    }
}

#[derive(Debug)]
pub struct DocumentationRenderer {
    frame: Rc<RefCell<DocumentationFrame>>,
    state_queue: Option<JsString>,
    first_render: bool,
}

impl Default for DocumentationRenderer {
    fn default() -> Self {
        DocumentationRenderer {
            frame: Rc::default(),
            state_queue: None,
            first_render: true,
        }
    }
}

impl DocumentationRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn frame(&self) -> &Rc<RefCell<DocumentationFrame>> {
        &self.frame
    }
}

impl<Cx: ?Sized> TabRenderer<Cx> for DocumentationRenderer {
    fn save_state(&self, _cx: &Cx) -> JsValue {
        JsValue::String(self.frame.borrow().hash.clone())
    }

    fn restore_state(&mut self, _cx: &mut Cx, state: &JsValue) -> Result<(), JsError> {
        if let JsValue::String(state) = state {
            let mut frame = self.frame.borrow_mut();
            if frame.loaded {
                frame.assign(state);
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
        let mut frame = self.frame.borrow_mut();
        if self.first_render {
            self.first_render = false;
            frame.src = Some(DOCS_URL);
        }
        if frame.loaded
            && let Some(state) = self.state_queue.take()
        {
            frame.assign(&state);
        }
        Ok(())
    }
}
