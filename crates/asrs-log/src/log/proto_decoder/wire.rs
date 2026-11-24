// Ported from protobufjs 8.3.0 (BSD-3-Clause, Copyright (c) 2016 Daniel Wirtz). See THIRD_PARTY_NOTICES.md.

//! protobufjs's `Reader`, and the behaviour of the code its `decoder`, `toObject` and
//! constructor generators emit for a reflected type.

use asrs_js::number;
use asrs_js::sort::array_sort;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue, array_index};

use super::error::{ProtoError, SyntaxKind};
use super::reflect::{FieldId, Kind, NodeId, Reflection, is_object_prototype_name, js, js_eq};
use super::value::{Foreign, JsDefault, Long, Message, Val, long_key_to_string};
use super::{child_key, friendly_schema_type as friendly};

const RECURSION_LIMIT: u32 = 100;

pub(super) struct Reader<'a> {
    buf: &'a [u8],
    pub pos: u64,
}

fn and127(b: Option<u8>) -> u32 {
    b.map_or(0, |b| u32::from(b & 127))
}

fn below128(b: Option<u8>) -> bool {
    b.is_some_and(|b| b < 128)
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    fn len(&self) -> u64 {
        self.buf.len() as u64
    }

    fn at(&self, pos: u64) -> Option<u8> {
        usize::try_from(pos)
            .ok()
            .and_then(|p| self.buf.get(p).copied())
    }

    fn out_of_range(&self, write_length: u64) -> ProtoError {
        ProtoError::IndexOutOfRange {
            pos: self.pos as f64,
            len: if write_length == 0 {
                1.0
            } else {
                write_length as f64
            },
            buf: self.len() as f64,
        }
    }

    pub fn uint32(&mut self) -> Result<u32, ProtoError> {
        let mut pos = self.pos;
        let mut value = and127(self.at(pos));
        for shift in [7, 14, 21] {
            let more = !below128(self.at(pos));
            pos += 1;
            if !more {
                self.pos = pos;
                return Ok(value);
            }
            value |= and127(self.at(pos)) << shift;
        }
        let more = !below128(self.at(pos));
        pos += 1;
        if !more {
            self.pos = pos;
            return Ok(value);
        }
        value |= self.at(pos).map_or(0, |b| u32::from(b & 15)) << 28;
        let more = !below128(self.at(pos));
        pos += 1;
        if !more {
            self.pos = pos;
            return Ok(value);
        }
        for _ in 0..5 {
            if pos >= self.len() {
                self.pos = pos;
                return Err(self.out_of_range(1));
            }
            let b = self.at(pos);
            pos += 1;
            if below128(b) {
                self.pos = pos;
                return Ok(value);
            }
        }
        self.pos = pos;
        Err(ProtoError::InvalidVarint)
    }

    pub fn tag(&mut self) -> Result<u32, ProtoError> {
        let mut pos = self.pos;
        let mut value = and127(self.at(pos));
        for shift in [7, 14, 21] {
            let more = !below128(self.at(pos));
            pos += 1;
            if !more {
                self.pos = pos;
                return Ok(value);
            }
            value |= and127(self.at(pos)) << shift;
        }
        let more = !below128(self.at(pos));
        pos += 1;
        if !more {
            self.pos = pos;
            return Ok(value);
        }
        value |= self.at(pos).map_or(0, |b| u32::from(b & 15)) << 28;
        let last = self.at(pos);
        self.pos = pos + 1;
        if last.is_some_and(|b| b < 128 && b & 112 == 0) {
            Ok(value)
        } else {
            Err(ProtoError::InvalidTag)
        }
    }

    fn long_varint(&mut self) -> Result<(u32, u32), ProtoError> {
        let mut lo = 0u32;
        let mut hi = 0u32;
        let remaining = |r: &Self| r.len() as i64 - r.pos as i64;
        if remaining(self) > 4 {
            for i in 0..4 {
                let b = self.at(self.pos);
                lo |= and127(b) << (i * 7);
                self.pos += 1;
                if below128(b) {
                    return Ok((lo, hi));
                }
            }
            let b = self.at(self.pos);
            lo |= and127(b) << 28;
            hi |= and127(b) >> 4;
            self.pos += 1;
            if below128(b) {
                return Ok((lo, hi));
            }
        } else {
            for i in 0..3 {
                if self.pos >= self.len() {
                    return Err(self.out_of_range(1));
                }
                let b = self.at(self.pos);
                lo |= and127(b) << (i * 7);
                self.pos += 1;
                if below128(b) {
                    return Ok((lo, hi));
                }
            }
            lo |= and127(self.at(self.pos)) << 21;
            self.pos += 1;
            return Ok((lo, hi));
        }
        let checked = remaining(self) <= 4;
        for i in 0..5 {
            if checked && self.pos >= self.len() {
                return Err(self.out_of_range(1));
            }
            let b = self.at(self.pos);
            hi |= and127(b).wrapping_shl(i * 7 + 3);
            self.pos += 1;
            if below128(b) {
                return Ok((lo, hi));
            }
        }
        Err(ProtoError::InvalidVarint)
    }

    fn bool(&mut self) -> Result<bool, ProtoError> {
        let mut value = false;
        for _ in 0..10 {
            if self.pos >= self.len() {
                return Err(self.out_of_range(1));
            }
            let b = self.at(self.pos).expect("in range");
            self.pos += 1;
            if b & 127 != 0 {
                value = true;
            }
            if b < 128 {
                return Ok(value);
            }
        }
        Err(ProtoError::InvalidVarint)
    }

    fn fixed(&mut self, width: u64, reported: u64) -> Result<&'a [u8], ProtoError> {
        if self.pos + width > self.len() {
            return Err(self.out_of_range(reported));
        }
        let start = self.pos as usize;
        self.pos += width;
        Ok(&self.buf[start..start + width as usize])
    }

    fn fixed32(&mut self) -> Result<u32, ProtoError> {
        let b = self.fixed(4, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn fixed64(&mut self) -> Result<(u32, u32), ProtoError> {
        let b = self.fixed(8, 8)?;
        Ok((
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
        ))
    }

    fn delimited(&mut self) -> Result<(u64, u64), ProtoError> {
        let length = u64::from(self.uint32()?);
        let start = self.pos;
        let end = start + length;
        if end > self.len() {
            return Err(self.out_of_range(length));
        }
        self.pos = end;
        Ok((start, end))
    }

    fn string(&mut self) -> Result<JsString, ProtoError> {
        let (start, end) = self.delimited()?;
        Ok(utf8_read(self.buf, start as usize, end as usize))
    }

    /// `r.raw(s, r.pos)`: the exact wire bytes of a skipped unknown field.
    fn slice(&self, start: u64, end: u64) -> Vec<u8> {
        self.buf
            .get(start as usize..end as usize)
            .map_or_else(Vec::new, |b| b.to_vec())
    }

    fn skip(&mut self, length: Option<u64>) -> Result<(), ProtoError> {
        match length {
            Some(n) => {
                if self.pos + n > self.len() {
                    return Err(self.out_of_range(n));
                }
                self.pos += n;
            }
            None => loop {
                if self.pos >= self.len() {
                    return Err(self.out_of_range(1));
                }
                let b = self.at(self.pos).expect("in range");
                self.pos += 1;
                if b & 128 == 0 {
                    break;
                }
            },
        }
        Ok(())
    }

    fn skip_type(&mut self, wire: u32, depth: u32, field: Option<u32>) -> Result<(), ProtoError> {
        if depth > RECURSION_LIMIT {
            return Err(ProtoError::MaxDepth);
        }
        if field == Some(0) {
            return Err(ProtoError::TagZero);
        }
        match wire {
            0 => self.skip(None),
            1 => self.skip(Some(8)),
            2 => {
                let n = self.uint32()?;
                self.skip(Some(u64::from(n)))
            }
            3 => loop {
                let tag = self.tag()?;
                let nested = tag >> 3;
                let wire = tag & 7;
                if nested == 0 {
                    return Err(ProtoError::TagZero);
                }
                if wire == 4 {
                    if field.is_some_and(|f| f != nested) {
                        return Err(ProtoError::InvalidEndGroup);
                    }
                    return Ok(());
                }
                self.skip_type(wire, depth + 1, Some(nested))?;
            },
            5 => self.skip(Some(4)),
            _ => Err(ProtoError::InvalidWireType {
                wire,
                offset: self.pos as f64,
            }),
        }
    }

    /// `reader[type]()` for a basic type name.
    fn read(&mut self, ty: &str) -> Result<Val, ProtoError> {
        Ok(match ty {
            "double" => {
                let b = self.fixed(8, 4)?;
                Val::Num(f64::from_le_bytes(b.try_into().expect("8 bytes")))
            }
            "float" => {
                let b = self.fixed(4, 4)?;
                Val::Num(widen(u32::from_le_bytes(b.try_into().expect("4 bytes"))))
            }
            "int32" => Val::Num(f64::from(self.uint32()? as i32)),
            "uint32" => Val::Num(f64::from(self.uint32()?)),
            "sint32" => {
                let v = self.uint32()?;
                Val::Num(f64::from(((v >> 1) ^ 0u32.wrapping_sub(v & 1)) as i32))
            }
            "fixed32" => Val::Num(f64::from(self.fixed32()?)),
            "sfixed32" => Val::Num(f64::from(self.fixed32()? as i32)),
            "int64" => {
                let (lo, hi) = self.long_varint()?;
                Val::Long(Long::from_bits(lo, hi, false))
            }
            "uint64" => {
                let (lo, hi) = self.long_varint()?;
                Val::Long(Long::from_bits(lo, hi, true))
            }
            "sint64" => {
                let (lo, hi) = self.long_varint()?;
                let mask = 0u32.wrapping_sub(lo & 1);
                Val::Long(Long::from_bits(
                    ((lo >> 1) | (hi << 31)) ^ mask,
                    (hi >> 1) ^ mask,
                    false,
                ))
            }
            "fixed64" => {
                let (lo, hi) = self.fixed64()?;
                Val::Long(Long::from_bits(lo, hi, true))
            }
            "sfixed64" => {
                let (lo, hi) = self.fixed64()?;
                Val::Long(Long::from_bits(lo, hi, false))
            }
            "bool" => Val::Bool(self.bool()?),
            "string" => Val::Str(self.string()?),
            "bytes" => {
                let (start, end) = self.delimited()?;
                Val::Bytes(self.buf[start as usize..end as usize].to_vec())
            }
            other => unreachable!("not a basic type: {other}"),
        })
    }
}

/// `util.utf8.read`: a lenient decoder that reads continuation bytes past `end` (and reads
/// `undefined`, which masks to 0, past the buffer).
fn utf8_read(buf: &[u8], start: usize, end: usize) -> JsString {
    let at = |i: usize| buf.get(i).map_or(0u32, |&b| u32::from(b & 0x3f));
    let mut out = Vec::new();
    let mut i = start;
    while i < end {
        let t = u32::from(buf[i]);
        i += 1;
        if t <= 0x7f {
            out.push(t as u16);
        } else if (0xc0..0xe0).contains(&t) {
            let c = (t & 0x1f) << 6 | at(i);
            i += 1;
            out.push(if c >= 0x80 { c as u16 } else { 0xfffd });
        } else if (0xe0..0xf0).contains(&t) {
            let c = (t & 0xf) << 12 | at(i) << 6 | at(i + 1);
            i += 2;
            out.push(if c >= 0x800 { c as u16 } else { 0xfffd });
        } else if t >= 0xf0 {
            let c = (t & 7) << 18 | at(i) << 12 | at(i + 1) << 6 | at(i + 2);
            i += 3;
            if !(0x10000..=0x10ffff).contains(&c) {
                out.push(0xfffd);
            } else {
                let c = c - 0x10000;
                out.push((0xd800 + (c >> 10)) as u16);
                out.push((0xdc00 + (c & 0x3ff)) as u16);
            }
        }
    }
    JsString::from_units(out)
}

fn basic_wire(ty: &str) -> Option<u32> {
    Some(match ty {
        "double" | "fixed64" | "sfixed64" => 1,
        "float" | "fixed32" | "sfixed32" => 5,
        "int32" | "uint32" | "sint32" | "int64" | "uint64" | "sint64" | "bool" => 0,
        "string" | "bytes" => 2,
        _ => return None,
    })
}

fn map_key_wire(ty: &str) -> u32 {
    match ty {
        "fixed32" | "sfixed32" => 5,
        "fixed64" | "sfixed64" => 1,
        "string" => 2,
        _ => 0,
    }
}

fn is_packable(ty: &str) -> bool {
    basic_wire(ty).is_some_and(|w| w != 2)
}

/// `util.safeProp`'s `.name` form for a name that starts with a digit: the SyntaxError V8
/// reports when compiling `m.<name>`.
pub(super) fn property_syntax_error(name: &JsString) -> Option<SyntaxKind> {
    let u = name.units();
    let word =
        |c: u16| u8::try_from(c).is_ok_and(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'$');
    let digit =
        |c: Option<&u16>| c.is_some_and(|&c| (u16::from(b'0')..=u16::from(b'9')).contains(&c));
    if u.is_empty() || !u.iter().all(|&c| word(c)) || !digit(u.first()) {
        return None;
    }
    let digits = |mut i: usize| -> Result<usize, SyntaxKind> {
        while i < u.len() {
            if digit(u.get(i)) {
                i += 1;
            } else if u[i] == u16::from(b'_') {
                if u.get(i + 1) == Some(&u16::from(b'_')) {
                    return Err(SyntaxKind::DoubleSeparator);
                }
                if !digit(u.get(i + 1)) {
                    return Err(SyntaxKind::TrailingSeparator);
                }
                i += 1;
            } else {
                break;
            }
        }
        Ok(i)
    };
    let mut i = match digits(0) {
        Ok(i) => i,
        Err(kind) => return Some(kind),
    };
    if i < u.len() && (u[i] == u16::from(b'e') || u[i] == u16::from(b'E')) {
        if !digit(u.get(i + 1)) {
            return Some(SyntaxKind::InvalidToken);
        }
        i = match digits(i + 1) {
            Ok(i) => i,
            Err(kind) => return Some(kind),
        };
    }
    Some(if i < u.len() {
        SyntaxKind::InvalidToken
    } else {
        SyntaxKind::UnexpectedNumber
    })
}

/// How a generated decoder `case` ends: `break` to the unknown-field path, run the oneof
/// setter, or `continue` straight away (the packed branch, which skips the setter).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Case {
    Break,
    Set,
    Continue,
}

