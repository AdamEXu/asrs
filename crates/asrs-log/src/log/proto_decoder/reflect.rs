// Ported from protobufjs 8.3.0 (BSD-3-Clause, Copyright (c) 2016 Daniel Wirtz). See THIRD_PARTY_NOTICES.md.

//! protobufjs's reflection tree (`Root`, `Namespace`, `Type`, `Enum`, `Service`, `Field`,
//! `OneOf`) as an arena, with the add, remove, define and lookup rules that decide what a
//! descriptor builds and what a type name resolves to.

use asrs_js::string::JsString;
use asrs_js::value::JsObject;

use super::error::ProtoError;
use super::value::{JsDefault, Long};

pub(super) type NodeId = usize;
pub(super) type FieldId = usize;
pub(super) type OneofId = usize;

pub(super) const ROOT: NodeId = 0;

/// The `Object.prototype` members a plain-object property read falls through to.
pub(super) const OBJECT_PROTOTYPE: [&str; 12] = [
    "__proto__",
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
    "toLocaleString",
];

pub(super) fn is_object_prototype_name(name: &JsString) -> bool {
    OBJECT_PROTOTYPE.iter().any(|p| js_eq(name, p))
}

pub(super) fn js_eq(s: &JsString, lit: &str) -> bool {
    s.units().iter().copied().eq(lit.encode_utf16())
}

