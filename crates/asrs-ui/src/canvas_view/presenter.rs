use std::sync::{Arc, OnceLock};

use asrs_canvas::Pixmap;
use asrs_canvas::pixels::unpremultiply;
use gpui_kit::{AnyWindowHandle, App, Bounds, Corners, RenderImage, Window, point, px, size};
use image::{Frame, RgbaImage};

use super::geometry::device_snap;

fn unpremultiply_table() -> &'static [[u8; 256]; 256] {
    static TABLE: OnceLock<Box<[[u8; 256]; 256]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = Box::new([[0u8; 256]; 256]);
        for (a, row) in table.iter_mut().enumerate() {
            for (v, out) in row.iter_mut().enumerate() {
                *out = unpremultiply([v as u8, 0, 0, a as u8])[0];
            }
        }
        table
    })
}

fn convert_in_place(pixels: &mut [u8], table: &[[u8; 256]; 256]) {
    for p in pixels.as_chunks_mut::<4>().0 {
        *p = match p[3] {
            255 => [p[2], p[1], p[0], 255],
            0 => [0; 4],
            a => {
                let row = &table[a as usize];
                [
                    row[p[2] as usize],
                    row[p[1] as usize],
                    row[p[0] as usize],
                    a,
                ]
            }
        };
    }
}

/// Turns a premultiplied pixmap into straight-alpha BGRA in place, byte-equal to
/// `asrs_canvas::pixels::to_straight_bgra` but table-driven and split across threads.
pub fn into_straight_bgra(pixmap: Pixmap) -> Vec<u8> {
    const BAND_BYTES: usize = 1 << 20;
    let mut data = pixmap.take();
    let table = unpremultiply_table();
    if data.len() <= BAND_BYTES {
        convert_in_place(&mut data, table);
        return data;
    }
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get().min(8));
    let band = data.len().div_ceil(threads).next_multiple_of(4);
    std::thread::scope(|scope| {
        for chunk in data.chunks_mut(band) {
            scope.spawn(move || convert_in_place(chunk, table));
        }
    });
    data
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

    pub fn present(&mut self, pixmap: Pixmap, window: &mut Window, cx: &mut App) {
        let (width, height) = (pixmap.width(), pixmap.height());
        self.present_bgra(width, height, into_straight_bgra(pixmap), window, cx);
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
        self.paint_rounded(origin, 0.0, window);
    }

    /// [`FramePresenter::paint`] clipped to corners of `radius` logical px.
    pub fn paint_rounded(&self, origin: [f32; 2], radius: f32, window: &mut Window) {
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
        let _ = window.paint_image(bounds, bounds, Corners::all(px(radius)), image, 0, false);
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
        pixels.resize(pixels.len() * 40, 0);
        let count = pixels.len() / 4;
        let mut pixmap = Pixmap::new(count as u32, 1).expect("pixmap");
        pixmap.data_mut().copy_from_slice(&pixels);
        let want = to_straight_bgra(&pixmap);
        assert_eq!(into_straight_bgra(pixmap), want);
    }
}
