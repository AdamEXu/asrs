//! The colours of selected text (`LayoutTheme`).

use gpui_kit::{App, Global, Rgba, Window};

use crate::platform;
use crate::theme::Appearance;

/// Blink's `Color::BlendWithWhite`: a translucent colour that looks like `rgb` on white.
pub fn blend_with_white(rgb: u32) -> u32 {
    let channels = [rgb >> 16 & 0xff, rgb >> 8 & 0xff, rgb & 0xff];
    let mut out = rgb << 8 | 0xff;
    for alpha in (153..=204).step_by(17) {
        let blended = channels.map(|c| {
            let alpha = alpha as f32 / 255.0;
            ((c as i32 - (255 - (alpha * 255.0).round() as i32)) as f32 / alpha) as i32
        });
        let [r, g, b] = blended.map(|c| c.clamp(0, 255) as u32);
        out = r << 24 | g << 16 | b << 8 | alpha;
        if blended.iter().all(|&c| c >= 0) {
            break;
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SelectionColors {
    /// `0xRRGGBBAA`.
    pub background: u32,
    /// What selected text is drawn in, `0xRRGGBB`, on platforms that change it.
    pub text: Option<u32>,
}

impl SelectionColors {
    /// `LayoutTheme`'s selection colours for a frame that is or is not focused and active.
    pub fn of(appearance: Appearance, active: bool) -> SelectionColors {
        let dark = appearance == Appearance::Dark;
        let system = platform::selection_colors(dark);
        let background = match (active, dark && cfg!(target_os = "macos")) {
            (true, true) => system.active << 8 | 204,
            (true, false) => blend_with_white(system.active),
            (false, _) => blend_with_white(system.inactive),
        };
        let text = system.text.map(|[a, i]| if active { a } else { i });
        SelectionColors { background, text }
    }

    pub fn rgba(&self) -> Rgba {
        gpui_kit::rgba(self.background)
    }
}

/// Whether the page is active in parity scenarios, whose windows never are.
#[derive(Clone, Copy, Default)]
pub struct ForcedActive(pub bool);

impl Global for ForcedActive {}

/// `FrameIsFocusedAndActive`.
pub fn page_active(window: &Window, cx: &App) -> bool {
    match cx.try_global::<ForcedActive>() {
        Some(forced) => forced.0,
        None => window.is_window_active(),
    }
}
