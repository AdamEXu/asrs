// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

use super::{BufferAttribute, Vector3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box3 {
    pub min: Vector3,
    pub max: Vector3,
}

impl Default for Box3 {
    fn default() -> Self {
        Box3::EMPTY
    }
}

impl Box3 {
    pub const EMPTY: Box3 = Box3 {
        min: Vector3::new(f64::INFINITY, f64::INFINITY, f64::INFINITY),
        max: Vector3::new(f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY),
    };

    pub fn from_buffer_attribute(attribute: &BufferAttribute) -> Self {
        let mut b = Box3::EMPTY;
        for i in 0..attribute.count() {
            b = b.expand_by_point(attribute.get_xyz(i));
        }
        b
    }

    pub fn expand_by_point(self, point: Vector3) -> Self {
        Box3 {
            min: self.min.min(point),
            max: self.max.max(point),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.max.x < self.min.x || self.max.y < self.min.y || self.max.z < self.min.z
    }

    pub fn center(&self) -> Vector3 {
        if self.is_empty() {
            Vector3::default()
        } else {
            (self.min + self.max).multiply_scalar(0.5)
        }
    }

    pub fn size(&self) -> Vector3 {
        if self.is_empty() {
            Vector3::default()
        } else {
            self.max - self.min
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sphere {
    pub center: Vector3,
    pub radius: f64,
}

impl Default for Sphere {
    fn default() -> Self {
        Sphere {
            center: Vector3::default(),
            radius: -1.0,
        }
    }
}
