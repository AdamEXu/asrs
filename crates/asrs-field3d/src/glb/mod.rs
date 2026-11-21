// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors) and AdvantageScope
// (BSD-3-Clause, Copyright 2021-2026 Littleton Robotics). See THIRD_PARTY_NOTICES.md.

//! Upstream's model pipeline: three's `GLTFLoader` reduced to the scene graph it hands to
//! `optimizeGeometries` (`loader`), `field3d/OptimizeGeometries.ts` (`optimize`), and the
//! `loadField` / `loadRobot` workers as a background job (`workers`).

#![deny(clippy::disallowed_methods)]

mod loader;
mod optimize;
mod workers;

use std::sync::Arc;

use asrs_js::string::JsString;
use asrs_js::value::JsValue;

use crate::three::{BufferGeometry, Graph, ObjectId};

pub use loader::{GltfError, GltfScene, load_gltf, sanitize_node_name};
pub use optimize::{
    CINEMATIC_MAX_RADIUS, FTC_MULTIPLIER, LOW_POWER_MAX_RADIUS, OptimizeError, Optimized,
    OptimizedMaterial, OptimizedMesh, STANDARD_MAX_RADIUS, optimize_geometries,
};
pub use workers::{
    Fetch, LoadError, LoadJob, LoadOptions, LoadedField, load_field, load_robot, read_file_url,
    spawn_load_field, spawn_load_robot, with_load_stack,
};

/// What `optimizeGeometries` reads from a mesh's material: `transparent`, `opacity` and, when
/// the material has one, `color`.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceMaterial {
    pub color: Option<[f64; 3]>,
    pub transparent: bool,
    pub opacity: f64,
}

impl SourceMaterial {
    /// `new MeshStandardMaterial()` and friends: white, opaque.
    pub fn white() -> Self {
        SourceMaterial {
            color: Some([1.0, 1.0, 1.0]),
            transparent: false,
            opacity: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MeshMaterial {
    Single(Arc<SourceMaterial>),
    /// A material array, which `optimizeGeometries` gives neither a color nor transparency.
    Array,
}

#[derive(Clone, Debug)]
pub enum ObjectKind {
    Object3D,
    Group,
    Mesh {
        geometry: Arc<BufferGeometry>,
        material: MeshMaterial,
    },
    /// Any other `Object3D.type` (`Points`, `Line`, `PerspectiveCamera`, ...), which
    /// `optimizeGeometries` skips.
    Other(&'static str),
}

/// One three object of a model scene. The name is a JS string, since names come from glTF JSON
/// and are matched with `toLowerCase`, `includes` and `===`.
#[derive(Clone, Debug)]
pub struct SceneObject {
    pub name: JsString,
    pub kind: ObjectKind,
}

impl SceneObject {
    pub fn new(name: JsString, kind: ObjectKind) -> Self {
        SceneObject { name, kind }
    }

    pub fn type_name(&self) -> &'static str {
        match self.kind {
            ObjectKind::Object3D => "Object3D",
            ObjectKind::Group => "Group",
            ObjectKind::Mesh { .. } => "Mesh",
            ObjectKind::Other(name) => name,
        }
    }
}

pub type SceneGraph = Graph<SceneObject>;

/// `object.getObjectByName(name)`: depth first, parent before children.
pub fn object_by_name(graph: &SceneGraph, root: ObjectId, name: &JsString) -> Option<ObjectId> {
    if graph.node(root).data.name == *name {
        return Some(root);
    }
    graph
        .node(root)
        .children()
        .iter()
        .find_map(|&child| object_by_name(graph, child, name))
}

/// JS truthiness of an optional property value.
pub(crate) fn truthy(value: Option<&JsValue>) -> bool {
    match value {
        None | Some(JsValue::Undefined | JsValue::Null) => false,
        Some(JsValue::Bool(b)) => *b,
        Some(JsValue::Number(n)) => *n != 0.0 && !n.is_nan(),
        Some(JsValue::String(s)) => !s.is_empty(),
        Some(JsValue::BigInt(_)) => true,
        Some(_) => true,
    }
}

/// `ToNumber` for the JSON values a glTF file can hold.
pub(crate) fn to_number(value: Option<&JsValue>) -> f64 {
    match value {
        None | Some(JsValue::Undefined) => f64::NAN,
        Some(JsValue::Null) => 0.0,
        Some(JsValue::Bool(b)) => f64::from(u8::from(*b)),
        Some(JsValue::Number(n)) => *n,
        Some(JsValue::String(s)) => asrs_js::number::to_number(s.units()),
        Some(JsValue::Array(items)) => match items.as_slice() {
            [] => 0.0,
            [single @ (JsValue::Number(_) | JsValue::String(_) | JsValue::Null)] => {
                to_number(Some(single))
            }
            _ => f64::NAN,
        },
        Some(_) => f64::NAN,
    }
}
