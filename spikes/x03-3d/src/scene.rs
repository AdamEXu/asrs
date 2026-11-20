use std::f64::consts::PI;
use std::path::Path;

use glam::{DMat3, DMat4, DQuat, DVec3};

pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Option<Vec<[u8; 4]>>,
    pub indices: Option<Vec<u32>>,
}

impl Mesh {
    pub fn bounding_center(&self) -> DVec3 {
        let mut min = DVec3::splat(f64::INFINITY);
        let mut max = DVec3::splat(f64::NEG_INFINITY);
        for p in &self.positions {
            let v = DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64);
            min = min.min(v);
            max = max.max(v);
        }
        (min + max) * 0.5
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Front,
    Back,
    Double,
}

#[derive(Clone, Copy, Debug)]
pub struct Material {
    pub color: [f32; 3],
    pub vertex_colors: bool,
    pub side: Side,
    pub transparent: bool,
    pub opacity: f32,
    pub depth_write: bool,
    pub specular: [f32; 3],
    pub shininess: f32,
}

impl Material {
    pub fn phong(hex: u32) -> Self {
        Material {
            color: hex_linear(hex),
            vertex_colors: false,
            side: Side::Front,
            transparent: false,
            opacity: 1.0,
            depth_write: true,
            specular: hex_linear(0x111111),
            shininess: 30.0,
        }
    }
}

pub struct Item {
    pub mesh: usize,
    pub model: DMat4,
    pub material: Material,
}

pub struct Scene {
    pub meshes: Vec<Mesh>,
    pub items: Vec<Item>,
    pub hemi_sky: [f32; 3],
    pub hemi_ground: [f32; 3],
    pub point_pos: DVec3,
    pub point_color: [f32; 3],
}

pub fn srgb_to_linear(c: f64) -> f64 {
    if c < 0.04045 {
        c * 0.0773993808
    } else {
        (c * 0.9478672986 + 0.0521327014).powf(2.4)
    }
}

pub fn hex_linear(hex: u32) -> [f32; 3] {
    let c = |s: u32| srgb_to_linear(((hex >> s) & 255) as f64 / 255.0) as f32;
    [c(16), c(8), c(0)]
}

fn deg(d: f64) -> f64 {
    d / (180.0 / PI)
}

pub fn rotation_sequence(steps: &[(char, f64)]) -> DQuat {
    let mut q = DQuat::IDENTITY;
    for &(axis, degrees) in steps {
        let a = match axis {
            'x' => DVec3::X,
            'y' => DVec3::Y,
            _ => DVec3::Z,
        };
        q = DQuat::from_axis_angle(a, deg(degrees)) * q;
    }
    q
}

pub fn wpilib_rotation() -> DQuat {
    rotation_sequence(&[('x', -90.0), ('y', 180.0)])
}

fn inches(v: f64) -> f64 {
    v / (1.0 / 0.0254)
}

fn feet(v: f64) -> f64 {
    v / (1.0 / (0.0254 * 12.0))
}

pub fn plane(width: f64, height: f64) -> Mesh {
    let (wh, hh) = (width / 2.0, height / 2.0);
    let mut positions = Vec::new();
    for iy in 0..2 {
        let y = iy as f64 * height - hh;
        for ix in 0..2 {
            let x = ix as f64 * width - wh;
            positions.push([x as f32, (-y) as f32, 0.0]);
        }
    }
    Mesh {
        normals: vec![[0.0, 0.0, 1.0]; 4],
        positions,
        colors: None,
        indices: Some(vec![0, 2, 1, 2, 3, 1]),
    }
}

