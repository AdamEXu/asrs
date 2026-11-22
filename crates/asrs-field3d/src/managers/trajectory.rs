use std::sync::Arc;

use super::{ManagerContext, ManagerError, ObjectManager, TrajectoryObj};
use crate::render::{FatLine, LineMaterial, LineSegmentsGeometry, NodeKind, Scene};
use crate::three::{Color, ObjectId};

/// `objectManagers/TrajectoryManager.ts`: one `Line2` through the poses. Upstream swaps in a new
/// `LineGeometry` when the pose count changes; `setPositions` replaces every buffer either way,
/// so here each update is a new geometry.
pub struct TrajectoryManager {
    root: ObjectId,
    line: ObjectId,
}

impl TrajectoryManager {
    pub fn new(ctx: ManagerContext, scene: &mut Scene) -> Self {
        let line = FatLine {
            geometry: LineSegmentsGeometry::from_segments(Vec::new()),
            material: Arc::new(LineMaterial::new(Color::hex(f64::from(0xff8c00)), 2.0)),
        };
        let line = scene.add(ctx.root, NodeKind::FatLine(line));
        TrajectoryManager {
            root: ctx.root,
            line,
        }
    }

    pub fn line(&self) -> ObjectId {
        self.line
    }
}

impl ObjectManager for TrajectoryManager {
    type Object = TrajectoryObj;

    fn dispose(&mut self, scene: &mut Scene) {
        scene.graph.remove(self.root, self.line);
    }

    fn set_object_data(
        &mut self,
        scene: &mut Scene,
        object: &TrajectoryObj,
    ) -> Result<(), ManagerError> {
        let node = scene.node_mut(self.line);
        if object.poses.len() <= 1 {
            node.visible = false;
            return Ok(());
        }
        node.visible = true;
        let NodeKind::FatLine(line) = &mut node.kind else {
            unreachable!("the trajectory node is a fat line");
        };
        let material = Arc::make_mut(&mut line.material);
        material.color = Color::style(&object.color);
        material.linewidth = if object.size.as_string().as_deref() == Some("bold") {
            6.0
        } else {
            2.0
        };
        let mut position_data = Vec::with_capacity(object.poses.len() * 3);
        for annotated in &object.poses {
            let mut translation = annotated.pose.translation;
            if annotated.annotation.is_2d_source {
                translation[2] = 0.02;
            }
            position_data.extend(translation);
        }
        line.geometry = LineSegmentsGeometry::from_points(&position_data);
        Ok(())
    }
}
