use asrs_js::math;

use super::math_utils::{DEG2RAD, RAD2DEG};
use super::{Matrix4, Object3D, Quaternion, Vector3};

/// A parentless three `PerspectiveCamera` (upstream never adds its camera to the scene).
#[derive(Clone, Debug, PartialEq)]
pub struct PerspectiveCamera {
    pub object: Object3D,
    pub fov: f64,
    pub zoom: f64,
    pub near: f64,
    pub far: f64,
    pub aspect: f64,
    pub projection_matrix: Matrix4,
    pub projection_matrix_inverse: Matrix4,
    pub matrix_world_inverse: Matrix4,
}

impl PerspectiveCamera {
    pub fn new(fov: f64, aspect: f64, near: f64, far: f64) -> Self {
        let mut object = Object3D::new();
        object.is_camera_or_light = true;
        let mut camera = PerspectiveCamera {
            object,
            fov,
            zoom: 1.0,
            near,
            far,
            aspect,
            projection_matrix: Matrix4::IDENTITY,
            projection_matrix_inverse: Matrix4::IDENTITY,
            matrix_world_inverse: Matrix4::IDENTITY,
        };
        camera.update_projection_matrix();
        camera
    }

    pub fn effective_fov(&self) -> f64 {
        RAD2DEG * 2.0 * math::atan(math::tan(DEG2RAD * 0.5 * self.fov) / self.zoom)
    }

    pub fn update_projection_matrix(&mut self) {
        self.projection_matrix =
            Matrix4::perspective_camera(self.fov, self.aspect, self.near, self.far, self.zoom);
        self.projection_matrix_inverse = self.projection_matrix.invert();
    }

    pub fn update_matrix(&mut self) {
        self.object.update_matrix();
    }

    /// `updateMatrixWorld(force)`, which for a camera also refreshes `matrixWorldInverse`.
    pub fn update_matrix_world(&mut self, force: bool) {
        let object = &mut self.object;
        if object.matrix_auto_update {
            object.update_matrix();
        }
        if object.matrix_world_needs_update || force {
            if object.matrix_world_auto_update {
                object.matrix_world = object.matrix;
            }
            object.matrix_world_needs_update = false;
        }
        self.matrix_world_inverse = self.object.matrix_world.invert();
    }

    /// `updateWorldMatrix(updateParents, updateChildren)`; unlike `updateMatrixWorld` it leaves
    /// `matrixWorldNeedsUpdate` set.
    pub fn update_world_matrix(&mut self) {
        let object = &mut self.object;
        if object.matrix_auto_update {
            object.update_matrix();
        }
        if object.matrix_world_auto_update {
            object.matrix_world = object.matrix;
        }
        self.matrix_world_inverse = self.object.matrix_world.invert();
    }

    /// `lookAt(target)`: refreshes the world matrix from the current (stale) quaternion first,
    /// then rotates so the camera's -z axis points at `target`.
    pub fn look_at(&mut self, target: Vector3) {
        self.update_world_matrix();
        let position = Vector3::from_matrix_position(&self.object.matrix_world);
        let m1 = Matrix4::IDENTITY.look_at(position, target, self.object.up);
        self.object
            .set_quaternion(Quaternion::from_rotation_matrix(&m1));
    }
}
