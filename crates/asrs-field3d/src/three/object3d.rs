// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

use super::{Euler, Matrix4, Quaternion, Vector3};

/// The transform state of a three `Object3D`.
///
/// three links `rotation` and `quaternion` through change callbacks: any quaternion write
/// recomputes the Euler angles, and any Euler write recomputes the quaternion. The setters
/// here make the same writes in the same order, so both stay bit-identical to three's.
#[derive(Clone, Debug, PartialEq)]
pub struct Object3D {
    pub position: Vector3,
    rotation: Euler,
    quaternion: Quaternion,
    pub scale: Vector3,
    pub up: Vector3,
    pub matrix: Matrix4,
    pub matrix_world: Matrix4,
    pub matrix_auto_update: bool,
    pub matrix_world_auto_update: bool,
    pub matrix_world_needs_update: bool,
    /// Cameras and lights look down their -z axis, so `lookAt` builds the inverse rotation.
    pub is_camera_or_light: bool,
}

impl Default for Object3D {
    fn default() -> Self {
        Object3D {
            position: Vector3::default(),
            rotation: Euler::default(),
            quaternion: Quaternion::IDENTITY,
            scale: Vector3::new(1.0, 1.0, 1.0),
            up: Vector3::new(0.0, 1.0, 0.0),
            matrix: Matrix4::IDENTITY,
            matrix_world: Matrix4::IDENTITY,
            matrix_auto_update: true,
            matrix_world_auto_update: true,
            matrix_world_needs_update: false,
            is_camera_or_light: false,
        }
    }
}

impl Object3D {
    pub fn new() -> Self {
        Object3D::default()
    }

    pub fn rotation(&self) -> Euler {
        self.rotation
    }

    pub fn quaternion(&self) -> Quaternion {
        self.quaternion
    }

    /// Any write to `quaternion` (`copy`, `set`, `multiply`, `setFromAxisAngle`, ...).
    pub fn set_quaternion(&mut self, q: Quaternion) {
        self.quaternion = q;
        self.rotation = Euler::from_quaternion(&q, self.rotation.order);
    }

    /// Any write to `rotation` (`set`, `copy`, `x = ...`, ...).
    pub fn set_rotation(&mut self, euler: Euler) {
        self.rotation = euler;
        self.quaternion = Quaternion::from_euler(&euler);
    }

    /// `rotation.setFromQuaternion(q)`: the angles come from `q`, then the quaternion is
    /// rebuilt from those angles, so it ends up as `q` round-tripped through Euler angles.
    pub fn set_rotation_from_quaternion(&mut self, q: &Quaternion) {
        self.set_rotation(Euler::from_quaternion(q, self.rotation.order));
    }

    /// `setRotationFromMatrix(m)`, which writes the quaternion.
    pub fn set_rotation_from_matrix(&mut self, m: &Matrix4) {
        self.set_quaternion(Quaternion::from_rotation_matrix(m));
    }

    pub fn set_rotation_from_axis_angle(&mut self, axis: Vector3, angle: f64) {
        self.set_quaternion(Quaternion::from_axis_angle(axis, angle));
    }

    pub fn rotate_on_axis(&mut self, axis: Vector3, angle: f64) {
        let q1 = Quaternion::from_axis_angle(axis, angle);
        self.set_quaternion(self.quaternion.multiply(q1));
    }

    pub fn rotate_on_world_axis(&mut self, axis: Vector3, angle: f64) {
        let q1 = Quaternion::from_axis_angle(axis, angle);
        self.set_quaternion(self.quaternion.premultiply(q1));
    }

    pub fn rotate_x(&mut self, angle: f64) {
        self.rotate_on_axis(Vector3::new(1.0, 0.0, 0.0), angle);
    }

    pub fn rotate_y(&mut self, angle: f64) {
        self.rotate_on_axis(Vector3::new(0.0, 1.0, 0.0), angle);
    }

    pub fn rotate_z(&mut self, angle: f64) {
        self.rotate_on_axis(Vector3::new(0.0, 0.0, 1.0), angle);
    }

    pub fn apply_quaternion(&mut self, q: &Quaternion) {
        self.set_quaternion(self.quaternion.premultiply(*q));
    }

