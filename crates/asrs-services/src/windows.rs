//! The window/screen surface `StateTracker` needs: window bounds by numeric id plus display
//! rectangles. Values are DIP `Bounds`/`Rectangle` numbers as Electron reports them.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub trait WindowSystem {
    /// `BrowserWindow.fromId(id)?.getBounds()`, `None` when the window is gone.
    fn window_bounds(&self, id: f64) -> Option<Rect>;

    /// `screen.getAllDisplays().map((d) => d.bounds)`. Never empty: with no screens attached
    /// Chromium still reports one display (`ScreenMac` falls back to the primary display).
    fn displays(&self) -> Vec<Rect>;

    /// `screen.getPrimaryDisplay().bounds`, which Electron always returns.
    fn primary_display(&self) -> Rect;
}
