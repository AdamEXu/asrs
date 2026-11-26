// Ported from AdvantageScope (BSD-3-Clause, Copyright 2021-2026 Littleton Robotics). See
// THIRD_PARTY_NOTICES.md.

//! `field3d/EvergreenField.ts`, `field3d/AxesField.ts` and `field3d/AxesTemplate.ts`: the
//! procedural fields. Each builder creates its subtree in `scene` without attaching it, as
//! upstream returns a detached `THREE.Object3D`, and returns the subtree's root.

#![deny(clippy::disallowed_methods)]

use std::sync::Arc;

use asrs_js::math;
use asrs_js::string::JsString;
use asrs_log::units;
use asrs_model::assets::{
    CoordinateSystem, alliance_station_width, frc_standard_field_length, frc_standard_field_width,
    ftc_standard_field_length, ftc_standard_field_width,
};

use crate::glb::{MeshMaterial, ObjectKind, SceneGraph, SceneObject, SourceMaterial};
use crate::render::{
    Geometry, Line, LineBasicMaterial, Materials, Mesh, NodeKind, PhongMaterial, Scene, Side,
};
use crate::three::primitives::{cylinder_geometry, plane_geometry, sphere_geometry};
use crate::three::{BufferGeometry, Color, ObjectId, Vector3};

