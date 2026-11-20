use asrs_js::math;

use super::{Euler, EulerOrder, Quaternion, Vector3};

/// Column-major like three's `elements`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix4 {
    pub elements: [f64; 16],
}

impl Default for Matrix4 {
    fn default() -> Self {
        Matrix4::IDENTITY
    }
}

impl Matrix4 {
    pub const IDENTITY: Matrix4 = Matrix4::from_rows([
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ]);

    /// `set`, which takes its arguments row by row.
    pub const fn from_rows(n: [f64; 16]) -> Self {
        Matrix4 {
            elements: [
                n[0], n[4], n[8], n[12], n[1], n[5], n[9], n[13], n[2], n[6], n[10], n[14], n[3],
                n[7], n[11], n[15],
            ],
        }
    }

    pub const fn from_array(elements: [f64; 16]) -> Self {
        Matrix4 { elements }
    }

    pub fn multiply(&self, m: &Matrix4) -> Self {
        Matrix4::multiply_matrices(self, m)
    }

    pub fn premultiply(&self, m: &Matrix4) -> Self {
        Matrix4::multiply_matrices(m, self)
    }

    pub fn multiply_matrices(a: &Matrix4, b: &Matrix4) -> Self {
        let ae = &a.elements;
        let be = &b.elements;
        let (a11, a12, a13, a14) = (ae[0], ae[4], ae[8], ae[12]);
        let (a21, a22, a23, a24) = (ae[1], ae[5], ae[9], ae[13]);
        let (a31, a32, a33, a34) = (ae[2], ae[6], ae[10], ae[14]);
        let (a41, a42, a43, a44) = (ae[3], ae[7], ae[11], ae[15]);
        let (b11, b12, b13, b14) = (be[0], be[4], be[8], be[12]);
        let (b21, b22, b23, b24) = (be[1], be[5], be[9], be[13]);
        let (b31, b32, b33, b34) = (be[2], be[6], be[10], be[14]);
        let (b41, b42, b43, b44) = (be[3], be[7], be[11], be[15]);
        let mut te = [0.0; 16];
        te[0] = a11 * b11 + a12 * b21 + a13 * b31 + a14 * b41;
        te[4] = a11 * b12 + a12 * b22 + a13 * b32 + a14 * b42;
        te[8] = a11 * b13 + a12 * b23 + a13 * b33 + a14 * b43;
        te[12] = a11 * b14 + a12 * b24 + a13 * b34 + a14 * b44;
        te[1] = a21 * b11 + a22 * b21 + a23 * b31 + a24 * b41;
        te[5] = a21 * b12 + a22 * b22 + a23 * b32 + a24 * b42;
        te[9] = a21 * b13 + a22 * b23 + a23 * b33 + a24 * b43;
        te[13] = a21 * b14 + a22 * b24 + a23 * b34 + a24 * b44;
        te[2] = a31 * b11 + a32 * b21 + a33 * b31 + a34 * b41;
        te[6] = a31 * b12 + a32 * b22 + a33 * b32 + a34 * b42;
        te[10] = a31 * b13 + a32 * b23 + a33 * b33 + a34 * b43;
        te[14] = a31 * b14 + a32 * b24 + a33 * b34 + a34 * b44;
        te[3] = a41 * b11 + a42 * b21 + a43 * b31 + a44 * b41;
        te[7] = a41 * b12 + a42 * b22 + a43 * b32 + a44 * b42;
        te[11] = a41 * b13 + a42 * b23 + a43 * b33 + a44 * b43;
        te[15] = a41 * b14 + a42 * b24 + a43 * b34 + a44 * b44;
        Matrix4 { elements: te }
    }

    pub fn multiply_scalar(&self, s: f64) -> Self {
        Matrix4 {
            elements: self.elements.map(|e| e * s),
        }
    }

