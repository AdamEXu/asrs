use super::Matrix4;

/// Column-major like three's `elements`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix3 {
    pub elements: [f64; 9],
}

impl Default for Matrix3 {
    fn default() -> Self {
        Matrix3::IDENTITY
    }
}

impl Matrix3 {
    pub const IDENTITY: Matrix3 = Matrix3 {
        elements: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    };

    /// `set`, which takes its arguments row by row.
    #[allow(clippy::too_many_arguments)]
    pub const fn from_rows(
        n11: f64,
        n12: f64,
        n13: f64,
        n21: f64,
        n22: f64,
        n23: f64,
        n31: f64,
        n32: f64,
        n33: f64,
    ) -> Self {
        Matrix3 {
            elements: [n11, n21, n31, n12, n22, n32, n13, n23, n33],
        }
    }

    pub fn from_matrix4(m: &Matrix4) -> Self {
        let me = &m.elements;
        Matrix3::from_rows(
            me[0], me[4], me[8], me[1], me[5], me[9], me[2], me[6], me[10],
        )
    }

    pub fn multiply_matrices(a: &Matrix3, b: &Matrix3) -> Self {
        let ae = &a.elements;
        let be = &b.elements;
        let (a11, a12, a13) = (ae[0], ae[3], ae[6]);
        let (a21, a22, a23) = (ae[1], ae[4], ae[7]);
        let (a31, a32, a33) = (ae[2], ae[5], ae[8]);
        let (b11, b12, b13) = (be[0], be[3], be[6]);
        let (b21, b22, b23) = (be[1], be[4], be[7]);
        let (b31, b32, b33) = (be[2], be[5], be[8]);
        let mut te = [0.0; 9];
        te[0] = a11 * b11 + a12 * b21 + a13 * b31;
        te[3] = a11 * b12 + a12 * b22 + a13 * b32;
        te[6] = a11 * b13 + a12 * b23 + a13 * b33;
        te[1] = a21 * b11 + a22 * b21 + a23 * b31;
        te[4] = a21 * b12 + a22 * b22 + a23 * b32;
        te[7] = a21 * b13 + a22 * b23 + a23 * b33;
        te[2] = a31 * b11 + a32 * b21 + a33 * b31;
        te[5] = a31 * b12 + a32 * b22 + a33 * b32;
        te[8] = a31 * b13 + a32 * b23 + a33 * b33;
        Matrix3 { elements: te }
    }

    pub fn determinant(&self) -> f64 {
        let te = &self.elements;
        let (a, b, c, d, e, f, g, h, i) = (
            te[0], te[1], te[2], te[3], te[4], te[5], te[6], te[7], te[8],
        );
        a * e * i - a * f * h - b * d * i + b * f * g + c * d * h - c * e * g
    }

    pub fn invert(&self) -> Self {
        let te = &self.elements;
        let (n11, n21, n31) = (te[0], te[1], te[2]);
        let (n12, n22, n32) = (te[3], te[4], te[5]);
        let (n13, n23, n33) = (te[6], te[7], te[8]);
        let t11 = n33 * n22 - n32 * n23;
        let t12 = n32 * n13 - n33 * n12;
        let t13 = n23 * n12 - n22 * n13;
        let det = n11 * t11 + n21 * t12 + n31 * t13;
        if det == 0.0 {
            return Matrix3 { elements: [0.0; 9] };
        }
        let det_inv = 1.0 / det;
        Matrix3 {
            elements: [
                t11 * det_inv,
                (n31 * n23 - n33 * n21) * det_inv,
                (n32 * n21 - n31 * n22) * det_inv,
                t12 * det_inv,
                (n33 * n11 - n31 * n13) * det_inv,
                (n31 * n12 - n32 * n11) * det_inv,
                t13 * det_inv,
                (n21 * n13 - n23 * n11) * det_inv,
                (n22 * n11 - n21 * n12) * det_inv,
            ],
        }
    }

    pub fn transpose(&self) -> Self {
        let mut m = self.elements;
        m.swap(1, 3);
        m.swap(2, 6);
        m.swap(5, 7);
        Matrix3 { elements: m }
    }

    pub fn normal_matrix(matrix4: &Matrix4) -> Self {
        Matrix3::from_matrix4(matrix4).invert().transpose()
    }
}