pub fn cylinder(r_top: f64, r_bottom: f64, height: f64, radial: u32) -> Mesh {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut indices = Vec::new();
    let half = height / 2.0;
    let slope = (r_bottom - r_top) / height;
    let mut rows = Vec::new();
    let mut index = 0u32;
    for y in 0..=1 {
        let v = y as f64;
        let radius = v * (r_bottom - r_top) + r_top;
        let mut row = Vec::new();
        for x in 0..=radial {
            let theta = x as f64 / radial as f64 * (PI * 2.0);
            let (s, c) = theta.sin_cos();
            positions.push([(radius * s) as f32, (-v * height + half) as f32, (radius * c) as f32]);
            let n = DVec3::new(s, slope, c).normalize();
            normals.push([n.x as f32, n.y as f32, n.z as f32]);
            row.push(index);
            index += 1;
        }
        rows.push(row);
    }
    for x in 0..radial as usize {
        let (a, b, c, d) = (rows[0][x], rows[1][x], rows[1][x + 1], rows[0][x + 1]);
        indices.extend_from_slice(&[a, b, d, b, c, d]);
    }
    for top in [true, false] {
        let radius = if top { r_top } else { r_bottom };
        let sign = if top { 1.0 } else { -1.0 };
        let center_start = index;
        for _ in 1..=radial {
            positions.push([0.0, (half * sign) as f32, 0.0]);
            normals.push([0.0, sign as f32, 0.0]);
            index += 1;
        }
        let center_end = index;
        for x in 0..=radial {
            let theta = x as f64 / radial as f64 * (PI * 2.0);
            let (s, c) = theta.sin_cos();
            positions.push([(radius * s) as f32, (half * sign) as f32, (radius * c) as f32]);
            normals.push([0.0, sign as f32, 0.0]);
            index += 1;
        }
        for x in 0..radial {
            let c = center_start + x;
            let i = center_end + x;
            if top {
                indices.extend_from_slice(&[i, i + 1, c]);
            } else {
                indices.extend_from_slice(&[i + 1, i, c]);
            }
        }
    }
    Mesh {
        positions,
        normals,
        colors: None,
        indices: Some(indices),
    }
}

struct Obj {
    q: DQuat,
    p: DVec3,
}

impl Obj {
    fn new() -> Self {
        Obj {
            q: DQuat::IDENTITY,
            p: DVec3::ZERO,
        }
    }
    fn rot_x(mut self, a: f64) -> Self {
        self.q *= DQuat::from_rotation_x(a);
        self
    }
    fn rot_y(mut self, a: f64) -> Self {
        self.q *= DQuat::from_rotation_y(a);
        self
    }
    fn rot_z(mut self, a: f64) -> Self {
        self.q *= DQuat::from_rotation_z(a);
        self
    }
    fn at(mut self, x: f64, y: f64, z: f64) -> Self {
        self.p = DVec3::new(x, y, z);
        self
    }
    fn matrix(&self) -> DMat4 {
        DMat4::from_rotation_translation(self.q, self.p)
    }
}

pub fn evergreen_frc(scene: &mut Scene, parent: DMat4) {
    let blue = 0x6379a6;
    let red = 0xa66363;
    let len = feet(54.0);
    let wid = feet(27.0);
    let add = |scene: &mut Scene, mesh: Mesh, obj: Obj, mut mat: Material, carpet: bool| {
        if !carpet {
            mat.specular = [0.0; 3];
            mat.shininess = 0.0;
        }
        scene.meshes.push(mesh);
        scene.items.push(Item {
            mesh: scene.meshes.len() - 1,
            model: parent * obj.matrix(),
            material: mat,
        });
    };
    let double = |hex| Material {
        side: Side::Double,
        ..Material::phong(hex)
    };
    let glass = |hex| Material {
        side: Side::Double,
        opacity: 0.25,
        transparent: true,
        ..Material::phong(hex)
    };

    add(scene, plane(len + 4.0, wid + 1.0), Obj::new(), double(0x888888), true);

    let tube = 0.02;
    let gh = inches(20.0);
    for y in [-wid / 2.0, wid / 2.0] {
        for z in [0.0, gh] {
            add(scene, cylinder(tube, tube, len, 12), Obj::new().rot_z(PI / 2.0).at(0.0, y, z), Material::phong(0xdddddd), false);
        }
        add(scene, plane(len, gh), Obj::new().rot_x(PI / 2.0).at(0.0, y, gh / 2.0), glass(0xffffff), false);
        let mut x = -len / 2.0;
        while x <= len / 2.0 {
            if x != -len / 2.0 {
                add(scene, cylinder(tube, tube, gh, 12), Obj::new().rot_x(PI / 2.0).at(x, y, gh / 2.0), Material::phong(0xdddddd), false);
            }
            x += len / 16.0;
        }
    }

    let asw = inches(69.0);
    let ash = inches(78.0);
    let assh = inches(36.75);
    let shelf = inches(12.25);
    let filler = (wid - asw * 3.0) / 2.0;
    for x in [-len / 2.0, len / 2.0] {
        let color = if x < 0.0 { red } else { blue };
        for z in [0.0, assh, ash] {
            let l = if z == assh { asw * 3.0 } else { wid };
            add(scene, cylinder(0.02, 0.02, l, 12), Obj::new().at(x, 0.0, z), Material::phong(0xdddddd), false);
        }
        for y in [-wid / 2.0, asw * -1.5, asw * -0.5, asw * 0.5, asw * 1.5, wid / 2.0] {
            add(scene, cylinder(tube, tube, ash, 12), Obj::new().rot_x(PI / 2.0).at(x, y, ash / 2.0), Material::phong(0xdddddd), false);
        }
        for y in [-wid / 2.0 + filler / 2.0, wid / 2.0 - filler / 2.0] {
            add(scene, plane(ash, filler), Obj::new().rot_y(PI / 2.0).at(x, y, ash / 2.0), double(color), false);
        }
        add(scene, plane(assh, asw * 3.0), Obj::new().rot_y(PI / 2.0).at(x, 0.0, assh / 2.0), double(color), false);
        add(
            scene,
            plane(ash - assh, asw * 3.0),
            Obj::new().rot_y(PI / 2.0).at(x, 0.0, assh + (ash - assh) / 2.0),
            glass(color),
            false,
        );
        let sx = x + (shelf / 2.0) * if x > 0.0 { 1.0 } else { -1.0 };
        add(scene, plane(shelf, asw * 3.0), Obj::new().at(sx, 0.0, assh), double(color), false);
    }
}

