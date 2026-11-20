use std::sync::Arc;

use asrs_canvas::Pixmap;
use asrs_canvas::pixels::unpremultiply;
use gpui_kit::{AnyWindowHandle, App, Bounds, Corners, RenderImage, Window, point, px, size};
use image::{Frame, RgbaImage};

use super::geometry::device_snap;

/// Straight-alpha BGRA of a premultiplied pixmap, equal to `asrs_canvas::pixels::to_straight_bgra`
/// with the opaque and clear pixels that dominate canvas frames taken without the divide.
pub fn straight_bgra(pixmap: &Pixmap) -> Vec<u8> {
    let data = pixmap.data();
    let mut out = vec![0u8; data.len()];
    for (dst, src) in out
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(data.as_chunks::<4>().0)
    {
        *dst = match src[3] {
            255 => [src[2], src[1], src[0], 255],
            0 => [0; 4],
            _ => {
                let [r, g, b, a] = unpremultiply(*src);
                [b, g, r, a]
            }
        };
    }
    out
}

/// Holds the one `RenderImage` a canvas shows. Every replacement removes the previous image from
/// the window's sprite atlas: GPUI never evicts atlas entries on its own, so a canvas that
/// redraws every frame would otherwise grow without bound.
#[derive(Default)]
pub struct FramePresenter {
    image: Option<Arc<RenderImage>>,
    window: Option<AnyWindowHandle>,
}

impl FramePresenter {
    pub fn image(&self) -> Option<&Arc<RenderImage>> {
        self.image.as_ref()
    }

    pub fn device_size(&self) -> Option<(u32, u32)> {
        self.image.as_ref().map(|image| {
            let s = image.size(0);
            (s.width.0 as u32, s.height.0 as u32)
        })
    }

    pub fn present(&mut self, pixmap: &Pixmap, window: &mut Window, cx: &mut App) {
        self.present_bgra(pixmap.width(), pixmap.height(), straight_bgra(pixmap), window, cx);
    }

    /// Shows `bgra`, straight alpha, `width` x `height` device pixels.
    pub fn present_bgra(
        &mut self,
        width: u32,
        height: u32,
        bgra: Vec<u8>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let image = RgbaImage::from_raw(width, height, bgra)
            .map(|buffer| Arc::new(RenderImage::new(vec![Frame::new(buffer)])));
        self.replace(image, window, cx);
    }

    pub fn clear(&mut self, window: &mut Window, cx: &mut App) {
        self.replace(None, window, cx);
    }

    fn replace(&mut self, image: Option<Arc<RenderImage>>, window: &mut Window, cx: &mut App) {
        let handle = window.window_handle();
        if self.window.is_some_and(|w| w != handle) {
            self.release(cx);
        } else if let Some(previous) = self.image.take() {
            let _ = window.drop_image(previous);
        }
        self.image = image;
        self.window = Some(handle);
    }

    /// Removes the image from its window's atlas outside of any window update, for when the
    /// canvas itself goes away.
    pub fn release(&mut self, cx: &mut App) {
        if let (Some(image), Some(handle)) = (self.image.take(), self.window.take()) {
            let _ = handle.update(cx, |_, window, _| window.drop_image(image));
        }
    }

    /// Paints the image one device pixel per bitmap pixel with its top-left corner where Chromium
    /// snaps the canvas box's origin.
    pub fn paint(&self, origin: [f32; 2], window: &mut Window) {
        let Some(image) = self.image.clone() else {
            return;
        };
        let dpr = window.scale_factor();
        let device = image.size(0);
        let to_px = |v: i32| px(v as f32 / dpr);
        let bounds = Bounds {
            origin: point(
                to_px(device_snap(origin[0], dpr)),
                to_px(device_snap(origin[1], dpr)),
            ),
            size: size(to_px(device.width.0), to_px(device.height.0)),
        };
        let _ = window.paint_image(bounds, bounds, Corners::default(), image, 0, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asrs_canvas::pixels::to_straight_bgra;

    #[test]
    fn fast_conversion_equals_skia_unpremul() {
        let mut pixels = Vec::new();
        for a in 0..=255u8 {
            for c in 0..=a {
                pixels.extend_from_slice(&[c, a - c, c / 2, a]);
            }
        }
        let count = pixels.len() / 4;
        let mut pixmap = Pixmap::new(count as u32, 1).expect("pixmap");
        pixmap.data_mut().copy_from_slice(&pixels);
        assert_eq!(straight_bgra(&pixmap), to_straight_bgra(&pixmap));
    }
}
