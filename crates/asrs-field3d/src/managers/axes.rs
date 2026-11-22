use std::sync::Arc;

use super::{
    AxesObj, InstanceSource, ManagerContext, ManagerError, ObjectManager, ResizableInstancedMesh,
    poses,
};
use crate::fields::{make_axes_template, to_scene_graph};
use crate::glb::optimize_geometries;
use crate::render::{Geometry, PhongMaterial, Scene, Side};
use crate::three::Vector3;

/// `objectManagers/AxesManager.ts`.
pub struct AxesManager {
    instances: Option<ResizableInstancedMesh>,
}

impl AxesManager {
    pub fn new(ctx: ManagerContext) -> Self {
        let mut scene = Scene::new();
        let template =
            make_axes_template(&mut scene, ctx.material_specular, ctx.material_shininess);
        let (mut graph, axes) = to_scene_graph(&scene, template);
        graph.object_mut(axes).scale = Vector3::new(0.25, 0.25, 0.25);
        let merged = optimize_geometries(
            &mut graph,
            axes,
            ctx.mode,
            ctx.material_specular,
            ctx.material_shininess,
            false,
            false,
            None,
        )
        .ok()
        .and_then(|result| result.normal.into_iter().next());
        let instances = merged.map(|mesh| {
            let material = PhongMaterial {
                vertex_colors: true,
                side: Side::Double,
                specular: ctx.material_specular,
                shininess: ctx.material_shininess,
                ..PhongMaterial::new()
            };
            let source = InstanceSource {
                geometry: Geometry::new(mesh.geometry),
                material: Arc::new(material).into(),
            };
            ResizableInstancedMesh::new(ctx.root, vec![source], None)
        });
        AxesManager { instances }
    }

    pub fn instances(&self) -> Option<&ResizableInstancedMesh> {
        self.instances.as_ref()
    }
}

impl ObjectManager for AxesManager {
    type Object = AxesObj;

    fn dispose(&mut self, scene: &mut Scene) {
        if let Some(instances) = &self.instances {
            instances.dispose(scene);
        }
    }

    fn set_object_data(&mut self, scene: &mut Scene, object: &AxesObj) -> Result<(), ManagerError> {
        if let Some(instances) = &mut self.instances {
            instances.set_poses(scene, &poses(&object.poses));
        }
        Ok(())
    }
}
