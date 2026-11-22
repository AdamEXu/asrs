use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use asrs_js::math;
use asrs_js::number;
use asrs_js::string::JsString;

use super::{
    AprilTagObj, ManagerContext, ManagerError, ObjectManager, concat, rotation3d_to_quaternion,
};
use crate::render::{
    Filter, Geometry, Materials, Mesh, MinFilter, NodeKind, PhongMaterial, Scene, Texture,
};
use crate::three::primitives::box_geometry;
use crate::three::{Color, ObjectId, Vector3};

const APRIL_TAG_THICKNESS: f64 = 0.01;
const APRIL_TAG_OFFSET: f64 = -0.005 + 1e-6;

/// A `TextureLoader.load` call that has not completed. Deliver the image with
/// [`AprilTagManager::texture_loaded`]; a URL that fails to load is simply never delivered.
#[derive(Clone, Debug, PartialEq)]
pub struct TextureRequest {
    pub tag: ObjectId,
    pub url: JsString,
}

struct TagEntry {
    id_str: JsString,
    active: bool,
    object: ObjectId,
}

/// `objectManagers/AprilTagManager.ts`.
pub struct AprilTagManager {
    ctx: ManagerContext,
    tags: Vec<TagEntry>,
    geometry: Vec<(JsString, Arc<Geometry>)>,
    white_material: Arc<PhongMaterial>,
    id_textures: Vec<Arc<Texture>>,
    requests: Vec<TextureRequest>,
}

fn zfill(mut s: JsString, length: usize) -> JsString {
    while s.len() < length {
        s = concat(&[&JsString::from("0"), &s]);
    }
    s
}

impl AprilTagManager {
    pub fn new(ctx: ManagerContext) -> Self {
        let white_material = Arc::new(PhongMaterial {
            color: Color::hex(f64::from(0xffffff)),
            specular: ctx.material_specular,
            shininess: ctx.material_shininess,
            ..PhongMaterial::new()
        });
        AprilTagManager {
            ctx,
            tags: Vec::new(),
            geometry: Vec::new(),
            white_material,
            id_textures: Vec::new(),
            requests: Vec::new(),
        }
    }

    /// The loads started since the last call, in request order.
    pub fn take_texture_requests(&mut self) -> Vec<TextureRequest> {
        std::mem::take(&mut self.requests)
    }

    /// The `TextureLoader` success callback: nearest filtering, and a textured material in the
    /// front-face slot of the tag the request came from.
    pub fn texture_loaded(&mut self, scene: &mut Scene, request: &TextureRequest, texture: Texture) {
        let mut texture = texture;
        texture.min_filter = MinFilter::Nearest;
        texture.mag_filter = Filter::Nearest;
        let texture = Arc::new(texture);
        self.id_textures.push(texture.clone());
        let material = Arc::new(PhongMaterial {
            map: Some(texture),
            specular: self.ctx.material_specular,
            shininess: self.ctx.material_shininess,
            ..PhongMaterial::new()
        });
        if let Some(mesh) = scene.mesh_mut(request.tag)
            && let Materials::Array(materials) = &mut mesh.material
        {
            materials[0] = material;
        }
    }

    fn geometry_for(&mut self, key: &JsString, family: &JsString, inches: f64) -> Arc<Geometry> {
        if let Some((_, geometry)) = self.geometry.iter().find(|(k, _)| k == key) {
            return geometry.clone();
        }
        let mut size = asrs_log::units::convert(inches, "inches", "meters")
            .expect("inches and meters are units");
        if family.as_string().as_deref() == Some("36h11") {
            size *= 10.0 / 8.0;
        } else {
            size *= 8.0 / 6.0;
        }
        let mut geometry = box_geometry(APRIL_TAG_THICKNESS, size, size, 1.0, 1.0, 1.0);
        geometry
            .translate(APRIL_TAG_OFFSET, 0.0, 0.0)
            .rotate_x(math::PI / 2.0);
        let geometry = Geometry::new(geometry);
        self.geometry.push((key.clone(), geometry.clone()));
        geometry
    }
}

impl ObjectManager for AprilTagManager {
    type Object = AprilTagObj;

    fn dispose(&mut self, scene: &mut Scene) {
        for tag in &self.tags {
            scene.graph.remove(self.ctx.root, tag.object);
        }
    }

    fn set_object_data(
        &mut self,
        scene: &mut Scene,
        object: &AprilTagObj,
    ) -> Result<(), ManagerError> {
        for tag in &mut self.tags {
            tag.active = false;
        }

        let family = &object.variant.family;
        let geometry_key = concat(&[
            family,
            &JsString::from("_"),
            &JsString::from(number::to_string(object.variant.inches)),
        ]);
        let geometry = self.geometry_for(&geometry_key, family, object.variant.inches);

        for annotated in &object.poses {
            let id = annotated.annotation.april_tag_id;
            let id_text = id.map(|id| JsString::from(number::to_string(id)));
            let id_str = concat(&[
                &geometry_key,
                &JsString::from("_"),
                id_text.as_ref().unwrap_or(&JsString::new()),
            ]);

            let index = match self.tags.iter().position(|t| !t.active && t.id_str == id_str) {
                Some(index) => {
                    self.tags[index].active = true;
                    index
                }
                None => {
                    let mut mesh = Mesh::new(
                        geometry.clone(),
                        vec![self.white_material.clone(); 6],
                    );
                    mesh.cast_shadow = true;
                    let object_id = scene.create(NodeKind::Mesh(mesh));
                    scene.graph.add(self.ctx.root, object_id);
                    let texture_name = match id_text {
                        None => JsString::from("smile"),
                        Some(text) => zfill(text, 3),
                    };
                    let url = concat(&[
                        &JsString::from("../www/textures/apriltag-"),
                        family,
                        &JsString::from("/"),
                        &texture_name,
                        &JsString::from(".png"),
                    ]);
                    self.requests.push(TextureRequest {
                        tag: object_id,
                        url,
                    });
                    self.tags.push(TagEntry {
                        id_str,
                        active: true,
                        object: object_id,
                    });
                    self.tags.len() - 1
                }
            };

            let tag = scene.graph.object_mut(self.tags[index].object);
            tag.set_rotation_from_quaternion(&rotation3d_to_quaternion(&annotated.pose.rotation));
            let [x, y, z] = annotated.pose.translation;
            tag.position = Vector3::new(x, y, z);
        }

        for tag in &self.tags {
            scene.node_mut(tag.object).visible = tag.active;
        }
        Ok(())
    }
}

/// Where a request's `../www/...` URL (relative to `www/hub.html`) points inside the directory
/// holding upstream's `www`, or `None` for a URL that leaves it.
pub fn april_tag_texture_path(app_root: &Path, url: &JsString) -> Option<PathBuf> {
    let url = url.as_string()?;
    let relative = url.strip_prefix("../")?;
    let mut path = app_root.to_path_buf();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => path.push(part),
            _ => return None,
        }
    }
    Some(path)
}

/// An image file as `TextureLoader` hands it to WebGL: 8-bit RGBA, straight alpha, no colour
/// conversion (three uploads `NoColorSpace` textures with `UNPACK_COLORSPACE_CONVERSION_WEBGL`
/// set to `NONE`).
pub fn decode_png_texture(bytes: &[u8]) -> Option<Texture> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    let pixels = &buffer[..info.buffer_size()];
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => pixels.to_vec(),
        png::ColorType::Rgb => pixels
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => pixels
            .chunks_exact(2)
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => pixels.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        png::ColorType::Indexed => return None,
    };
    Some(Texture::new(info.width, info.height, rgba.into()))
}
