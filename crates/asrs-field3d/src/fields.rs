// Ported from AdvantageScope (BSD-3-Clause, Copyright 2021-2026 Littleton Robotics). See
// THIRD_PARTY_NOTICES.md.

//! `field3d/EvergreenField.ts`, `field3d/AxesField.ts` and `field3d/AxesTemplate.ts`: the
//! procedural fields. Each builder creates its subtree in `scene` without attaching it, as
//! upstream returns a detached `THREE.Object3D`, and returns the subtree's root.

#![deny(clippy::disallowed_methods)]

use asrs_model::assets::CoordinateSystem;

use crate::glb::SceneGraph;
use crate::render::Scene;
use crate::three::{Color, ObjectId};

/// `makeEvergreenField(materialSpecular, materialShininess, isFTC)`.
pub fn make_evergreen_field(
    scene: &mut Scene,
    material_specular: Color,
    material_shininess: f64,
    is_ftc: bool,
) -> ObjectId {
    let _ = (scene, material_specular, material_shininess, is_ftc);
    todo!("T06: port EvergreenField.ts")
}

/// `makeAxesField(materialSpecular, materialShininess, coordinateSystem, isFTC)`.
pub fn make_axes_field(
    scene: &mut Scene,
    material_specular: Color,
    material_shininess: f64,
    coordinate_system: CoordinateSystem,
    is_ftc: bool,
) -> ObjectId {
    let _ = (
        scene,
        material_specular,
        material_shininess,
        coordinate_system,
        is_ftc,
    );
    todo!("T06: port AxesField.ts")
}

/// `makeAxesTemplate(materialSpecular, materialShininess)`.
pub fn make_axes_template(
    scene: &mut Scene,
    material_specular: Color,
    material_shininess: f64,
) -> ObjectId {
    let _ = (scene, material_specular, material_shininess);
    todo!("T06: port AxesTemplate.ts")
}

/// The subtree under `root` as the scene graph `optimizeGeometries` reads, for callers that
/// optimize a procedural field (`AxesManager`, `XRRenderer`). Returns the copy of `root`.
pub fn to_scene_graph(scene: &Scene, root: ObjectId) -> (SceneGraph, ObjectId) {
    let _ = (scene, root);
    todo!("T06: convert a render subtree to a glb::SceneGraph")
}
