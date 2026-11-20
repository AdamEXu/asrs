use std::ops::{Add, Neg, Sub};

use asrs_js::math;

use super::math_utils::clamp;
use super::{Euler, Matrix3, Matrix4, Quaternion};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vector3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vector3 {
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Vector3 { x, y, z }
    }

    pub fn add_scalar(self, s: f64) -> Self {
        Vector3::new(self.x + s, self.y + s, self.z + s)
    }

    pub fn add_scaled_vector(self, v: Vector3, s: f64) -> Self {
        Vector3::new(self.x + v.x * s, self.y + v.y * s, self.z + v.z * s)
    }

    pub fn multiply(self, v: Vector3) -> Self {
        Vector3::new(self.x * v.x, self.y * v.y, self.z * v.z)
    }

    pub fn multiply_scalar(self, s: f64) -> Self {
        Vector3::new(self.x * s, self.y * s, self.z * s)
    }

    pub fn divide_scalar(self, s: f64) -> Self {
        self.multiply_scalar(1.0 / s)
    }

    pub fn min(self, v: Vector3) -> Self {
        Vector3::new(
            math::min2(self.x, v.x),
            math::min2(self.y, v.y),
            math::min2(self.z, v.z),
        )
    }

    pub fn max(self, v: Vector3) -> Self {
        Vector3::new(
            math::max2(self.x, v.x),
            math::max2(self.y, v.y),
            math::max2(self.z, v.z),
        )
    }

    pub fn dot(self, v: Vector3) -> f64 {
        self.x * v.x + self.y * v.y + self.z * v.z
    }

    pub fn length_sq(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z
    }

    pub fn length(self) -> f64 {
        math::sqrt(self.x * self.x + self.y * self.y + self.z * self.z)
    }

    pub fn normalize(self) -> Self {
        let length = self.length();
        self.divide_scalar(if length == 0.0 || length.is_nan() {
            1.0
        } else {
            length
        })
    }

    pub fn set_length(self, length: f64) -> Self {
        self.normalize().multiply_scalar(length)
    }

    pub fn lerp(self, v: Vector3, alpha: f64) -> Self {
        Vector3::new(
            self.x + (v.x - self.x) * alpha,
            self.y + (v.y - self.y) * alpha,
            self.z + (v.z - self.z) * alpha,
        )
    }

    pub fn lerp_vectors(v1: Vector3, v2: Vector3, alpha: f64) -> Self {
        Vector3::new(
            v1.x + (v2.x - v1.x) * alpha,
            v1.y + (v2.y - v1.y) * alpha,
            v1.z + (v2.z - v1.z) * alpha,
        )
    }

    pub fn cross(self, b: Vector3) -> Self {
        Vector3::new(
            self.y * b.z - self.z * b.y,
            self.z * b.x - self.x * b.z,
            self.x * b.y - self.y * b.x,
        )
    }

    pub fn distance_to_squared(self, v: Vector3) -> f64 {
        let (dx, dy, dz) = (self.x - v.x, self.y - v.y, self.z - v.z);
        dx * dx + dy * dy + dz * dz
    }

    pub fn distance_to(self, v: Vector3) -> f64 {
        math::sqrt(self.distance_to_squared(v))
    }

    pub fn angle_to(self, v: Vector3) -> f64 {
        let denominator = math::sqrt(self.length_sq() * v.length_sq());
        if denominator == 0.0 {
            return math::PI / 2.0;
        }
        let theta = self.dot(v) / denominator;
        math::acos(clamp(theta, -1.0, 1.0))
    }

    pub fn apply_matrix3(self, m: &Matrix3) -> Self {
        let (x, y, z) = (self.x, self.y, self.z);
        let e = &m.elements;
        Vector3::new(
            e[0] * x + e[3] * y + e[6] * z,
            e[1] * x + e[4] * y + e[7] * z,
            e[2] * x + e[5] * y + e[8] * z,
        )
    }

    pub fn apply_normal_matrix(self, m: &Matrix3) -> Self {
        self.apply_matrix3(m).normalize()
    }

    pub fn apply_matrix4(self, m: &Matrix4) -> Self {
        let (x, y, z) = (self.x, self.y, self.z);
        let e = &m.elements;
        let w = 1.0 / (e[3] * x + e[7] * y + e[11] * z + e[15]);
        Vector3::new(
            (e[0] * x + e[4] * y + e[8] * z + e[12]) * w,
            (e[1] * x + e[5] * y + e[9] * z + e[13]) * w,
            (e[2] * x + e[6] * y + e[10] * z + e[14]) * w,
        )
    }

    pub fn apply_quaternion(self, q: &Quaternion) -> Self {
        let (vx, vy, vz) = (self.x, self.y, self.z);
        let (qx, qy, qz, qw) = (q.x, q.y, q.z, q.w);
        let tx = 2.0 * (qy * vz - qz * vy);
        let ty = 2.0 * (qz * vx - qx * vz);
        let tz = 2.0 * (qx * vy - qy * vx);
        Vector3::new(
            vx + qw * tx + qy * tz - qz * ty,
            vy + qw * ty + qz * tx - qx * tz,
            vz + qw * tz + qx * ty - qy * tx,
        )
    }

    pub fn apply_euler(self, euler: &Euler) -> Self {
        self.apply_quaternion(&Quaternion::from_euler(euler))
    }

    pub fn apply_axis_angle(self, axis: Vector3, angle: f64) -> Self {
        self.apply_quaternion(&Quaternion::from_axis_angle(axis, angle))
    }

    pub fn transform_direction(self, m: &Matrix4) -> Self {
        let (x, y, z) = (self.x, self.y, self.z);
        let e = &m.elements;
        Vector3::new(
            e[0] * x + e[4] * y + e[8] * z,
            e[1] * x + e[5] * y + e[9] * z,
            e[2] * x + e[6] * y + e[10] * z,
        )
        .normalize()
    }

    pub fn from_matrix_position(m: &Matrix4) -> Self {
        Vector3::new(m.elements[12], m.elements[13], m.elements[14])
    }

    pub fn from_matrix_column(m: &Matrix4, index: usize) -> Self {
        let o = index * 4;
        Vector3::new(m.elements[o], m.elements[o + 1], m.elements[o + 2])
    }

    pub fn from_matrix_scale(m: &Matrix4) -> Self {
        Vector3::new(
            Vector3::from_matrix_column(m, 0).length(),
            Vector3::from_matrix_column(m, 1).length(),
            Vector3::from_matrix_column(m, 2).length(),
        )
    }

    pub fn from_spherical_coords(radius: f64, phi: f64, theta: f64) -> Self {
        let sin_phi_radius = math::sin(phi) * radius;
        Vector3::new(
            sin_phi_radius * math::sin(theta),
            math::cos(phi) * radius,
            sin_phi_radius * math::cos(theta),
        )
    }

    pub fn to_array(self) -> [f64; 3] {
        [self.x, self.y, self.z]
    }
}

impl Add for Vector3 {
    type Output = Vector3;

    fn add(self, v: Vector3) -> Vector3 {
        Vector3::new(self.x + v.x, self.y + v.y, self.z + v.z)
    }
}

impl Sub for Vector3 {
    type Output = Vector3;

    fn sub(self, v: Vector3) -> Vector3 {
        Vector3::new(self.x - v.x, self.y - v.y, self.z - v.z)
    }
}

impl Neg for Vector3 {
    type Output = Vector3;

    fn neg(self) -> Vector3 {
        Vector3::new(-self.x, -self.y, -self.z)
    }
}
