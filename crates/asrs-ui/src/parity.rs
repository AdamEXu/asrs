//! What a parity scenario reads back from a painted frame, in place of the DOM dump the
//! Electron driver takes: boxes by parity id (`parity/probes.json` maps upstream selectors to
//! them), masks, the text runs every [`Text`](crate::text::Text) painted, and the pixels of
//! every canvas and 3D view. Recording happens only in `parity` builds while a [`Recorder`]
//! global is installed; elsewhere [`Probed`] is a plain wrapper.

use gpui_kit::{
    AnyElement, App, Bounds, Element, ElementId, FocusHandle, Global, GlobalElementId,
    InspectorElementId, IntoElement, LayoutId, Pixels, SharedString, Window,
};

/// `[x, y, width, height]` in CSS px of the window content.
pub type Rect = [f64; 4];

pub fn rect_of(bounds: Bounds<Pixels>) -> Rect {
    [
        bounds.origin.x,
        bounds.origin.y,
        bounds.size.width,
        bounds.size.height,
    ]
    .map(|v| f64::from(f32::from(v)))
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextRun {
    pub text: String,
    /// One box per line, as `Range.getClientRects()` gives them for a text node.
    pub rects: Vec<Rect>,
}

/// A 2D canvas bitmap, straight alpha like `getImageData`.
#[derive(Clone, Debug, PartialEq)]
pub struct CanvasDump {
    pub rect: Rect,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// A 3D view's readback: the frame as rendered, the edge mask gate 4 needs (true within 2 px of a
/// geometry edge) and the render mode (`standard`, `low-power` or `cinematic`).
#[derive(Clone, Debug, PartialEq)]
pub struct GlDump {
    pub rect: Rect,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub edges: Vec<bool>,
    pub mode: &'static str,
}

#[derive(Default, PartialEq)]
pub struct Recorder {
    pub boxes: Vec<(SharedString, Rect)>,
    pub masks: Vec<(String, Rect)>,
    pub texts: Vec<TextRun>,
    pub canvases: Vec<CanvasDump>,
    pub gl: Vec<GlDump>,
}

impl Global for Recorder {}

fn with(cx: &mut App, f: impl FnOnce(&mut Recorder)) {
    if cfg!(feature = "parity") && cx.has_global::<Recorder>() {
        f(cx.global_mut::<Recorder>());
    }
}

pub fn record_box(cx: &mut App, id: &SharedString, bounds: Bounds<Pixels>) {
    with(cx, |r| r.boxes.push((id.clone(), rect_of(bounds))));
}

/// The reason the Electron driver gives the mask over `document.activeElement` when it is a
/// text input.
pub const FOCUSED_INPUT: &str = "focused text input";

/// A region gate 5 masks on both sides, such as a focused text input and its caret.
pub fn record_mask(cx: &mut App, reason: &str, bounds: Bounds<Pixels>) {
    with(cx, |r| r.masks.push((reason.to_owned(), rect_of(bounds))));
}

/// The focused text input's `value`, and its selection in UTF-16 where its type has one.
#[derive(Clone, Debug, PartialEq)]
pub struct FocusedInput {
    pub handle: FocusHandle,
    pub value: String,
    pub selection: Option<[usize; 2]>,
}

impl Global for FocusedInput {}

pub fn record_focused_input(cx: &mut App, input: impl FnOnce() -> FocusedInput) {
    if cfg!(feature = "parity") {
        cx.set_global(input());
    }
}

pub fn record_text(cx: &mut App, run: TextRun) {
    with(cx, |r| r.texts.push(run));
}

pub fn record_canvas(cx: &mut App, dump: impl FnOnce() -> CanvasDump) {
    with(cx, |r| r.canvases.push(dump()));
}

pub fn record_gl(cx: &mut App, dump: impl FnOnce() -> GlDump) {
    with(cx, |r| r.gl.push(dump()));
}

/// An element whose painted box a parity capture reports under `id`.
pub struct Probed {
    id: SharedString,
    child: AnyElement,
}

pub fn probed(id: impl Into<SharedString>, child: impl IntoElement) -> Probed {
    Probed {
        id: id.into(),
        child: child.into_any_element(),
    }
}

impl IntoElement for Probed {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for Probed {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        record_box(cx, &self.id, bounds);
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}