    pub fn determinant(&self) -> f64 {
        let te = &self.elements;
        let (n11, n12, n13, n14) = (te[0], te[4], te[8], te[12]);
        let (n21, n22, n23, n24) = (te[1], te[5], te[9], te[13]);
        let (n31, n32, n33, n34) = (te[2], te[6], te[10], te[14]);
        let (n41, n42, n43, n44) = (te[3], te[7], te[11], te[15]);
        n41 * (n14 * n23 * n32 - n13 * n24 * n32 - n14 * n22 * n33
            + n12 * n24 * n33
            + n13 * n22 * n34
            - n12 * n23 * n34)
            + n42
                * (n11 * n23 * n34 - n11 * n24 * n33 + n14 * n21 * n33 - n13 * n21 * n34
                    + n13 * n24 * n31
                    - n14 * n23 * n31)
            + n43
                * (n11 * n24 * n32 - n11 * n22 * n34 - n14 * n21 * n32
                    + n12 * n21 * n34
                    + n14 * n22 * n31
                    - n12 * n24 * n31)
            + n44
                * (-n13 * n22 * n31 - n11 * n23 * n32 + n11 * n22 * n33 + n13 * n21 * n32
                    - n12 * n21 * n33
                    + n12 * n23 * n31)
    }

    pub fn transpose(&self) -> Self {
        let mut te = self.elements;
        te.swap(1, 4);
        te.swap(2, 8);
        te.swap(6, 9);
        te.swap(3, 12);
        te.swap(7, 13);
        te.swap(11, 14);
        Matrix4 { elements: te }
    }

    pub fn set_position(&self, v: Vector3) -> Self {
        let mut te = self.elements;
        te[12] = v.x;
        te[13] = v.y;
        te[14] = v.z;
        Matrix4 { elements: te }
    }

    pub fn invert(&self) -> Self {
        let te = &self.elements;
        let (n11, n21, n31, n41) = (te[0], te[1], te[2], te[3]);
        let (n12, n22, n32, n42) = (te[4], te[5], te[6], te[7]);
        let (n13, n23, n33, n43) = (te[8], te[9], te[10], te[11]);
        let (n14, n24, n34, n44) = (te[12], te[13], te[14], te[15]);
        let t11 =
            n23 * n34 * n42 - n24 * n33 * n42 + n24 * n32 * n43 - n22 * n34 * n43 - n23 * n32 * n44
                + n22 * n33 * n44;
        let t12 =
            n14 * n33 * n42 - n13 * n34 * n42 - n14 * n32 * n43 + n12 * n34 * n43 + n13 * n32 * n44
                - n12 * n33 * n44;
        let t13 =
            n13 * n24 * n42 - n14 * n23 * n42 + n14 * n22 * n43 - n12 * n24 * n43 - n13 * n22 * n44
                + n12 * n23 * n44;
        let t14 =
            n14 * n23 * n32 - n13 * n24 * n32 - n14 * n22 * n33 + n12 * n24 * n33 + n13 * n22 * n34
                - n12 * n23 * n34;
        let det = n11 * t11 + n21 * t12 + n31 * t13 + n41 * t14;
        if det == 0.0 {
            return Matrix4 {
                elements: [0.0; 16],
            };
        }
        let det_inv = 1.0 / det;
        let mut out = [0.0; 16];
        out[0] = t11 * det_inv;
        out[1] = (n24 * n33 * n41 - n23 * n34 * n41 - n24 * n31 * n43
            + n21 * n34 * n43
            + n23 * n31 * n44
            - n21 * n33 * n44)
            * det_inv;
        out[2] = (n22 * n34 * n41 - n24 * n32 * n41 + n24 * n31 * n42
            - n21 * n34 * n42
            - n22 * n31 * n44
            + n21 * n32 * n44)
            * det_inv;
        out[3] = (n23 * n32 * n41 - n22 * n33 * n41 - n23 * n31 * n42
            + n21 * n33 * n42
            + n22 * n31 * n43
            - n21 * n32 * n43)
            * det_inv;
        out[4] = t12 * det_inv;
        out[5] = (n13 * n34 * n41 - n14 * n33 * n41 + n14 * n31 * n43
            - n11 * n34 * n43
            - n13 * n31 * n44
            + n11 * n33 * n44)
            * det_inv;
        out[6] = (n14 * n32 * n41 - n12 * n34 * n41 - n14 * n31 * n42
            + n11 * n34 * n42
            + n12 * n31 * n44
            - n11 * n32 * n44)
            * det_inv;
        out[7] = (n12 * n33 * n41 - n13 * n32 * n41 + n13 * n31 * n42
            - n11 * n33 * n42
            - n12 * n31 * n43
            + n11 * n32 * n43)
            * det_inv;
        out[8] = t13 * det_inv;
        out[9] = (n14 * n23 * n41 - n13 * n24 * n41 - n14 * n21 * n43
            + n11 * n24 * n43
            + n13 * n21 * n44
            - n11 * n23 * n44)
            * det_inv;
        out[10] = (n12 * n24 * n41 - n14 * n22 * n41 + n14 * n21 * n42
            - n11 * n24 * n42
            - n12 * n21 * n44
            + n11 * n22 * n44)
            * det_inv;
        out[11] = (n13 * n22 * n41 - n12 * n23 * n41 - n13 * n21 * n42
            + n11 * n23 * n42
            + n12 * n21 * n43
            - n11 * n22 * n43)
            * det_inv;
        out[12] = t14 * det_inv;
        out[13] = (n13 * n24 * n31 - n14 * n23 * n31 + n14 * n21 * n33
            - n11 * n24 * n33
            - n13 * n21 * n34
            + n11 * n23 * n34)
            * det_inv;
        out[14] = (n14 * n22 * n31 - n12 * n24 * n31 - n14 * n21 * n32
            + n11 * n24 * n32
            + n12 * n21 * n34
            - n11 * n22 * n34)
            * det_inv;
        out[15] = (n12 * n23 * n31 - n13 * n22 * n31 + n13 * n21 * n32
            - n11 * n23 * n32
            - n12 * n21 * n33
            + n11 * n22 * n33)
            * det_inv;
        Matrix4 { elements: out }
    }