/// What `m.x` evaluates to: an own value, a prototype `defaultValue`, or a member the
/// runtime resolves without a stored value (oneof getter, `$type`, prototype functions,
/// boxed-primitive members).
enum Prop<'m> {
    Val(&'m Val),
    Default(JsDefault),
    /// `ctor.prototype.$type`: the message's `Type` object.
    TypeObj(NodeId),
    /// A function on a prototype chain; the string is the member name (also the
    /// function's `.name`).
    Function(JsString),
    Str(JsString),
    Num(f64),
    Bool(bool),
    Undefined,
}

fn val_truthy(v: &Val) -> bool {
    match v {
        Val::Num(x) => *x != 0.0 && !x.is_nan(),
        Val::Bool(b) => *b,
        Val::Str(s) => !s.units().is_empty(),
        Val::Undefined => false,
        _ => true,
    }
}

fn prop_truthy(p: &Prop<'_>) -> bool {
    match p {
        Prop::Val(v) => val_truthy(v),
        Prop::Default(d) => d.is_truthy(),
        Prop::TypeObj(_) | Prop::Function(_) => true,
        Prop::Str(s) => !s.units().is_empty(),
        Prop::Num(x) => *x != 0.0 && !x.is_nan(),
        Prop::Bool(b) => *b,
        Prop::Undefined => false,
    }
}

fn prop_nullish(p: &Prop<'_>) -> bool {
    match p {
        Prop::Undefined | Prop::Val(Val::Undefined) => true,
        Prop::Default(d) => d.is_nullish(),
        _ => false,
    }
}

/// `util.safeProp(name)`: `.name` for identifiers, `["name"]` otherwise.
fn safe_prop(name: &JsString) -> JsString {
    let u = name.units();
    let first_ok = |c: u16| {
        c == u16::from(b'$')
            || c == u16::from(b'_')
            || u8::try_from(c).is_ok_and(|c| c.is_ascii_alphabetic())
    };
    let ident = |c: u16| first_ok(c) || u8::try_from(c).is_ok_and(|c| c.is_ascii_digit());
    let is_ident = !u.is_empty() && first_ok(u[0]) && u.iter().all(|&c| ident(c));
    if is_ident {
        js(&format!(".{}", name.to_string_lossy()))
    } else {
        js(&format!("[\"{}\"]", name.to_string_lossy()))
    }
}

