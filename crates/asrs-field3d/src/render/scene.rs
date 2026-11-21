// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use asrs_js::math;

use crate::three::{BufferGeometry, Color, Graph, Matrix4, ObjectId, Sphere, Vector3};

static NEXT_RESOURCE_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_MATERIAL_ID: AtomicU64 = AtomicU64::new(0);

fn next_resource_id() -> u64 {
    NEXT_RESOURCE_ID.fetch_add(1, Ordering::Relaxed)
}

/// An immutable `BufferGeometry` as uploaded to the GPU. Changing geometry means building a new
/// one, which gets a new upload key, as `needsUpdate` would trigger a re-upload in three.
#[derive(Debug)]
pub struct Geometry {
    key: u64,
    pub geometry: BufferGeometry,
    bounding_sphere: Sphere,
}

impl Geometry {
    /// Keeps a bounding sphere the geometry already carries (three never recomputes one that is
    /// set), otherwise computes it as the first render would.
    pub fn new(mut geometry: BufferGeometry) -> Arc<Self> {
        if geometry.bounding_sphere.is_none() {
            geometry.compute_bounding_sphere();
        }
        let bounding_sphere = geometry.bounding_sphere.unwrap_or_default();
        Arc::new(Geometry {
            key: next_resource_id(),
            geometry,
            bounding_sphere,
        })
    }

    pub fn key(&self) -> u64 {
        self.key
    }

