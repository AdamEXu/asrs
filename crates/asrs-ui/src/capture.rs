use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::{
    AnyWindowHandle, App, AssetSource, Entity, HeadlessAppContext, Pixels,
    PlatformHeadlessRenderer, Render, Result, Size, Window, WindowHandle, platform,
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
    transparent: Arc<AtomicBool>,
}

/// GPUI's headless Metal renderer clears every frame to opaque black. A transparent window's
/// real layer clears to alpha 0 instead, which is what `capturePage` gives where the vibrancy
/// backdrop shows, so windows opened as transparent get a renderer with that clear.
#[cfg(target_os = "macos")]
struct TransparentMetal(gpui_apple::metal_renderer::MetalRenderer);

#[cfg(target_os = "macos")]
impl PlatformHeadlessRenderer for TransparentMetal {
    fn render_scene_to_image(
        &mut self,
        scene: &gpui_kit::Scene,
        size: Size<gpui_kit::DevicePixels>,
    ) -> Result<image::RgbaImage> {
        self.0.render_scene_to_image(scene, size)
    }

    fn render_scene(
        &mut self,
        scene: &gpui_kit::Scene,
        size: Size<gpui_kit::DevicePixels>,
    ) -> Result<()> {
        self.0.render_scene(scene, size)
    }

    fn sprite_atlas(&self) -> Arc<dyn gpui_kit::PlatformAtlas> {
        self.0.sprite_atlas().clone()
    }
}

fn renderer(transparent: bool) -> Result<Option<Box<dyn PlatformHeadlessRenderer>>> {
    #[cfg(target_os = "macos")]
    if transparent {
        let pool = Arc::new(parking_lot::Mutex::new(
            gpui_apple::metal_renderer::InstanceBufferPool::default(),
        ));
        let mut metal = gpui_apple::metal_renderer::MetalRenderer::new_headless(pool)?;
        metal.update_transparency(true);
        return Ok(Some(Box::new(TransparentMetal(metal))));
    }
    #[cfg(windows)]
    return Ok(Some(Box::new(
        crate::platform::win::offscreen::ShownWindow::open(transparent)?,
    )));
    #[cfg(not(windows))]
    {
        let _ = transparent;
        platform::current_headless_renderer()
    }
}

fn text_system() -> Result<Arc<dyn gpui_kit::PlatformTextSystem>> {
    #[cfg(windows)]
    return Ok(crate::platform::win::offscreen::platform().text_system());
    #[cfg(not(windows))]
    {
        if platform::current_headless_renderer()?.is_none() {
            anyhow::bail!("this platform has no headless renderer");
        }
        Ok(platform::current_platform(true).text_system())
    }
}

impl HeadlessCapture {
    /// Must run on the main thread: on macOS the text system comes from a `MacPlatform`, which
    /// panics anywhere else, so tests using this need `harness = false`.
    pub fn new(assets: Arc<dyn AssetSource>) -> Result<Self> {
        let text_system = text_system()?;
        let transparent = Arc::new(AtomicBool::new(false));
        let flag = transparent.clone();
        let mut cx = HeadlessAppContext::with_platform(text_system, assets, move || {
            renderer(flag.load(Ordering::Relaxed))
        });
        cx.update(crate::init);
        cx.run_until_parked();
        Ok(Self { cx, transparent })
    }

    /// Whether windows opened from now on are transparent: frames clear to alpha 0 rather than
    /// opaque black (macOS and Windows; on Linux windows are opaque).
    pub fn set_transparent_windows(&mut self, transparent: bool) {
        self.transparent.store(transparent, Ordering::Relaxed);
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
