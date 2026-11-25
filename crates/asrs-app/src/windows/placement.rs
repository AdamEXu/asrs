//! Where Electron's `BrowserWindow` constructor puts a window, and what of its size that leaves.

use asrs_services::window_manager::WindowOptions;
use asrs_services::windows::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub origin: (f64, f64),
    pub size: (f64, f64),
}

/// `gfx::Rect::AdjustToFit`, one axis of it: where an interval goes to be in another, and the
/// length it keeps.
fn adjust_to_fit((start, length): (f64, f64), (into, space): (f64, f64)) -> (f64, f64) {
    let length = length.min(space);
    let start = if start < into {
        into
    } else {
        (into + space).min(start + length) - length
    };
    (start, length)
}

/// `NativeWindowMac`. `screen` and `work_area` are the primary display's and `title_bar` is the
/// height of a standard title bar.
///
/// The widget opens in the middle of the screen, moved and shrunk into its work area. Making it
/// resizable takes it out of the style mask and puts it back, and for each AppKit works the frame
/// out again from the content: for windows without a frame Electron answers with the frame as its
/// own content rect, so each adds a title bar on top. Windows without a frame, and those of a
/// content size, get the size they asked for last.
pub fn mac(options: &WindowOptions, screen: (f64, f64), work_area: Rect, title_bar: f64) -> Placed {
    let frameless = options.title_bar_style.is_some();
    let middle = |space: f64, length: f64| ((space - length) / 2.0).round();
    let (x, width) = adjust_to_fit(
        (middle(screen.0, options.width), options.width),
        (work_area.x, work_area.width),
    );
    let (y, height) = adjust_to_fit(
        (middle(screen.1, options.height), options.height),
        (work_area.y, work_area.height),
    );
    let grown = if frameless && options.resizable != Some(false) {
        2.0 * title_bar
    } else {
        0.0
    };
    let asked = frameless || options.use_content_size;
    Placed {
        origin: given(options).unwrap_or((x, y - grown)),
        size: if asked {
            (options.width, options.height)
        } else {
            (width, height)
        },
    }
}

/// The options' `x` and `y`, which Electron reads as integers.
pub fn given(options: &WindowOptions) -> Option<(f64, f64)> {
    let (x, y) = options.x.zip(options.y)?;
    Some((x.trunc(), y.trunc()))
}

/// `DesktopWindowTreeHostPlatform::CenterWindow` in the work area of the display that a window
/// opened at the screen's origin is on.
pub fn x11(options: &WindowOptions, work_area: Rect) -> (f64, f64) {
    given(options).unwrap_or_else(|| {
        let middle = |start: f64, space: f64, length: f64| {
            start + ((space - length.min(space)) / 2.0).trunc()
        };
        (
            middle(work_area.x, work_area.width, options.width),
            middle(work_area.y, work_area.height, options.height),
        )
    })
}