pub(super) fn js(s: &str) -> JsString {
    JsString::from(s)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Member {
    Node(NodeId),
    Field(FieldId),
    OneOf(OneofId),
    Method,
}

#[derive(Clone, Debug)]
pub(super) struct Node {
    pub name: JsString,
    pub parent: Option<NodeId>,
    pub nested: Option<JsObject<Member>>,
    pub kind: Kind,
}

#[derive(Clone, Debug)]
pub(super) enum Kind {
    Root,
    Namespace,
    Type(TypeData),
    Enum(EnumData),
    Service(JsObject<()>),
}

#[derive(Clone, Debug, Default)]
pub(super) struct TypeData {
    pub fields: JsObject<FieldId>,
    pub oneofs: Option<JsObject<OneofId>>,
    pub reserved: Option<Vec<Reserved>>,
    /// The repeated and map fields the generated constructor initialises, fixed when the
    /// constructor is first built.
    pub ctor: Option<Vec<FieldId>>,
    pub decode_ready: bool,
    /// The fields the current generated `toObject` covers.
    pub to_object: Option<Vec<FieldId>>,
}

#[derive(Clone, Debug)]
pub(super) enum Reserved {
    Range(f64, f64),
    Name(JsString),
}

#[derive(Clone, Debug, Default)]
pub(super) struct EnumData {
    pub values: JsObject<f64>,
    pub values_by_id: JsObject<JsString>,
}

#[derive(Clone, Debug)]
pub(super) struct Field {
    pub name: JsString,
    pub id: f64,
    pub ty: JsString,
    pub repeated: bool,
    pub required: bool,
    pub extend: Option<JsString>,
    pub map_key: Option<JsString>,
    pub default: Option<JsDefault>,
    pub proto3_optional: bool,
    pub part_of: Option<OneofId>,
    pub parent: Option<NodeId>,
    pub declaring_field: Option<FieldId>,
    pub extension_field: Option<FieldId>,
    pub long: bool,
    pub resolved: bool,
    pub resolved_type: Option<NodeId>,
    pub type_default: JsDefault,
    pub default_value: JsDefault,
}

impl Field {
    pub fn new(name: JsString, id: f64, ty: JsString) -> Self {
        let long = is_long_type(&ty);
        Field {
            name,
            id,
            ty,
            repeated: false,
            required: false,
            extend: None,
            map_key: None,
            default: None,
            proto3_optional: false,
            part_of: None,
            parent: None,
            declaring_field: None,
            extension_field: None,
            long,
            resolved: false,
            resolved_type: None,
            type_default: JsDefault::Null,
            default_value: JsDefault::Null,
        }
    }

    pub fn is_bytes(&self) -> bool {
        js_eq(&self.ty, "bytes")
    }
}

#[derive(Clone, Debug)]
pub(super) struct OneOf {
    pub name: JsString,
    pub oneof: Vec<JsString>,
    pub fields: Vec<FieldId>,
    pub parent: Option<NodeId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Filter {
    Any,
    Type,
    TypeOrEnum,
}

/// The basic scalar types `types.defaults` knows, plus its `undefined` key (the table bakes
/// one more value than it has names).
pub(super) fn basic_default(ty: &JsString) -> Option<JsDefault> {
    let s = ty.as_string()?;
    Some(match s.as_str() {
        "double" | "float" | "int32" | "uint32" | "sint32" | "fixed32" | "sfixed32" | "int64"
        | "uint64" | "sint64" | "fixed64" | "sfixed64" => JsDefault::Number(0.0),
        "bool" => JsDefault::Bool(false),
        "string" => JsDefault::String(JsString::new()),
        "bytes" => JsDefault::EmptyArray,
        "undefined" => JsDefault::Null,
        _ => return None,
    })
}

pub(super) fn is_long_type(ty: &JsString) -> bool {
    matches!(
        ty.as_string().as_deref(),
        Some("int64" | "uint64" | "sint64" | "fixed64" | "sfixed64")
    )
}

#[derive(Clone, Debug)]
pub(super) struct Reflection {
    pub nodes: Vec<Node>,
    pub fields: Vec<Field>,
    pub oneofs: Vec<OneOf>,
    pub files: Vec<JsString>,
    pub deferred: Vec<FieldId>,
    fully_qualified: JsObject<NodeId>,
}

impl Reflection {
    pub fn new() -> Self {
        Reflection {
            nodes: vec![Node {
                name: JsString::new(),
                parent: None,
                nested: None,
                kind: Kind::Root,
            }],
            fields: Vec::new(),
            oneofs: Vec::new(),
            files: Vec::new(),
            deferred: Vec::new(),
            fully_qualified: JsObject::new(),
        }
    }

    pub fn new_node(&mut self, name: JsString, kind: Kind) -> NodeId {
        self.nodes.push(Node {
            name,
            parent: None,
            nested: None,
            kind,
        });
        self.nodes.len() - 1
    }

    pub fn new_field(&mut self, field: Field) -> FieldId {
        self.fields.push(field);
        self.fields.len() - 1
    }

    pub fn new_oneof(&mut self, name: JsString) -> OneofId {
        self.oneofs.push(OneOf {
            name,
            oneof: Vec::new(),
            fields: Vec::new(),
            parent: None,
        });
        self.oneofs.len() - 1
    }

    pub fn type_data(&self, node: NodeId) -> &TypeData {
        match &self.nodes[node].kind {
            Kind::Type(data) => data,
            _ => panic!("node {node} is not a Type"),
        }
    }

    pub fn type_data_mut(&mut self, node: NodeId) -> &mut TypeData {
        match &mut self.nodes[node].kind {
            Kind::Type(data) => data,
            _ => panic!("node {node} is not a Type"),
        }
    }

    pub fn is_type(&self, node: NodeId) -> bool {
        matches!(self.nodes[node].kind, Kind::Type(_))
    }

    pub fn is_enum(&self, node: NodeId) -> bool {
        matches!(self.nodes[node].kind, Kind::Enum(_))
    }

    fn is_namespace(&self, node: NodeId) -> bool {
        !matches!(self.nodes[node].kind, Kind::Enum(_))
    }

    pub fn fields_array(&self, node: NodeId) -> Vec<FieldId> {
        self.type_data(node).fields.values().copied().collect()
    }

    fn nested_array(&self, node: NodeId) -> Vec<Member> {
        self.nodes[node]
            .nested
            .as_ref()
            .map(|n| n.values().copied().collect())
            .unwrap_or_default()
    }

    fn member_name(&self, member: Member) -> JsString {
        match member {
            Member::Node(n) => self.nodes[n].name.clone(),
            Member::Field(f) => self.fields[f].name.clone(),
            Member::OneOf(o) => self.oneofs[o].name.clone(),
            Member::Method => unreachable!("methods are never added through add()"),
        }
    }

    fn member_parent(&self, member: Member) -> Option<NodeId> {
        match member {
            Member::Node(n) => self.nodes[n].parent,
            Member::Field(f) => self.fields[f].parent,
            Member::OneOf(o) => self.oneofs[o].parent,
            Member::Method => None,
        }
    }

    fn set_parent(&mut self, member: Member, parent: Option<NodeId>) {
        match member {
            Member::Node(n) => self.nodes[n].parent = parent,
            Member::Field(f) => {
                self.fields[f].parent = parent;
                self.fields[f].resolved = false;
            }
            Member::OneOf(o) => self.oneofs[o].parent = parent,
            Member::Method => {}
        }
    }

    pub fn root_of(&self, mut node: NodeId) -> NodeId {
        while let Some(p) = self.nodes[node].parent {
            node = p;
        }
        node
    }

    fn attached(&self, parent: Option<NodeId>) -> bool {
        parent.is_some_and(|p| self.root_of(p) == ROOT)
    }

    pub fn full_name(&self, member: Member) -> JsString {
        let mut path = vec![self.member_name(member)];
        let mut ptr = self.member_parent(member);
        while let Some(p) = ptr {
            path.push(self.nodes[p].name.clone());
            ptr = self.nodes[p].parent;
        }
        join(path.iter().rev(), '.')
    }

    /// `ReflectionObject#toString`, as error messages print a scope.
    pub fn describe(&self, node: NodeId) -> JsString {
        let class = match self.nodes[node].kind {
            Kind::Root => "Root",
            Kind::Namespace => "Namespace",
            Kind::Type(_) => "Type",
            Kind::Enum(_) => "Enum",
            Kind::Service(_) => "Service",
        };
        let full = self.full_name(Member::Node(node));
        let mut s = js(class);
        if !full.is_empty() {
            s.push_unit(u16::from(b' '));
            s.push_str(&full);
        }
        s
    }

    pub fn get(&self, node: NodeId, name: &JsString) -> Option<Member> {
        let n = &self.nodes[node];
        match &n.kind {
            Kind::Type(data) => {
                if let Some(f) = data.fields.get_key(name) {
                    return Some(Member::Field(*f));
                }
                if let Some(o) = data.oneofs.as_ref().and_then(|o| o.get_key(name)) {
                    return Some(Member::OneOf(*o));
                }
            }
            Kind::Service(methods) if methods.get_key(name).is_some() => {
                return Some(Member::Method);
            }
            _ => {}
        }
        n.nested
            .as_ref()
            .and_then(|nested| nested.get_key(name))
            .copied()
    }

    fn clear_type_cache(&mut self, node: NodeId) {
        self.type_data_mut(node).decode_ready = false;
    }

    /// `object.add(member)` for a namespace-like object.
    pub fn add(&mut self, node: NodeId, member: Member) -> Result<(), ProtoError> {
        match self.nodes[node].kind {
            Kind::Type(_) => self.type_add(node, member),
            Kind::Service(_) => {
                let name = self.member_name(member);
                if self.get(node, &name).is_some() {
                    return Err(ProtoError::DuplicateName {
                        name,
                        scope: self.describe(node),
                    });
                }
                self.namespace_add(node, member)
            }
            Kind::Enum(_) => unreachable!("extensions never target an enum"),
            Kind::Root | Kind::Namespace => self.namespace_add(node, member),
        }
    }

    pub fn service_add_method(&mut self, node: NodeId, name: JsString) -> Result<(), ProtoError> {
        if self.get(node, &name).is_some() {
            return Err(ProtoError::DuplicateName {
                name,
                scope: self.describe(node),
            });
        }
        if js_eq(&name, "__proto__") {
            return Ok(());
        }
        if let Kind::Service(methods) = &mut self.nodes[node].kind {
            methods.insert(name, ());
        }
        Ok(())
    }

    fn type_add(&mut self, node: NodeId, member: Member) -> Result<(), ProtoError> {
        let name = self.member_name(member);
        if self.get(node, &name).is_some() {
            return Err(ProtoError::DuplicateName {
                name,
                scope: self.describe(node),
            });
        }
        match member {
            Member::Field(f) if self.fields[f].extend.is_none() => {
                let id = self.fields[f].id;
                let data = self.type_data(node);
                if data
                    .fields
                    .values()
                    .any(|&other| self.fields[other].id == id)
                {
                    return Err(ProtoError::DuplicateId {
                        id,
                        scope: self.describe(node),
                    });
                }
                if let Some(reserved) = &data.reserved {
                    if reserved
                        .iter()
                        .any(|r| matches!(r, Reserved::Range(lo, hi) if *lo <= id && *hi > id))
                    {
                        return Err(ProtoError::ReservedId {
                            id,
                            scope: self.describe(node),
                        });
                    }
                    if reserved
                        .iter()
                        .any(|r| matches!(r, Reserved::Name(n) if *n == name))
                    {
                        return Err(ProtoError::ReservedName {
                            name,
                            scope: self.describe(node),
                        });
                    }
                }
                if js_eq(&name, "__proto__") {
                    return Ok(());
                }
                if let Some(old) = self.fields[f].parent {
                    self.remove(old, member)?;
                }
                self.type_data_mut(node).fields.insert(name, f);
                self.on_add(member, node)?;
                self.clear_type_cache(node);
                Ok(())
            }
            Member::OneOf(o) => {
                if js_eq(&name, "__proto__") {
                    return Ok(());
                }
                self.type_data_mut(node)
                    .oneofs
                    .get_or_insert_with(JsObject::new)
                    .insert(name, o);
                self.on_add(member, node)?;
                self.clear_type_cache(node);
                Ok(())
            }
            _ => self.namespace_add(node, member),
        }
    }

    fn namespace_add(&mut self, node: NodeId, member: Member) -> Result<(), ProtoError> {
        let valid = match member {
            Member::Field(f) => self.fields[f].extend.is_some(),
            Member::Node(_) | Member::OneOf(_) => true,
            Member::Method => false,
        };
        if !valid {
            return Err(ProtoError::InvalidNestedObject);
        }
        let name = self.member_name(member);
        if js_eq(&name, "__proto__") {
            return Ok(());
        }
        if self.nodes[node].nested.is_none() {
            self.nodes[node].nested = Some(JsObject::new());
        } else if let Some(prev) = self.get(node, &name) {
            let merge = match (prev, member) {
                (Member::Node(p), Member::Node(o)) => {
                    matches!(self.nodes[p].kind, Kind::Namespace | Kind::Root)
                        && self.is_namespace(o)
                }
                _ => false,
            };
            if !merge {
                return Err(ProtoError::DuplicateName {
                    name,
                    scope: self.describe(node),
                });
            }
            let (Member::Node(prev), Member::Node(object)) = (prev, member) else {
                unreachable!()
            };
            for nested in self.nested_array(prev) {
                self.add(object, nested)?;
            }
            self.remove(node, Member::Node(prev))?;
            if self.nodes[node].nested.is_none() {
                self.nodes[node].nested = Some(JsObject::new());
            }
        }
        self.nodes[node]
            .nested
            .as_mut()
            .expect("nested exists")
            .insert(name, member);
        self.on_add(member, node)
    }

    pub fn remove(&mut self, node: NodeId, member: Member) -> Result<(), ProtoError> {
        let name = self.member_name(member);
        if let Kind::Type(data) = &mut self.nodes[node].kind {
            match member {
                Member::Field(f) if self.fields[f].extend.is_none() => {
                    data.fields.remove_key(&name);
                    self.fields[f].parent = None;
                    self.on_remove(member, node)?;
                    self.clear_type_cache(node);
                    return Ok(());
                }
                Member::OneOf(o) => {
                    if let Some(oneofs) = &mut data.oneofs {
                        oneofs.remove_key(&name);
                    }
                    self.oneofs[o].parent = None;
                    for f in self.oneofs[o].fields.clone() {
                        if let Some(p) = self.fields[f].parent {
                            self.remove(p, Member::Field(f))?;
                        }
                    }
                    self.on_remove(member, node)?;
                    self.clear_type_cache(node);
                    return Ok(());
                }
                _ => {}
            }
        }
        let nested = self.nodes[node].nested.as_mut().expect("member is nested");
        nested.remove_key(&name);
        if nested.is_empty() {
            self.nodes[node].nested = None;
        }
        self.on_remove(member, node)
    }

    fn on_add(&mut self, member: Member, parent: NodeId) -> Result<(), ProtoError> {
        if let Some(old) = self.member_parent(member)
            && old != parent
        {
            self.remove(old, member)?;
        }
        self.set_parent(member, Some(parent));
        if self.root_of(parent) == ROOT {
            self.handle_add(member)?;
        }
        Ok(())
    }

    fn on_remove(&mut self, member: Member, parent: NodeId) -> Result<(), ProtoError> {
        if self.root_of(parent) == ROOT {
            self.handle_remove(member)?;
        }
        self.set_parent(member, None);
        Ok(())
    }

    fn handle_add(&mut self, member: Member) -> Result<(), ProtoError> {
        match member {
            Member::Field(f) => {
                if self.fields[f].extend.is_some()
                    && self.fields[f].extension_field.is_none()
                    && !self.try_handle_extension(f)?
                {
                    self.deferred.push(f);
                }
            }
            Member::Node(n) if self.is_enum(n) => {
                let full = self.full_name(member);
                self.fully_qualified.insert(full, n);
            }
            Member::OneOf(_) | Member::Method => {}
            Member::Node(n) => {
                if self.is_type(n) {
                    let mut i = 0;
                    while i < self.deferred.len() {
                        if self.try_handle_extension(self.deferred[i])? {
                            self.deferred.remove(i);
                        } else {
                            i += 1;
                        }
                    }
                }
                for nested in self.nested_array(n) {
                    self.handle_add(nested)?;
                }
                if self.is_type(n) {
                    let full = self.full_name(member);
                    self.fully_qualified.insert(full, n);
                }
            }
        }
        Ok(())
    }

    fn handle_remove(&mut self, member: Member) -> Result<(), ProtoError> {
        match member {
            Member::Field(f) => {
                if self.fields[f].extend.is_some() {
                    if let Some(sister) = self.fields[f].extension_field {
                        if let Some(p) = self.fields[sister].parent {
                            self.remove(p, Member::Field(sister))?;
                        }
                        self.fields[f].extension_field = None;
                    } else if let Some(i) = self.deferred.iter().position(|&d| d == f) {
                        self.deferred.remove(i);
                    }
                }
            }
            Member::Node(n) if !self.is_enum(n) => {
                for nested in self.nested_array(n) {
                    self.handle_remove(nested)?;
                }
            }
            _ => {}
        }
        if member != Member::Method {
            let full = self.full_name(member);
            self.fully_qualified.remove_key(&full);
        }
        Ok(())
    }

    fn try_handle_extension(&mut self, f: FieldId) -> Result<bool, ProtoError> {
        let Some(scope) = self.fields[f].parent else {
            return Ok(false);
        };
        let extend = self.fields[f].extend.clone().expect("extension field");
        let Some(target) = self.lookup(scope, &extend, Filter::Any) else {
            return Ok(false);
        };
        let Member::Node(target) = target else {
            return Err(ProtoError::NotAFunction("extendedType.get"));
        };
        if self.is_enum(target) {
            return Err(ProtoError::NotAFunction("extendedType.get"));
        }
        let declaring = &self.fields[f];
        let mut sister = Field::new(
            self.full_name(Member::Field(f)),
            declaring.id,
            declaring.ty.clone(),
        );
        sister.repeated = declaring.repeated;
        sister.default = declaring.default.clone();
        sister.proto3_optional = declaring.proto3_optional;
        if self.get(target, &sister.name).is_some() {
            return Ok(true);
        }
        sister.declaring_field = Some(f);
        let sister = self.new_field(sister);
        self.fields[f].extension_field = Some(sister);
        self.add(target, Member::Field(sister))?;
        Ok(true)
    }

    /// `Namespace#define(path)`, used for a file's package.
    pub fn define(&mut self, path: &JsString) -> Result<NodeId, ProtoError> {
        let parts = split_dots(path);
        if parts.first().is_some_and(|p| p.is_empty()) {
            return Err(ProtoError::PathMustBeRelative);
        }
        if parts.len() > 100 {
            return Err(ProtoError::MaxDepth);
        }
        let mut ptr = ROOT;
        for part in parts {
            if let Some(nested) = &self.nodes[ptr].nested {
                if let Some(&existing) = nested.get_key(&part) {
                    match existing {
                        Member::Node(n) if self.is_namespace(n) => {
                            ptr = n;
                            continue;
                        }
                        _ => return Err(ProtoError::PathConflict),
                    }
                }
                if is_object_prototype_name(&part) {
                    return Err(ProtoError::PathConflict);
                }
            }
            let ns = self.new_node(part, Kind::Namespace);
            self.add(ptr, Member::Node(ns))?;
            ptr = ns;
        }
        Ok(ptr)
    }

    /// `Namespace#lookup(path, filterTypes)` for a string path.
    pub fn lookup(&self, from: NodeId, path: &JsString, filter: Filter) -> Option<Member> {
        if path.is_empty() {
            return Some(Member::Node(from));
        }
        if js_eq(path, ".") {
            return Some(Member::Node(self.root_of(from)));
        }
        self.lookup_parts(from, &split_dots(path), filter)
    }

    fn accepts(&self, member: Member, filter: Filter) -> bool {
        match (filter, member) {
            (Filter::Any, _) => true,
            (Filter::Type, Member::Node(n)) => self.is_type(n),
            (Filter::TypeOrEnum, Member::Node(n)) => self.is_type(n) || self.is_enum(n),
            _ => false,
        }
    }

    fn lookup_parts(&self, from: NodeId, parts: &[JsString], filter: Filter) -> Option<Member> {
        if parts.is_empty() {
            return Some(Member::Node(from));
        }
        if parts[0].is_empty() {
            return self.lookup_parts(self.root_of(from), &parts[1..], filter);
        }
        let flat = join(parts.iter(), '.');
        if self.root_of(from) == ROOT {
            let mut key = js(".");
            key.push_str(&flat);
            if let Some(&n) = self.fully_qualified.get_key(&key)
                && self.accepts(Member::Node(n), filter)
            {
                return Some(Member::Node(n));
            }
        }
        if let Some(found) = self.lookup_impl(from, parts)
            && self.accepts(found, filter)
        {
            return Some(found);
        }
        let mut current = from;
        while let Some(parent) = self.nodes[current].parent {
            if let Some(found) = self.lookup_impl(parent, parts)
                && self.accepts(found, filter)
            {
                return Some(found);
            }
            current = parent;
        }
        None
    }

    fn lookup_impl(&self, node: NodeId, parts: &[JsString]) -> Option<Member> {
        if let Some(found) = self.get(node, &parts[0]) {
            if parts.len() == 1 {
                return Some(found);
            }
            return match found {
                Member::Node(n) if self.is_namespace(n) => self.lookup_impl(n, &parts[1..]),
                _ => None,
            };
        }
        for nested in self.nested_array(node) {
            if let Member::Node(n) = nested
                && self.is_namespace(n)
                && let Some(found) = self.lookup_impl(n, parts)
            {
                return Some(found);
            }
        }
        None
    }

    pub fn oneof_add(&mut self, o: OneofId, f: FieldId) -> Result<(), ProtoError> {
        let parent = self.oneofs[o].parent;
        if let Some(fp) = self.fields[f].parent
            && Some(fp) != parent
        {
            self.remove(fp, Member::Field(f))?;
        }
        let name = self.fields[f].name.clone();
        self.oneofs[o].oneof.push(name);
        self.oneofs[o].fields.push(f);
        self.fields[f].part_of = Some(o);
        if let Some(parent) = parent {
            for member in self.oneofs[o].fields.clone() {
                if self.fields[member].parent.is_none() {
                    self.add(parent, Member::Field(member))?;
                }
            }
        }
        Ok(())
    }

    /// `Field#resolve()`.
    pub fn resolve_field(&mut self, f: FieldId) -> Result<(), ProtoError> {
        if self.fields[f].resolved {
            return Ok(());
        }
        if let Some(key) = &self.fields[f].map_key
            && !is_map_key_type(key)
        {
            return Err(ProtoError::InvalidKeyType(key.clone()));
        }
        let ty = self.fields[f].ty.clone();
        let mut type_default = match basic_default(&ty) {
            Some(d) => {
                if self.fields[f].proto3_optional {
                    JsDefault::Null
                } else {
                    d
                }
            }
            None => {
                let scope = match self.fields[f].declaring_field {
                    Some(d) => self.fields[d].parent,
                    None => self.fields[f].parent,
                }
                .expect("resolving a detached field");
                let Some(found) = self.lookup(scope, &ty, Filter::TypeOrEnum) else {
                    return Err(ProtoError::NoSuchTypeOrEnum {
                        path: ty,
                        scope: self.describe(scope),
                    });
                };
                let Member::Node(found) = found else {
                    unreachable!("filtered lookups return nodes")
                };
                self.fields[f].resolved_type = Some(found);
                match &self.nodes[found].kind {
                    Kind::Type(_) => JsDefault::Null,
                    Kind::Enum(data) => match data.values.values().next() {
                        Some(&v) => JsDefault::Number(v),
                        None => JsDefault::Undefined,
                    },
                    _ => return Err(ProtoError::CannotConvertToObject),
                }
            }
        };
        if let Some(default) = self.fields[f].default.clone() {
            type_default = default;
            if let (Some(rt), JsDefault::String(name)) =
                (self.fields[f].resolved_type, &type_default)
                && let Kind::Enum(data) = &self.nodes[rt].kind
            {
                type_default = enum_value_lookup(data, name);
            }
        }
        let field = &mut self.fields[f];
        if field.long {
            let unsigned = field.ty.units().first() == Some(&u16::from(b'u'));
            type_default = JsDefault::Long(Long::from_default(&type_default, unsigned));
        } else if field.is_bytes()
            && let JsDefault::String(text) = &type_default
        {
            type_default = JsDefault::Bytes(super::value::bytes_default(text));
        }
        field.default_value = if field.map_key.is_some() {
            JsDefault::EmptyObject
        } else if field.repeated {
            JsDefault::EmptyArray
        } else {
            type_default.clone()
        };
        field.type_default = type_default;
        let parent = field.parent;
        self.fields[f].resolved = self.attached(parent);
        Ok(())
    }
}

/// `enum.values[name]`: own names, then the `valuesById` prototype, then `Object.prototype`.
fn enum_value_lookup(data: &EnumData, name: &JsString) -> JsDefault {
    if let Some(&v) = data.values.get_key(name) {
        return JsDefault::Number(v);
    }
    if let Some(n) = data.values_by_id.get_key(name) {
        return JsDefault::String(n.clone());
    }
    if js_eq(name, "__proto__") {
        return JsDefault::Object(data.values_by_id.clone());
    }
    if is_object_prototype_name(name) {
        return JsDefault::Function(name.clone());
    }
    JsDefault::Undefined
}

pub(super) fn is_map_key_type(ty: &JsString) -> bool {
    matches!(
        ty.as_string().as_deref(),
        Some(
            "int32"
                | "uint32"
                | "sint32"
                | "fixed32"
                | "sfixed32"
                | "int64"
                | "uint64"
                | "sint64"
                | "fixed64"
                | "sfixed64"
                | "bool"
                | "string"
        )
    )
}

pub(super) fn split_dots(path: &JsString) -> Vec<JsString> {
    path.split(Some(&js(".")), None)
}

fn join<'a>(parts: impl Iterator<Item = &'a JsString>, sep: char) -> JsString {
    let mut out = JsString::new();
    for (i, part) in parts.enumerate() {
        if i > 0 {
            out.push_unit(sep as u16);
        }
        out.push_str(part);
    }
    out
}
