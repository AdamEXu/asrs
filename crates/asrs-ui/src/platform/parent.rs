use gpui_kit::Window;
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};

/// A window's native handle, for building a child view in it while `App` is not borrowed. Only
/// good until the window closes, so it is used right after it is taken.
pub struct Parent(RawWindowHandle);

impl Parent {
    pub fn of(window: &Window) -> Result<Parent, HandleError> {
        Ok(Parent(HasWindowHandle::window_handle(window)?.as_raw()))
    }
}

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: the window was open when the handle was taken, on the thread that closes
        // windows. Only WebView2 runs other tasks before it is done, and a stale HWND is an error.
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}
