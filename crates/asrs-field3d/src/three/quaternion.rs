use asrs_js::math;

use super::math_utils::clamp;
use super::{Euler, EulerOrder, Matrix4, Vector3};

const EPSILON: f64 = f64::EPSILON;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quaternion {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Default for Quaternion {
    fn default() -> Self {
        Quaternion::IDENTITY
    }
}

impl Quaternion {
    pub const IDENTITY: Quaternion = Quaternion::new(0.0, 0.0, 0.0, 1.0);

    pub const fn new(x: f64, y: f64, z: f64, w: f64) -> Self {
        Quaternion { x, y, z, w }
    }

    pub fn from_euler(euler: &Euler) -> Self {
        let (x, y, z) = (euler.x, euler.y, euler.z);
        let c1 = math::cos(x / 2.0);
        let c2 = math::cos(y / 2.0);
        let c3 = math::cos(z / 2.0);
        let s1 = math::sin(x / 2.0);
        let s2 = math::sin(y / 2.0);
        let s3 = math::sin(z / 2.0);
        match euler.order {
            EulerOrder::Xyz => Quaternion::new(
                s1 * c2 * c3 + c1 * s2 * s3,
                c1 * s2 * c3 - s1 * c2 * s3,
                c1 * c2 * s3 + s1 * s2 * c3,
                c1 * c2 * c3 - s1 * s2 * s3,
            ),
            EulerOrder::Yxz => Quaternion::new(
                s1 * c2 * c3 + c1 * s2 * s3,
                c1 * s2 * c3 - s1 * c2 * s3,
                c1 * c2 * s3 - s1 * s2 * c3,
                c1 * c2 * c3 + s1 * s2 * s3,
            ),
            EulerOrder::Zxy => Quaternion::new(
                s1 * c2 * c3 - c1 * s2 * s3,
                c1 * s2 * c3 + s1 * c2 * s3,
                c1 * c2 * s3 + s1 * s2 * c3,
                c1 * c2 * c3 - s1 * s2 * s3,
            ),
            EulerOrder::Zyx => Quaternion::new(
                s1 * c2 * c3 - c1 * s2 * s3,
                c1 * s2 * c3 + s1 * c2 * s3,
                c1 * c2 * s3 - s1 * s2 * c3,
                c1 * c2 * c3 + s1 * s2 * s3,
            ),
            EulerOrder::Yzx => Quaternion::new(
                s1 * c2 * c3 + c1 * s2 * s3,
                c1 * s2 * c3 + s1 * c2 * s3,
                c1 * c2 * s3 - s1 * s2 * c3,
                c1 * c2 * c3 - s1 * s2 * s3,
            ),
            EulerOrder::Xzy => Quaternion::new(
                s1 * c2 * c3 - c1 * s2 * s3,
                c1 * s2 * c3 - s1 * c2 * s3,
                c1 * c2 * s3 + s1 * s2 * c3,
                c1 * c2 * c3 + s1 * s2 * s3,
            ),
        }
    }

    pub fn from_axis_angle(axis: Vector3, angle: f64) -> Self {
        let half_angle = angle / 2.0;
        let s = math::sin(half_angle);
        Quaternion::new(axis.x * s, axis.y * s, axis.z * s, math::cos(half_angle))
    }

    pub fn from_rotation_matrix(m: &Matrix4) -> Self {
        let te = &m.elements;
        let (m11, m12, m13) = (te[0], te[4], te[8]);
        let (m21, m22, m23) = (te[1], te[5], te[9]);
        let (m31, m32, m33) = (te[2], te[6], te[10]);
        let trace = m11 + m22 + m33;
        if trace > 0.0 {
            let s = 0.5 / math::sqrt(trace + 1.0);
            Quaternion::new((m32 - m23) * s, (m13 - m31) * s, (m21 - m12) * s, 0.25 / s)
        } else if m11 > m22 && m11 > m33 {
            let s = 2.0 * math::sqrt(1.0 + m11 - m22 - m33);
            Quaternion::new(0.25 * s, (m12 + m21) / s, (m13 + m31) / s, (m32 - m23) / s)
        } else if m22 > m33 {
            let s = 2.0 * math::sqrt(1.0 + m22 - m11 - m33);
            Quaternion::new((m12 + m21) / s, 0.25 * s, (m23 + m32) / s, (m13 - m31) / s)
        } else {
            let s = 2.0 * math::sqrt(1.0 + m33 - m11 - m22);
            Quaternion::new((m13 + m31) / s, (m23 + m32) / s, 0.25 * s, (m21 - m12) / s)
        }
    }

    pub fn from_unit_vectors(from: Vector3, to: Vector3) -> Self {
        let r = from.dot(to) + 1.0;
        let q = if r < EPSILON {
            if math::abs(from.x) > math::abs(from.z) {
                Quaternion::new(-from.y, from.x, 0.0, 0.0)
            } else {
                Quaternion::new(0.0, -from.z, from.y, 0.0)
            }
        } else {
            Quaternion::new(
                from.y * to.z - from.z * to.y,
                from.z * to.x - from.x * to.z,
                from.x * to.y - from.y * to.x,
                r,
            )
        };
        q.normalize()
    }

    pub fn conjugate(self) -> Self {
        Quaternion::new(-self.x, -self.y, -self.z, self.w)
    }

    pub fn invert(self) -> Self {
        self.conjugate()
    }

    pub fn dot(self, v: Quaternion) -> f64 {
        self.x * v.x + self.y * v.y + self.z * v.z + self.w * v.w
    }

    pub fn length_sq(self) -> f64 {
        self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w
    }

    pub fn length(self) -> f64 {
        math::sqrt(self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w)
    }

    pub fn normalize(self) -> Self {
        let l = self.length();
        if l == 0.0 {
            Quaternion::IDENTITY
        } else {
            let l = 1.0 / l;
            Quaternion::new(self.x * l, self.y * l, self.z * l, self.w * l)
        }
    }

    pub fn multiply(self, q: Quaternion) -> Self {
        Quaternion::multiply_quaternions(self, q)
    }

    pub fn premultiply(self, q: Quaternion) -> Self {
        Quaternion::multiply_quaternions(q, self)
    }

    pub fn multiply_quaternions(a: Quaternion, b: Quaternion) -> Self {
        let (qax, qay, qaz, qaw) = (a.x, a.y, a.z, a.w);
        let (qbx, qby, qbz, qbw) = (b.x, b.y, b.z, b.w);
        Quaternion::new(
            qax * qbw + qaw * qbx + qay * qbz - qaz * qby,
            qay * qbw + qaw * qby + qaz * qbx - qax * qbz,
            qaz * qbw + qaw * qbz + qax * qby - qay * qbx,
            qaw * qbw - qax * qbx - qay * qby - qaz * qbz,
        )
    }

    pub fn angle_to(self, q: Quaternion) -> f64 {
        2.0 * math::acos(math::abs(clamp(self.dot(q), -1.0, 1.0)))
    }

    pub fn rotate_towards(self, q: Quaternion, step: f64) -> Self {
        let angle = self.angle_to(q);
        if angle == 0.0 {
            return self;
        }
        let t = math::min2(1.0, step / angle);
        self.slerp(q, t)
    }

    #[allow(clippy::float_cmp)]
    pub fn slerp(self, qb: Quaternion, t: f64) -> Self {
        if t == 0.0 {
            return self;
        }
        if t == 1.0 {
            return qb;
        }
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        let mut cos_half_theta = w * qb.w + x * qb.x + y * qb.y + z * qb.z;
        let mut out = if cos_half_theta < 0.0 {
            cos_half_theta = -cos_half_theta;
            Quaternion::new(-qb.x, -qb.y, -qb.z, -qb.w)
        } else {
            qb
        };
        if cos_half_theta >= 1.0 {
            return self;
        }
        let sqr_sin_half_theta = 1.0 - cos_half_theta * cos_half_theta;
        if sqr_sin_half_theta <= EPSILON {
            let s = 1.0 - t;
            out.w = s * w + t * out.w;
            out.x = s * x + t * out.x;
            out.y = s * y + t * out.y;
            out.z = s * z + t * out.z;
            return out.normalize();
        }
        let sin_half_theta = math::sqrt(sqr_sin_half_theta);
        let half_theta = math::atan2(sin_half_theta, cos_half_theta);
        let ratio_a = math::sin((1.0 - t) * half_theta) / sin_half_theta;
        let ratio_b = math::sin(t * half_theta) / sin_half_theta;
        Quaternion::new(
            x * ratio_a + out.x * ratio_b,
            y * ratio_a + out.y * ratio_b,
            z * ratio_a + out.z * ratio_b,
            w * ratio_a + out.w * ratio_b,
        )
    }

    pub fn to_array(self) -> [f64; 4] {
        [self.x, self.y, self.z, self.w]
    }
}
