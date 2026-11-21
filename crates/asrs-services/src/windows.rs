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

    /// `screen.getAllDisplays().map((d) => d.bounds)`.
    fn displays(&self) -> Vec<Rect>;

    /// `screen.getPrimaryDisplay().bounds`; `None` when there are no displays.
    fn primary_display(&self) -> Option<Rect>;
}
