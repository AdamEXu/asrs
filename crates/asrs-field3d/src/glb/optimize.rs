// Ported from AdvantageScope (BSD-3-Clause, Copyright 2021-2026 Littleton Robotics) and three.js
// r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

use std::collections::BTreeMap;
use std::sync::Arc;

use asrs_js::math;
use asrs_js::string::JsString;
use asrs_model::preferences::Field3dMode;

use super::{MeshMaterial, ObjectKind, SceneGraph};
use crate::render::{Geometry, Mesh, PhongMaterial, Side};
use crate::three::{
    ArrayData, BufferAttribute, BufferGeometry, Color, ObjectId, Vector3, merge_geometries,
};

pub const LOW_POWER_MAX_RADIUS: f64 = 0.08;
pub const STANDARD_MAX_RADIUS: f64 = 0.04;
pub const CINEMATIC_MAX_RADIUS: f64 = 0.02;
pub const FTC_MULTIPLIER: f64 = 0.25;

/// The `MeshPhongMaterial` `optimizeGeometries` gives a merged mesh. `vertexColors` is always
/// on and `side` is always `DoubleSide`.
#[derive(Clone, Debug, PartialEq)]
pub struct OptimizedMaterial {
    pub specular: Color,
    pub shininess: f64,
    pub transparent: bool,
    pub opacity: f64,
    pub depth_write: bool,
}

/// One merged mesh, named `normal`, `transparent` or `carpet` as upstream names it.
#[derive(Clone, Debug, PartialEq)]
pub struct OptimizedMesh {
    pub name: &'static str,
    pub geometry: BufferGeometry,
    pub material: OptimizedMaterial,
    pub cast_shadow: bool,
    pub receive_shadow: bool,
}

impl OptimizedMesh {
    /// The mesh as the renderer holds it after `ObjectLoader.parse` of the worker's `toJSON()`:
    /// colors travel as hex, so `color` and `specular` come back through `setHex(getHex())`.
    /// Each call makes a new material, as each parse does.
    pub fn to_render_mesh(&self) -> Mesh {
        let material = PhongMaterial {
            color: Color::hex(Color::WHITE.get_hex()),
            emissive: Color::hex(0.0),
            specular: Color::hex(self.material.specular.get_hex()),
            shininess: self.material.shininess,
            opacity: self.material.opacity,
            transparent: self.material.transparent,
            side: Side::Double,
            depth_write: self.material.depth_write,
            vertex_colors: true,
            ..PhongMaterial::new()
        };
        let mut mesh = Mesh::new(Geometry::new(self.geometry.clone()), Arc::new(material));
        mesh.cast_shadow = self.cast_shadow;
        mesh.receive_shadow = self.receive_shadow;
        mesh
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Optimized {
    pub normal: Vec<OptimizedMesh>,
    pub transparent: Vec<OptimizedMesh>,
    pub carpet: Vec<OptimizedMesh>,
}

/// Where upstream throws inside `optimizeGeometries`, so its worker never answers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OptimizeError {
    /// A sliced mesh whose spiral index is not a usable array index: `bucket[NaN].push` throws
    /// a `TypeError`, an infinite index never stops growing the bucket list.
    BadSliceIndex(String),
}

impl std::fmt::Display for OptimizeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OptimizeError::BadSliceIndex(i) => write!(f, "slice index {i} is not an array index"),
        }
    }
}

type Buckets = BTreeMap<u64, Vec<BufferGeometry>>;

