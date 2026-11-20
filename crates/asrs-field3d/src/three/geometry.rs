use asrs_js::math;

use super::{
    ArrayData, ArrayKind, Box3, BufferAttribute, Matrix3, Matrix4, Quaternion, Sphere, Vector3,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Group {
    pub start: usize,
    pub count: usize,
    pub material_index: usize,
}

/// A `BufferGeometry` with plain attributes and no morph targets. Attributes keep JS property
/// order (insertion order), which is the order `mergeGeometries` walks them in.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BufferGeometry {
    pub index: Option<BufferAttribute>,
    attributes: Vec<(String, BufferAttribute)>,
    pub groups: Vec<Group>,
    pub bounding_box: Option<Box3>,
    pub bounding_sphere: Option<Sphere>,
}

impl BufferGeometry {
    pub fn new() -> Self {
        BufferGeometry::default()
    }

    /// `setIndex(array)`: a `Uint32Array` once any index reaches 65535, else `Uint16Array`.
    pub fn set_index(&mut self, indices: &[f64]) {
        let kind = if indices.iter().any(|&i| i >= 65535.0) {
            ArrayKind::Uint32
        } else {
            ArrayKind::Uint16
        };
        let mut array = ArrayData::zeroed(kind, indices.len());
        for (i, &v) in indices.iter().enumerate() {
            array.set(i, v);
        }
        self.index = Some(BufferAttribute::new(array, 1, false));
    }

    pub fn attributes(&self) -> impl Iterator<Item = (&str, &BufferAttribute)> {
        self.attributes.iter().map(|(n, a)| (n.as_str(), a))
    }

    pub fn attribute_names(&self) -> impl Iterator<Item = &str> {
        self.attributes.iter().map(|(n, _)| n.as_str())
    }

    pub fn attribute(&self, name: &str) -> Option<&BufferAttribute> {
        self.attributes
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, a)| a)
    }

    pub fn attribute_mut(&mut self, name: &str) -> Option<&mut BufferAttribute> {
        self.attributes
            .iter_mut()
            .find(|(n, _)| n == name)
            .map(|(_, a)| a)
    }

    pub fn has_attribute(&self, name: &str) -> bool {
        self.attribute(name).is_some()
    }

    /// Replacing an attribute keeps its position, as assigning an existing property does.
    pub fn set_attribute(&mut self, name: &str, attribute: BufferAttribute) {
        match self.attribute_mut(name) {
            Some(slot) => *slot = attribute,
            None => self.attributes.push((name.to_owned(), attribute)),
        }
    }

    pub fn delete_attribute(&mut self, name: &str) {
        self.attributes.retain(|(n, _)| n != name);
    }

    pub fn add_group(&mut self, start: usize, count: usize, material_index: usize) {
        self.groups.push(Group {
            start,
            count,
            material_index,
        });
    }

    pub fn apply_matrix4(&mut self, matrix: &Matrix4) -> &mut Self {
        if let Some(position) = self.attribute_mut("position") {
            position.apply_matrix4(matrix);
        }
        if let Some(normal) = self.attribute_mut("normal") {
            normal.apply_normal_matrix(&Matrix3::normal_matrix(matrix));
        }
        if let Some(tangent) = self.attribute_mut("tangent") {
            tangent.transform_direction(matrix);
        }
        if self.bounding_box.is_some() {
            self.compute_bounding_box();
        }
        if self.bounding_sphere.is_some() {
            self.compute_bounding_sphere();
        }
        self
    }

    pub fn apply_quaternion(&mut self, q: &Quaternion) -> &mut Self {
        self.apply_matrix4(&Matrix4::make_rotation_from_quaternion(q))
    }

    pub fn rotate_x(&mut self, angle: f64) -> &mut Self {
        self.apply_matrix4(&Matrix4::make_rotation_x(angle))
    }

    pub fn rotate_y(&mut self, angle: f64) -> &mut Self {
        self.apply_matrix4(&Matrix4::make_rotation_y(angle))
    }

    pub fn rotate_z(&mut self, angle: f64) -> &mut Self {
        self.apply_matrix4(&Matrix4::make_rotation_z(angle))
    }

    pub fn translate(&mut self, x: f64, y: f64, z: f64) -> &mut Self {
        self.apply_matrix4(&Matrix4::make_translation(x, y, z))
    }

    pub fn scale(&mut self, x: f64, y: f64, z: f64) -> &mut Self {
        self.apply_matrix4(&Matrix4::make_scale(x, y, z))
    }

    /// `setFromPoints`. Its `point.z || 0` turns NaN and `-0` into `+0`.
    pub fn set_from_points(&mut self, points: &[Vector3]) -> &mut Self {
        let mut position = Vec::with_capacity(points.len() * 3);
        for p in points {
            let z = if p.z == 0.0 || p.z.is_nan() { 0.0 } else { p.z };
            position.extend_from_slice(&[p.x, p.y, z]);
        }
        self.set_attribute("position", BufferAttribute::float32(&position, 3));
        self
    }

    pub fn compute_bounding_box(&mut self) {
        self.bounding_box = Some(match self.attribute("position") {
            Some(position) => Box3::from_buffer_attribute(position),
            None => Box3::EMPTY,
        });
    }

    pub fn compute_bounding_sphere(&mut self) {
        let Some(position) = self.attribute("position") else {
            self.bounding_sphere.get_or_insert_with(Sphere::default);
            return;
        };
        let center = Box3::from_buffer_attribute(position).center();
        let mut max_radius_sq = 0.0;
        for i in 0..position.count() {
            max_radius_sq = math::max2(
                max_radius_sq,
                center.distance_to_squared(position.get_xyz(i)),
            );
        }
        self.bounding_sphere = Some(Sphere {
            center,
            radius: math::sqrt(max_radius_sq),
        });
    }
}