    pub fn translate_on_axis(&mut self, axis: Vector3, distance: f64) {
        let v1 = axis.apply_quaternion(&self.quaternion);
        self.position = self.position + v1.multiply_scalar(distance);
    }

    pub fn translate_x(&mut self, distance: f64) {
        self.translate_on_axis(Vector3::new(1.0, 0.0, 0.0), distance);
    }

    pub fn translate_y(&mut self, distance: f64) {
        self.translate_on_axis(Vector3::new(0.0, 1.0, 0.0), distance);
    }

    pub fn translate_z(&mut self, distance: f64) {
        self.translate_on_axis(Vector3::new(0.0, 0.0, 1.0), distance);
    }

    pub fn update_matrix(&mut self) {
        self.matrix = Matrix4::compose(self.position, &self.quaternion, self.scale);
        self.matrix_world_needs_update = true;
    }

    /// `applyMatrix4`: premultiplies the local matrix and decomposes it back.
    pub fn apply_matrix4(&mut self, matrix: &Matrix4) {
        if self.matrix_auto_update {
            self.update_matrix();
        }
        self.matrix = self.matrix.premultiply(matrix);
        let (position, quaternion, scale) = self.matrix.decompose();
        self.position = position;
        self.set_quaternion(quaternion);
        self.scale = scale;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(pub usize);

#[derive(Clone, Debug)]
pub struct Node<T> {
    pub object: Object3D,
    pub name: String,
    pub data: T,
    parent: Option<ObjectId>,
    children: Vec<ObjectId>,
}

impl<T> Node<T> {
    pub fn parent(&self) -> Option<ObjectId> {
        self.parent
    }

    pub fn children(&self) -> &[ObjectId] {
        &self.children
    }
}

/// An arena of `Object3D` nodes with three's parent and child links. Removing a child only
/// detaches it; its node stays in the arena.
#[derive(Clone, Debug, Default)]
pub struct Graph<T> {
    nodes: Vec<Node<T>>,
}

impl<T> Graph<T> {
    pub fn new() -> Self {
        Graph { nodes: Vec::new() }
    }

    pub fn create(&mut self, name: &str, data: T) -> ObjectId {
        self.nodes.push(Node {
            object: Object3D::new(),
            name: name.to_owned(),
            data,
            parent: None,
            children: Vec::new(),
        });
        ObjectId(self.nodes.len() - 1)
    }

    pub fn node(&self, id: ObjectId) -> &Node<T> {
        &self.nodes[id.0]
    }

    pub fn node_mut(&mut self, id: ObjectId) -> &mut Node<T> {
        &mut self.nodes[id.0]
    }

    pub fn object(&self, id: ObjectId) -> &Object3D {
        &self.nodes[id.0].object
    }

    pub fn object_mut(&mut self, id: ObjectId) -> &mut Object3D {
        &mut self.nodes[id.0].object
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// `parent.add(child)`. Adding an object to itself is ignored, as in three.
    pub fn add(&mut self, parent: ObjectId, child: ObjectId) {
        if parent == child {
            return;
        }
        self.remove_from_parent(child);
        self.nodes[child.0].parent = Some(parent);
        self.nodes[parent.0].children.push(child);
    }

    pub fn remove(&mut self, parent: ObjectId, child: ObjectId) {
        let children = &mut self.nodes[parent.0].children;
        if let Some(i) = children.iter().position(|&c| c == child) {
            children.remove(i);
            self.nodes[child.0].parent = None;
        }
    }

    pub fn remove_from_parent(&mut self, child: ObjectId) {
        if let Some(parent) = self.nodes[child.0].parent {
            self.remove(parent, child);
        }
    }

    /// `traverse`: depth first, parent before children, children in insertion order.
    pub fn traverse(&self, root: ObjectId, f: &mut impl FnMut(ObjectId, &Node<T>)) {
        f(root, &self.nodes[root.0]);
        for &child in &self.nodes[root.0].children {
            self.traverse(child, f);
        }
    }

    pub fn descendants(&self, root: ObjectId) -> Vec<ObjectId> {
        let mut out = Vec::new();
        self.traverse(root, &mut |id, _| out.push(id));
        out
    }

    fn update_own_world_matrix(&mut self, id: ObjectId) {
        let parent_world = self.nodes[id.0]
            .parent
            .map(|p| self.nodes[p.0].object.matrix_world);
        let object = &mut self.nodes[id.0].object;
        if object.matrix_world_auto_update {
            object.matrix_world = match parent_world {
                None => object.matrix,
                Some(parent) => Matrix4::multiply_matrices(&parent, &object.matrix),
            };
        }
    }

    pub fn update_world_matrix(
        &mut self,
        id: ObjectId,
        update_parents: bool,
        update_children: bool,
    ) {
        if update_parents && let Some(parent) = self.nodes[id.0].parent {
            self.update_world_matrix(parent, true, false);
        }
        if self.nodes[id.0].object.matrix_auto_update {
            self.nodes[id.0].object.update_matrix();
        }
        self.update_own_world_matrix(id);
        if update_children {
            for child in self.nodes[id.0].children.clone() {
                self.update_world_matrix(child, false, true);
            }
        }
    }

    pub fn update_matrix_world(&mut self, id: ObjectId, mut force: bool) {
        if self.nodes[id.0].object.matrix_auto_update {
            self.nodes[id.0].object.update_matrix();
        }
        if self.nodes[id.0].object.matrix_world_needs_update || force {
            self.update_own_world_matrix(id);
            self.nodes[id.0].object.matrix_world_needs_update = false;
            force = true;
        }
        for child in self.nodes[id.0].children.clone() {
            self.update_matrix_world(child, force);
        }
    }

    pub fn world_position(&mut self, id: ObjectId) -> Vector3 {
        self.update_world_matrix(id, true, false);
        Vector3::from_matrix_position(&self.nodes[id.0].object.matrix_world)
    }

    pub fn world_quaternion(&mut self, id: ObjectId) -> Quaternion {
        self.update_world_matrix(id, true, false);
        self.nodes[id.0].object.matrix_world.decompose().1
    }

    pub fn world_scale(&mut self, id: ObjectId) -> Vector3 {
        self.update_world_matrix(id, true, false);
        self.nodes[id.0].object.matrix_world.decompose().2
    }

    pub fn world_direction(&mut self, id: ObjectId) -> Vector3 {
        self.update_world_matrix(id, true, false);
        let e = &self.nodes[id.0].object.matrix_world.elements;
        Vector3::new(e[8], e[9], e[10]).normalize()
    }

    pub fn local_to_world(&mut self, id: ObjectId, v: Vector3) -> Vector3 {
        self.update_world_matrix(id, true, false);
        v.apply_matrix4(&self.nodes[id.0].object.matrix_world)
    }

    pub fn world_to_local(&mut self, id: ObjectId, v: Vector3) -> Vector3 {
        self.update_world_matrix(id, true, false);
        v.apply_matrix4(&self.nodes[id.0].object.matrix_world.invert())
    }

    /// `lookAt(target)`.
    pub fn look_at(&mut self, id: ObjectId, target: Vector3) {
        self.update_world_matrix(id, true, false);
        let object = &self.nodes[id.0].object;
        let position = Vector3::from_matrix_position(&object.matrix_world);
        let m1 = if object.is_camera_or_light {
            Matrix4::IDENTITY.look_at(position, target, object.up)
        } else {
            Matrix4::IDENTITY.look_at(target, position, object.up)
        };
        let mut q = Quaternion::from_rotation_matrix(&m1);
        if let Some(parent) = self.nodes[id.0].parent {
            let m1 = Matrix4::extract_rotation(&self.nodes[parent.0].object.matrix_world);
            q = q.premultiply(Quaternion::from_rotation_matrix(&m1).invert());
        }
        self.nodes[id.0].object.set_quaternion(q);
    }

    /// `parent.attach(child)`: reparents while keeping the child's world transform.
    pub fn attach(&mut self, parent: ObjectId, child: ObjectId) {
        self.update_world_matrix(parent, true, false);
        let mut m1 = self.nodes[parent.0].object.matrix_world.invert();
        if let Some(old) = self.nodes[child.0].parent {
            self.update_world_matrix(old, true, false);
            m1 = m1.multiply(&self.nodes[old.0].object.matrix_world);
        }
        self.nodes[child.0].object.apply_matrix4(&m1);
        self.remove_from_parent(child);
        self.nodes[child.0].parent = Some(parent);
        self.nodes[parent.0].children.push(child);
        self.update_world_matrix(child, false, true);
    }
}