/// `optimizeGeometries(object, mode, materialSpecular, materialShininess,
/// enableSimplification, isFTC, slicingSize)`.
#[allow(clippy::too_many_arguments)]
pub fn optimize_geometries(
    graph: &mut SceneGraph,
    root: ObjectId,
    mode: Field3dMode,
    material_specular: Color,
    material_shininess: f64,
    enable_simplification: bool,
    is_ftc: bool,
    slicing_size: Option<f64>,
) -> Result<Optimized, OptimizeError> {
    let mut normal = Buckets::new();
    let mut transparent = Buckets::new();
    let mut carpet = Buckets::new();
    let limit = match mode {
        Field3dMode::LowPower => LOW_POWER_MAX_RADIUS,
        Field3dMode::Standard => STANDARD_MAX_RADIUS,
        Field3dMode::Cinematic => CINEMATIC_MAX_RADIUS,
    } * if is_ftc { FTC_MULTIPLIER } else { 1.0 };
    let carpet_name = JsString::from("carpet");
    let no_simplify = JsString::from("NOSIMPLIFY");

    for id in graph.descendants(root) {
        let (source, material) = match &graph.node(id).data.kind {
            ObjectKind::Mesh { geometry, material } => (geometry.clone(), material.clone()),
            _ => continue,
        };
        let mut geometry = (*source).clone();
        graph.update_world_matrix(id, true, false);
        geometry.apply_matrix4(&graph.object(id).matrix_world);

        if !geometry.has_attribute("normal") || !geometry.has_attribute("position") {
            continue;
        }
        let others: Vec<String> = geometry
            .attribute_names()
            .filter(|n| *n != "normal" && *n != "position")
            .map(str::to_owned)
            .collect();
        for name in others {
            geometry.delete_attribute(&name);
        }

        let position = geometry
            .attribute("position")
            .expect("checked above")
            .clone();
        let mut is_transparent = false;
        if let MeshMaterial::Single(material) = &material {
            is_transparent = material.transparent && material.opacity < 0.75;
            if let Some(color) = material.color {
                let rgb = color.map(|v| v * 255.0);
                let mut colors = vec![0u8; 3 * position.count()];
                for (i, c) in colors.iter_mut().enumerate() {
                    *c = math::to_uint8(rgb[i % 3]);
                }
                geometry.set_attribute(
                    "color",
                    BufferAttribute::new(ArrayData::Uint8(colors), 3, true),
                );
            }
        }

        let vertices: Vec<Vector3> = (0..position.count()).map(|i| position.get_xyz(i)).collect();
        let mut center = Vector3::default();
        for &v in &vertices {
            center = center + v;
        }
        center = center.divide_scalar(vertices.len() as f64);
        let max_radius = vertices.iter().fold(0.0, |prev, v| {
            let dist = v.distance_to(center);
            if dist > prev { dist } else { prev }
        });
        let name = &graph.node(id).data.name;
        let include =
            !(enable_simplification && !name.includes(&no_simplify, None) && max_radius < limit);
        if !include {
            continue;
        }

        let mut output_index = 0.0;
        if let Some(size) = slicing_size
            && max_radius * 2.0 < size
        {
            output_index = 1.0
                + asrs_log::util::get_spiral_index(
                    math::floor(center.x / size),
                    math::floor(center.y / size),
                );
        }
        if !(output_index >= 0.0 && output_index.fract() == 0.0 && output_index < 4294967295.0) {
            return Err(OptimizeError::BadSliceIndex(format!("{output_index}")));
        }
        let bucket = if name.to_lower_case().includes(&carpet_name, None) {
            &mut carpet
        } else if is_transparent {
            &mut transparent
        } else {
            &mut normal
        };
        bucket
            .entry(output_index as u64)
            .or_default()
            .push(geometry);
    }

    let merged = |buckets: Buckets, name: &'static str| -> Vec<OptimizedMesh> {
        buckets
            .into_values()
            .filter_map(|group| merge_geometries(&group, false).ok())
            .map(|geometry| {
                let is_carpet = name == "carpet";
                let is_transparent = name == "transparent";
                let cinematic = mode == Field3dMode::Cinematic;
                OptimizedMesh {
                    name,
                    geometry,
                    material: OptimizedMaterial {
                        specular: material_specular,
                        shininess: if is_carpet { 0.0 } else { material_shininess },
                        transparent: is_transparent,
                        opacity: if is_transparent { 0.2 } else { 1.0 },
                        depth_write: !is_transparent,
                    },
                    cast_shadow: cinematic && !is_carpet,
                    receive_shadow: cinematic && is_carpet,
                }
            })
            .collect()
    };
    Ok(Optimized {
        normal: merged(normal, "normal"),
        transparent: merged(transparent, "transparent"),
        carpet: merged(carpet, "carpet"),
    })
}
