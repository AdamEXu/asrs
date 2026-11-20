use std::sync::Arc;

use gpui_kit::{
    AnyWindowHandle, App, AssetSource, Entity, HeadlessAppContext, Pixels, Render, Result, Size,
    Window, WindowHandle, platform,
};

#[derive(Clone, PartialEq, Eq)]
pub struct Capture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl std::fmt::Debug for Capture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Capture({}x{})", self.width, self.height)
    }
}

impl Capture {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.rgba[i],
            self.rgba[i + 1],
            self.rgba[i + 2],
            self.rgba[i + 3],
        ]
    }
}

pub struct HeadlessCapture {
    cx: HeadlessAppContext,
}

impl HeadlessCapture {
    /// Must run on the main thread: on macOS the text system comes from a `MacPlatform`, which
    /// panics anywhere else, so tests using this need `harness = false`.
    pub fn new(assets: Arc<dyn AssetSource>) -> Result<Self> {
        if platform::current_headless_renderer()?.is_none() {
            anyhow::bail!("this platform has no headless renderer");
        }
        let text_system = platform::current_platform(true).text_system();
        let mut cx = HeadlessAppContext::with_platform(text_system, assets, || {
            platform::current_headless_renderer()
        });
        cx.update(crate::init);
        cx.run_until_parked();
        Ok(Self { cx })
    }

    pub fn context(&mut self) -> &mut HeadlessAppContext {
        &mut self.cx
    }

    pub fn open_window<V: Render + 'static>(
        &mut self,
        size: Size<Pixels>,
        scale_factor: f32,
        build_root: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
    ) -> Result<WindowHandle<V>> {
        let handle = self.cx.open_window(size, build_root)?;
        self.set_scale_factor(handle.into(), scale_factor)?;
        Ok(handle)
    }

    pub fn set_scale_factor(&mut self, window: AnyWindowHandle, scale_factor: f32) -> Result<()> {
        if !(scale_factor.is_finite() && scale_factor > 0.0) {
            anyhow::bail!("invalid scale factor {scale_factor}");
        }
        self.cx.simulate_scale_factor_change(window, scale_factor);
        let actual = self
            .cx
            .update_window(window, |_, window, _| window.scale_factor())?;
        if actual != scale_factor {
            anyhow::bail!("window scale factor is {actual}, expected {scale_factor}");
        }
        Ok(())
    }

    pub fn capture(&mut self, window: AnyWindowHandle) -> Result<Capture> {
        self.cx
            .update_window(window, |_, window, _| window.refresh())?;
        self.cx.run_until_parked();
        let image = self.cx.capture_screenshot(window)?;
        Ok(Capture {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        })
    }
}