const ARRAY_MEMBERS: &[&str] = &[
    "at",
    "concat",
    "copyWithin",
    "entries",
    "every",
    "fill",
    "filter",
    "find",
    "findIndex",
    "findLast",
    "findLastIndex",
    "flat",
    "flatMap",
    "forEach",
    "includes",
    "indexOf",
    "join",
    "keys",
    "lastIndexOf",
    "map",
    "pop",
    "push",
    "reduce",
    "reduceRight",
    "reverse",
    "shift",
    "slice",
    "some",
    "sort",
    "splice",
    "toLocaleString",
    "toReversed",
    "toSorted",
    "toSpliced",
    "toString",
    "unshift",
    "values",
    "valueOf",
    "with",
];

fn is_array_member(name: &JsString) -> bool {
    ARRAY_MEMBERS.iter().any(|m| js_eq(name, m)) || is_object_prototype_name(name)
}

fn is_fn_member(name: &JsString) -> bool {
    matches!(name.to_string_lossy().as_str(), "call" | "apply" | "bind")
        || is_object_prototype_name(name)
}

/// `fn.length` (the declared parameter count) for the prototype functions modeled here.
fn arity(name: &JsString) -> f64 {
    match name.to_string_lossy().as_str() {
        "isPrototypeOf"
        | "propertyIsEnumerable"
        | "hasOwnProperty"
        | "constructor"
        | "__lookupGetter__"
        | "__lookupSetter__"
        | "call"
        | "bind"
        | "push"
        | "concat"
        | "join"
        | "indexOf"
        | "lastIndexOf"
        | "forEach"
        | "map"
        | "filter"
        | "every"
        | "some"
        | "find"
        | "findIndex"
        | "findLast"
        | "findLastIndex"
        | "includes"
        | "at"
        | "flat"
        | "flatMap"
        | "unshift"
        | "fill"
        | "toSpliced" => 1.0,
        "__defineGetter__" | "__defineSetter__" | "apply" | "slice" | "reduce" | "reduceRight"
        | "splice" | "sort" | "with" | "copyWithin" => 2.0,
        _ => 0.0,
    }
}

/// `array.length`: one past the greatest index prop.
fn array_len(m: &Message) -> u32 {
    m.own
        .iter()
        .filter_map(|(k, _)| array_index(k.units()))
        .max()
        .map_or(0, |i| i + 1)
}

/// `hasOwnProperty` truth on the modeled object.
fn has_own(m: &Message, name: &JsString) -> bool {
    m.own.get_key(name).is_some()
        || (js_eq(name, "$unknowns") && m.hidden.is_some())
        || (matches!(m.foreign, Some(Foreign::Array)) && js_eq(name, "length"))
        || (matches!(m.foreign, Some(Foreign::Function { .. }))
            && (js_eq(name, "name") || js_eq(name, "length")))
}

/// `delete m.<name>`: removes the own prop, including the `$unknowns` slot.
fn delete_prop(m: &mut Message, name: &JsString) {
    m.own.remove_key(name);
    if js_eq(name, "$unknowns") {
        m.hidden = None;
    }
}

/// The `<type> '<value>'` tail of `Cannot create property 'x' on <type> '<value>'`.
fn scalar_desc(v: &Val) -> JsString {
    let (kind, text) = match v {
        Val::Num(x) => ("number", number::to_string(*x)),
        Val::Str(s) => ("string", s.to_string_lossy()),
        Val::Bool(b) => ("boolean", if *b { "true".into() } else { "false".into() }),
        _ => ("object", String::new()),
    };
    js(&format!("{kind} '{text}'"))
}

fn prop_scalar_desc(p: &Prop<'_>) -> JsString {
    scalar_desc(&match p {
        Prop::Val(v) => (*v).clone(),
        Prop::Str(s) => Val::Str(s.clone()),
        Prop::Num(x) => Val::Num(*x),
        Prop::Bool(b) => Val::Bool(*b),
        Prop::Default(d) => default_val(d),
        _ => Val::Undefined,
    })
}

/// A truthy `JsDefault` as the merge-target value `decode` receives.
fn default_val(d: &JsDefault) -> Val {
    match d {
        JsDefault::Undefined | JsDefault::Null => Val::Undefined,
        JsDefault::Bool(b) => Val::Bool(*b),
        JsDefault::Number(x) => Val::Num(*x),
        JsDefault::String(s) => Val::Str(s.clone()),
        JsDefault::Bytes(b) => Val::Bytes(b.clone()),
        JsDefault::EmptyArray => Val::Array(Vec::new()),
        JsDefault::EmptyObject => Val::Map(JsObject::new()),
        JsDefault::Long(l) => Val::Long(*l),
        JsDefault::Function(_) => Val::Undefined,
        JsDefault::Object(o) => {
            let mut map = JsObject::new();
            for (k, v) in o.iter() {
                map.insert(k.clone(), Val::Str(v.clone()));
            }
            Val::Map(map)
        }
    }
}

/// The `Message` `decode` merges into when the target `m.x` resolves to a truthy
/// non-message value: enumerable own props go to `own`, the prototype to `foreign`.
fn foreign_message(ty: NodeId, v: Val) -> Message {
    let (own, foreign) = match v {
        Val::Array(items) => {
            let mut own = JsObject::new();
            for (i, item) in items.into_iter().enumerate() {
                own.insert(js(&number::to_string(i as f64)), item);
            }
            (own, Foreign::Array)
        }
        Val::Bytes(b) => {
            let mut own = JsObject::new();
            for (i, byte) in b.iter().enumerate() {
                own.insert(js(&number::to_string(i as f64)), Val::Num(f64::from(*byte)));
            }
            (own, Foreign::Bytes)
        }
        Val::Long(l) => {
            let mut own = JsObject::new();
            own.insert(js("low"), Val::Num(f64::from(l.low)));
            own.insert(js("high"), Val::Num(f64::from(l.high)));
            own.insert(js("unsigned"), Val::Bool(l.unsigned));
            (own, Foreign::Long)
        }
        Val::Map(map) => (map, Foreign::Object),
        Val::Msg(_) => unreachable!("messages merge into messages"),
        scalar @ (Val::Num(_) | Val::Bool(_) | Val::Str(_) | Val::Undefined) => {
            (JsObject::new(), Foreign::Scalar(scalar))
        }
    };
    Message {
        ty,
        own,
        hidden: None,
        foreign: Some(foreign),
    }
}

/// `v.<name>` on a decoded value: own props first (maps carry entries, foreign props are
/// in `own`), then the boxing prototype.
fn str_prop(s: &JsString, name: &JsString) -> Prop<'static> {
    if js_eq(name, "length") {
        Prop::Num(s.len() as f64)
    } else if let Some(i) = array_index(name.units()) {
        s.units().get(i as usize).map_or(Prop::Undefined, |&u| {
            Prop::Str(JsString::from_units(vec![u]))
        })
    } else if is_object_prototype_name(name) {
        Prop::Function(name.clone())
    } else {
        Prop::Undefined
    }
}

fn boxed_prop(name: &JsString) -> Prop<'static> {
    if is_object_prototype_name(name) {
        Prop::Function(name.clone())
    } else {
        Prop::Undefined
    }
}

/// `d.<name>` on a prototype `defaultValue`.
fn default_prop(d: &JsDefault, name: &JsString) -> Prop<'static> {
    match d {
        JsDefault::Undefined | JsDefault::Null => Prop::Undefined,
        JsDefault::Bool(_) | JsDefault::Number(_) => boxed_prop(name),
        JsDefault::String(s) => str_prop(s, name),
        JsDefault::Bytes(b) => {
            if js_eq(name, "length") {
                Prop::Num(b.len() as f64)
            } else if let Some(i) = array_index(name.units()) {
                b.get(i as usize)
                    .map_or(Prop::Undefined, |&x| Prop::Num(f64::from(x)))
            } else {
                boxed_prop(name)
            }
        }
        JsDefault::EmptyArray => {
            if js_eq(name, "length") {
                Prop::Num(0.0)
            } else if is_array_member(name) {
                Prop::Function(name.clone())
            } else {
                Prop::Undefined
            }
        }
        JsDefault::EmptyObject => boxed_prop(name),
        JsDefault::Long(l) => {
            if js_eq(name, "low") {
                Prop::Num(f64::from(l.low))
            } else if js_eq(name, "high") {
                Prop::Num(f64::from(l.high))
            } else if js_eq(name, "unsigned") {
                Prop::Bool(l.unsigned)
            } else {
                boxed_prop(name)
            }
        }
        JsDefault::Function(n) => {
            if js_eq(name, "name") {
                Prop::Str(n.clone())
            } else if js_eq(name, "length") {
                Prop::Num(arity(n))
            } else if is_fn_member(name) {
                Prop::Function(name.clone())
            } else {
                Prop::Undefined
            }
        }
        JsDefault::Object(o) => o
            .get_key(name)
            .map_or_else(|| boxed_prop(name), |v| Prop::Str(v.clone())),
    }
}

