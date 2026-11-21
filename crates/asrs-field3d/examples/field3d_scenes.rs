//! Renders the 3D reference scenes (`parity/field3d/scenes/*.json`) with the port, the
//! counterpart of `oracle/field3d/page.ts`. Usage:
//! `field3d_scenes <scenes dir> <out dir> [<scene>...]`. Writes `<scene>@<n>x-<theme>.png`
//! (straight RGBA) and `<scene>@<n>x.edges.png` (the gate's edge mask) for scales 1 and 2.

#![allow(clippy::disallowed_methods)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use asrs_field3d::render::{
    FatLine, Filter, FrameOptions, Geometry, Line, LineBasicMaterial, LineMaterial,
    LineSegmentsGeometry, Mesh, MinFilter, NodeKind, PhongMaterial, Renderer, Scene, Side,
    Texture, build_frame, edge_mask, swap_red_blue,
};
use asrs_field3d::three::{
    ArrayData, BufferAttribute, BufferGeometry, Color, Euler, EulerOrder, Object3D, ObjectId,
    PerspectiveCamera, Vector3, primitives,
};
use asrs_parity::json::{self, Json};
use asrs_parity::pixel::{self, Rgba};

fn num(j: &Json, key: &str) -> Option<f64> {
    j.get(key).and_then(Json::as_f64)
}

fn text(j: &Json, key: &str) -> Option<String> {
    j.get(key)
        .and_then(Json::as_str)
        .and_then(|s| s.as_string())
}

fn vec3(j: &Json, key: &str) -> Option<Vector3> {
    let a = j.get(key)?.as_array()?;
    Some(Vector3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
}

fn flag(j: &Json, key: &str) -> Option<bool> {
    match j.get(key) {
        Some(Json::Bool(b)) => Some(*b),
        _ => None,
    }
}

fn numbers(j: &Json, key: &str) -> Vec<f64> {
    j.get(key)
        .and_then(Json::as_array)
        .map(|a| a.iter().filter_map(Json::as_f64).collect())
        .unwrap_or_default()
}

fn arg(args: &[Json], i: usize) -> f64 {
    match &args[i] {
        Json::Bool(b) => f64::from(u8::from(*b)),
        other => other.as_f64().expect("numeric geometry argument"),
    }
}

fn geometry(spec: &Json) -> Arc<Geometry> {
    let args = spec
        .get("args")
        .and_then(Json::as_array)
        .expect("geometry args");
    let a = |i| arg(args, i);
    let flag = |i| a(i) != 0.0;
    let mut g: BufferGeometry = match text(spec, "type").as_deref() {
        Some("box") => primitives::box_geometry(a(0), a(1), a(2), a(3), a(4), a(5)),
        Some("plane") => primitives::plane_geometry(a(0), a(1), a(2), a(3)),
        Some("cylinder") => {
            primitives::cylinder_geometry(a(0), a(1), a(2), a(3), a(4), flag(5), a(6), a(7))
        }
        Some("cone") => primitives::cone_geometry(a(0), a(1), a(2), a(3), flag(4), a(5), a(6)),
        Some("sphere") => primitives::sphere_geometry(a(0), a(1), a(2), a(3), a(4), a(5), a(6)),
        other => panic!("unknown geometry {other:?}"),
    };
    if let Some(palette) = spec.get("palette").and_then(Json::as_array) {
        let count = g.attribute("position").map_or(0, BufferAttribute::count);
        let mut colors = vec![0u8; count * 3];
        for i in 0..count {
            let c = palette[i % palette.len()]
                .as_array()
                .expect("palette colour");
            for k in 0..3 {
                colors[i * 3 + k] = c[k].as_f64().expect("channel") as u8;
            }
        }
        g.set_attribute(
            "color",
            BufferAttribute::new(ArrayData::Uint8(colors), 3, true),
        );
    }
    Geometry::new(g)
}

fn checker(spec: &[Json]) -> Texture {
    let v = |i: usize| spec[i].as_f64().expect("checker value") as u32;
    let (size, cell, a, b) = (v(0), v(1), v(2), v(3));
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let hex = if (x / cell + y / cell) % 2 == 0 { a } else { b };
            let o = ((y * size + x) * 4) as usize;
            rgba[o..o + 4].copy_from_slice(&[(hex >> 16) as u8, (hex >> 8) as u8, hex as u8, 255]);
        }
    }
    Texture::new(size, size, rgba.into())
}

