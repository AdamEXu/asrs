#![allow(unsafe_code)]

use std::ffi::c_ulong;

use wry::raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, WindowHandle, XlibWindowHandle,
};

/// The id of an X11 window of another process.
pub struct Parent(pub c_ulong);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = RawWindowHandle::Xlib(XlibWindowHandle::new(self.0));
        // SAFETY: an X11 window is named by a number; one that is gone is a protocol error.
        Ok(unsafe { WindowHandle::borrow_raw(handle) })
    }
}
