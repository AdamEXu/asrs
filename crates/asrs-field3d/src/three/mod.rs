//! The three.js 0.168 subset upstream's 3D field uses: math types, `Object3D` transforms,
//! `BufferGeometry` with typed-array attributes, the primitive generators and
//! `BufferGeometryUtils.mergeGeometries`.
//!
//! Every computation follows three's operation order in `f64` and stores into the same typed
//! array kinds, with V8's `Math` functions from `asrs_js::math`, so attribute arrays come out
//! bit-identical to three's.

#![deny(clippy::disallowed_methods)]

mod attribute;
mod box3;
mod camera;
mod color;
mod euler;
mod geometry;
pub mod math_utils;
mod matrix3;
mod matrix4;
mod merge;
mod object3d;
pub mod primitives;
mod quaternion;
mod spherical;
mod vector3;

pub use attribute::{ArrayData, ArrayKind, BufferAttribute};
pub use box3::{Box3, Sphere};
pub use camera::PerspectiveCamera;
pub use color::Color;
pub use euler::{Euler, EulerOrder};
pub use geometry::{BufferGeometry, Group};
pub use matrix3::Matrix3;
pub use matrix4::Matrix4;
pub use merge::{MergeError, merge_attributes, merge_geometries};
pub use object3d::{Graph, Node, Object3D, ObjectId};
pub use quaternion::Quaternion;
pub use spherical::Spherical;
pub use vector3::Vector3;