struct Geo {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Option<Vec<[u8; 4]>>,
    indices: Option<Vec<u32>>,
}

fn to_uint8(v: f64) -> u8 {
    if !v.is_finite() {
        return 0;
    }
    (v.trunc().rem_euclid(256.0)) as u8
}

pub struct RobotStats {
    pub primitives: usize,
    pub kept: usize,
    pub normal_vertices: usize,
    pub transparent_vertices: usize,
}

pub fn load_robot(path: &Path, rotations: &[(char, f64)], position: DVec3, max_radius: f64) -> anyhow::Result<(Vec<(Mesh, bool)>, RobotStats)> {
    let (doc, buffers, _) = gltf::import(path)?;
    let scene = doc.default_scene().or_else(|| doc.scenes().next()).unwrap();
    let root = DMat4::from_rotation_translation(rotation_sequence(rotations), position);
    let mut normal = Vec::new();
    let mut transparent = Vec::new();
    let mut stats = RobotStats {
        primitives: 0,
        kept: 0,
        normal_vertices: 0,
        transparent_vertices: 0,
    };

    fn visit(
        node: gltf::Node,
        parent: DMat4,
        buffers: &[gltf::buffer::Data],
        max_radius: f64,
        normal: &mut Vec<Geo>,
        transparent: &mut Vec<Geo>,
        stats: &mut RobotStats,
    ) {
        let local = DMat4::from_cols_array_2d(&node.transform().matrix().map(|c| c.map(|v| v as f64)));
        let world = parent * local;
        if let Some(mesh) = node.mesh() {
            let normal_matrix = DMat3::from_mat4(world).inverse().transpose();
            let name = node.name().unwrap_or("").to_string() + mesh.name().unwrap_or("");
            for prim in mesh.primitives() {
                if prim.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                stats.primitives += 1;
                let reader = prim.reader(|b| Some(&buffers[b.index()]));
                let (Some(pos), Some(nrm)) = (reader.read_positions(), reader.read_normals()) else {
                    continue;
                };
                let positions: Vec<[f32; 3]> = pos
                    .map(|p| {
                        let v = world.transform_point3(DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64));
                        [v.x as f32, v.y as f32, v.z as f32]
                    })
                    .collect();
                let normals: Vec<[f32; 3]> = nrm
                    .map(|n| {
                        let v = (normal_matrix * DVec3::new(n[0] as f64, n[1] as f64, n[2] as f64)).normalize_or_zero();
                        [v.x as f32, v.y as f32, v.z as f32]
                    })
                    .collect();
                let indices = reader.read_indices().map(|i| i.into_u32().collect());
                let mat = prim.material();
                let f = mat.pbr_metallic_roughness().base_color_factor();
                let rgb = [f[0] as f64 * 255.0, f[1] as f64 * 255.0, f[2] as f64 * 255.0];
                let color = [to_uint8(rgb[0]), to_uint8(rgb[1]), to_uint8(rgb[2]), 255];
                let is_transparent = mat.alpha_mode() == gltf::material::AlphaMode::Blend && f[3] < 0.75;

                let mut center = DVec3::ZERO;
                for p in &positions {
                    center += DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64);
                }
                center *= 1.0 / positions.len() as f64;
                let max_r = positions.iter().fold(0.0f64, |prev, p| {
                    let d = DVec3::new(p[0] as f64, p[1] as f64, p[2] as f64).distance(center);
                    if d > prev { d } else { prev }
                });
                if !name.contains("NOSIMPLIFY") && max_r < max_radius {
                    continue;
                }
                stats.kept += 1;
                let geo = Geo {
                    colors: Some(vec![color; positions.len()]),
                    positions,
                    normals,
                    indices,
                };
                if is_transparent {
                    transparent.push(geo);
                } else {
                    normal.push(geo);
                }
            }
        }
        for child in node.children() {
            visit(child, world, buffers, max_radius, normal, transparent, stats);
        }
    }

    for node in scene.nodes() {
        visit(node, root, &buffers, max_radius, &mut normal, &mut transparent, &mut stats);
    }

    let mut out = Vec::new();
    if let Some(m) = merge(normal) {
        stats.normal_vertices = m.positions.len();
        out.push((m, false));
    }
    if let Some(m) = merge(transparent) {
        stats.transparent_vertices = m.positions.len();
        out.push((m, true));
    }
    Ok((out, stats))
}

