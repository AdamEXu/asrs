use asrs_js::math;

use super::{Matrix3, Matrix4, Vector3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrayKind {
    Float32,
    Uint8,
    Int8,
    Uint16,
    Int16,
    Uint32,
    Int32,
}

impl ArrayKind {
    /// The typed array constructor's name.
    pub fn name(self) -> &'static str {
        match self {
            ArrayKind::Float32 => "Float32Array",
            ArrayKind::Uint8 => "Uint8Array",
            ArrayKind::Int8 => "Int8Array",
            ArrayKind::Uint16 => "Uint16Array",
            ArrayKind::Int16 => "Int16Array",
            ArrayKind::Uint32 => "Uint32Array",
            ArrayKind::Int32 => "Int32Array",
        }
    }
}

/// A typed array as three holds it in `BufferAttribute.array`.
#[derive(Clone, Debug, PartialEq)]
pub enum ArrayData {
    Float32(Vec<f32>),
    Uint8(Vec<u8>),
    Int8(Vec<i8>),
    Uint16(Vec<u16>),
    Int16(Vec<i16>),
    Uint32(Vec<u32>),
    Int32(Vec<i32>),
}

impl ArrayData {
    /// `new TypedArray(length)`.
    pub fn zeroed(kind: ArrayKind, len: usize) -> Self {
        match kind {
            ArrayKind::Float32 => ArrayData::Float32(vec![0.0; len]),
            ArrayKind::Uint8 => ArrayData::Uint8(vec![0; len]),
            ArrayKind::Int8 => ArrayData::Int8(vec![0; len]),
            ArrayKind::Uint16 => ArrayData::Uint16(vec![0; len]),
            ArrayKind::Int16 => ArrayData::Int16(vec![0; len]),
            ArrayKind::Uint32 => ArrayData::Uint32(vec![0; len]),
            ArrayKind::Int32 => ArrayData::Int32(vec![0; len]),
        }
    }

    /// `new TypedArray(numbers)`, converting each element as a typed array store does.
    pub fn from_numbers(kind: ArrayKind, values: &[f64]) -> Self {
        let mut data = ArrayData::zeroed(kind, values.len());
        for (i, &v) in values.iter().enumerate() {
            data.set(i, v);
        }
        data
    }

    pub fn kind(&self) -> ArrayKind {
        match self {
            ArrayData::Float32(_) => ArrayKind::Float32,
            ArrayData::Uint8(_) => ArrayKind::Uint8,
            ArrayData::Int8(_) => ArrayKind::Int8,
            ArrayData::Uint16(_) => ArrayKind::Uint16,
            ArrayData::Int16(_) => ArrayKind::Int16,
            ArrayData::Uint32(_) => ArrayKind::Uint32,
            ArrayData::Int32(_) => ArrayKind::Int32,
        }
    }

    pub fn len(&self) -> usize {
        match self {
            ArrayData::Float32(a) => a.len(),
            ArrayData::Uint8(a) => a.len(),
            ArrayData::Int8(a) => a.len(),
            ArrayData::Uint16(a) => a.len(),
            ArrayData::Int16(a) => a.len(),
            ArrayData::Uint32(a) => a.len(),
            ArrayData::Int32(a) => a.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, i: usize) -> f64 {
        match self {
            ArrayData::Float32(a) => f64::from(a[i]),
            ArrayData::Uint8(a) => f64::from(a[i]),
            ArrayData::Int8(a) => f64::from(a[i]),
            ArrayData::Uint16(a) => f64::from(a[i]),
            ArrayData::Int16(a) => f64::from(a[i]),
            ArrayData::Uint32(a) => f64::from(a[i]),
            ArrayData::Int32(a) => f64::from(a[i]),
        }
    }

    pub fn set(&mut self, i: usize, v: f64) {
        match self {
            ArrayData::Float32(a) => a[i] = v as f32,
            ArrayData::Uint8(a) => a[i] = math::to_uint8(v),
            ArrayData::Int8(a) => a[i] = math::to_int8(v),
            ArrayData::Uint16(a) => a[i] = math::to_uint16(v),
            ArrayData::Int16(a) => a[i] = math::to_int16(v),
            ArrayData::Uint32(a) => a[i] = math::to_uint32(v),
            ArrayData::Int32(a) => a[i] = math::to_int32(v),
        }
    }

    /// `TypedArray.prototype.set(source, offset)` for a source of the same kind.
    pub fn copy_from(&mut self, source: &ArrayData, offset: usize) {
        fn copy<T: Copy>(dst: &mut [T], src: &[T], offset: usize) {
            dst[offset..offset + src.len()].copy_from_slice(src);
        }
        match (self, source) {
            (ArrayData::Float32(d), ArrayData::Float32(s)) => copy(d, s, offset),
            (ArrayData::Uint8(d), ArrayData::Uint8(s)) => copy(d, s, offset),
            (ArrayData::Int8(d), ArrayData::Int8(s)) => copy(d, s, offset),
            (ArrayData::Uint16(d), ArrayData::Uint16(s)) => copy(d, s, offset),
            (ArrayData::Int16(d), ArrayData::Int16(s)) => copy(d, s, offset),
            (ArrayData::Uint32(d), ArrayData::Uint32(s)) => copy(d, s, offset),
            (ArrayData::Int32(d), ArrayData::Int32(s)) => copy(d, s, offset),
            (d, s) => {
                for i in 0..s.len() {
                    d.set(offset + i, s.get(i));
                }
            }
        }
    }
}

/// `MathUtils.denormalize`.
fn denormalize(value: f64, kind: ArrayKind) -> f64 {
    match kind {
        ArrayKind::Float32 => value,
        ArrayKind::Uint32 => value / 4294967295.0,
        ArrayKind::Uint16 => value / 65535.0,
        ArrayKind::Uint8 => value / 255.0,
        ArrayKind::Int32 => math::max2(value / 2147483647.0, -1.0),
        ArrayKind::Int16 => math::max2(value / 32767.0, -1.0),
        ArrayKind::Int8 => math::max2(value / 127.0, -1.0),
    }
}

/// `MathUtils.normalize`.
fn normalize(value: f64, kind: ArrayKind) -> f64 {
    match kind {
        ArrayKind::Float32 => value,
        ArrayKind::Uint32 => math::round(value * 4294967295.0),
        ArrayKind::Uint16 => math::round(value * 65535.0),
        ArrayKind::Uint8 => math::round(value * 255.0),
        ArrayKind::Int32 => math::round(value * 2147483647.0),
        ArrayKind::Int16 => math::round(value * 32767.0),
        ArrayKind::Int8 => math::round(value * 127.0),
    }
}

/// A non-interleaved `BufferAttribute`.
#[derive(Clone, Debug, PartialEq)]
pub struct BufferAttribute {
    pub array: ArrayData,
    pub item_size: usize,
    pub normalized: bool,
}

impl BufferAttribute {
    pub fn new(array: ArrayData, item_size: usize, normalized: bool) -> Self {
        BufferAttribute {
            array,
            item_size,
            normalized,
        }
    }

