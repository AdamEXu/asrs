//! protobufjs's `Reader`, and the behaviour of the code its `decoder`, `toObject` and
//! constructor generators emit for a reflected type.

use asrs_js::number;
use asrs_js::sort::array_sort;
use asrs_js::string::JsString;
use asrs_js::value::{JsObject, JsValue};

use super::error::{ProtoError, SyntaxKind};
use super::reflect::{FieldId, Kind, NodeId, Reflection, js, js_eq};
use super::value::{JsDefault, Long, Message, Val, long_key_to_string};

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

const UNMODELED_NAMES: [&str; 2] = ["$type", "$unknowns"];

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

    fn check_modeled(&self, ty: NodeId) -> Result<(), ProtoError> {
        let data = self.type_data(ty);
        let named = |n: &JsString, list: &[&str]| list.iter().any(|l| js_eq(n, l));
        if data.fields.keys().any(|n| named(n, &UNMODELED_NAMES)) {
            return Err(ProtoError::Unmodeled(
                "a field named $type or $unknowns shadows a protobufjs message property",
            ));
        }
        if let Some(oneofs) = &data.oneofs
            && oneofs
                .keys()
                .any(|n| named(n, &["$type", "$unknowns", "hasOwnProperty"]))
        {
            return Err(ProtoError::Unmodeled(
                "a oneof accessor shadows a protobufjs message property",
            ));
        }
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
        Ok(Message { ty, own })
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
        self.check_modeled(ty)?;
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
            let tag = r.tag()?;
            if fields.is_empty() {
                r.skip_type(tag & 7, depth, Some(tag))?;
                continue;
            }
            let wire = tag & 7;
            let number = tag >> 3;
            let field = fields
                .iter()
                .copied()
                .find(|&f| self.fields[f].id == f64::from(number));
            let handled = match field {
                Some(f) => self.decode_field(f, &mut m, r, wire, depth)?,
                None => false,
            };
            if handled {
                if let (Some(f), Some(o)) = (field, field.and_then(|f| self.fields[f].part_of)) {
                    let keep = self.fields[f].name.clone();
                    for name in self.oneofs[o].oneof.clone() {
                        if name != keep {
                            m.own.remove_key(&name);
                        }
                    }
                }
            } else {
                r.skip_type(wire, depth, Some(number))?;
            }
        }
        for f in fields {
            let field = &self.fields[f];
            if field.required && m.own.get_key(&field.name).is_none() {
                return Err(ProtoError::MissingRequired(field.name.clone()));
            }
        }
        Ok(m)
    }

    /// One `case` of the generated decoder. Returns false where the case `break`s to the
    /// unknown-field path.
    fn decode_field(
        &mut self,
        f: FieldId,
        m: &mut Message,
        r: &mut Reader<'_>,
        wire: u32,
        depth: u32,
    ) -> Result<bool, ProtoError> {
        let ty = self.field_ty_name(f);
        let name = self.fields[f].name.clone();
        let basic = basic_wire(&ty);
        if let Some(key_ty) = self.fields[f].map_key.clone() {
            if wire != 2 {
                return Ok(false);
            }
            if !matches!(m.own.get_key(&name), Some(Val::Map(_))) {
                m.own.insert(name.clone(), Val::Map(JsObject::new()));
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
            if let Some(Val::Map(map)) = m.own.get_units_mut(name.units()) {
                map.insert(key, value);
            }
            return Ok(true);
        }
        if self.fields[f].repeated {
            let reset = |m: &mut Message| {
                if !matches!(m.own.get_key(&name), Some(Val::Array(a)) if !a.is_empty()) {
                    m.own.insert(name.clone(), Val::Array(Vec::new()));
                }
            };
            if is_packable(&ty) && wire == 2 {
                reset(m);
                let end = u64::from(r.uint32()?) + r.pos;
                while r.pos < end {
                    let v = r.read(&ty)?;
                    push(m, &name, v);
                }
                return Ok(true);
            }
            if wire != basic.unwrap_or(2) {
                return Ok(false);
            }
            reset(m);
            let v = match basic {
                Some(_) => r.read(&ty)?,
                None => {
                    let vt = self.message_type(f).ok_or(ProtoError::ReadNull("decode"))?;
                    let len = u64::from(r.uint32()?);
                    Val::Msg(Box::new(self.decode(vt, r, Some(len), depth + 1, None)?))
                }
            };
            push(m, &name, v);
            return Ok(true);
        }
        match basic {
            None => {
                if wire != 2 {
                    return Ok(false);
                }
                let vt = self.message_type(f).ok_or(ProtoError::ReadNull("decode"))?;
                let len = u64::from(r.uint32()?);
                let target = match m.own.get_units_mut(name.units()) {
                    Some(Val::Msg(existing)) => Some(std::mem::replace(
                        existing.as_mut(),
                        Message {
                            ty: vt,
                            own: JsObject::new(),
                        },
                    )),
                    Some(_) => {
                        return Err(ProtoError::Unmodeled("merging into a non-message value"));
                    }
                    None if self.fields[f].default_value.is_truthy() => {
                        return Err(ProtoError::Unmodeled(
                            "a message field with a truthy default merges into a primitive",
                        ));
                    }
                    None => None,
                };
                let decoded = self.decode(vt, r, Some(len), depth + 1, target)?;
                m.own.insert(name, Val::Msg(Box::new(decoded)));
            }
            Some(w) => {
                if wire != w {
                    return Ok(false);
                }
                let v = r.read(&ty)?;
                m.own.insert(name, v);
            }
        }
        Ok(true)
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
            } else if let JsDefault::Long(l) = field.type_default {
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
        let has_own_property = fields
            .iter()
            .any(|&f| js_eq(&self.fields[f].name, "hasOwnProperty"));
        for &f in &sorted {
            let name = self.fields[f].name.clone();
            let own = m.own.get_key(&name);
            if self.fields[f].map_key.is_some() {
                if let Some(Val::Map(map)) = own
                    && !map.is_empty()
                {
                    let key_ty = self.fields[f].map_key.clone().expect("map");
                    let long_key = super::reflect::is_long_type(&key_ty);
                    let unsigned = js_eq(&key_ty, "uint64") || js_eq(&key_ty, "fixed64");
                    let mut out = JsObject::new();
                    for (k, v) in map.iter() {
                        let key = if long_key {
                            long_key_to_string(k, unsigned)
                        } else {
                            k.clone()
                        };
                        let converted = self.convert(f, v)?;
                        out.insert(key, converted);
                    }
                    d.insert(name, JsValue::Object(out));
                }
            } else if self.fields[f].repeated {
                if let Some(Val::Array(items)) = own
                    && !items.is_empty()
                {
                    let mut out = Vec::with_capacity(items.len());
                    for item in items {
                        out.push(self.convert(f, item)?);
                    }
                    d.insert(name, JsValue::Array(out));
                }
            } else {
                let present = own.is_some() || !self.fields[f].default_value.is_nullish();
                if !present {
                    continue;
                }
                if has_own_property {
                    return Err(ProtoError::NotAFunction("m.hasOwnProperty"));
                }
                if let Some(v) = own {
                    let converted = self.convert(f, v)?;
                    d.insert(name, converted);
                }
            }
        }
        Ok(JsValue::Object(d))
    }

    /// `genValuePartial_toObject`.
    fn convert(&mut self, f: FieldId, v: &Val) -> Result<JsValue, ProtoError> {
        if let (Some(vt), Val::Msg(msg)) = (self.message_type(f), v) {
            return self.message_to_object(vt, msg);
        }
        Ok(match v {
            Val::Num(x) => JsValue::Number(*x),
            Val::Bool(b) => JsValue::Bool(*b),
            Val::Str(s) => JsValue::String(s.clone()),
            Val::Bytes(b) => JsValue::Bytes(b.clone()),
            Val::Long(l) => l.to_js(),
            Val::Array(items) if items.is_empty() => JsValue::Array(Vec::new()),
            Val::Msg(_) | Val::Array(_) | Val::Map(_) => {
                return Err(ProtoError::Unmodeled(
                    "a message value in a non-message field",
                ));
            }
        })
    }

    /// `ProtoDecoder.decode`'s `findSchemaTypes` walk over the decoded message's own keys.
    pub(super) fn schema_types(&self, m: &Message) -> JsObject<JsString> {
        let mut out = JsObject::new();
        for (key, value) in m.own.iter() {
            self.find_schema_types(value, key.clone(), &mut out);
        }
        out
    }

    fn find_schema_types(&self, value: &Val, key: JsString, out: &mut JsObject<JsString>) {
        match value {
            Val::Array(items) => {
                let mut array_type = None;
                for (i, item) in items.iter().enumerate() {
                    self.find_schema_types(item, child_key(&key, &js(&format!("{i}"))), out);
                    if matches!(item, Val::Msg(_))
                        && let Some(Val::Msg(first)) = items.first()
                    {
                        array_type = Some(friendly(&self.nodes[first.ty].name));
                    }
                }
                if let Some(mut t) = array_type {
                    t.push_str(&js("[]"));
                    out.insert(key, t);
                }
            }
            Val::Msg(msg) => {
                out.insert(key.clone(), friendly(&self.nodes[msg.ty].name));
                for (child, v) in msg.own.iter() {
                    self.find_schema_types(v, child_key(&key, child), out);
                }
            }
            _ => {}
        }
    }

    pub(super) fn type_kind_is_root(&self, node: NodeId) -> bool {
        matches!(self.nodes[node].kind, Kind::Root)
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

/// `array.push(value)`. V8 quiets a signalling NaN stored as an array element; whether it
/// also canonicalises the payload depends on inline-cache and tiering state, which no vector
/// pins, so only the quieting is reproduced.
fn push(m: &mut Message, name: &JsString, v: Val) {
    let v = match v {
        Val::Num(x) if x.is_nan() => Val::Num(f64::from_bits(x.to_bits() | 0x0008_0000_0000_0000)),
        other => other,
    };
    if let Some(Val::Array(items)) = m.own.get_units_mut(name.units()) {
        items.push(v);
    }
}

pub(super) fn child_key(key: &JsString, child: &JsString) -> JsString {
    let mut out = key.clone();
    out.push_unit(u16::from(b'/'));
    out.push_str(child);
    out
}

/// `ProtoDecoder.getFriendlySchemaType`.
pub(super) fn friendly(schema_type: &JsString) -> JsString {
    let dot = js(".");
    let mut result = schema_type.clone();
    if schema_type.includes(&dot, None) {
        result = schema_type
            .split(Some(&dot), None)
            .pop()
            .unwrap_or_default();
    }
    let prefix = js("Protobuf");
    if result.starts_with(&prefix, None) {
        result = result.slice(prefix.len() as f64, None);
    }
    result
}
