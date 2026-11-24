// Ported from protobufjs 8.3.0 (BSD-3-Clause, Copyright (c) 2016 Daniel Wirtz). See THIRD_PARTY_NOTICES.md.

use asrs_js::number;
use asrs_js::string::JsString;

/// Everything upstream's `ProtoDecoder` can throw, with the JS error it throws.
#[derive(Clone, Debug, PartialEq)]
pub enum ProtoError {
    NoSuchType(JsString),
    NoSuchTypeOrEnum {
        path: JsString,
        scope: JsString,
    },
    /// `lookupType` returned the root namespace (the name was empty or only dots).
    DecodeNotAFunction,
    DuplicateName {
        name: JsString,
        scope: JsString,
    },
    DuplicateId {
        id: f64,
        scope: JsString,
    },
    ReservedId {
        id: f64,
        scope: JsString,
    },
    ReservedName {
        name: JsString,
        scope: JsString,
    },
    IllegalLabel(f64),
    IllegalType(f64),
    IllegalTypeName(JsString),
    InvalidMapEntry(JsString),
    InvalidKeyType(JsString),
    PathMustBeRelative,
    PathConflict,
    MaxDepth,
    NegativeFieldId,
    InvalidNestedObject,
    /// A call on a value that is not a function, e.g. `extendedType.get is not a function`.
    NotAFunction(&'static str),
    /// `undefined[key] = value`.
    SetOnUndefined(JsString),
    /// `Object.defineProperty` on a primitive.
    DefineOnPrimitive,
    ReadUndefined(&'static str),
    ReadNull(&'static str),
    CannotConvertToObject,
    NotArrayLike,
    /// A generated protobufjs function failed to compile because a field or oneof name is
    /// emitted as `.name` and starts with a digit.
    Syntax(SyntaxKind),
    IndexOutOfRange {
        pos: f64,
        len: f64,
        buf: f64,
    },
    InvalidVarint,
    InvalidTag,
    TagZero,
    InvalidEndGroup,
    InvalidWireType {
        wire: u32,
        offset: f64,
    },
    MissingRequired(JsString),
    /// A protobufjs behaviour this port does not reproduce; see the variant's text. Upstream
    /// throws something else here, so its JS name is one upstream never produces.
    Unmodeled(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyntaxKind {
    UnexpectedNumber,
    InvalidToken,
    DoubleSeparator,
    TrailingSeparator,
}

impl ProtoError {
    pub fn js_name(&self) -> &'static str {
        use ProtoError::*;
        match self {
            DecodeNotAFunction
            | NegativeFieldId
            | InvalidNestedObject
            | NotAFunction(_)
            | SetOnUndefined(_)
            | DefineOnPrimitive
            | ReadUndefined(_)
            | ReadNull(_)
            | CannotConvertToObject
            | NotArrayLike => "TypeError",
            Syntax(_) => "SyntaxError",
            IndexOutOfRange { .. } => "RangeError",
            MissingRequired(_) => "ProtocolError",
            Unmodeled(_) => "UnmodeledError",
            _ => "Error",
        }
    }

    pub fn js_message(&self) -> JsString {
        use Part::{J, S};
        use ProtoError::*;
        let n = |x: f64| number::to_string(x);
        match self {
            NoSuchType(path) => cat(&[S("no such type: "), J(path)]),
            NoSuchTypeOrEnum { path, scope } => {
                cat(&[S("no such Type or Enum '"), J(path), S("' in "), J(scope)])
            }
            DecodeNotAFunction => cat(&[S("type.decode is not a function")]),
            DuplicateName { name, scope } => {
                cat(&[S("duplicate name '"), J(name), S("' in "), J(scope)])
            }
            DuplicateId { id, scope } => {
                cat(&[S(&format!("duplicate id {} in ", n(*id))), J(scope)])
            }
            ReservedId { id, scope } => {
                cat(&[S(&format!("id {} is reserved in ", n(*id))), J(scope)])
            }
            ReservedName { name, scope } => {
                cat(&[S("name '"), J(name), S("' is reserved in "), J(scope)])
            }
            IllegalLabel(label) => cat(&[S(&format!("illegal label: {}", n(*label)))]),
            IllegalType(ty) => cat(&[S(&format!("illegal type: {}", n(*ty)))]),
            IllegalTypeName(name) => cat(&[S("illegal type name: "), J(name)]),
            InvalidMapEntry(name) => cat(&[S("invalid map entry descriptor: "), J(name)]),
            InvalidKeyType(name) => cat(&[S("invalid key type: "), J(name)]),
            PathMustBeRelative => cat(&[S("path must be relative")]),
            PathConflict => cat(&[S("path conflicts with non-namespace objects")]),
            MaxDepth => cat(&[S("max depth exceeded")]),
            NegativeFieldId => cat(&[S("id must be a non-negative integer")]),
            InvalidNestedObject => cat(&[S("object must be a valid nested object")]),
            NotAFunction(expr) => cat(&[S(expr), S(" is not a function")]),
            SetOnUndefined(key) => cat(&[
                S("Cannot set properties of undefined (setting '"),
                J(key),
                S("')"),
            ]),
            DefineOnPrimitive => cat(&[S("Object.defineProperty called on non-object")]),
            ReadUndefined(prop) => cat(&[
                S("Cannot read properties of undefined (reading '"),
                S(prop),
                S("')"),
            ]),
            ReadNull(prop) => cat(&[
                S("Cannot read properties of null (reading '"),
                S(prop),
                S("')"),
            ]),
            CannotConvertToObject => cat(&[S("Cannot convert undefined or null to object")]),
            NotArrayLike => cat(&[S("CreateListFromArrayLike called on non-object")]),
            Syntax(kind) => cat(&[S(match kind {
                SyntaxKind::UnexpectedNumber => "Unexpected number",
                SyntaxKind::InvalidToken => "Invalid or unexpected token",
                SyntaxKind::DoubleSeparator => {
                    "Only one underscore is allowed as numeric separator"
                }
                SyntaxKind::TrailingSeparator => {
                    "Numeric separators are not allowed at the end of numeric literals"
                }
            })]),
            IndexOutOfRange { pos, len, buf } => cat(&[S(&format!(
                "index out of range: {} + {} > {}",
                n(*pos),
                n(*len),
                n(*buf)
            ))]),
            InvalidVarint => cat(&[S("invalid varint encoding")]),
            InvalidTag => cat(&[S("invalid tag encoding")]),
            TagZero => cat(&[S("illegal tag: field number 0")]),
            InvalidEndGroup => cat(&[S("invalid end group tag")]),
            InvalidWireType { wire, offset } => cat(&[S(&format!(
                "invalid wire type {wire} at offset {}",
                n(*offset)
            ))]),
            MissingRequired(name) => cat(&[S("missing required '"), J(name), S("'")]),
            Unmodeled(what) => cat(&[S(what)]),
        }
    }
}

enum Part<'a> {
    S(&'a str),
    J(&'a JsString),
}

fn cat(parts: &[Part<'_>]) -> JsString {
    let mut s = JsString::new();
    for part in parts {
        match part {
            Part::S(text) => s.push_str(&JsString::from(*text)),
            Part::J(text) => s.push_str(text),
        }
    }
    s
}
