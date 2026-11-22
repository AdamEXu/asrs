use std::sync::Arc;

use asrs_log::geometry::Pose3d;

use super::rotation3d_to_quaternion;
use crate::render::{Geometry, Materials, Mesh, Scene};
use crate::three::{Object3D, ObjectId, Vector3};

/// The geometry and material an instanced mesh is built from.
#[derive(Clone, Debug)]
pub struct InstanceSource {
    pub geometry: Arc<Geometry>,
    pub material: Materials,
}

/// `field3d/ResizableInstancedMesh.ts`: one `InstancedMesh` per source, recreated at the next
/// power of two whenever the pose count outgrows it. Unused instances sit at `1e6`.
#[derive(Debug)]
pub struct ResizableInstancedMesh {
    parent: ObjectId,
    sources: Vec<InstanceSource>,
    cast_shadow: Vec<bool>,
    count: usize,
    dummy: Object3D,
    meshes: Vec<Option<ObjectId>>,
}

impl ResizableInstancedMesh {
    pub fn new(parent: ObjectId, sources: Vec<InstanceSource>, cast_shadow: Option<&[bool]>) -> Self {
        let cast_shadow = (0..sources.len())
            .map(|i| cast_shadow.and_then(|c| c.get(i)).copied().unwrap_or(true))
            .collect();
        ResizableInstancedMesh {
            parent,
            meshes: vec![None; sources.len()],
            sources,
            cast_shadow,
            count: 0,
            dummy: Object3D::new(),
        }
    }

    /// Detaches the meshes. They stay in `meshes`, so a later `set_poses` keeps writing to
    /// them and a resize adds new ones to the parent again, as upstream's disposed instance does.
    pub fn dispose(&self, scene: &mut Scene) {
        for mesh in self.meshes.iter().flatten() {
            scene.graph.remove(self.parent, *mesh);
        }
    }

    pub fn set_poses(&mut self, scene: &mut Scene, poses: &[Pose3d]) {
        if poses.len() > self.count {
            if self.count == 0 {
                self.count = 1;
            }
            while self.count < poses.len() {
                self.count *= 2;
            }
            for (i, slot) in self.meshes.iter_mut().enumerate() {
                if let Some(mesh) = *slot {
                    scene.graph.remove(self.parent, mesh);
                }
                let source = &self.sources[i];
                let mut mesh =
                    Mesh::instanced(source.geometry.clone(), source.material.clone(), self.count);
                mesh.cast_shadow = self.cast_shadow[i];
                let id = scene.add_mesh(self.parent, mesh);
                scene.node_mut(id).frustum_culled = false;
                *slot = Some(id);
            }
        }

        for i in 0..self.count {
            if let Some(pose) = poses.get(i) {
                let [x, y, z] = pose.translation;
                self.dummy.position = Vector3::new(x, y, z);
                self.dummy
                    .set_rotation_from_quaternion(&rotation3d_to_quaternion(&pose.rotation));
            } else {
                self.dummy.position = Vector3::new(1e6, 1e6, 1e6);
            }
            self.dummy.update_matrix();
            for mesh in self.meshes.iter().flatten() {
                if let Some(instances) = scene.mesh_mut(*mesh).and_then(|m| m.instances.as_mut()) {
                    instances.set_matrix_at(i, &self.dummy.matrix);
                }
            }
        }
    }

    /// An in-place edit of source `index`'s geometry (`geometry.translate(...)` upstream), which
    /// every mesh already built from it sees.
    pub fn set_geometry(&mut self, scene: &mut Scene, index: usize, geometry: Arc<Geometry>) {
        self.sources[index].geometry = geometry.clone();
        if let Some(mesh) = self.meshes[index].and_then(|id| scene.mesh_mut(id)) {
            mesh.geometry = geometry;
        }
    }

    /// An in-place edit of source `index`'s material, such as a new texture on it.
    pub fn set_material(&mut self, scene: &mut Scene, index: usize, material: Materials) {
        self.sources[index].material = material.clone();
        if let Some(mesh) = self.meshes[index].and_then(|id| scene.mesh_mut(id)) {
            mesh.material = material;
        }
    }

    /// The current mesh of each source, `None` before the first resize.
    pub fn meshes(&self) -> &[Option<ObjectId>] {
        &self.meshes
    }
}