    /// `new Float32BufferAttribute(numbers, itemSize)`.
    pub fn float32(values: &[f64], item_size: usize) -> Self {
        BufferAttribute::new(
            ArrayData::from_numbers(ArrayKind::Float32, values),
            item_size,
            false,
        )
    }

    pub fn count(&self) -> usize {
        self.array.len() / self.item_size
    }

    pub fn get_component(&self, index: usize, component: usize) -> f64 {
        let v = self.array.get(index * self.item_size + component);
        if self.normalized {
            denormalize(v, self.array.kind())
        } else {
            v
        }
    }

    pub fn set_component(&mut self, index: usize, component: usize, value: f64) {
        let value = if self.normalized {
            normalize(value, self.array.kind())
        } else {
            value
        };
        self.array.set(index * self.item_size + component, value);
    }

    pub fn get_x(&self, index: usize) -> f64 {
        self.get_component(index, 0)
    }

    pub fn get_y(&self, index: usize) -> f64 {
        self.get_component(index, 1)
    }

    pub fn get_z(&self, index: usize) -> f64 {
        self.get_component(index, 2)
    }

    pub fn get_w(&self, index: usize) -> f64 {
        self.get_component(index, 3)
    }

    /// `Vector3.fromBufferAttribute`.
    pub fn get_xyz(&self, index: usize) -> Vector3 {
        Vector3::new(self.get_x(index), self.get_y(index), self.get_z(index))
    }

    pub fn set_xyz(&mut self, index: usize, v: Vector3) {
        self.set_component(index, 0, v.x);
        self.set_component(index, 1, v.y);
        self.set_component(index, 2, v.z);
    }

    pub fn apply_matrix3(&mut self, m: &Matrix3) {
        if self.item_size == 2 {
            for i in 0..self.count() {
                let (x, y) = (self.get_x(i), self.get_y(i));
                let e = &m.elements;
                self.set_component(i, 0, e[0] * x + e[3] * y + e[6]);
                self.set_component(i, 1, e[1] * x + e[4] * y + e[7]);
            }
        } else if self.item_size == 3 {
            for i in 0..self.count() {
                let v = self.get_xyz(i).apply_matrix3(m);
                self.set_xyz(i, v);
            }
        }
    }

    pub fn apply_matrix4(&mut self, m: &Matrix4) {
        for i in 0..self.count() {
            let v = self.get_xyz(i).apply_matrix4(m);
            self.set_xyz(i, v);
        }
    }

    pub fn apply_normal_matrix(&mut self, m: &Matrix3) {
        for i in 0..self.count() {
            let v = self.get_xyz(i).apply_normal_matrix(m);
            self.set_xyz(i, v);
        }
    }

    pub fn transform_direction(&mut self, m: &Matrix4) {
        for i in 0..self.count() {
            let v = self.get_xyz(i).transform_direction(m);
            self.set_xyz(i, v);
        }
    }
}
