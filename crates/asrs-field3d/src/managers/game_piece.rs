use std::collections::HashMap;
use std::sync::Arc;

use asrs_js::string::JsString;

use super::{
    GamePieceObj, InstanceSource, ManagerContext, ManagerError, ObjectManager,
    ResizableInstancedMesh, poses,
};
use crate::render::{Geometry, PhongMaterial, Scene};
use crate::three::BufferGeometry;

/// The field's game piece meshes by name (`fieldPieces`, a plain object upstream).
pub type GamePieces = HashMap<JsString, InstanceSource>;

const OBJECT_PROTOTYPE_KEYS: [&str; 12] = [
    "constructor",
    "__defineGetter__",
    "__defineSetter__",
    "hasOwnProperty",
    "__lookupGetter__",
    "__lookupSetter__",
    "isPrototypeOf",
    "propertyIsEnumerable",
    "toString",
    "valueOf",
    "__proto__",
    "toLocaleString",
];

/// `variant in gamePieces`, which also finds `Object.prototype`'s members. Their value has no
/// `geometry` or `material`, so `new InstancedMesh(undefined, undefined, n)` falls back to an
/// empty geometry: nothing draws. (Upstream's fallback material is a `MeshBasicMaterial`; with no
/// vertices its type is unobservable.)
fn lookup(pieces: &GamePieces, variant: &JsString) -> Option<InstanceSource> {
    if let Some(source) = pieces.get(variant) {
        return Some(source.clone());
    }
    let inherited = variant
        .as_string()
        .is_some_and(|name| OBJECT_PROTOTYPE_KEYS.contains(&name.as_str()));
    inherited.then(|| InstanceSource {
        geometry: Geometry::new(BufferGeometry::new()),
        material: Arc::new(PhongMaterial::new()).into(),
    })
}

/// `objectManagers/GamePieceManager.ts`.
pub struct GamePieceManager {
    ctx: ManagerContext,
    game_pieces: Arc<GamePieces>,
    instances: Option<ResizableInstancedMesh>,
    last_variant: JsString,
}

impl GamePieceManager {
    pub fn new(ctx: ManagerContext, game_pieces: Arc<GamePieces>) -> Self {
        GamePieceManager {
            ctx,
            game_pieces,
            instances: None,
            last_variant: JsString::new(),
        }
    }

    pub fn instances(&self) -> Option<&ResizableInstancedMesh> {
        self.instances.as_ref()
    }
}

impl ObjectManager for GamePieceManager {
    type Object = GamePieceObj;

    fn dispose(&mut self, scene: &mut Scene) {
        if let Some(instances) = &self.instances {
            instances.dispose(scene);
        }
    }

    fn set_object_data(
        &mut self,
        scene: &mut Scene,
        object: &GamePieceObj,
    ) -> Result<(), ManagerError> {
        if object.variant != self.last_variant || self.instances.is_none() {
            self.last_variant = object.variant.clone();
            if let Some(instances) = &self.instances {
                instances.dispose(scene);
            }
            if let Some(source) = lookup(&self.game_pieces, &object.variant) {
                self.instances = Some(ResizableInstancedMesh::new(
                    self.ctx.root,
                    vec![source],
                    None,
                ));
            }
        }
        if let Some(instances) = &mut self.instances {
            instances.set_poses(scene, &poses(&object.poses));
        }
        Ok(())
    }
}
