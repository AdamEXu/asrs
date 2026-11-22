// Ported from AdvantageScope (BSD-3-Clause, Copyright 2021-2026 Littleton Robotics) and three.js
// r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

//! `field3d/ObjectManager.ts`, `ResizableInstancedMesh.ts` and the object managers that turn one
//! `Field3dRendererCommand_AnyObj` into scene objects under a root group.

#![deny(clippy::disallowed_methods)]

mod april_tag;
mod axes;
mod canvas_texture;
mod cone;
mod game_piece;
mod heatmap;
mod instanced;
mod robot;
mod trajectory;

use asrs_js::string::JsString;
use asrs_log::geometry::{AnnotatedPose3d, Rotation3d};
use asrs_model::preferences::Field3dMode;

use crate::render::Scene;
use crate::three::{Color, ObjectId, Quaternion};

pub use april_tag::{AprilTagManager, TextureRequest, april_tag_texture_path, decode_png_texture};
pub use axes::AxesManager;
pub use canvas_texture::CanvasTexture;
pub use cone::ConeManager;
pub use game_piece::{GamePieceManager, GamePieces};
pub use heatmap::{HeatmapField, HeatmapManager};
pub use instanced::{InstanceSource, ResizableInstancedMesh};
pub use robot::{
    RobotKind, RobotLoadRequest, RobotManager, RobotMechanisms, RobotObj, SwerveModuleSet,
};
pub use trajectory::TrajectoryManager;

/// `rotation3dToQuaternion` from `Field3dRendererImpl.ts`: WPILib's `[w, x, y, z]` as a three
/// quaternion, unnormalized.
pub fn rotation3d_to_quaternion(input: &Rotation3d) -> Quaternion {
    Quaternion::new(input[1], input[2], input[3], input[0])
}

/// `quaternionToRotation3d` from `Field3dRendererImpl.ts`.
pub fn quaternion_to_rotation3d(input: &Quaternion) -> Rotation3d {
    [input.w, input.x, input.y, input.z]
}

/// The constructor arguments every `ObjectManager` shares (`requestRender` excepted: the managers
/// that call it, `AprilTagManager` and `RobotManager`, leave it to whoever delivers their textures
/// or models).
#[derive(Clone, Debug)]
pub struct ManagerContext {
    pub root: ObjectId,
    pub material_specular: Color,
    pub material_shininess: f64,
    pub mode: Field3dMode,
    pub is_xr: bool,
}

/// An exception `setObjectData` threw; the rest of that call did not run.
#[derive(Clone, Debug, PartialEq)]
pub struct ManagerError(pub String);

impl std::fmt::Display for ManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ManagerError {}

pub trait ObjectManager {
    type Object;

    /// Removes the objects from the scene.
    fn dispose(&mut self, scene: &mut Scene);

    /// `setResolution`. Nothing reads it: `LineSegments2` copies the renderer viewport into its
    /// material before every draw, which `build_frame` does from `FrameOptions::viewport`.
    fn set_resolution(&mut self, _resolution: [f64; 2]) {}

    fn set_object_data(
        &mut self,
        scene: &mut Scene,
        object: &Self::Object,
    ) -> Result<(), ManagerError>;
}

/// `Field3dRendererCommand_GamePieceObj`.
#[derive(Clone, Debug, PartialEq)]
pub struct GamePieceObj {
    pub variant: JsString,
    pub poses: Vec<AnnotatedPose3d>,
}

/// `Field3dRendererCommand_AprilTagVariant`.
#[derive(Clone, Debug, PartialEq)]
pub struct AprilTagVariant {
    pub family: JsString,
    pub inches: f64,
}

/// `Field3dRendererCommand_AprilTagObj` and `_AprilTagBuiltInObj`.
#[derive(Clone, Debug, PartialEq)]
pub struct AprilTagObj {
    pub poses: Vec<AnnotatedPose3d>,
    pub variant: AprilTagVariant,
}

/// `Field3dRendererCommand_AxesObj`.
#[derive(Clone, Debug, PartialEq)]
pub struct AxesObj {
    pub poses: Vec<AnnotatedPose3d>,
}

/// `Field3dRendererCommand_ConeObj`. `position` stays a string: anything but `center`, `back`
/// and `front` has no offset, which upstream turns into a NaN translation.
#[derive(Clone, Debug, PartialEq)]
pub struct ConeObj {
    pub color: JsString,
    pub position: JsString,
    pub poses: Vec<AnnotatedPose3d>,
}

/// `Field3dRendererCommand_TrajectoryObj`.
#[derive(Clone, Debug, PartialEq)]
pub struct TrajectoryObj {
    pub color: JsString,
    pub size: JsString,
    pub poses: Vec<AnnotatedPose3d>,
}

/// `Field3dRendererCommand_HeatmapObj`.
#[derive(Clone, Debug, PartialEq)]
pub struct HeatmapObj {
    pub poses: Vec<AnnotatedPose3d>,
}

fn concat(parts: &[&JsString]) -> JsString {
    let mut out = JsString::new();
    for part in parts {
        out.push_str(part);
    }
    out
}

fn poses(objects: &[AnnotatedPose3d]) -> Vec<asrs_log::geometry::Pose3d> {
    objects.iter().map(|x| x.pose).collect()
}