/// `Object.keys(p)` for `m.x` values: enumerable own props only.
fn js_keys(p: &Prop<'_>) -> Vec<JsString> {
    let index_keys = |n: usize| (0..n).map(|i| js(&number::to_string(i as f64))).collect();
    match p {
        Prop::Val(Val::Map(map)) => map.keys().cloned().collect(),
        Prop::Val(Val::Msg(m)) => m.own.keys().cloned().collect(),
        Prop::Val(Val::Array(items)) => index_keys(items.len()),
        Prop::Val(Val::Bytes(b)) => index_keys(b.len()),
        Prop::Val(Val::Str(s)) | Prop::Str(s) => index_keys(s.len()),
        Prop::Val(Val::Long(_)) => vec![js("low"), js("high"), js("unsigned")],
        Prop::Default(JsDefault::String(s)) => index_keys(s.len()),
        Prop::Default(JsDefault::Bytes(b)) => index_keys(b.len()),
        Prop::Default(JsDefault::Long(_)) => vec![js("low"), js("high"), js("unsigned")],
        Prop::Default(JsDefault::Object(o)) => o.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

impl Reflection {
    fn field_ty_name(&self, f: FieldId) -> String {
        let field = &self.fields[f];
        if field.resolved_type.is_some_and(|t| self.is_enum(t)) {
            "int32".to_owned()
        } else {
            field.ty.to_string_lossy()
        }
    }

    fn message_type(&self, f: FieldId) -> Option<NodeId> {
        self.fields[f].resolved_type.filter(|&t| self.is_type(t))
    }

    fn sorted_by_id(&self, fields: &[FieldId]) -> Vec<FieldId> {
        let mut sorted = fields.to_vec();
        array_sort(&mut sorted, |&a, &b| self.fields[a].id - self.fields[b].id);
        sorted
    }

    /// `m.<name>`: own enumerable props, then the non-enumerable `$unknowns` slot, then
    /// the prototype of whatever `m` stands in for.
    fn prop<'m>(&self, m: &'m Message, name: &JsString) -> Prop<'m> {
        if let Some(v) = m.own.get_key(name) {
            return Prop::Val(v);
        }
        if js_eq(name, "$unknowns")
            && let Some(v) = &m.hidden
        {
            return Prop::Val(v);
        }
        match &m.foreign {
            Some(foreign) => foreign_prop(m, foreign, name),
            None => self.proto_prop(m.ty, name, m),
        }
    }

    /// `ctor.prototype.<name>`: the oneof accessor, the field's data-property default,
    /// `$type`, then `Object.prototype` members.
    fn proto_prop<'m>(&self, ty: NodeId, name: &JsString, m: &'m Message) -> Prop<'m> {
        let data = self.type_data(ty);
        if let Some(oneofs) = &data.oneofs
            && let Some(&o) = oneofs.get_key(name)
        {
            return match self.oneof_get(m, o) {
                Some(member) => Prop::Str(member),
                None => Prop::Undefined,
            };
        }
        if let Some(&f) = data.fields.get_key(name) {
            return Prop::Default(self.fields[f].default_value.clone());
        }
        if js_eq(name, "$type") {
            return Prop::TypeObj(ty);
        }
        if is_object_prototype_name(name) {
            return Prop::Function(name.clone());
        }
        Prop::Undefined
    }

    /// The oneof getter: the last member in `keys` order still present and not
    /// `undefined`.
    fn oneof_get(&self, m: &Message, o: usize) -> Option<JsString> {
        let members = self.oneofs[o].oneof.clone();
        m.own
            .keys()
            .rev()
            .find(|k| {
                members.iter().any(|n| n == *k) && !matches!(m.own.get_key(k), Some(Val::Undefined))
            })
            .cloned()
    }

    /// `v.<name>` on a decoded value.
    fn v_prop<'m>(&self, v: &'m Val, name: &JsString) -> Prop<'m> {
        match v {
            Val::Msg(mm) => self.prop(mm, name),
            Val::Map(map) => map.get_key(name).map_or(Prop::Undefined, Prop::Val),
            Val::Array(items) => {
                if js_eq(name, "length") {
                    Prop::Num(items.len() as f64)
                } else if let Some(i) = array_index(name.units()) {
                    items.get(i as usize).map_or(Prop::Undefined, Prop::Val)
                } else if is_array_member(name) {
                    Prop::Function(name.clone())
                } else {
                    Prop::Undefined
                }
            }
            Val::Bytes(b) => {
                if js_eq(name, "length") {
                    Prop::Num(b.len() as f64)
                } else if let Some(i) = array_index(name.units()) {
                    b.get(i as usize)
                        .map_or(Prop::Undefined, |&x| Prop::Num(f64::from(x)))
                } else {
                    boxed_prop(name)
                }
            }
            Val::Long(l) => {
                if js_eq(name, "low") {
                    Prop::Num(f64::from(l.low))
                } else if js_eq(name, "high") {
                    Prop::Num(f64::from(l.high))
                } else if js_eq(name, "unsigned") {
                    Prop::Bool(l.unsigned)
                } else {
                    boxed_prop(name)
                }
            }
            Val::Str(s) => str_prop(s, name),
            Val::Num(_) | Val::Bool(_) | Val::Undefined => boxed_prop(name),
        }
    }

    /// `p.<name>` for a resolved `m.x`.
    fn prop_member<'m>(&self, p: &Prop<'m>, name: &JsString) -> Prop<'m> {
        match p {
            Prop::Val(v) => self.v_prop(v, name),
            Prop::Str(s) => str_prop(s, name),
            Prop::Num(_) | Prop::Bool(_) => boxed_prop(name),
            Prop::Function(n) => {
                if js_eq(name, "name") {
                    Prop::Str(n.clone())
                } else if js_eq(name, "length") {
                    Prop::Num(arity(n))
                } else if is_fn_member(name) {
                    Prop::Function(name.clone())
                } else {
                    Prop::Undefined
                }
            }
            Prop::Default(d) => default_prop(d, name),
            Prop::TypeObj(ty) => {
                if js_eq(name, "name") {
                    Prop::Str(self.nodes[*ty].name.clone())
                } else {
                    Prop::Undefined
                }
            }
            Prop::Undefined => Prop::Undefined,
        }
    }

    /// `m.<name> = v`: own and hidden props shadow the prototype; the oneof setter runs
    /// only for an accessor name; writes onto a primitive throw.
    fn write_prop(&mut self, m: &mut Message, name: &JsString, v: Val) -> Result<(), ProtoError> {
        if m.own.get_key(name).is_some() {
            m.own.insert(name.clone(), v);
            return Ok(());
        }
        if js_eq(name, "$unknowns") && m.hidden.is_some() {
            m.hidden = Some(v);
            return Ok(());
        }
        match &m.foreign {
            Some(Foreign::Scalar(scalar)) => {
                return Err(ProtoError::CannotCreate {
                    name: name.clone(),
                    on: scalar_desc(scalar),
                });
            }
            Some(_) => {}
            None => {
                let accessor = self
                    .type_data(m.ty)
                    .oneofs
                    .as_ref()
                    .and_then(|oneofs| oneofs.get_key(name).copied());
                if let Some(o) = accessor {
                    // `util.oneOfSetter`: `delete this[member]` for every member but the
                    // assigned name.
                    let keep = match &v {
                        Val::Str(s) => Some(s.clone()),
                        _ => None,
                    };
                    for member in self.oneofs[o].oneof.clone() {
                        if Some(&member) != keep.as_ref() {
                            delete_prop(m, &member);
                        }
                    }
                    return Ok(());
                }
            }
        }
        m.own.insert(name.clone(), v);
        Ok(())
    }

    /// `m.hasOwnProperty(name)`: resolves the member itself, which an own or prototype
    /// value can shadow with a non-function.
    fn call_has_own(&self, m: &Message, name: &JsString) -> Result<bool, ProtoError> {
        match self.prop(m, &js("hasOwnProperty")) {
            Prop::Function(_) | Prop::Default(JsDefault::Function(_)) => Ok(has_own(m, name)),
            _ => Err(ProtoError::DynNotAFunction(js("m.hasOwnProperty"))),
        }
    }

    /// `m.<name>.push(v)`, re-reading `m.<name>` like generated code does.
    fn push(&mut self, m: &mut Message, name: &JsString, v: Val) -> Result<(), ProtoError> {
        let v = match v {
            Val::Num(x) if x.is_nan() => {
                Val::Num(f64::from_bits(x.to_bits() | 0x0008_0000_0000_0000))
            }
            other => other,
        };
        let arr_msg = |mm: &Message| matches!(mm.foreign, Some(Foreign::Array));
        let in_own = m.own.get_key(name);
        let in_hidden = if js_eq(name, "$unknowns") {
            m.hidden.as_ref()
        } else {
            None
        };
        let pushable = matches!(in_own, Some(Val::Array(_)))
            || matches!(in_hidden, Some(Val::Array(_)))
            || matches!(in_own, Some(Val::Msg(mm)) if arr_msg(mm))
            || matches!(in_hidden, Some(Val::Msg(mm)) if arr_msg(mm));
        if pushable {
            if let Some(Val::Array(items)) = m.own.get_units_mut(name.units()) {
                items.push(v);
                return Ok(());
            }
            if js_eq(name, "$unknowns")
                && let Some(Val::Array(items)) = m.hidden.as_mut()
            {
                items.push(v);
                return Ok(());
            }
            let slot = m.own.get_units_mut(name.units()).or_else(|| {
                if js_eq(name, "$unknowns") {
                    m.hidden.as_mut()
                } else {
                    None
                }
            });
            if let Some(Val::Msg(mm)) = slot {
                let len = array_len(mm);
                mm.own.insert(js(&number::to_string(f64::from(len))), v);
            }
            return Ok(());
        }
        let p = self.prop(m, name);
        match p {
            Prop::Undefined | Prop::Val(Val::Undefined) => Err(ProtoError::ReadProp {
                nullish: "undefined",
                name: js("push"),
            }),
            Prop::Default(JsDefault::Null) => Err(ProtoError::ReadProp {
                nullish: "null",
                name: js("push"),
            }),
            _ => {
                let mut expr = js("m");
                expr.push_str(&safe_prop(name));
                expr.push_str(&js(".push"));
                Err(ProtoError::DynNotAFunction(expr))
            }
        }
    }

    /// The tail of an unknown-field `case`: `r.skipType` already ran, so this is
    /// `util.makeProp(m, "$unknowns", false)` followed by
    /// `(m.$unknowns || (m.$unknowns = [])).push(r.raw(s, r.pos))`.
    fn push_unknown(&mut self, m: &mut Message, r: &Reader<'_>, s: u64) -> Result<(), ProtoError> {
        let name = js("$unknowns");
        if !has_own(m, &name) {
            if matches!(m.foreign, Some(Foreign::Scalar(_))) {
                return Err(ProtoError::DefinePrimitive);
            }
            m.hidden = Some(Val::Undefined);
        }
        let raw = Val::Bytes(r.slice(s, r.pos));
        enum T {
            OwnArray,
            HiddenArray,
            MsgArray,
            Falsy,
            NoPush,
        }
        let target = match self.prop(m, &name) {
            Prop::Val(Val::Array(_)) => {
                if m.own.get_key(&name).is_some() {
                    T::OwnArray
                } else {
                    T::HiddenArray
                }
            }
            Prop::Val(Val::Msg(mm)) if matches!(mm.foreign, Some(Foreign::Array)) => T::MsgArray,
            Prop::Val(v) if val_truthy(v) => T::NoPush,
            Prop::Val(_) => T::Falsy,
            p if prop_truthy(&p) => T::NoPush,
            _ => T::Falsy,
        };
        match target {
            T::OwnArray => {
                if let Some(Val::Array(items)) = m.own.get_units_mut(name.units()) {
                    items.push(raw);
                }
            }
            T::HiddenArray => {
                if let Some(Val::Array(items)) = m.hidden.as_mut() {
                    items.push(raw);
                }
            }
            T::MsgArray => {
                let slot = m.own.get_units_mut(name.units()).or_else(|| {
                    if js_eq(&name, "$unknowns") {
                        m.hidden.as_mut()
                    } else {
                        None
                    }
                });
                if let Some(Val::Msg(mm)) = slot {
                    let len = array_len(mm);
                    mm.own.insert(js(&number::to_string(f64::from(len))), raw);
                }
            }
            T::NoPush => return Err(ProtoError::DynNotAFunction(js("m.$unknowns.push"))),
            T::Falsy => self.write_prop(m, &name, Val::Array(vec![raw]))?,
        }
        Ok(())
    }

    /// The `Type#ctor` getter: builds the generated constructor once.
    fn ensure_ctor(&mut self, ty: NodeId) -> Result<(), ProtoError> {
        if self.type_data(ty).ctor.is_some() {
            return Ok(());
        }
        let fields = self.fields_array(ty);
        let initialised: Vec<FieldId> = fields
            .iter()
            .copied()
            .filter(|&f| self.fields[f].map_key.is_some() || self.fields[f].repeated)
            .collect();
        for &f in &initialised {
            if let Some(kind) = property_syntax_error(&self.fields[f].name) {
                return Err(ProtoError::Syntax(kind));
            }
        }
        self.type_data_mut(ty).ctor = Some(initialised);
        for f in fields {
            self.resolve_field(f)?;
        }
        Ok(())
    }

    /// `Type#setup()`: resolves fields, then generates (in order) the encoder, decoder,
    /// verifier, fromObject and toObject functions.
    fn setup(&mut self, ty: NodeId) -> Result<(), ProtoError> {
        let fields = self.fields_array(ty);
        for &f in &fields {
            self.resolve_field(f)?;
        }
        for f in self.sorted_by_id(&fields) {
            if let Some(kind) = property_syntax_error(&self.fields[f].name) {
                return Err(ProtoError::Syntax(kind));
            }
        }
        self.ensure_ctor(ty)?;
        for &f in &fields {
            if let Some(o) = self.fields[f].part_of
                && let Some(kind) = property_syntax_error(&self.oneofs[o].name)
            {
                return Err(ProtoError::Syntax(kind));
            }
        }
        self.type_data_mut(ty).decode_ready = true;
        for f in self.sorted_by_id(&fields) {
            let field = &self.fields[f];
            if field.part_of.is_some()
                || field.repeated
                || field.map_key.is_some()
                || !field.is_bytes()
            {
                continue;
            }
            match field.type_default {
                JsDefault::Null | JsDefault::Undefined => {
                    return Err(ProtoError::CannotConvertToObject);
                }
                JsDefault::Number(_) | JsDefault::Bool(_) | JsDefault::Long(_) => {
                    return Err(ProtoError::NotArrayLike);
                }
                _ => {}
            }
        }
        self.type_data_mut(ty).to_object = Some(fields);
        Ok(())
    }

    fn new_instance(&mut self, ty: NodeId) -> Result<Message, ProtoError> {
        self.ensure_ctor(ty)?;
        let mut own = JsObject::new();
        for &f in self.type_data(ty).ctor.as_ref().expect("ctor built") {
            let field = &self.fields[f];
            let value = if field.map_key.is_some() {
                Val::Map(JsObject::new())
            } else {
                Val::Array(Vec::new())
            };
            own.insert(field.name.clone(), value);
        }
        Ok(Message {
            ty,
            own,
            hidden: None,
            foreign: None,
        })
    }

    /// `Type#decode(reader, length, undefined, depth, target)`.
    pub(super) fn decode(
        &mut self,
        ty: NodeId,
        r: &mut Reader<'_>,
        length: Option<u64>,
        depth: u32,
        target: Option<Message>,
    ) -> Result<Message, ProtoError> {
        if !self.type_data(ty).decode_ready {
            self.setup(ty)?;
        }
        if depth > RECURSION_LIMIT {
            return Err(ProtoError::MaxDepth);
        }
        let end = length.map_or(r.len(), |l| r.pos + l);
        let mut m = match target {
            Some(m) => m,
            None => self.new_instance(ty)?,
        };
        let fields = self.fields_array(ty);
        while r.pos < end {
            let s = r.pos;
            let tag = r.tag()?;
            if fields.is_empty() {
                // `r.skipType(t&7, q, t)`: no `switch` ran, so `fieldNumber` is the
                // whole tag: `fieldNumber === 0` only for a literal zero byte.
                r.skip_type(tag & 7, depth, Some(tag))?;
                self.push_unknown(&mut m, r, s)?;
                continue;
            }
            let wire = tag & 7;
            let number = tag >> 3;
            let field = fields
                .iter()
                .copied()
                .find(|&f| self.fields[f].id == f64::from(number));
            let case = match field {
                Some(f) => self.decode_field(f, &mut m, r, wire, depth)?,
                None => Case::Break,
            };
            if case == Case::Break {
                r.skip_type(wire, depth, Some(number))?;
                self.push_unknown(&mut m, r, s)?;
            } else if let (Case::Set, Some(f), Some(o)) =
                (case, field, field.and_then(|f| self.fields[f].part_of))
            {
                // `m.<accessor> = "<member>"`: the oneof setter deletes sibling members.
                let accessor = self.oneofs[o].name.clone();
                let member = self.fields[f].name.clone();
                self.write_prop(&mut m, &accessor, Val::Str(member))?;
            }
        }
        for f in fields {
            let field = &self.fields[f];
            if field.required && !has_own(&m, &field.name) {
                return Err(ProtoError::MissingRequired(field.name.clone()));
            }
        }
        Ok(m)
    }

    /// One `case` of the generated decoder.
    fn decode_field(
        &mut self,
        f: FieldId,
        m: &mut Message,
        r: &mut Reader<'_>,
        wire: u32,
        depth: u32,
    ) -> Result<Case, ProtoError> {
        let ty = self.field_ty_name(f);
        let name = self.fields[f].name.clone();
        let basic = basic_wire(&ty);
        if let Some(key_ty) = self.fields[f].map_key.clone() {
            if wire != 2 {
                return Ok(Case::Break);
            }
            // `if (m.x === util.emptyObject) m.x = {}`: only the shared prototype
            // default gets replaced; `m.x[k] = v` then writes onto whatever `m.x` is.
            let reset = matches!(self.prop(m, &name), Prop::Default(JsDefault::EmptyObject));
            if reset {
                self.write_prop(m, &name, Val::Map(JsObject::new()))?;
            }
            let key_ty = key_ty.to_string_lossy();
            let end = u64::from(r.uint32()?) + r.pos;
            let mut key = match key_ty.as_str() {
                "string" => Val::Str(JsString::new()),
                "bool" => Val::Bool(false),
                _ => Val::Num(0.0),
            };
            let mut value: Option<Val> = match ty.as_str() {
                "bool" => Some(Val::Bool(false)),
                "string" => Some(Val::Str(JsString::new())),
                "bytes" => Some(Val::Array(Vec::new())),
                t if basic_wire(t).is_some() => Some(Val::Num(0.0)),
                _ => None,
            };
            while r.pos < end {
                let tag = r.tag()?;
                let w = tag & 7;
                let n = tag >> 3;
                if n == 1 && w == map_key_wire(&key_ty) {
                    key = r.read(&key_ty)?;
                    continue;
                }
                if n == 2 && w == basic.unwrap_or(2) {
                    value = Some(match basic {
                        Some(_) => r.read(&ty)?,
                        None => {
                            let vt = self.message_type(f).ok_or(ProtoError::ReadNull("decode"))?;
                            let len = u64::from(r.uint32()?);
                            Val::Msg(Box::new(self.decode(vt, r, Some(len), depth + 1, None)?))
                        }
                    });
                    continue;
                }
                r.skip_type(w, depth, Some(n))?;
            }
            let key = match key {
                Val::Long(l) => l.to_hash(),
                Val::Str(s) => s,
                Val::Bool(b) => js(if b { "true" } else { "false" }),
                Val::Num(x) => js(&number::to_string(x)),
                _ => unreachable!("map keys are scalars"),
            };
            let value = match value {
                Some(v) => v,
                None => {
                    let vt = self.message_type(f).ok_or(ProtoError::ReadNull("ctor"))?;
                    Val::Msg(Box::new(self.new_instance(vt)?))
                }
            };
            // `if (k === "__proto__") util.makeProp(m.x, k)` runs first for string-key
            // maps: on a primitive `m.x`, defineProperty throws before `m.x[k] = v`.
            let proto_key = js_eq(&key, "__proto__") && key_ty == "string";
            // `m.x[k] = v`: writes onto objects, throws on nullish and primitives.
            enum Entry {
                InPlace,
                Seed(JsObject<Val>),
                Scalar(JsString),
            }
            let entry = {
                let p = self.prop(m, &name);
                match &p {
                    Prop::Val(Val::Map(_)) => Entry::InPlace,
                    Prop::Undefined | Prop::Val(Val::Undefined) => {
                        if proto_key {
                            return Err(ProtoError::DefinePrimitive);
                        }
                        return Err(ProtoError::CannotSet {
                            on: "undefined",
                            name: key,
                        });
                    }
                    Prop::Default(JsDefault::Null) => {
                        if proto_key {
                            return Err(ProtoError::DefinePrimitive);
                        }
                        return Err(ProtoError::CannotSet {
                            on: "null",
                            name: key,
                        });
                    }
                    _ => match seed_object(&p) {
                        Some(map) => Entry::Seed(map),
                        None => {
                            if proto_key {
                                return Err(ProtoError::DefinePrimitive);
                            }
                            Entry::Scalar(prop_scalar_desc(&p))
                        }
                    },
                }
            };
            match entry {
                Entry::InPlace => {
                    let slot = m.own.get_units_mut(name.units()).or_else(|| {
                        if js_eq(&name, "$unknowns") {
                            m.hidden.as_mut()
                        } else {
                            None
                        }
                    });
                    if let Some(Val::Map(map)) = slot {
                        map.insert(key, value);
                    }
                }
                Entry::Seed(mut map) => {
                    map.insert(key, value);
                    m.own.insert(name, Val::Map(map));
                }
                Entry::Scalar(on) => return Err(ProtoError::CannotCreate { name: key, on }),
            }
            return Ok(Case::Set);
        }
        if self.fields[f].repeated {
            // `if (!(m.x && m.x.length)) m.x = []`, then `m.x.push(v)` per element.
            let keep = {
                let p = self.prop(m, &name);
                prop_truthy(&p) && prop_truthy(&self.prop_member(&p, &js("length")))
            };
            if is_packable(&ty) && wire == 2 {
                if !keep {
                    self.write_prop(m, &name, Val::Array(Vec::new()))?;
                }
                let end = u64::from(r.uint32()?) + r.pos;
                while r.pos < end {
                    let v = r.read(&ty)?;
                    self.push(m, &name, v)?;
                }
                return Ok(Case::Continue);
            }
            if wire != basic.unwrap_or(2) {
                return Ok(Case::Break);
            }
            if !keep {
                self.write_prop(m, &name, Val::Array(Vec::new()))?;
            }
            let v = match basic {
                Some(_) => r.read(&ty)?,
                None => {
                    let vt = self.message_type(f).ok_or(ProtoError::ReadNull("decode"))?;
                    let len = u64::from(r.uint32()?);
                    Val::Msg(Box::new(self.decode(vt, r, Some(len), depth + 1, None)?))
                }
            };
            self.push(m, &name, v)?;
            return Ok(Case::Set);
        }
        match basic {
            None => {
                if wire != 2 {
                    return Ok(Case::Break);
                }
                let vt = self.message_type(f).ok_or(ProtoError::ReadNull("decode"))?;
                let len = u64::from(r.uint32()?);
                // `m.x = types[i].decode(r, len, d+1, m.x)`: a truthy `m.x` is the merge
                // target, whatever kind of value it is.
                enum Merge {
                    Msg,
                    Val(Val),
                    Fn(JsString),
                }
                let target = {
                    let p = self.prop(m, &name);
                    if !prop_truthy(&p) {
                        None
                    } else {
                        Some(match p {
                            Prop::Val(Val::Msg(_)) => Merge::Msg,
                            Prop::Val(v) => Merge::Val(v.clone()),
                            Prop::Default(JsDefault::Function(n)) => Merge::Fn(n),
                            Prop::Default(d) => Merge::Val(default_val(&d)),
                            Prop::Function(n) => Merge::Fn(n),
                            Prop::Str(s) => Merge::Val(Val::Str(s)),
                            Prop::Num(x) => Merge::Val(Val::Num(x)),
                            Prop::Bool(b) => Merge::Val(Val::Bool(b)),
                            Prop::TypeObj(_) | Prop::Undefined => {
                                unreachable!("merge targets are truthy field-name reads")
                            }
                        })
                    }
                };
                let target = match target {
                    Some(Merge::Msg) => {
                        let slot = m.own.get_units_mut(name.units()).or_else(|| {
                            if js_eq(&name, "$unknowns") {
                                m.hidden.as_mut()
                            } else {
                                None
                            }
                        });
                        match slot {
                            Some(Val::Msg(existing)) => Some(std::mem::replace(
                                existing.as_mut(),
                                Message {
                                    ty: vt,
                                    own: JsObject::new(),
                                    hidden: None,
                                    foreign: None,
                                },
                            )),
                            _ => None,
                        }
                    }
                    Some(Merge::Val(v)) => Some(foreign_message(vt, v)),
                    Some(Merge::Fn(n)) => Some(Message {
                        ty: vt,
                        own: JsObject::new(),
                        hidden: None,
                        foreign: Some(Foreign::Function {
                            arity: arity(&n),
                            name: n,
                        }),
                    }),
                    None => None,
                };
                let decoded = self.decode(vt, r, Some(len), depth + 1, target)?;
                self.write_prop(m, &name, Val::Msg(Box::new(decoded)))?;
            }
            Some(w) => {
                if wire != w {
                    return Ok(Case::Break);
                }
                let v = r.read(&ty)?;
                self.write_prop(m, &name, v)?;
            }
        }
        Ok(Case::Set)
    }

    /// `Type#toObject(message, {defaults: true})`.
    pub(super) fn message_to_object(
        &mut self,
        ty: NodeId,
        m: &Message,
    ) -> Result<JsValue, ProtoError> {
        if self.type_data(ty).to_object.is_none() {
            self.setup(ty)?;
        }
        let fields = self
            .type_data(ty)
            .to_object
            .clone()
            .expect("toObject generated");
        let sorted = self.sorted_by_id(&fields);
        let mut d = JsObject::new();
        if sorted.is_empty() {
            return Ok(JsValue::Object(d));
        }
        for &f in &sorted {
            let field = &self.fields[f];
            if field.part_of.is_none() && field.repeated {
                d.insert(field.name.clone(), JsValue::Array(Vec::new()));
            }
        }
        for &f in &sorted {
            let field = &self.fields[f];
            if field.part_of.is_none() && !field.repeated && field.map_key.is_some() {
                d.insert(field.name.clone(), JsValue::Object(JsObject::new()));
            }
        }
        for &f in &sorted {
            let field = &self.fields[f];
            if field.part_of.is_some() || field.repeated || field.map_key.is_some() {
                continue;
            }
            let value = if field.resolved_type.is_some_and(|t| self.is_enum(t)) {
                field.type_default.json_literal()
            } else if let JsDefault::Long(l) = field.type_default.clone() {
                l.to_js()
            } else if field.is_bytes() {
                match &field.type_default {
                    JsDefault::Bytes(b) => JsValue::Bytes(b.clone()),
                    _ => JsValue::Bytes(Vec::new()),
                }
            } else {
                field.type_default.json_literal()
            };
            d.insert(field.name.clone(), value);
        }
        for &f in &sorted {
            let name = self.fields[f].name.clone();
            let p = self.prop(m, &name);
            if self.fields[f].map_key.is_some() {
                // `if (m.x && (ks2 = Object.keys(m.x)).length) { d.x = {}; for k:
                // d.x[k] = convert(m.x[k]) }`
                if !prop_truthy(&p) {
                    continue;
                }
                let keys = js_keys(&p);
                if keys.is_empty() {
                    continue;
                }
                let key_ty = self.fields[f].map_key.clone().expect("map");
                let long_key = super::reflect::is_long_type(&key_ty);
                let unsigned = js_eq(&key_ty, "uint64") || js_eq(&key_ty, "fixed64");
                let mut out = JsObject::new();
                for k in keys {
                    let member = self.prop_member(&p, &k);
                    let converted = self.convert(f, &member)?;
                    let key = if long_key {
                        long_key_to_string(&k, unsigned)
                    } else {
                        k
                    };
                    out.insert(key, converted);
                }
                d.insert(name, JsValue::Object(out));
            } else if self.fields[f].repeated {
                // `if (m.x && m.x.length) { d.x = []; for j: d.x[j] = convert(m.x[j]) }`
                if !prop_truthy(&p) {
                    continue;
                }
                let Prop::Num(len) = self.prop_member(&p, &js("length")) else {
                    continue;
                };
                if !(len > 0.0 && len.is_finite()) {
                    continue;
                }
                let mut out = Vec::with_capacity(len as usize);
                for j in 0..len as usize {
                    let item = self.prop_member(&p, &js(&number::to_string(j as f64)));
                    out.push(self.convert(f, &item)?);
                }
                d.insert(name, JsValue::Array(out));
            } else {
                if prop_nullish(&p) {
                    continue;
                }
                if !self.call_has_own(m, &name)? {
                    continue;
                }
                let converted = self.convert(f, &p)?;
                d.insert(name, converted);
            }
        }
        Ok(JsValue::Object(d))
    }

    /// `genValuePartial_toObject`: `types[i].toObject(m.x, o)` for message fields,
    /// `d.x = m.x` for everything else (`o.enums`/`o.longs`/`o.bytes`/`o.json` are unset).
    fn convert(&mut self, f: FieldId, p: &Prop<'_>) -> Result<JsValue, ProtoError> {
        if let Some(vt) = self.message_type(f) {
            let em = match p {
                Prop::Val(Val::Msg(msg)) => return self.message_to_object(vt, msg),
                Prop::Val(v) => foreign_message(vt, (*v).clone()),
                Prop::Default(JsDefault::Undefined) => {
                    return Err(self.nullish_to_object_error(vt, false));
                }
                Prop::Default(JsDefault::Null) => {
                    return Err(self.nullish_to_object_error(vt, true));
                }
                Prop::Default(d) => foreign_message(vt, default_val(d)),
                Prop::Str(s) => {
                    let mut own = JsObject::new();
                    for (i, &u) in s.units().iter().enumerate() {
                        own.insert(
                            js(&number::to_string(i as f64)),
                            Val::Str(JsString::from_units(vec![u])),
                        );
                    }
                    Message {
                        ty: vt,
                        own,
                        hidden: None,
                        foreign: Some(Foreign::Scalar(Val::Str(s.clone()))),
                    }
                }
                Prop::Num(x) => Message {
                    ty: vt,
                    own: JsObject::new(),
                    hidden: None,
                    foreign: Some(Foreign::Scalar(Val::Num(*x))),
                },
                Prop::Bool(b) => Message {
                    ty: vt,
                    own: JsObject::new(),
                    hidden: None,
                    foreign: Some(Foreign::Scalar(Val::Bool(*b))),
                },
                Prop::Function(n) => Message {
                    ty: vt,
                    own: JsObject::new(),
                    hidden: None,
                    foreign: Some(Foreign::Function {
                        name: n.clone(),
                        arity: arity(n),
                    }),
                },
                Prop::TypeObj(_) => Message {
                    ty: vt,
                    own: JsObject::new(),
                    hidden: None,
                    foreign: Some(Foreign::Object),
                },
                Prop::Undefined => return Err(self.nullish_to_object_error(vt, false)),
            };
            return self.message_to_object(vt, &em);
        }
        // `d.x = m.x`: the raw value.
        Ok(match p {
            Prop::Val(v) => val_to_js(v)?,
            Prop::Default(d) => default_to_js(d),
            Prop::Str(s) => JsValue::String(s.clone()),
            Prop::Num(x) => JsValue::Number(*x),
            Prop::Bool(b) => JsValue::Bool(*b),
            Prop::Undefined => JsValue::Undefined,
            Prop::Function(_) => {
                return Err(ProtoError::Unmodeled("a function as a field value"));
            }
            Prop::TypeObj(_) => {
                return Err(ProtoError::Unmodeled("a Type object as a field value"));
            }
        })
    }

    /// `types[i].toObject(undefined)` / `toObject(null)`: `d.x` literals emit first, then
    /// the first field's `m.x` read throws on the nullish receiver.
    fn nullish_to_object_error(&mut self, vt: NodeId, null: bool) -> ProtoError {
        if self.type_data(vt).to_object.is_none() {
            let _ = self.setup(vt);
        }
        let fields = self.type_data(vt).to_object.clone().unwrap_or_default();
        let sorted = self.sorted_by_id(&fields);
        ProtoError::ReadProp {
            nullish: if null { "null" } else { "undefined" },
            name: sorted
                .first()
                .map_or_else(JsString::new, |&f| self.fields[f].name.clone()),
        }
    }

    /// `ProtoDecoder.decode`'s `findSchemaTypes` walk over the decoded message's own keys.
    pub(super) fn schema_types(&self, m: &Message) -> Result<JsObject<JsString>, ProtoError> {
        let mut out = JsObject::new();
        for (key, value) in m.own.iter() {
            self.find_schema_types(value, key.clone(), &mut out)?;
        }
        Ok(out)
    }

    /// `v.$type`: the prop read `root.$type` and `item.$type` perform.
    fn type_prop<'m>(&'m self, v: &'m Val) -> Prop<'m> {
        self.v_prop(v, &js("$type"))
    }

    /// `p.name`, resolved the way `friendly(root.$type.name)` reads it.
    fn name_of<'m>(&'m self, p: &Prop<'m>) -> Result<JsString, ProtoError> {
        let name = match p {
            Prop::TypeObj(ty) => return Ok(self.nodes[*ty].name.clone()),
            Prop::Function(n) => return Ok(n.clone()),
            Prop::Val(v) => self.v_prop(v, &js("name")),
            Prop::Str(_) | Prop::Num(_) | Prop::Bool(_) => Prop::Undefined,
            Prop::Default(d) => default_prop(d, &js("name")),
            Prop::Undefined => {
                return Err(ProtoError::ReadProp {
                    nullish: "undefined",
                    name: js("name"),
                });
            }
        };
        match name {
            Prop::Val(Val::Str(s)) => Ok(s.clone()),
            Prop::Str(s) | Prop::Default(JsDefault::String(s)) => Ok(s),
            Prop::Undefined => Err(ProtoError::ReadProp {
                nullish: "undefined",
                name: js("includes"),
            }),
            _ => Err(ProtoError::DynNotAFunction(js("schemaType.includes"))),
        }
    }

    fn find_schema_types(
        &self,
        value: &Val,
        key: JsString,
        out: &mut JsObject<JsString>,
    ) -> Result<(), ProtoError> {
        // `typeof root === "object"`: `Array.isArray` first, then `root.$type`.
        let items: Option<Vec<&Val>> = match value {
            Val::Array(items) => Some(items.iter().collect()),
            Val::Msg(mm) if matches!(mm.foreign, Some(Foreign::Array)) => Some(
                mm.own
                    .iter()
                    .filter(|(k, _)| array_index(k.units()).is_some())
                    .map(|(_, v)| v)
                    .collect(),
            ),
            _ => None,
        };
        if let Some(items) = items {
            let mut types: Vec<JsString> = Vec::new();
            for (i, item) in items.iter().enumerate() {
                self.find_schema_types(
                    item,
                    child_key(&key, &js(&number::to_string(i as f64))),
                    out,
                )?;
                if prop_truthy(&self.type_prop(item)) {
                    // `friendly(root[0].$type.name)`: the first element's name.
                    let name = self.name_of(&self.type_prop(items[0]))?;
                    let t = friendly(&name);
                    if !types.iter().any(|s| s == &t) {
                        types.push(t);
                    }
                }
            }
            if types.len() == 1 {
                let mut t = types.pop().expect("len checked");
                t.push_str(&js("[]"));
                out.insert(key, t);
            }
            return Ok(());
        }
        let tp = self.type_prop(value);
        if !prop_truthy(&tp) {
            return Ok(());
        }
        out.insert(key.clone(), friendly(&self.name_of(&tp)?));
        match value {
            Val::Msg(mm) => {
                for (child, v) in mm.own.iter() {
                    self.find_schema_types(v, child_key(&key, child), out)?;
                }
            }
            Val::Map(map) => {
                for (child, v) in map.iter() {
                    self.find_schema_types(v, child_key(&key, child), out)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn type_kind_is_root(&self, node: NodeId) -> bool {
        matches!(self.nodes[node].kind, Kind::Root)
    }
}

/// `v[k] = x` writes onto an object: the enumerable own props `m.x` starts with, or
/// `None` when `m.x` is a primitive (which throws `Cannot create property` instead).
fn seed_object(p: &Prop<'_>) -> Option<JsObject<Val>> {
    let mut out = JsObject::new();
    match p {
        Prop::Val(Val::Array(items)) => {
            for (i, v) in items.iter().enumerate() {
                out.insert(js(&number::to_string(i as f64)), v.clone());
            }
        }
        Prop::Val(Val::Bytes(b)) | Prop::Default(JsDefault::Bytes(b)) => {
            for (i, &x) in b.iter().enumerate() {
                out.insert(js(&number::to_string(i as f64)), Val::Num(f64::from(x)));
            }
        }
        Prop::Val(Val::Long(l)) | Prop::Default(JsDefault::Long(l)) => {
            out.insert(js("low"), Val::Num(f64::from(l.low)));
            out.insert(js("high"), Val::Num(f64::from(l.high)));
            out.insert(js("unsigned"), Val::Bool(l.unsigned));
        }
        Prop::Val(Val::Msg(m)) => out = m.own.clone(),
        Prop::Val(Val::Map(map)) => out = map.clone(),
        Prop::Default(JsDefault::Object(o)) => {
            for (k, v) in o.iter() {
                out.insert(k.clone(), Val::Str(v.clone()));
            }
        }
        Prop::Default(JsDefault::EmptyArray | JsDefault::EmptyObject | JsDefault::Function(_))
        | Prop::Function(_)
        | Prop::TypeObj(_) => {}
        _ => return None,
    }
    Some(out)
}

/// `d.x = m.x` where `m.x` is a decoded value.
fn val_to_js(v: &Val) -> Result<JsValue, ProtoError> {
    Ok(match v {
        Val::Undefined => JsValue::Undefined,
        Val::Num(x) => JsValue::Number(*x),
        Val::Bool(b) => JsValue::Bool(*b),
        Val::Str(s) => JsValue::String(s.clone()),
        Val::Bytes(b) => JsValue::Bytes(b.clone()),
        Val::Long(l) => l.to_js(),
        Val::Array(items) => JsValue::Array(items.iter().map(val_to_js).collect::<Result<_, _>>()?),
        Val::Map(map) => {
            let mut out = JsObject::new();
            for (k, v) in map.iter() {
                out.insert(k.clone(), val_to_js(v)?);
            }
            JsValue::Object(out)
        }
        Val::Msg(_) => {
            return Err(ProtoError::Unmodeled(
                "a message instance as a raw field value",
            ));
        }
    })
}

/// `d.x = m.x` where `m.x` is a prototype `defaultValue`.
fn default_to_js(d: &JsDefault) -> JsValue {
    match d {
        JsDefault::Undefined | JsDefault::Function(_) => JsValue::Undefined,
        JsDefault::Null => JsValue::Null,
        JsDefault::Bool(b) => JsValue::Bool(*b),
        JsDefault::Number(x) => JsValue::Number(*x),
        JsDefault::String(s) => JsValue::String(s.clone()),
        JsDefault::Bytes(b) => JsValue::Bytes(b.clone()),
        JsDefault::EmptyArray => JsValue::Array(Vec::new()),
        JsDefault::EmptyObject => JsValue::Object(JsObject::new()),
        JsDefault::Long(l) => l.to_js(),
        JsDefault::Object(o) => {
            let mut out = JsObject::new();
            for (k, v) in o.iter() {
                out.insert(k.clone(), JsValue::String(v.clone()));
            }
            JsValue::Object(out)
        }
    }
}

/// `v.<name>` where `v` stands in for a non-message `m` under `decode`.
fn foreign_prop<'m>(m: &'m Message, foreign: &'m Foreign, name: &JsString) -> Prop<'m> {
    match foreign {
        Foreign::Array => {
            if js_eq(name, "length") {
                Prop::Num(f64::from(array_len(m)))
            } else if is_array_member(name) {
                Prop::Function(name.clone())
            } else {
                Prop::Undefined
            }
        }
        Foreign::Function {
            name: fn_name,
            arity,
        } => {
            if js_eq(name, "name") {
                Prop::Str(fn_name.clone())
            } else if js_eq(name, "length") {
                Prop::Num(*arity)
            } else if is_fn_member(name) {
                Prop::Function(name.clone())
            } else {
                Prop::Undefined
            }
        }
        _ => boxed_prop(name),
    }
}

/// `Float32Array` element to JS number: NaNs come back quiet with the payload shifted up.
fn widen(bits: u32) -> f64 {
    if bits & 0x7f80_0000 == 0x7f80_0000 && bits & 0x007f_ffff != 0 {
        let sign = u64::from(bits >> 31) << 63;
        let payload = u64::from(bits & 0x007f_ffff) << 29;
        return f64::from_bits(sign | 0x7ff8_0000_0000_0000 | payload);
    }
    f64::from(f32::from_bits(bits))
}
