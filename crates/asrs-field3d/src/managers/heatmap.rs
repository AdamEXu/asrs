use std::sync::Arc;

use asrs_js::math;
use asrs_js::string::JsString;
use asrs_log::geometry::{self, Alliance, ConvertPose, annotated_pose_3d_to_2d};
use asrs_model::assets::{Config3dField, CoordinateSystem};
use asrs_model::preferences::CoordinateSystemPreference;
use asrs_tabs::heatmap::Heatmap;

use super::{HeatmapObj, ManagerContext, ManagerError, ObjectManager};
use crate::render::{Geometry, Mesh, PhongMaterial, Scene, Texture};
use crate::three::primitives::plane_geometry;
use crate::three::{ObjectId, Vector3};

/// What `HeatmapManager` reads from the selected field's config.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeatmapField {
    pub width_inches: f64,
    pub height_inches: f64,
    pub coordinate_system: CoordinateSystem,
}

impl From<&Config3dField> for HeatmapField {
    fn from(config: &Config3dField) -> Self {
        HeatmapField {
            width_inches: config.width_inches,
            height_inches: config.height_inches,
            coordinate_system: config.coordinate_system,
        }
    }
}

type FieldSource = Box<dyn Fn() -> Option<HeatmapField>>;
type PreferenceSource = Box<dyn Fn() -> Option<CoordinateSystemPreference>>;

/// `objectManagers/HeatmapManager.ts`: the shared `Heatmap` drawn onto a field-sized plane.
pub struct HeatmapManager {
    root: ObjectId,
    height_pixels: f64,
    get_field_config: FieldSource,
    get_coordinate_system: PreferenceSource,
    heatmap: Heatmap,
    heatmap_canvas: u64,
    canvas: Option<u64>,
    mesh: Option<ObjectId>,
    is_red_alliance: bool,
}

impl HeatmapManager {
    /// `get_field_config` is the renderer's field config cache; `get_coordinate_system` reads
    /// `window.preferences?.coordinateSystem`, `None` while preferences are unset.
    pub fn new(
        ctx: ManagerContext,
        get_field_config: FieldSource,
        get_coordinate_system: PreferenceSource,
    ) -> Self {
        HeatmapManager {
            root: ctx.root,
            height_pixels: if ctx.is_xr { 25.0 } else { 800.0 },
            get_field_config,
            get_coordinate_system,
            heatmap: Heatmap::new(),
            heatmap_canvas: 0,
            canvas: None,
            mesh: None,
            is_red_alliance: false,
        }
    }

    pub fn set_is_red_alliance(&mut self, is_red_alliance: bool) {
        self.is_red_alliance = is_red_alliance;
    }

    pub fn heatmap(&self) -> &Heatmap {
        &self.heatmap
    }

    pub fn mesh(&self) -> Option<ObjectId> {
        self.mesh
    }

    /// Which canvas the plane's texture shows: it changes each time the heatmap recreates its
    /// h337 instance, as the `<canvas>` element does upstream.
    pub fn canvas(&self) -> Option<u64> {
        self.canvas
    }

    /// The canvas as a texture. WebGL samples an empty canvas's texture as opaque black.
    fn texture(&mut self) -> Texture {
        match self.heatmap.image_data() {
            Ok(Some(image)) => Texture::new(image.width, image.height, image.data.into()),
            _ => Texture::new(1, 1, vec![0, 0, 0, 255].into()),
        }
    }
}

impl ObjectManager for HeatmapManager {
    type Object = HeatmapObj;

    fn dispose(&mut self, scene: &mut Scene) {
        if let Some(mesh) = self.mesh {
            scene.graph.remove(self.root, mesh);
        }
    }

    fn set_object_data(
        &mut self,
        scene: &mut Scene,
        object: &HeatmapObj,
    ) -> Result<(), ManagerError> {
        let Some(field_config) = (self.get_field_config)() else {
            return Ok(());
        };

        let inches = |v: f64| {
            asrs_log::units::convert(v, "inches", "meters").expect("inches and meters are units")
        };
        let field_dimensions = [
            inches(field_config.width_inches),
            inches(field_config.height_inches),
        ];
        let pixel_dimensions = [
            math::round(self.height_pixels * (field_dimensions[0] / field_dimensions[1])),
            self.height_pixels,
        ];
        let coordinate_system = match (self.get_coordinate_system)() {
            Some(CoordinateSystemPreference::Automatic) => {
                geometry::CoordinateSystem::from(field_config.coordinate_system)
            }
            Some(preference) => {
                geometry::CoordinateSystem::parse(&JsString::from(preference.as_str()))
            }
            None => geometry::CoordinateSystem::CenterRed,
        };
        let alliance = if self.is_red_alliance {
            Alliance::Red
        } else {
            Alliance::Blue
        };
        let translations: Vec<[f64; 2]> = object
            .poses
            .iter()
            .map(|x| {
                annotated_pose_3d_to_2d(x.clone())
                    .convert_from_coordinate_system(
                        &coordinate_system,
                        alliance,
                        field_dimensions[0],
                        field_dimensions[1],
                    )
                    .pose
                    .translation
            })
            .collect();
        let outcome = self
            .heatmap
            .update(&translations, pixel_dimensions, field_dimensions);
        if outcome.created {
            self.heatmap_canvas += 1;
        }
        if let Some(error) = outcome.error {
            return Err(ManagerError(format!("{error:?}")));
        }

        if self.canvas != Some(self.heatmap_canvas) {
            self.canvas = Some(self.heatmap_canvas);
            if let Some(mesh) = self.mesh {
                scene.graph.remove(self.root, mesh);
            }
            let material = PhongMaterial {
                map: Some(Arc::new(self.texture())),
                transparent: true,
                ..PhongMaterial::new()
            };
            let plane = plane_geometry(field_dimensions[0], field_dimensions[1], 1.0, 1.0);
            let mesh = scene.add_mesh(
                self.root,
                Mesh::new(Geometry::new(plane), Arc::new(material)),
            );
            scene.graph.object_mut(mesh).position = Vector3::new(0.0, 0.0, 0.02);
            self.mesh = Some(mesh);
        } else if outcome.set_data.is_some()
            && let Some(mesh) = self.mesh
        {
            let texture = Arc::new(self.texture());
            if let Some(mesh) = scene.mesh_mut(mesh)
                && let Some(material) = mesh.material.single()
            {
                let material = Arc::new(PhongMaterial {
                    map: Some(texture),
                    ..(**material).clone()
                });
                mesh.material = material.into();
            }
        }
        Ok(())
    }
}