fn material(spec: Option<&Json>) -> Arc<PhongMaterial> {
    let mut m = PhongMaterial::new();
    let Some(spec) = spec else {
        return Arc::new(m);
    };
    if let Some(v) = num(spec, "color") {
        m.color = Color::hex(v);
    }
    if let Some(v) = num(spec, "specular") {
        m.specular = Color::hex(v);
    }
    if let Some(v) = num(spec, "shininess") {
        m.shininess = v;
    }
    if let Some(v) = num(spec, "emissive") {
        m.emissive = Color::hex(v);
    }
    if let Some(side) = text(spec, "side") {
        m.side = match side.as_str() {
            "back" => Side::Back,
            "double" => Side::Double,
            _ => Side::Front,
        };
    }
    let flag = |key| match spec.get(key) {
        Some(Json::Bool(b)) => Some(*b),
        _ => None,
    };
    if let Some(v) = flag("transparent") {
        m.transparent = v;
    }
    if let Some(v) = num(spec, "opacity") {
        m.opacity = v;
    }
    if let Some(v) = flag("depthWrite") {
        m.depth_write = v;
    }
    if let Some(v) = flag("depthTest") {
        m.depth_test = v;
    }
    if let Some(v) = flag("vertexColors") {
        m.vertex_colors = v;
    }
    if let Some(map) = spec.get("map") {
        let mut texture = checker(
            map.get("checker")
                .and_then(Json::as_array)
                .expect("checker"),
        );
        if text(map, "filter").as_deref() == Some("nearest") {
            texture.min_filter = MinFilter::Nearest;
            texture.mag_filter = Filter::Nearest;
        }
        m.map = Some(Arc::new(texture));
    }
    Arc::new(m)
}

fn place(object: &mut Object3D, spec: &Json) {
    if let Some(p) = vec3(spec, "position") {
        object.position = p;
    }
    if let Some(r) = vec3(spec, "rotation") {
        object.set_rotation(Euler::new(r.x, r.y, r.z, EulerOrder::Xyz));
    }
    if let Some(s) = vec3(spec, "scale") {
        object.scale = s;
    }
}

