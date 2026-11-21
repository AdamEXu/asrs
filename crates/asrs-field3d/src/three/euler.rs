// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

use asrs_js::math;

use super::math_utils::clamp;
use super::{Matrix4, Quaternion};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EulerOrder {
    #[default]
    Xyz,
    Yxz,
    Zxy,
    Zyx,
    Yzx,
    Xzy,
}

impl EulerOrder {
    pub fn name(self) -> &'static str {
        match self {
            EulerOrder::Xyz => "XYZ",
            EulerOrder::Yxz => "YXZ",
            EulerOrder::Zxy => "ZXY",
            EulerOrder::Zyx => "ZYX",
            EulerOrder::Yzx => "YZX",
            EulerOrder::Xzy => "XZY",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [
            EulerOrder::Xyz,
            EulerOrder::Yxz,
            EulerOrder::Zxy,
            EulerOrder::Zyx,
            EulerOrder::Yzx,
            EulerOrder::Xzy,
        ]
        .into_iter()
        .find(|o| o.name() == name)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Euler {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub order: EulerOrder,
}

impl Euler {
    pub const fn new(x: f64, y: f64, z: f64, order: EulerOrder) -> Self {
        Euler { x, y, z, order }
    }

    pub fn from_rotation_matrix(m: &Matrix4, order: EulerOrder) -> Self {
        let te = &m.elements;
        let (m11, m12, m13) = (te[0], te[4], te[8]);
        let (m21, m22, m23) = (te[1], te[5], te[9]);
        let (m31, m32, m33) = (te[2], te[6], te[10]);
        let regular = |v: f64| math::abs(v) < 0.9999999;
        let (x, y, z) = match order {
            EulerOrder::Xyz => {
                let y = math::asin(clamp(m13, -1.0, 1.0));
                if regular(m13) {
                    (math::atan2(-m23, m33), y, math::atan2(-m12, m11))
                } else {
                    (math::atan2(m32, m22), y, 0.0)
                }
            }
            EulerOrder::Yxz => {
                let x = math::asin(-clamp(m23, -1.0, 1.0));
                if regular(m23) {
                    (x, math::atan2(m13, m33), math::atan2(m21, m22))
                } else {
                    (x, math::atan2(-m31, m11), 0.0)
                }
            }
            EulerOrder::Zxy => {
                let x = math::asin(clamp(m32, -1.0, 1.0));
                if regular(m32) {
                    (x, math::atan2(-m31, m33), math::atan2(-m12, m22))
                } else {
                    (x, 0.0, math::atan2(m21, m11))
                }
            }
            EulerOrder::Zyx => {
                let y = math::asin(-clamp(m31, -1.0, 1.0));
                if regular(m31) {
                    (math::atan2(m32, m33), y, math::atan2(m21, m11))
                } else {
                    (0.0, y, math::atan2(-m12, m22))
                }
            }
            EulerOrder::Yzx => {
                let z = math::asin(clamp(m21, -1.0, 1.0));
                if regular(m21) {
                    (math::atan2(-m23, m22), math::atan2(-m31, m11), z)
                } else {
                    (0.0, math::atan2(m13, m33), z)
                }
            }
            EulerOrder::Xzy => {
                let z = math::asin(-clamp(m12, -1.0, 1.0));
                if regular(m12) {
                    (math::atan2(m32, m22), math::atan2(m13, m11), z)
                } else {
                    (math::atan2(-m23, m33), 0.0, z)
                }
            }
        };
        Euler::new(x, y, z, order)
    }

    pub fn from_quaternion(q: &Quaternion, order: EulerOrder) -> Self {
        Euler::from_rotation_matrix(&Matrix4::make_rotation_from_quaternion(q), order)
    }

    pub fn reorder(self, order: EulerOrder) -> Self {
        Euler::from_quaternion(&Quaternion::from_euler(&self), order)
    }
}
