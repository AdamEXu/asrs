use std::sync::Arc;

use asrs_canvas::{Canvas2d, Raster};

use crate::render::Texture;

/// A 2D canvas wrapped in a `CanvasTexture`. `canvas` keeps every call made since the canvas was
/// created; `update` (`needsUpdate = true`) rasterizes the new ones and makes the texture the GPU
/// re-uploads.
pub struct CanvasTexture {
    canvas: Canvas2d,
    raster: Raster<'static>,
    applied: usize,
    version: u32,
    texture: Arc<Texture>,
}

impl CanvasTexture {
    pub(super) fn new(width: u32, height: u32) -> Self {
        let raster = Raster::new(width, height).expect("canvas size is valid");
        let texture = Arc::new(Texture::new(width, height, read_pixels(&raster).into()));
        CanvasTexture {
            canvas: Canvas2d::new(width, height),
            raster,
            applied: 0,
            version: 1,
            texture,
        }
    }

    pub fn canvas(&self) -> &Canvas2d {
        &self.canvas
    }

    pub(super) fn canvas_mut(&mut self) -> &mut Canvas2d {
        &mut self.canvas
    }

    pub fn texture(&self) -> &Arc<Texture> {
        &self.texture
    }

    /// three's `texture.version`: 1 from the `CanvasTexture` constructor, plus one per update.
    pub fn version(&self) -> u32 {
        self.version
    }

    /// How many of `canvas().calls` the texture shows.
    pub fn applied(&self) -> usize {
        self.applied
    }

    pub(super) fn update(&mut self) {
        for call in &self.canvas.calls[self.applied..] {
            self.raster.apply(call);
        }
        self.applied = self.canvas.calls.len();
        self.version += 1;
        self.texture = Arc::new(self.texture.with_pixels(read_pixels(&self.raster).into()));
    }
}

/// The canvas as WebGL uploads it with `UNPACK_PREMULTIPLY_ALPHA_WEBGL` off: straight alpha.
fn read_pixels(raster: &Raster<'_>) -> Vec<u8> {
    let pixmap = raster.pixmap();
    asrs_canvas::pixels::read_rect(pixmap, 0, 0, pixmap.width(), pixmap.height())
        .expect("whole-canvas read")
        .data
}
