use std::sync::Arc;

use asrs_js::math;
use asrs_js::string::JsString;

use super::{
    AxesObj, InstanceSource, ManagerContext, ManagerError, ObjectManager, ResizableInstancedMesh,
    poses,
};
use crate::glb::{
    MeshMaterial, ObjectKind, SceneGraph, SceneObject, SourceMaterial, optimize_geometries,
};
use crate::render::{Geometry, PhongMaterial, Scene, Side};
use crate::three::primitives::{cylinder_geometry, sphere_geometry};
use crate::three::{BufferGeometry, Color, ObjectId, Vector3};

/// `field3d/AxesTemplate.ts` as the scene graph `optimizeGeometries` reads: a white centre sphere
/// and red, green and blue unit cylinders along x, y and z.
fn axes_template(graph: &mut SceneGraph) -> ObjectId {
    const RADIUS: f64 = 0.02;
    let axes = graph.create("", SceneObject::new(JsString::new(), ObjectKind::Object3D));
    let mut add =
        |geometry: BufferGeometry, hex: u32, place: &dyn Fn(&mut crate::three::Object3D)| {
            let material = SourceMaterial {
                color: Some(Color::hex(f64::from(hex)).to_array()),
                transparent: false,
                opacity: 1.0,
            };
            let kind = ObjectKind::Mesh {
                geometry: Arc::new(geometry),
                material: MeshMaterial::Single(Arc::new(material)),
            };
            let id = graph.create("", SceneObject::new(JsString::new(), kind));
            place(graph.object_mut(id));
            graph.add(axes, id);
        };
    let tau = math::PI * 2.0;
    let cylinder = || cylinder_geometry(RADIUS, RADIUS, 1.0, 8.0, 1.0, false, 0.0, tau);
    add(
        sphere_geometry(RADIUS, 8.0, 4.0, 0.0, tau, 0.0, math::PI),
        0xffffff,
        &|_| {},
    );
    add(cylinder(), 0xff0000, &|o| {
        o.position = Vector3::new(0.5, 0.0, 0.0);
        o.rotate_z(math::PI / 2.0);
    });
    add(cylinder(), 0x00ff00, &|o| {
        o.position = Vector3::new(0.0, 0.5, 0.0);
    });
    add(cylinder(), 0x2020ff, &|o| {
        o.position = Vector3::new(0.0, 0.0, 0.5);
        o.rotate_x(math::PI / 2.0);
    });
    axes
}

/// `objectManagers/AxesManager.ts`.
pub struct AxesManager {
    instances: Option<ResizableInstancedMesh>,
}

impl AxesManager {
    pub fn new(ctx: ManagerContext) -> Self {
        let mut graph = SceneGraph::new();
        let axes = axes_template(&mut graph);
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
