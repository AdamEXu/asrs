//! `src/shared/renderers/VideoRenderer.ts` and the `<img>` it drives, as Blink's `ImageLoader`
//! loads it: a new `src` keeps the old image's natural size until the next microtask, and what is
//! painted stays the last image that finished until the new one does.

use std::cell::RefCell;
use std::rc::Rc;

use asrs_js::string::{JsError, JsString};
use asrs_js::value::JsValue;
use asrs_model::tab_controller::{TabCommand, TabRenderer};

/// What the element paints.
#[derive(Clone, Debug, PartialEq)]
pub enum Shown {
    Image(JsString),
    /// The image failed to load.
    Broken,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Loading,
    Loaded([f64; 2]),
    Failed,
}

#[derive(Clone, Debug)]
struct Content {
    src: JsString,
    state: State,
}

impl Content {
    fn natural_size(&self) -> [f64; 2] {
        match self.state {
            State::Loaded(size) => size,
            State::Loading | State::Failed => [0.0; 2],
        }
    }
}

/// The `<img>`: its attributes, and the loads its page performs through
/// [`VideoImage::begin_load`] and [`VideoImage::loaded`].
#[derive(Debug, Default)]
pub struct VideoImage {
    hidden: bool,
    src: Option<JsString>,
    content: Option<Content>,
    shown: Option<Shown>,
    /// The `src` whose load starts in the next microtask.
    pending: Option<JsString>,
}

impl VideoImage {
    pub fn hidden(&self) -> bool {
        self.hidden
    }

    /// `getAttribute("src")`.
    pub fn src(&self) -> Option<&JsString> {
        self.src.as_ref()
    }

    /// `[naturalWidth, naturalHeight]`.
    pub fn natural_size(&self) -> [f64; 2] {
        self.content
            .as_ref()
            .map_or([0.0; 2], Content::natural_size)
    }

    pub fn shown(&self) -> Option<&Shown> {
        self.shown.as_ref().filter(|_| !self.hidden)
    }

    /// The microtask that starts the load of a new `src`, which it returns.
    pub fn begin_load(&mut self) -> Option<JsString> {
        let src = self.pending.take()?;
        self.content = Some(Content {
            src: src.clone(),
            state: State::Loading,
        });
        Some(src)
    }

    /// The load of `src` ended with the image's natural size, or failed. Ignored once the element
    /// has moved on to another image.
    pub fn loaded(&mut self, src: &JsString, size: Option<[f64; 2]>) {
        let Some(content) = &mut self.content else {
            return;
        };
        if content.src != *src || content.state != State::Loading {
            return;
        }
        let (state, shown) = match size {
            Some(size) => (State::Loaded(size), Shown::Image(src.clone())),
            None => (State::Failed, Shown::Broken),
        };
        content.state = state;
        self.shown = Some(shown);
    }

    /// `img.src = src`. An image that is loading or loaded is found in the memory cache at once;
    /// a failed one is fetched again.
    fn set_src(&mut self, src: &JsString) {
        self.src = Some(src.clone());
        self.pending = None;
        if src.is_empty() {
            self.content = None;
            self.shown = None;
            return;
        }
        let cached = self
            .content
            .as_ref()
            .is_some_and(|c| c.src == *src && c.state != State::Failed);
        if !cached {
            self.pending = Some(src.clone());
        }
    }
}

pub struct VideoRenderer {
    image: Rc<RefCell<VideoImage>>,
    aspect_ratio: Option<f64>,
}

impl VideoRenderer {
    pub fn new(image: Rc<RefCell<VideoImage>>) -> VideoRenderer {
        VideoRenderer {
            image,
            aspect_ratio: None,
        }
    }

    pub fn image(&self) -> &Rc<RefCell<VideoImage>> {
        &self.image
    }

    pub fn aspect_ratio(&self) -> Option<f64> {
        self.aspect_ratio
    }

    pub fn render_command(&mut self, command: &JsValue) {
        let JsValue::String(command) = command else {
            return;
        };
        let mut image = self.image.borrow_mut();
        image.hidden = command.is_empty();
        image.set_src(command);
        let [width, height] = image.natural_size();
        self.aspect_ratio = (width > 0.0 && height > 0.0).then(|| width / height);
    }
}

impl<Cx: ?Sized> TabRenderer<Cx> for VideoRenderer {
    fn save_state(&self, _: &Cx) -> JsValue {
        JsValue::Null
    }

    fn restore_state(&mut self, _: &mut Cx, _: &JsValue) -> Result<(), JsError> {
        Ok(())
    }

    fn aspect_ratio(&self, _: &Cx) -> Option<f64> {
        self.aspect_ratio
    }

    fn render(&mut self, _: &mut Cx, command: &dyn TabCommand) -> Result<(), JsError> {
        self.render_command(&command.to_js());
        Ok(())
    }
}