/// `makeEvergreenField(materialSpecular, materialShininess, isFTC)`.
pub fn make_evergreen_field(
    scene: &mut Scene,
    material_specular: Color,
    material_shininess: f64,
    is_ftc: bool,
) -> ObjectId {
    let field = scene.create(NodeKind::Group);
    let field_length = if is_ftc {
        ftc_standard_field_length()
    } else {
        frc_standard_field_length()
    };
    let field_width = if is_ftc {
        ftc_standard_field_width()
    } else {
        frc_standard_field_width()
    };
    let blue_color = 0x6379a6;
    let red_color = 0xa66363;
    let tau = math::PI * 2.0;

    let carpet = add_mesh(
        scene,
        field,
        plane_geometry(
            if is_ftc {
                field_length
            } else {
                field_length + 4.0
            },
            if is_ftc {
                field_width
            } else {
                field_width + 1.0
            },
            1.0,
            1.0,
        ),
        phong_double(0x888888),
    );
    scene.graph.node_mut(carpet).name = "carpet".to_owned();

    let tube_radius = if is_ftc { 0.01 } else { 0.02 };
    let guardrail_height = units::convert(if is_ftc { 12.0 } else { 20.0 }, "inches", "meters")
        .expect("inches and meters are units");
    for y in [-field_width / 2.0, field_width / 2.0] {
        for z in [0.0, guardrail_height] {
            let guardrail = add_mesh(
                scene,
                field,
                cylinder_geometry(
                    tube_radius,
                    tube_radius,
                    field_length,
                    12.0,
                    1.0,
                    false,
                    0.0,
                    tau,
                ),
                phong(0xdddddd),
            );
            let object = scene.graph.object_mut(guardrail);
            object.rotate_z(math::PI / 2.0);
            object.position = Vector3::new(0.0, y, z);
        }
        let panel = add_mesh(
            scene,
            field,
            plane_geometry(field_length, guardrail_height, 1.0, 1.0),
            phong_glass(0xffffff),
        );
        let object = scene.graph.object_mut(panel);
        object.rotate_x(math::PI / 2.0);
        object.position = Vector3::new(0.0, y, guardrail_height / 2.0);
        let mut x = -field_length / 2.0;
        while x <= field_length / 2.0 {
            if is_ftc || x != -frc_standard_field_length() / 2.0 {
                let guardrail = add_mesh(
                    scene,
                    field,
                    cylinder_geometry(
                        tube_radius,
                        tube_radius,
                        guardrail_height,
                        12.0,
                        1.0,
                        false,
                        0.0,
                        tau,
                    ),
                    phong(0xdddddd),
                );
                let object = scene.graph.object_mut(guardrail);
                object.rotate_x(math::PI / 2.0);
                object.position = Vector3::new(x, y, guardrail_height / 2.0);
            }
            x += if is_ftc {
                field_length / 3.0
            } else {
                field_length / 16.0
            };
        }
    }

    if is_ftc {
        for x in [-field_length / 2.0, field_length / 2.0] {
            for z in [0.0, guardrail_height] {
                let guardrail = add_mesh(
                    scene,
                    field,
                    cylinder_geometry(
                        tube_radius,
                        tube_radius,
                        ftc_standard_field_width(),
                        12.0,
                        1.0,
                        false,
                        0.0,
                        tau,
                    ),
                    phong(0xdddddd),
                );
                scene.graph.object_mut(guardrail).position = Vector3::new(x, 0.0, z);
            }
            let panel = add_mesh(
                scene,
                field,
                plane_geometry(field_length, guardrail_height, 1.0, 1.0),
                phong_double(if x < 0.0 { red_color } else { blue_color }),
            );
            let object = scene.graph.object_mut(panel);
            object.rotate_x(math::PI / 2.0);
            object.rotate_y(math::PI / 2.0);
            object.position = Vector3::new(x, 0.0, guardrail_height / 2.0);
            let mut y = -ftc_standard_field_width() / 2.0;
            while y <= ftc_standard_field_width() / 2.0 {
                let guardrail = add_mesh(
                    scene,
                    field,
                    cylinder_geometry(
                        tube_radius,
                        tube_radius,
                        guardrail_height,
                        12.0,
                        1.0,
                        false,
                        0.0,
                        tau,
                    ),
                    phong(0xdddddd),
                );
                let object = scene.graph.object_mut(guardrail);
                object.rotate_x(math::PI / 2.0);
                object.position = Vector3::new(x, y, guardrail_height / 2.0);
                y += ftc_standard_field_width() / 3.0;
            }
        }
    } else {
        let station_width = alliance_station_width();
        let station_height =
            units::convert(78.0, "inches", "meters").expect("inches and meters are units");
        let station_solid_height =
            units::convert(36.75, "inches", "meters").expect("inches and meters are units");
        let station_shelf_depth =
            units::convert(12.25, "inches", "meters").expect("inches and meters are units");
        let filler_width = (frc_standard_field_width() - station_width * 3.0) / 2.0;
        for x in [-field_length / 2.0, field_length / 2.0] {
            for z in [0.0, station_solid_height, station_height] {
                let guardrail = add_mesh(
                    scene,
                    field,
                    cylinder_geometry(
                        0.02,
                        0.02,
                        if z == station_solid_height {
                            station_width * 3.0
                        } else {
                            frc_standard_field_width()
                        },
                        12.0,
                        1.0,
                        false,
                        0.0,
                        tau,
                    ),
                    phong(0xdddddd),
                );
                scene.graph.object_mut(guardrail).position = Vector3::new(x, 0.0, z);
            }
            for y in [
                -frc_standard_field_width() / 2.0,
                station_width * -1.5,
                station_width * -0.5,
                station_width * 0.5,
                station_width * 1.5,
                frc_standard_field_width() / 2.0,
            ] {
                let guardrail = add_mesh(
                    scene,
                    field,
                    cylinder_geometry(
                        tube_radius,
                        tube_radius,
                        station_height,
                        12.0,
                        1.0,
                        false,
                        0.0,
                        tau,
                    ),
                    phong(0xdddddd),
                );
                let object = scene.graph.object_mut(guardrail);
                object.rotate_x(math::PI / 2.0);
                object.position = Vector3::new(x, y, station_height / 2.0);
            }
            for y in [
                -frc_standard_field_width() / 2.0 + filler_width / 2.0,
                frc_standard_field_width() / 2.0 - filler_width / 2.0,
            ] {
                let filler = add_mesh(
                    scene,
                    field,
                    plane_geometry(station_height, filler_width, 1.0, 1.0),
                    phong_double(if x < 0.0 { red_color } else { blue_color }),
                );
                let object = scene.graph.object_mut(filler);
                object.rotate_y(math::PI / 2.0);
                object.position = Vector3::new(x, y, station_height / 2.0);
            }
            let alliance_wall = add_mesh(
                scene,
                field,
                plane_geometry(station_solid_height, station_width * 3.0, 1.0, 1.0),
                phong_double(if x < 0.0 { red_color } else { blue_color }),
            );
            let object = scene.graph.object_mut(alliance_wall);
            object.rotate_y(math::PI / 2.0);
            object.position = Vector3::new(x, 0.0, station_solid_height / 2.0);
            let alliance_glass = add_mesh(
                scene,
                field,
                plane_geometry(
                    station_height - station_solid_height,
                    station_width * 3.0,
                    1.0,
                    1.0,
                ),
                phong_glass(if x < 0.0 { red_color } else { blue_color }),
            );
            let object = scene.graph.object_mut(alliance_glass);
            object.rotate_y(math::PI / 2.0);
            object.position = Vector3::new(
                x,
                0.0,
                station_solid_height + (station_height - station_solid_height) / 2.0,
            );
            let alliance_shelves = add_mesh(
                scene,
                field,
                plane_geometry(station_shelf_depth, station_width * 3.0, 1.0, 1.0),
                phong_double(if x < 0.0 { red_color } else { blue_color }),
            );
            scene.graph.object_mut(alliance_shelves).position = Vector3::new(
                x + (station_shelf_depth / 2.0) * (if x > 0.0 { 1.0 } else { -1.0 }),
                0.0,
                station_solid_height,
            );
        }
    }

    for id in scene.graph.descendants(field) {
        let is_carpet = scene.graph.node(id).name == "carpet";
        let Some(mesh) = scene.mesh_mut(id) else {
            continue;
        };
        if let Materials::Single(material) = &mut mesh.material {
            if !is_carpet {
                let material = Arc::make_mut(material);
                material.specular = material_specular;
                material.shininess = material_shininess;
            }
            mesh.cast_shadow = !is_carpet;
            mesh.receive_shadow = true;
        }
    }

    field
}