fn build(spec: &Json) -> (Scene, PerspectiveCamera) {
    let mut scene = Scene::new();
    let mut named: HashMap<String, ObjectId> = HashMap::new();
    for node in spec.get("nodes").and_then(Json::as_array).expect("nodes") {
        let parent = text(node, "parent").map_or(scene.root, |p| named[&p]);
        let color = |key| Color::hex(num(node, key).unwrap_or(16777215.0));
        let intensity = num(node, "intensity").unwrap_or(1.0);
        let mut spot_target = None;
        let id = match text(node, "type").as_deref() {
            Some("group") => scene.create(NodeKind::Group),
            Some("mesh") => scene.create(NodeKind::Mesh(Mesh::new(
                geometry(node.get("geometry").expect("geometry")),
                material(node.get("material")),
            ))),
            Some("line2") => scene.create(NodeKind::FatLine(FatLine {
                geometry: LineSegmentsGeometry::from_points(&numbers(node, "points")),
                material: Arc::new(LineMaterial::new(
                    color("color"),
                    num(node, "linewidth").unwrap_or(1.0),
                )),
            })),
            Some("line") => {
                let points: Vec<Vector3> = node
                    .get("points")
                    .and_then(Json::as_array)
                    .expect("points")
                    .iter()
                    .map(|p| {
                        let a = p.as_array().expect("point");
                        Vector3::new(
                            a[0].as_f64().unwrap_or(0.0),
                            a[1].as_f64().unwrap_or(0.0),
                            a[2].as_f64().unwrap_or(0.0),
                        )
                    })
                    .collect();
                scene.create(NodeKind::Line(Line::from_points(
                    &points,
                    LineBasicMaterial::new(color("color")),
                )))
            }
            Some("grid") => scene.create(NodeKind::Line(Line::grid_helper(
                num(node, "size").unwrap_or(10.0),
                num(node, "divisions").unwrap_or(10.0),
                Color::hex(num(node, "color1").unwrap_or(4473924.0)),
                Color::hex(num(node, "color2").unwrap_or(8947848.0)),
            ))),
            Some("instanced") => {
                let count = num(node, "count").expect("count") as usize;
                let geometry = geometry(node.get("geometry").expect("geometry"));
                let mut mesh = Mesh::instanced(geometry, material(node.get("material")), count);
                let instances = node
                    .get("instances")
                    .and_then(Json::as_array)
                    .unwrap_or_default();
                let data = mesh.instances.as_mut().expect("instances");
                for i in 0..count {
                    let mut dummy = Object3D::new();
                    match instances.get(i) {
                        Some(instance) => place(&mut dummy, instance),
                        None => dummy.position = Vector3::new(1e6, 1e6, 1e6),
                    }
                    dummy.update_matrix();
                    data.set_matrix_at(i, &dummy.matrix);
                }
                let id = scene.create(NodeKind::Mesh(mesh));
                scene.node_mut(id).frustum_culled = false;
                id
            }
            Some("hemisphere") => {
                let id = scene.add_hemisphere_light(
                    parent,
                    color("color"),
                    color("groundColor"),
                    intensity,
                );
                scene.graph.remove(parent, id);
                id
            }
            Some("point") => {
                let id = scene.add_point_light(parent, color("color"), intensity);
                scene.graph.remove(parent, id);
                if let NodeKind::PointLight(light) = &mut scene.node_mut(id).kind {
                    light.distance = num(node, "distance").unwrap_or(0.0);
                    light.decay = num(node, "decay").unwrap_or(2.0);
                }
                id
            }
            Some("spot") => {
                let (light, target) = scene.create_spot_light(
                    color("color"),
                    intensity,
                    num(node, "distance").unwrap_or(0.0),
                    num(node, "angle").unwrap_or(std::f64::consts::PI / 3.0),
                    num(node, "penumbra").unwrap_or(0.0),
                    num(node, "decay").unwrap_or(2.0),
                );
                scene.graph.object_mut(target).position = vec3(node, "target").expect("target");
                if let NodeKind::SpotLight(spot) = &mut scene.node_mut(light).kind {
                    spot.cast_shadow = flag(node, "castShadow").unwrap_or(false);
                    if let Some(shadow) = node.get("shadow") {
                        if let Some(size) = num(shadow, "mapSize") {
                            spot.shadow.map_size = [size, size];
                        }
                        if let Some(bias) = num(shadow, "bias") {
                            spot.shadow.bias = bias;
                        }
                    }
                }
                spot_target = Some(target);
                light
            }
            other => panic!("unknown node type {other:?}"),
        };
        place(scene.graph.object_mut(id), node);
        if let Some(Json::Bool(visible)) = node.get("visible") {
            scene.node_mut(id).visible = *visible;
        }
        if let Some(order) = num(node, "renderOrder") {
            scene.node_mut(id).render_order = order;
        }
        if let Some(culled) = flag(node, "frustumCulled") {
            scene.node_mut(id).frustum_culled = culled;
        }
        if let Some(mesh) = scene.mesh_mut(id) {
            mesh.cast_shadow = flag(node, "castShadow").unwrap_or(false);
            mesh.receive_shadow = flag(node, "receiveShadow").unwrap_or(false);
        }
        scene.graph.add(parent, id);
        if let Some(target) = spot_target {
            scene.graph.add(parent, target);
        }
        if let Some(name) = text(node, "name") {
            named.insert(name, id);
        }
    }
    let camera_spec = spec.get("camera").expect("camera");
    let mut camera = PerspectiveCamera::new(num(spec, "fov").expect("fov"), 2.0, 0.15, 1000.0);
    camera.object.position = vec3(camera_spec, "position").expect("camera position");
    camera.look_at(vec3(camera_spec, "target").expect("camera target"));
    (scene, camera)
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(scenes_dir), Some(out_dir)) = (args.first(), args.get(1)) else {
        return Err("usage: field3d_scenes <scenes dir> <out dir> [<scene>...]".into());
    };
    let only = &args[2..];
    let mut names: Vec<String> = std::fs::read_dir(scenes_dir)
        .map_err(|e| format!("{scenes_dir}: {e}"))?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".json").map(str::to_owned))
        .filter(|n| only.is_empty() || only.contains(n))
        .collect();
    names.sort();
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let mut renderer = Renderer::new().map_err(|e| e.to_string())?;
    eprintln!(
        "adapter: {} ({:?})",
        renderer.adapter_info().name,
        renderer.adapter_info().backend
    );
    for name in names {
        let path = Path::new(scenes_dir).join(format!("{name}.json"));
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let spec = json::parse(&text).map_err(|e| e.to_string())?;
        let size = spec.get("size").and_then(Json::as_array).expect("size");
        let (w, h) = (
            size[0].as_f64().unwrap_or(0.0),
            size[1].as_f64().unwrap_or(0.0),
        );
        let antialias = flag(&spec, "antialias").unwrap_or(false);
        for scale in [1.0, 2.0] {
            let (mut scene, mut camera) = build(&spec);
            camera.aspect = w / h;
            camera.update_projection_matrix();
            let options = FrameOptions {
                shadow_map: flag(&spec, "shadows").unwrap_or(false),
                ..FrameOptions::new(w, h, scale, antialias)
            };
            for dark in [true, false] {
                let background = if dark { "#222222" } else { "#ffffff" };
                scene.background = Some(Color::style(&background.into()));
                let frame = build_frame(&mut scene, &mut camera, options);
                let mut pixels = Vec::new();
                renderer.render(&frame, &mut pixels);
                swap_red_blue(&mut pixels);
                let image = Rgba {
                    width: options.width,
                    height: options.height,
                    data: pixels,
                };
                let theme = if dark { "dark" } else { "light" };
                let file = Path::new(out_dir).join(format!("{name}@{scale}x-{theme}.png"));
                pixel::write_png(&file, &image)?;
                if dark {
                    let edges = edge_mask(&renderer.render_aux(&frame));
                    let mask = Rgba {
                        width: options.width,
                        height: options.height,
                        data: edges
                            .iter()
                            .flat_map(|&e| if e { [255; 4] } else { [0, 0, 0, 255] })
                            .collect(),
                    };
                    pixel::write_png(
                        &Path::new(out_dir).join(format!("{name}@{scale}x.edges.png")),
                        &mask,
                    )?;
                }
            }
        }
        eprintln!("  {name}");
    }
    Ok(())
}
