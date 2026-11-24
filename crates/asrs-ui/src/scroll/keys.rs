//! The keys that scroll the scroller with the focus, or the one clicked last.

use gpui_kit::Keystroke;

/// `kPixelsPerLineStep`.
pub(super) const LINE: f64 = 40.0;
/// How much of a scroller a page goes by at least, and the most device px it leaves.
const PAGE_FRACTION: f64 = 0.875;
const PAGE_OVERLAP: f64 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    Line([f64; 2]),
    /// Down, or up where negative.
    Page(f64),
    /// To the end, or the start where negative.
    Document(f64),
}

/// A page of a scroller with `client` px, in whole device px.
pub fn page_step(client: f64, scale: f64) -> f64 {
    let device = (client * scale).round();
    let page = (device * PAGE_FRACTION)
        .floor()
        .max(device - PAGE_OVERLAP)
        .max(1.0);
    page / scale
}

impl Step {
    /// How far it goes in a scroller with `client` px and `max` to scroll by.
    pub fn delta(self, client: [f64; 2], max: [f64; 2], scale: f64) -> [f64; 2] {
        match self {
            Step::Line(direction) => direction.map(|d| d * LINE),
            Step::Page(direction) => [0.0, direction * page_step(client[1], scale)],
            Step::Document(direction) => [0.0, direction * (max[1] + client[1])],
        }
    }
}

/// What a key scrolls by, if it is one that scrolls.
pub fn step(keystroke: &Keystroke) -> Option<Step> {
    let modifiers = keystroke.modifiers;
    let mac = cfg!(target_os = "macos");
    // Alt makes a key a system key on Windows.
    let paging = !cfg!(windows);
    let key = keystroke.key.as_str();
    if key == "space" {
        let plain = !modifiers.control && !modifiers.alt && !modifiers.platform;
        let direction = if modifiers.shift { -1.0 } else { 1.0 };
        return plain.then_some(Step::Page(direction));
    }
    if modifiers.shift {
        return None;
    }
    match (modifiers.control, modifiers.alt, modifiers.platform, key) {
        (false, false, false, "up") => Some(Step::Line([0.0, -1.0])),
        (false, false, false, "down") => Some(Step::Line([0.0, 1.0])),
        (false, false, false, "left") => Some(Step::Line([-1.0, 0.0])),
        (false, false, false, "right") => Some(Step::Line([1.0, 0.0])),
        (false, false, false, "pageup") => Some(Step::Page(-1.0)),
        (false, false, false, "pagedown") => Some(Step::Page(1.0)),
        (false, false, false, "home") => Some(Step::Document(-1.0)),
        (false, false, false, "end") => Some(Step::Document(1.0)),
        (true, false, false, "home") if !mac => Some(Step::Document(-1.0)),
        (true, false, false, "end") if !mac => Some(Step::Document(1.0)),
        (false, true, false, "up") if paging => Some(Step::Page(-1.0)),
        (false, true, false, "down") if paging => Some(Step::Page(1.0)),
        (false, false, true, "up") if mac => Some(Step::Document(-1.0)),
        (false, false, true, "down") if mac => Some(Step::Document(1.0)),
        _ => None,
    }
}
