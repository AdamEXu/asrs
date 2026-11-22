use std::sync::Arc;

use asrs_canvas::{Canvas2d, Raster};
use asrs_js::math;
use asrs_js::string::JsString;

use super::{
    ConeObj, InstanceSource, ManagerContext, ManagerError, ObjectManager, ResizableInstancedMesh,
    poses,
};
use crate::render::{Geometry, Materials, PhongMaterial, Scene, Texture};
use crate::three::BufferGeometry;
use crate::three::primitives::cone_geometry;

/// A 2D canvas wrapped in a `CanvasTexture`. `canvas` keeps every call made since the canvas was
/// created; `update` (`needsUpdate = true`) rasterizes the new ones and makes the texture the GPU
/// re-uploads.
pub struct CanvasTexture {
    canvas: Canvas2d,
    raster: Raster<'static>,
    applied: usize,
    texture: Arc<Texture>,
}

impl CanvasTexture {
    fn new(width: u32, height: u32) -> Self {
        let raster = Raster::new(width, height).expect("canvas size is valid");
        let texture = Arc::new(Texture::new(width, height, read_pixels(&raster).into()));
        CanvasTexture {
            canvas: Canvas2d::new(width, height),
            raster,
            applied: 0,
            texture,
        }
    }

    pub fn canvas(&self) -> &Canvas2d {
        &self.canvas
    }

    pub fn texture(&self) -> &Arc<Texture> {
        &self.texture
    }

    fn update(&mut self) {
        for call in &self.canvas.calls[self.applied..] {
            self.raster.apply(call);
        }
        self.applied = self.canvas.calls.len();
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

fn offset(position: &JsString) -> f64 {
    match position.as_string().as_deref() {
        Some("center") => 0.0,
        Some("front") => -0.125,
        Some("back") => 0.125,
        _ => f64::NAN,
    }
}

/// `objectManagers/ConeManager.ts`.
pub struct ConeManager {
    instances: ResizableInstancedMesh,
    geometry: BufferGeometry,
    main_material: Arc<PhongMaterial>,
    second_material: Arc<PhongMaterial>,
    base_material: Arc<PhongMaterial>,
    main: CanvasTexture,
    base: CanvasTexture,
    last_position: JsString,
    last_color: JsString,
}

impl ConeManager {
    pub fn new(ctx: ManagerContext) -> Self {
        let main = CanvasTexture::new(100, 100);
        let base = CanvasTexture::new(100, 100);

        let mut geometry = cone_geometry(0.06, 0.25, 16.0, 32.0, false, 0.0, math::PI * 2.0);
        geometry.rotate_z(-math::PI / 2.0);
        geometry.rotate_x(-math::PI / 2.0);

        let phong = |map: Option<Arc<Texture>>| {
            Arc::new(PhongMaterial {
                map,
                specular: ctx.material_specular,
                shininess: ctx.material_shininess,
                ..PhongMaterial::new()
            })
        };
        let main_material = phong(Some(main.texture.clone()));
        let second_material = phong(None);
        let base_material = phong(Some(base.texture.clone()));

        let instances = ResizableInstancedMesh::new(
            ctx.root,
            vec![InstanceSource {
                geometry: Geometry::new(geometry.clone()),
                material: Materials::Array(vec![
                    main_material.clone(),
                    second_material.clone(),
                    base_material.clone(),
                ]),
            }],
            None,
        );
        ConeManager {
            instances,
            geometry,
            main_material,
            second_material,
            base_material,
            main,
            base,
            last_position: JsString::from("center"),
            last_color: JsString::new(),
        }
    }

    pub fn instances(&self) -> &ResizableInstancedMesh {
        &self.instances
    }

    /// The canvas behind the side material's texture.
    pub fn main_canvas(&self) -> &CanvasTexture {
        &self.main
    }

    /// The canvas behind the base material's texture.
    pub fn base_canvas(&self) -> &CanvasTexture {
        &self.base
    }

    fn update_textures(&mut self, scene: &mut Scene) {
        self.main.update();
        self.base.update();
        let with_map = |material: &Arc<PhongMaterial>, texture: &Arc<Texture>| {
            Arc::new(PhongMaterial {
                map: Some(texture.clone()),
                ..(**material).clone()
            })
        };
        self.main_material = with_map(&self.main_material, &self.main.texture);
        self.base_material = with_map(&self.base_material, &self.base.texture);
        let materials = Materials::Array(vec![
            self.main_material.clone(),
            self.second_material.clone(),
            self.base_material.clone(),
        ]);
        self.instances.set_material(scene, 0, materials);
    }
}

impl ObjectManager for ConeManager {
    type Object = ConeObj;

    fn dispose(&mut self, scene: &mut Scene) {
        self.instances.dispose(scene);
    }

    fn set_object_data(&mut self, scene: &mut Scene, object: &ConeObj) -> Result<(), ManagerError> {
        if object.position != self.last_position {
            let delta = offset(&object.position) - offset(&self.last_position);
            self.geometry.translate(delta, 0.0, 0.0);
            self.instances
                .set_geometry(scene, 0, Geometry::new(self.geometry.clone()));
            self.last_position = object.position.clone();
        }

        if object.color != self.last_color {
            let color = object.color.to_string_lossy();
            self.main.canvas.set_fill_style(color.as_str());
            self.base.canvas.set_fill_style(color.as_str());
            self.main.canvas.fill_rect(0.0, 0.0, 100.0, 100.0);
            self.base.canvas.fill_rect(0.0, 0.0, 100.0, 100.0);
            self.main.canvas.set_fill_style("black");
            self.main.canvas.fill_rect(20.0, 0.0, 10.0, 100.0);
            self.update_textures(scene);
            self.last_color = object.color.clone();
        }

        self.instances.set_poses(scene, &poses(&object.poses));
        Ok(())
    }
}
