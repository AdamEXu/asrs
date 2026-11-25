// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

use std::sync::Arc;

use asrs_js::math;

use super::{Matrix3, Matrix4, Vector3};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArrayKind {
    Float32,
    Float64,
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
            ArrayKind::Float64 => "Float64Array",
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
    Float64(Vec<f64>),
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
            ArrayKind::Float64 => ArrayData::Float64(vec![0.0; len]),
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
            ArrayData::Float64(_) => ArrayKind::Float64,
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
            ArrayData::Float64(a) => a.len(),
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
            ArrayData::Float64(a) => a[i],
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
            ArrayData::Float64(a) => a[i] = v,
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
            (ArrayData::Float64(d), ArrayData::Float64(s)) => copy(d, s, offset),
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
        ArrayKind::Float32 | ArrayKind::Float64 => value,
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
        ArrayKind::Float32 | ArrayKind::Float64 => value,
        ArrayKind::Uint32 => math::round(value * 4294967295.0),
        ArrayKind::Uint16 => math::round(value * 65535.0),
        ArrayKind::Uint8 => math::round(value * 255.0),
        ArrayKind::Int32 => math::round(value * 2147483647.0),
        ArrayKind::Int16 => math::round(value * 32767.0),
        ArrayKind::Int8 => math::round(value * 127.0),
    }
}

/// A `BufferAttribute`, holding its own tuples. An `InterleavedBufferAttribute` is stored
/// de-interleaved with `interleaved` set: it reads and writes the same values, and it lacks the
/// `gpuType` property, which makes `mergeAttributes` refuse to mix it with plain attributes.
#[derive(Clone, Debug, PartialEq)]
pub struct BufferAttribute {
    /// Shared by the geometries of every primitive that uses the accessor, until one writes.
    pub array: Arc<ArrayData>,
    pub item_size: usize,
    pub normalized: bool,
    pub interleaved: bool,
}

fn get_component(
    array: &ArrayData,
    item_size: usize,
    normalized: bool,
    index: usize,
    component: usize,
) -> f64 {
    let v = array.get(index * item_size + component);
    if normalized {
        denormalize(v, array.kind())
    } else {
        v
    }
}

fn set_component(
    array: &mut ArrayData,
    item_size: usize,
    normalized: bool,
    index: usize,
    component: usize,
    value: f64,
) {
    let value = if normalized {
        normalize(value, array.kind())
    } else {
        value
    };
    array.set(index * item_size + component, value);
}

impl BufferAttribute {
    pub fn new(array: ArrayData, item_size: usize, normalized: bool) -> Self {
        BufferAttribute {
            array: Arc::new(array),
            item_size,
            normalized,
            interleaved: false,
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
        get_component(
            &self.array,
            self.item_size,
            self.normalized,
            index,
            component,
        )
    }

    pub fn set_component(&mut self, index: usize, component: usize, value: f64) {
        let (item_size, normalized) = (self.item_size, self.normalized);
        let array = Arc::make_mut(&mut self.array);
        set_component(array, item_size, normalized, index, component, value);
    }

    /// Replaces the x, y and z of every item.
    fn map_xyz(&mut self, f: impl Fn(Vector3) -> Vector3) {
        let count = self.count();
        let (item_size, normalized) = (self.item_size, self.normalized);
        let array = Arc::make_mut(&mut self.array);
        for i in 0..count {
            let [x, y, z] = [0, 1, 2].map(|c| get_component(array, item_size, normalized, i, c));
            let v = f(Vector3::new(x, y, z));
            for (c, value) in [v.x, v.y, v.z].into_iter().enumerate() {
                set_component(array, item_size, normalized, i, c, value);
            }
        }
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
            self.map_xyz(|v| v.apply_matrix3(m));
        }
    }

    pub fn apply_matrix4(&mut self, m: &Matrix4) {
        self.map_xyz(|v| v.apply_matrix4(m));
    }

    pub fn apply_normal_matrix(&mut self, m: &Matrix3) {
        self.map_xyz(|v| v.apply_normal_matrix(m));
    }

    pub fn transform_direction(&mut self, m: &Matrix4) {
        self.map_xyz(|v| v.transform_direction(m));
    }
}