/// `makeAxesField(materialSpecular, materialShininess, coordinateSystem, isFTC)`.
pub fn make_axes_field(
    scene: &mut Scene,
    material_specular: Color,
    material_shininess: f64,
    coordinate_system: CoordinateSystem,
    is_ftc: bool,
) -> ObjectId {
    let field = scene.create(NodeKind::Group);
    let field_length = if is_ftc {
        ftc_standard_field_length()
    } else {
        frc_standard_field_length()
    };
    let field_width = if is_ftc {
        ftc_standard_field_width()
    } else {
        frc_standard_field_width()
    };

    let axes = make_axes_template(scene, material_specular, material_shininess);
    scene.graph.add(field, axes);
    if is_ftc {
        scene.graph.object_mut(axes).scale = Vector3::new(0.5, 0.5, 0.5);
    }
    match coordinate_system {
        CoordinateSystem::WallAlliance => {
            let object = scene.graph.object_mut(axes);
            object.set_rotation_from_axis_angle(Vector3::new(0.0, 0.0, 1.0), math::PI);
            object.position = Vector3::new(field_length / 2.0, field_width / 2.0, 0.0);
            let axes_clone = clone_subtree(scene, axes, field);
            let object = scene.graph.object_mut(axes_clone);
            object.set_rotation_from_axis_angle(Vector3::new(0.0, 0.0, 1.0), 0.0);
            object.position = Vector3::new(-field_length / 2.0, -field_width / 2.0, 0.0);
        }
        CoordinateSystem::WallBlue => {
            let object = scene.graph.object_mut(axes);
            object.set_rotation_from_axis_angle(Vector3::new(0.0, 0.0, 1.0), math::PI);
            object.position = Vector3::new(field_length / 2.0, field_width / 2.0, 0.0);
        }
        CoordinateSystem::CenterRotated => {
            let object = scene.graph.object_mut(axes);
            object.set_rotation_from_axis_angle(Vector3::new(0.0, 0.0, 1.0), -math::PI / 2.0);
            object.position = Vector3::new(0.0, 0.0, 0.0);
        }
        CoordinateSystem::CenterRed => {
            let object = scene.graph.object_mut(axes);
            object.set_rotation_from_axis_angle(Vector3::new(0.0, 0.0, 1.0), 0.0);
            object.position = Vector3::new(0.0, 0.0, 0.0);
        }
    }

    let outline = Line::from_points(
        &[
            Vector3::new(-field_length / 2.0, -field_width / 2.0, 0.0),
            Vector3::new(field_length / 2.0, -field_width / 2.0, 0.0),
            Vector3::new(field_length / 2.0, field_width / 2.0, 0.0),
            Vector3::new(-field_length / 2.0, field_width / 2.0, 0.0),
            Vector3::new(-field_length / 2.0, -field_width / 2.0, 0.0),
        ],
        LineBasicMaterial::new(Color::hex(f64::from(0x444444))),
    );
    scene.add(field, NodeKind::Line(outline));

    field
}

/// `makeAxesTemplate(materialSpecular, materialShininess)`.
pub fn make_axes_template(
    scene: &mut Scene,
    material_specular: Color,
    material_shininess: f64,
) -> ObjectId {
    let axes = scene.create(NodeKind::Object3D);
    let radius = 0.02;
    let tau = math::PI * 2.0;

    axis_mesh(
        scene,
        axes,
        sphere_geometry(radius, 8.0, 4.0, 0.0, tau, 0.0, math::PI),
        0xffffff,
        material_specular,
        material_shininess,
    );

    let x_axis = axis_mesh(
        scene,
        axes,
        cylinder_geometry(radius, radius, 1.0, 8.0, 1.0, false, 0.0, tau),
        0xff0000,
        material_specular,
        material_shininess,
    );
    let object = scene.graph.object_mut(x_axis);
    object.position = Vector3::new(0.5, 0.0, 0.0);
    object.rotate_z(math::PI / 2.0);

    let y_axis = axis_mesh(
        scene,
        axes,
        cylinder_geometry(radius, radius, 1.0, 8.0, 1.0, false, 0.0, tau),
        0x00ff00,
        material_specular,
        material_shininess,
    );
    scene.graph.object_mut(y_axis).position = Vector3::new(0.0, 0.5, 0.0);

    let z_axis = axis_mesh(
        scene,
        axes,
        cylinder_geometry(radius, radius, 1.0, 8.0, 1.0, false, 0.0, tau),
        0x2020ff,
        material_specular,
        material_shininess,
    );
    let object = scene.graph.object_mut(z_axis);
    object.position = Vector3::new(0.0, 0.0, 0.5);
    object.rotate_x(math::PI / 2.0);

    axes
}

