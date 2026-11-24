//! The renderer of headless windows, which Windows has none of: a shown window that draws their
//! scenes and reads them back with GPUI's `render_to_image`. The scene's device size is the
//! window's client size, whatever the display's scale is.

use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{
    Bounds, DevicePixels, Empty, Platform, PlatformAtlas, PlatformHeadlessRenderer, PlatformWindow,
    Result, Scene, Size, WindowBackgroundAppearance, WindowHandle, WindowId, WindowKind,
    WindowParams, point, px, size,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
    SetWindowPos, TranslateMessage,
};

use super::window::{client_rect, frame};

thread_local! {
    static PLATFORM: Rc<dyn Platform> = gpui_kit::platform::current_platform(false);
}

pub fn platform() -> Rc<dyn Platform> {
    PLATFORM.with(Rc::clone)
}

pub struct ShownWindow {
    window: Box<dyn PlatformWindow>,
    hwnd: HWND,
}

/// Nothing runs the platform's loop, and the system waits for a window's answers.
fn pump() {
    let mut message = MSG::default();
    // SAFETY: the messages are this thread's, and `message` outlives each call.
    unsafe {
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

impl ShownWindow {
    pub fn open(transparent: bool) -> Result<Self> {
        let handle = WindowHandle::<Empty>::new(WindowId::from(1));
        let window = platform().open_window(
            handle.into(),
            WindowParams {
                bounds: Bounds::new(point(px(0.0), px(0.0)), size(px(256.0), px(256.0))),
                titlebar: None,
                kind: WindowKind::PopUp,
                is_movable: false,
                app_owns_titlebar_drag: false,
                is_resizable: false,
                is_minimizable: false,
                focus: false,
                show: true,
                icon: None,
                display_id: None,
                app_id: None,
                window_min_size: None,
            },
        )?;
        if transparent {
            window.set_background_appearance(WindowBackgroundAppearance::Transparent);
        }
        let hwnd = match HasWindowHandle::window_handle(&*window).map(|handle| handle.as_raw()) {
            Ok(RawWindowHandle::Win32(handle)) => HWND(handle.hwnd.get() as _),
            other => anyhow::bail!("not a Win32 window: {other:?}"),
        };
        pump();
        Ok(Self { window, hwnd })
    }

    fn resize(&self, size: Size<DevicePixels>) -> Result<()> {
        let (width, height) = (size.width.0, size.height.0);
        let client = || {
            client_rect(self.hwnd)
                .map(|client| (client.right - client.left, client.bottom - client.top))
        };
        if client() != Some((width, height)) {
            let frame = frame(self.hwnd).ok_or_else(|| anyhow::anyhow!("the window is closed"))?;
            let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
            let (w, h) = (width + frame.width, height + frame.height);
            // SAFETY: the window is open.
            unsafe { SetWindowPos(self.hwnd, None, 0, 0, w, h, flags)? };
        }
        pump();
        let actual = client();
        if actual != Some((width, height)) {
            anyhow::bail!("the window's content is {actual:?}, not {width}x{height}");
        }
        Ok(())
    }
}

impl PlatformHeadlessRenderer for ShownWindow {
    fn render_scene_to_image(
        &mut self,
        scene: &Scene,
        size: Size<DevicePixels>,
    ) -> Result<image::RgbaImage> {
        self.resize(size)?;
        self.window.draw(scene);
        self.window.render_to_image(scene)
    }

    fn render_scene(&mut self, scene: &Scene, size: Size<DevicePixels>) -> Result<()> {
        self.resize(size)?;
        self.window.draw(scene);
        Ok(())
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.window.sprite_atlas()
    }
}

impl Drop for ShownWindow {
    fn drop(&mut self) {
        pump();
    }
}