    pub fn bounding_sphere(&self) -> Sphere {
        self.bounding_sphere
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Filter {
    Nearest,
    Linear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MinFilter {
    Nearest,
    Linear,
    NearestMipmapNearest,
    LinearMipmapNearest,
    NearestMipmapLinear,
    LinearMipmapLinear,
}

impl MinFilter {
    pub fn uses_mipmaps(self) -> bool {
        !matches!(self, MinFilter::Nearest | MinFilter::Linear)
    }
}

/// An RGBA8 texture with three's defaults for an image or canvas source (`flipY`, linear
/// magnification, trilinear minification, generated mipmaps, clamp to edge, no colour space
/// conversion). Each new set of pixels is a new texture, as `needsUpdate` is in three.
#[derive(Clone, Debug)]
pub struct Texture {
    key: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<[u8]>,
    pub flip_y: bool,
    pub mag_filter: Filter,
    pub min_filter: MinFilter,
    pub generate_mipmaps: bool,
}

impl Texture {
    pub fn new(width: u32, height: u32, rgba: Arc<[u8]>) -> Self {
        assert_eq!(rgba.len(), width as usize * height as usize * 4);
        Texture {
            key: next_resource_id(),
            width,
            height,
            rgba,
            flip_y: true,
            mag_filter: Filter::Linear,
            min_filter: MinFilter::LinearMipmapLinear,
            generate_mipmaps: true,
        }
    }

    pub fn key(&self) -> u64 {
        self.key
    }

    pub fn with_pixels(&self, rgba: Arc<[u8]>) -> Self {
        assert_eq!(rgba.len(), self.rgba.len());
        Texture {
            key: next_resource_id(),
            rgba,
            ..self.clone()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Side {
    Front,
    Back,
    Double,
}

/// `MeshPhongMaterial` with the properties upstream sets. `id` comes from one counter in creation
/// order like three's `Material.id`, which the opaque sort compares; a clone keeps it, as
/// mutating a three material in place does.
#[derive(Clone, Debug)]
pub struct PhongMaterial {
    pub id: u64,
    pub color: Color,
    pub emissive: Color,
    pub emissive_intensity: f64,
    pub specular: Color,
    pub shininess: f64,
    pub opacity: f64,
    pub transparent: bool,
    pub side: Side,
    pub depth_test: bool,
    pub depth_write: bool,
    pub vertex_colors: bool,
    pub map: Option<Arc<Texture>>,
    pub visible: bool,
}

impl Default for PhongMaterial {
    fn default() -> Self {
        PhongMaterial {
            id: NEXT_MATERIAL_ID.fetch_add(1, Ordering::Relaxed),
            color: Color::hex(0xffffff as f64),
            emissive: Color::hex(0x000000 as f64),
            emissive_intensity: 1.0,
            specular: Color::hex(0x111111 as f64),
            shininess: 30.0,
            opacity: 1.0,
            transparent: false,
            side: Side::Front,
            depth_test: true,
            depth_write: true,
            vertex_colors: false,
            map: None,
            visible: true,
        }
    }
}

impl PhongMaterial {
    pub fn new() -> Self {
        PhongMaterial::default()
    }
}

/// `InstancedMesh` instance data: Float32 matrices plus the lazily computed and then never
/// refreshed `boundingSphere` that three sorts by.
#[derive(Clone, Debug)]
pub struct Instances {
    pub matrices: Arc<Vec<[f32; 16]>>,
    bounding_sphere: Option<Sphere>,
}

impl Instances {
    pub fn new(count: usize) -> Self {
        let identity = matrix_to_f32(&Matrix4::IDENTITY);
        Instances {
            matrices: Arc::new(vec![identity; count]),
            bounding_sphere: None,
        }
    }

    pub fn count(&self) -> usize {
        self.matrices.len()
    }

    pub fn set_matrix_at(&mut self, index: usize, matrix: &Matrix4) {
        Arc::make_mut(&mut self.matrices)[index] = matrix_to_f32(matrix);
    }

    pub fn matrix_at(&self, index: usize) -> Matrix4 {
        Matrix4::from_array(self.matrices[index].map(f64::from))
    }

    pub fn compute_bounding_sphere(&mut self, geometry: &Geometry) {
        let mut sphere = Sphere::default();
        for i in 0..self.count() {
            let instance = sphere_apply_matrix4(geometry.bounding_sphere(), &self.matrix_at(i));
            sphere = sphere_union(sphere, instance);
        }
        self.bounding_sphere = Some(sphere);
    }

    pub fn bounding_sphere(&self) -> Option<Sphere> {
        self.bounding_sphere
    }
}

pub(crate) fn matrix_to_f32(m: &Matrix4) -> [f32; 16] {
    m.elements.map(|v| v as f32)
}

fn sphere_apply_matrix4(sphere: Sphere, m: &Matrix4) -> Sphere {
    Sphere {
        center: sphere.center.apply_matrix4(m),
        radius: sphere.radius * m.max_scale_on_axis(),
    }
}

fn sphere_expand_by_point(sphere: Sphere, point: Vector3) -> Sphere {
    if sphere.radius < 0.0 {
        return Sphere {
            center: point,
            radius: 0.0,
        };
    }
    let v = point - sphere.center;
    let length_sq = v.length_sq();
    if length_sq > sphere.radius * sphere.radius {
        let length = math::sqrt(length_sq);
        let delta = (length - sphere.radius) * 0.5;
        Sphere {
            center: sphere.center.add_scaled_vector(v, delta / length),
            radius: sphere.radius + delta,
        }
    } else {
        sphere
    }
}

fn sphere_union(sphere: Sphere, other: Sphere) -> Sphere {
    if other.radius < 0.0 {
        return sphere;
    }
    if sphere.radius < 0.0 {
        return other;
    }
    if sphere.center == other.center {
        return Sphere {
            center: sphere.center,
            radius: math::max2(sphere.radius, other.radius),
        };
    }
    let v2 = (other.center - sphere.center).set_length(other.radius);
    let sphere = sphere_expand_by_point(sphere, other.center + v2);
    sphere_expand_by_point(sphere, other.center - v2)
}

#[derive(Clone, Debug)]
pub struct Mesh {
    pub geometry: Arc<Geometry>,
    pub material: Arc<PhongMaterial>,
    pub instances: Option<Instances>,
    pub cast_shadow: bool,
    pub receive_shadow: bool,
}

impl Mesh {
    pub fn new(geometry: Arc<Geometry>, material: Arc<PhongMaterial>) -> Self {
        Mesh {
            geometry,
            material,
            instances: None,
            cast_shadow: false,
            receive_shadow: false,
        }
    }

    pub fn instanced(geometry: Arc<Geometry>, material: Arc<PhongMaterial>, count: usize) -> Self {
        Mesh {
            instances: Some(Instances::new(count)),
            ..Mesh::new(geometry, material)
        }
    }
}

#[derive(Clone, Debug)]
pub struct HemisphereLight {
    pub color: Color,
    pub ground_color: Color,
    pub intensity: f64,
}

#[derive(Clone, Debug)]
pub struct PointLight {
    pub color: Color,
    pub intensity: f64,
    pub distance: f64,
    pub decay: f64,
    pub cast_shadow: bool,
}

#[derive(Clone, Debug)]
pub struct SpotLight {
    pub color: Color,
    pub intensity: f64,
    pub distance: f64,
    pub angle: f64,
    pub penumbra: f64,
    pub decay: f64,
    pub target: ObjectId,
    pub cast_shadow: bool,
}

#[derive(Clone, Debug)]
pub enum NodeKind {
    Group,
    Mesh(Mesh),
    HemisphereLight(HemisphereLight),
    PointLight(PointLight),
    SpotLight(SpotLight),
}

#[derive(Clone, Debug)]
pub struct SceneNode {
    pub visible: bool,
    pub render_order: f64,
    pub kind: NodeKind,
}

impl SceneNode {
    pub fn new(kind: NodeKind) -> Self {
        SceneNode {
            visible: true,
            render_order: 0.0,
            kind,
        }
    }
}

/// A three `Scene`: the arena holds every `Object3D` and the root is the scene itself. Ids
/// follow creation order, which stands in for three's `Object3D.id` in the sort.
#[derive(Clone, Debug)]
pub struct Scene {
    pub graph: Graph<SceneNode>,
    pub root: ObjectId,
    pub background: Option<Color>,
}

impl Default for Scene {
    fn default() -> Self {
        Scene::new()
    }
}

impl Scene {
    pub fn new() -> Self {
        let mut graph = Graph::new();
        let root = graph.create("", SceneNode::new(NodeKind::Group));
        Scene {
            graph,
            root,
            background: None,
        }
    }

    pub fn create(&mut self, kind: NodeKind) -> ObjectId {
        self.graph.create("", SceneNode::new(kind))
    }

    pub fn add(&mut self, parent: ObjectId, kind: NodeKind) -> ObjectId {
        let id = self.create(kind);
        self.graph.add(parent, id);
        id
    }

    pub fn add_group(&mut self, parent: ObjectId) -> ObjectId {
        self.add(parent, NodeKind::Group)
    }

    pub fn add_mesh(&mut self, parent: ObjectId, mesh: Mesh) -> ObjectId {
        self.add(parent, NodeKind::Mesh(mesh))
    }

    pub fn add_hemisphere_light(
        &mut self,
        parent: ObjectId,
        color: Color,
        ground_color: Color,
        intensity: f64,
    ) -> ObjectId {
        let id = self.add(
            parent,
            NodeKind::HemisphereLight(HemisphereLight {
                color,
                ground_color,
                intensity,
            }),
        );
        self.place_at_default_up(id);
        id
    }

    pub fn add_point_light(&mut self, parent: ObjectId, color: Color, intensity: f64) -> ObjectId {
        self.add(
            parent,
            NodeKind::PointLight(PointLight {
                color,
                intensity,
                distance: 0.0,
                decay: 2.0,
                cast_shadow: false,
            }),
        )
    }

    /// `new SpotLight(...)`: the light and its detached `target`, which the caller adds to the
    /// scene as upstream does (`group.add(light, light.target)`).
    pub fn create_spot_light(
        &mut self,
        color: Color,
        intensity: f64,
        distance: f64,
        angle: f64,
        penumbra: f64,
        decay: f64,
    ) -> (ObjectId, ObjectId) {
        let target = self.create(NodeKind::Group);
        let light = self.create(NodeKind::SpotLight(SpotLight {
            color,
            intensity,
            distance,
            angle,
            penumbra,
            decay,
            target,
            cast_shadow: false,
        }));
        self.place_at_default_up(light);
        (light, target)
    }

    fn place_at_default_up(&mut self, id: ObjectId) {
        let object = self.graph.object_mut(id);
        object.position = Vector3::new(0.0, 1.0, 0.0);
        object.update_matrix();
    }

    pub fn node(&self, id: ObjectId) -> &SceneNode {
        &self.graph.node(id).data
    }

    pub fn node_mut(&mut self, id: ObjectId) -> &mut SceneNode {
        &mut self.graph.node_mut(id).data
    }

    pub fn mesh_mut(&mut self, id: ObjectId) -> Option<&mut Mesh> {
        match &mut self.node_mut(id).kind {
            NodeKind::Mesh(mesh) => Some(mesh),
            _ => None,
        }
    }
}