    pub fn scale(&self, v: Vector3) -> Self {
        let mut te = self.elements;
        for (column, s) in [v.x, v.y, v.z].into_iter().enumerate() {
            for row in 0..4 {
                te[column * 4 + row] *= s;
            }
        }
        Matrix4 { elements: te }
    }

    pub fn max_scale_on_axis(&self) -> f64 {
        let te = &self.elements;
        let x = te[0] * te[0] + te[1] * te[1] + te[2] * te[2];
        let y = te[4] * te[4] + te[5] * te[5] + te[6] * te[6];
        let z = te[8] * te[8] + te[9] * te[9] + te[10] * te[10];
        math::sqrt(math::max(&[x, y, z]))
    }

    pub fn make_translation(x: f64, y: f64, z: f64) -> Self {
        Matrix4::from_rows([
            1.0, 0.0, 0.0, x, //
            0.0, 1.0, 0.0, y, //
            0.0, 0.0, 1.0, z, //
            0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn make_rotation_x(theta: f64) -> Self {
        let (c, s) = (math::cos(theta), math::sin(theta));
        Matrix4::from_rows([
            1.0, 0.0, 0.0, 0.0, //
            0.0, c, -s, 0.0, //
            0.0, s, c, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn make_rotation_y(theta: f64) -> Self {
        let (c, s) = (math::cos(theta), math::sin(theta));
        Matrix4::from_rows([
            c, 0.0, s, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            -s, 0.0, c, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn make_rotation_z(theta: f64) -> Self {
        let (c, s) = (math::cos(theta), math::sin(theta));
        Matrix4::from_rows([
            c, -s, 0.0, 0.0, //
            s, c, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn make_rotation_axis(axis: Vector3, angle: f64) -> Self {
        let c = math::cos(angle);
        let s = math::sin(angle);
        let t = 1.0 - c;
        let (x, y, z) = (axis.x, axis.y, axis.z);
        let (tx, ty) = (t * x, t * y);
        Matrix4::from_rows([
            tx * x + c,
            tx * y - s * z,
            tx * z + s * y,
            0.0,
            tx * y + s * z,
            ty * y + c,
            ty * z - s * x,
            0.0,
            tx * z - s * y,
            ty * z + s * x,
            t * z * z + c,
            0.0,
            0.0,
            0.0,
            0.0,
            1.0,
        ])
    }

    pub fn make_scale(x: f64, y: f64, z: f64) -> Self {
        Matrix4::from_rows([
            x, 0.0, 0.0, 0.0, //
            0.0, y, 0.0, 0.0, //
            0.0, 0.0, z, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn make_rotation_from_quaternion(q: &Quaternion) -> Self {
        Matrix4::compose(Vector3::new(0.0, 0.0, 0.0), q, Vector3::new(1.0, 1.0, 1.0))
    }

    pub fn make_rotation_from_euler(euler: &Euler) -> Self {
        let (x, y, z) = (euler.x, euler.y, euler.z);
        let (a, b) = (math::cos(x), math::sin(x));
        let (c, d) = (math::cos(y), math::sin(y));
        let (e, f) = (math::cos(z), math::sin(z));
        let mut te = Matrix4::IDENTITY.elements;
        match euler.order {
            EulerOrder::Xyz => {
                let (ae, af, be, bf) = (a * e, a * f, b * e, b * f);
                te[0] = c * e;
                te[4] = -c * f;
                te[8] = d;
                te[1] = af + be * d;
                te[5] = ae - bf * d;
                te[9] = -b * c;
                te[2] = bf - ae * d;
                te[6] = be + af * d;
                te[10] = a * c;
            }
            EulerOrder::Yxz => {
                let (ce, cf, de, df) = (c * e, c * f, d * e, d * f);
                te[0] = ce + df * b;
                te[4] = de * b - cf;
                te[8] = a * d;
                te[1] = a * f;
                te[5] = a * e;
                te[9] = -b;
                te[2] = cf * b - de;
                te[6] = df + ce * b;
                te[10] = a * c;
            }
            EulerOrder::Zxy => {
                let (ce, cf, de, df) = (c * e, c * f, d * e, d * f);
                te[0] = ce - df * b;
                te[4] = -a * f;
                te[8] = de + cf * b;
                te[1] = cf + de * b;
                te[5] = a * e;
                te[9] = df - ce * b;
                te[2] = -a * d;
                te[6] = b;
                te[10] = a * c;
            }
            EulerOrder::Zyx => {
                let (ae, af, be, bf) = (a * e, a * f, b * e, b * f);
                te[0] = c * e;
                te[4] = be * d - af;
                te[8] = ae * d + bf;
                te[1] = c * f;
                te[5] = bf * d + ae;
                te[9] = af * d - be;
                te[2] = -d;
                te[6] = b * c;
                te[10] = a * c;
            }
            EulerOrder::Yzx => {
                let (ac, ad, bc, bd) = (a * c, a * d, b * c, b * d);
                te[0] = c * e;
                te[4] = bd - ac * f;
                te[8] = bc * f + ad;
                te[1] = f;
                te[5] = a * e;
                te[9] = -b * e;
                te[2] = -d * e;
                te[6] = ad * f + bc;
                te[10] = ac - bd * f;
            }
            EulerOrder::Xzy => {
                let (ac, ad, bc, bd) = (a * c, a * d, b * c, b * d);
                te[0] = c * e;
                te[4] = -f;
                te[8] = d * e;
                te[1] = ac * f + bd;
                te[5] = a * e;
                te[9] = ad * f - bc;
                te[2] = bc * f - ad;
                te[6] = b * e;
                te[10] = bd * f + ac;
            }
        }
        Matrix4 { elements: te }
    }

    pub fn compose(position: Vector3, quaternion: &Quaternion, scale: Vector3) -> Self {
        let (x, y, z, w) = (quaternion.x, quaternion.y, quaternion.z, quaternion.w);
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let (xx, xy, xz) = (x * x2, x * y2, x * z2);
        let (yy, yz, zz) = (y * y2, y * z2, z * z2);
        let (wx, wy, wz) = (w * x2, w * y2, w * z2);
        let (sx, sy, sz) = (scale.x, scale.y, scale.z);
        Matrix4 {
            elements: [
                (1.0 - (yy + zz)) * sx,
                (xy + wz) * sx,
                (xz - wy) * sx,
                0.0,
                (xy - wz) * sy,
                (1.0 - (xx + zz)) * sy,
                (yz + wx) * sy,
                0.0,
                (xz + wy) * sz,
                (yz - wx) * sz,
                (1.0 - (xx + yy)) * sz,
                0.0,
                position.x,
                position.y,
                position.z,
                1.0,
            ],
        }
    }

    pub fn decompose(&self) -> (Vector3, Quaternion, Vector3) {
        let te = &self.elements;
        let mut sx = Vector3::new(te[0], te[1], te[2]).length();
        let sy = Vector3::new(te[4], te[5], te[6]).length();
        let sz = Vector3::new(te[8], te[9], te[10]).length();
        if self.determinant() < 0.0 {
            sx = -sx;
        }
        let position = Vector3::new(te[12], te[13], te[14]);
        let mut m1 = *self;
        let (inv_sx, inv_sy, inv_sz) = (1.0 / sx, 1.0 / sy, 1.0 / sz);
        for (i, inv) in [
            (0, inv_sx),
            (1, inv_sx),
            (2, inv_sx),
            (4, inv_sy),
            (5, inv_sy),
            (6, inv_sy),
            (8, inv_sz),
            (9, inv_sz),
            (10, inv_sz),
        ] {
            m1.elements[i] *= inv;
        }
        (
            position,
            Quaternion::from_rotation_matrix(&m1),
            Vector3::new(sx, sy, sz),
        )
    }

    pub fn extract_rotation(m: &Matrix4) -> Self {
        let me = &m.elements;
        let scale_x = 1.0 / Vector3::from_matrix_column(m, 0).length();
        let scale_y = 1.0 / Vector3::from_matrix_column(m, 1).length();
        let scale_z = 1.0 / Vector3::from_matrix_column(m, 2).length();
        Matrix4 {
            elements: [
                me[0] * scale_x,
                me[1] * scale_x,
                me[2] * scale_x,
                0.0,
                me[4] * scale_y,
                me[5] * scale_y,
                me[6] * scale_y,
                0.0,
                me[8] * scale_z,
                me[9] * scale_z,
                me[10] * scale_z,
                0.0,
                0.0,
                0.0,
                0.0,
                1.0,
            ],
        }
    }

    /// `lookAt`, which only writes the rotation part of `self`.
    pub fn look_at(&self, eye: Vector3, target: Vector3, up: Vector3) -> Self {
        let mut z = eye - target;
        if z.length_sq() == 0.0 {
            z.z = 1.0;
        }
        z = z.normalize();
        let mut x = up.cross(z);
        if x.length_sq() == 0.0 {
            if math::abs(up.z) == 1.0 {
                z.x += 0.0001;
            } else {
                z.z += 0.0001;
            }
            z = z.normalize();
            x = up.cross(z);
        }
        x = x.normalize();
        let y = z.cross(x);
        let mut te = self.elements;
        te[0] = x.x;
        te[4] = y.x;
        te[8] = z.x;
        te[1] = x.y;
        te[5] = y.y;
        te[9] = z.y;
        te[2] = x.z;
        te[6] = y.z;
        te[10] = z.z;
        Matrix4 { elements: te }
    }

    pub fn make_perspective(
        left: f64,
        right: f64,
        top: f64,
        bottom: f64,
        near: f64,
        far: f64,
    ) -> Self {
        let x = 2.0 * near / (right - left);
        let y = 2.0 * near / (top - bottom);
        let a = (right + left) / (right - left);
        let b = (top + bottom) / (top - bottom);
        let c = -(far + near) / (far - near);
        let d = (-2.0 * far * near) / (far - near);
        Matrix4::from_rows([
            x, 0.0, a, 0.0, //
            0.0, y, b, 0.0, //
            0.0, 0.0, c, d, //
            0.0, 0.0, -1.0, 0.0,
        ])
    }

    /// `PerspectiveCamera.updateProjectionMatrix` without a view offset or film offset.
    pub fn perspective_camera(fov: f64, aspect: f64, near: f64, far: f64, zoom: f64) -> Self {
        let top = near * math::tan(super::math_utils::DEG2RAD * 0.5 * fov) / zoom;
        let height = 2.0 * top;
        let width = aspect * height;
        let left = -0.5 * width;
        Matrix4::make_perspective(left, left + width, top, top - height, near, far)
    }

    pub fn make_orthographic(
        left: f64,
        right: f64,
        top: f64,
        bottom: f64,
        near: f64,
        far: f64,
    ) -> Self {
        let w = 1.0 / (right - left);
        let h = 1.0 / (top - bottom);
        let p = 1.0 / (far - near);
        let x = (right + left) * w;
        let y = (top + bottom) * h;
        let z = (far + near) * p;
        let z_inv = -2.0 * p;
        Matrix4::from_rows([
            2.0 * w,
            0.0,
            0.0,
            -x,
            0.0,
            2.0 * h,
            0.0,
            -y,
            0.0,
            0.0,
            z_inv,
            -z,
            0.0,
            0.0,
            0.0,
            1.0,
        ])
    }
}