fn merge(group: Vec<Geo>) -> Option<Mesh> {
    if group.is_empty() {
        return None;
    }
    let indexed = group[0].indices.is_some();
    if group.iter().any(|g| g.indices.is_some() != indexed || g.colors.is_some() != group[0].colors.is_some()) {
        return None;
    }
    let mut m = Mesh {
        positions: Vec::new(),
        normals: Vec::new(),
        colors: group[0].colors.as_ref().map(|_| Vec::new()),
        indices: indexed.then(Vec::new),
    };
    for g in group {
        let offset = m.positions.len() as u32;
        if let (Some(dst), Some(src)) = (m.indices.as_mut(), g.indices) {
            dst.extend(src.into_iter().map(|i| i + offset));
        }
        if let (Some(dst), Some(src)) = (m.colors.as_mut(), g.colors) {
            dst.extend(src);
        }
        m.positions.extend(g.positions);
        m.normals.extend(g.normals);
    }
    Some(m)
}

pub struct SceneConfig {
    pub robot_path: std::path::PathBuf,
    pub robot_translation: DVec3,
    pub robot_rotation: DQuat,
}

pub fn build_standard(cfg: &SceneConfig) -> anyhow::Result<(Scene, RobotStats)> {
    let w = DMat4::from_quat(wpilib_rotation());
    let mut scene = Scene {
        meshes: Vec::new(),
        items: Vec::new(),
        hemi_sky: [2.0; 3],
        hemi_ground: hex_linear(0x444444).map(|c| c * 2.0),
        point_pos: w.transform_point3(DVec3::new(0.0, 0.0, 10.0)),
        point_color: [0.5; 3],
    };
    evergreen_frc(&mut scene, w);
    let (robot, stats) = load_robot(&cfg.robot_path, &[('z', 90.0)], DVec3::new(0.12, 3.17, 0.0), 0.04)?;
    let pose = DMat4::from_rotation_translation(cfg.robot_rotation, cfg.robot_translation);
    for (mesh, is_transparent) in robot {
        scene.meshes.push(mesh);
        let mut material = Material {
            color: [1.0; 3],
            vertex_colors: true,
            side: Side::Double,
            transparent: false,
            opacity: 1.0,
            depth_write: true,
            specular: [0.0; 3],
            shininess: 0.0,
        };
        if is_transparent {
            material.transparent = true;
            material.opacity = 0.2;
            material.depth_write = false;
        }
        scene.items.push(Item {
            mesh: scene.meshes.len() - 1,
            model: w * pose,
            material,
        });
    }
    Ok((scene, stats))
}