/// The subtree under `root` as the scene graph `optimizeGeometries` reads, for callers that
/// optimize a procedural field (`AxesManager`, `XRRenderer`). Returns the copy of `root`.
pub fn to_scene_graph(scene: &Scene, root: ObjectId) -> (SceneGraph, ObjectId) {
    fn copy(
        scene: &Scene,
        src: ObjectId,
        graph: &mut SceneGraph,
        parent: Option<ObjectId>,
    ) -> ObjectId {
        let node = scene.graph.node(src);
        let kind = match &node.data.kind {
            NodeKind::Object3D => ObjectKind::Object3D,
            NodeKind::Group => ObjectKind::Group,
            NodeKind::Mesh(mesh) => ObjectKind::Mesh {
                geometry: mesh.geometry.geometry.clone(),
                material: match &mesh.material {
                    Materials::Single(material) => MeshMaterial::Single(Arc::new(SourceMaterial {
                        color: Some(material.color.to_array()),
                        transparent: material.transparent,
                        opacity: material.opacity,
                    })),
                    Materials::Array(_) => MeshMaterial::Array,
                },
            },
            NodeKind::Line(line) => ObjectKind::Other(if line.segments {
                "LineSegments"
            } else {
                "Line"
            }),
            NodeKind::FatLine(_) => ObjectKind::Other("Line2"),
            NodeKind::HemisphereLight(_) => ObjectKind::Other("HemisphereLight"),
            NodeKind::PointLight(_) => ObjectKind::Other("PointLight"),
            NodeKind::SpotLight(_) => ObjectKind::Other("SpotLight"),
        };
        let id = graph.create(
            &node.name,
            SceneObject::new(JsString::from(&node.name), kind),
        );
        *graph.object_mut(id) = node.object.clone();
        if let Some(parent) = parent {
            graph.add(parent, id);
        }
        for &child in node.children() {
            copy(scene, child, graph, Some(id));
        }
        id
    }
    let mut graph = SceneGraph::new();
    let root = copy(scene, root, &mut graph, None);
    (graph, root)
}

fn add_mesh(
    scene: &mut Scene,
    parent: ObjectId,
    geometry: BufferGeometry,
    material: PhongMaterial,
) -> ObjectId {
    scene.add_mesh(
        parent,
        Mesh::new(Geometry::new(geometry), Arc::new(material)),
    )
}

fn phong(hex: u32) -> PhongMaterial {
    PhongMaterial {
        color: Color::hex(f64::from(hex)),
        ..PhongMaterial::new()
    }
}

fn phong_double(hex: u32) -> PhongMaterial {
    PhongMaterial {
        color: Color::hex(f64::from(hex)),
        side: Side::Double,
        ..PhongMaterial::new()
    }
}

fn phong_glass(hex: u32) -> PhongMaterial {
    PhongMaterial {
        color: Color::hex(f64::from(hex)),
        side: Side::Double,
        opacity: 0.25,
        transparent: true,
        ..PhongMaterial::new()
    }
}

fn axis_mesh(
    scene: &mut Scene,
    axes: ObjectId,
    geometry: BufferGeometry,
    hex: u32,
    material_specular: Color,
    material_shininess: f64,
) -> ObjectId {
    let id = add_mesh(
        scene,
        axes,
        geometry,
        PhongMaterial {
            color: Color::hex(f64::from(hex)),
            specular: material_specular,
            shininess: material_shininess,
            ..PhongMaterial::new()
        },
    );
    if let Some(mesh) = scene.mesh_mut(id) {
        mesh.cast_shadow = true;
        mesh.receive_shadow = true;
    }
    id
}

/// `Object3D.clone()`: every node copied recursively, `Mesh` clones sharing their geometry and
/// material as three's `Mesh.copy` does.
fn clone_subtree(scene: &mut Scene, src: ObjectId, parent: ObjectId) -> ObjectId {
    let node = scene.graph.node(src);
    let data = node.data.clone();
    let object = node.object.clone();
    let name = node.name.clone();
    let children = node.children().to_vec();
    let id = scene.graph.create(&name, data);
    *scene.graph.object_mut(id) = object;
    scene.graph.add(parent, id);
    for child in children {
        clone_subtree(scene, child, id);
    }
    id
}
