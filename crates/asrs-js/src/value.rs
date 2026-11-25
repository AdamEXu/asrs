//! JS values and ordinary objects with V8's own-property order: array-index keys (canonical
//! decimal integers up to 2^32 - 2) ascending, then every other key in insertion order.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::fmt;

use indexmap::IndexMap;

use crate::math::BigInt;
use crate::string::JsString;

#[derive(Default)]
pub enum JsValue {
    #[default]
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(JsString),
    Array(Vec<JsValue>),
    Object(JsObject),
    /// A `Uint8Array`.
    Bytes(Vec<u8>),
    BigInt(BigInt),
    /// A `Date`, holding its time value (NaN for an invalid date).
    Date(f64),
    /// A msgpack `ExtData` instance (`{type, data}` own properties), produced by the
    /// default extension codec for non-timestamp extension types.
    ExtData(i8, Vec<u8>),
}

impl JsValue {
    /// Moves every direct child's array items and object values into `out`, leaving `self`
    /// an empty container. Used by `Drop` so a nested value frees its whole subtree on a
    /// heap worklist instead of one stack frame per level.
    fn move_children(&mut self, out: &mut Vec<JsValue>) {
        match self {
            JsValue::Array(items) => out.append(items),
            JsValue::Object(object) => {
                out.extend(std::mem::take(object).into_iter().map(|(_, v)| v));
            }
            _ => {}
        }
    }

    /// Consumes the value into a `JsValue::String`'s string.
    pub fn into_string(mut self) -> Option<JsString> {
        match &mut self {
            JsValue::String(s) => Some(std::mem::take(s)),
            _ => None,
        }
    }

    /// Consumes the value into a `JsValue::Array`'s items.
    pub fn into_array(mut self) -> Option<Vec<JsValue>> {
        match &mut self {
            JsValue::Array(items) => Some(std::mem::take(items)),
            _ => None,
        }
    }

    /// Consumes the value into a `JsValue::Object`'s object.
    pub fn into_object(mut self) -> Option<JsObject> {
        match &mut self {
            JsValue::Object(object) => Some(std::mem::take(object)),
            _ => None,
        }
    }

    /// Consumes the value into a `JsValue::Bytes`'s bytes.
    pub fn into_bytes(mut self) -> Option<Vec<u8>> {
        match &mut self {
            JsValue::Bytes(bytes) => Some(std::mem::take(bytes)),
            _ => None,
        }
    }

    fn clone_leaf(&self) -> Self {
        match self {
            JsValue::Undefined => JsValue::Undefined,
            JsValue::Null => JsValue::Null,
            JsValue::Bool(b) => JsValue::Bool(*b),
            JsValue::Number(x) => JsValue::Number(*x),
            JsValue::String(s) => JsValue::String(s.clone()),
            JsValue::Bytes(b) => JsValue::Bytes(b.clone()),
            JsValue::BigInt(b) => JsValue::BigInt(b.clone()),
            JsValue::Date(t) => JsValue::Date(*t),
            JsValue::ExtData(kind, data) => JsValue::ExtData(*kind, data.clone()),
            JsValue::Array(_) | JsValue::Object(_) => {
                unreachable!("clone descends into containers")
            }
        }
    }
}

thread_local! {
    static NESTING: Cell<usize> = const { Cell::new(0) };
}

/// Containers nested deeper than this are dropped, cloned and compared on a heap worklist;
/// shallower ones take the plain recursive path, which is faster.
const MAX_RECURSIVE_NESTING: usize = 256;

/// Runs `f` one container level deeper on this thread, or returns `None` once
/// [`MAX_RECURSIVE_NESTING`] levels are already on the stack.
fn nested<R>(f: impl FnOnce() -> R) -> Option<R> {
    struct Restore<'a>(&'a Cell<usize>, usize);
    impl Drop for Restore<'_> {
        fn drop(&mut self) {
            self.0.set(self.1);
        }
    }
    NESTING.with(|nesting| {
        let depth = nesting.get();
        if depth >= MAX_RECURSIVE_NESTING {
            return None;
        }
        nesting.set(depth + 1);
        let _restore = Restore(nesting, depth);
        Some(f())
    })
}

impl Drop for JsValue {
    fn drop(&mut self) {
        let has_children = match self {
            JsValue::Array(items) => !items.is_empty(),
            JsValue::Object(object) => !object.is_empty(),
            _ => false,
        };
        if !has_children {
            return;
        }
        let shallow = nested(|| match self {
            JsValue::Array(items) => drop(std::mem::take(items)),
            JsValue::Object(object) => drop(std::mem::take(object)),
            _ => {}
        });
        if shallow.is_some() {
            return;
        }
        let mut worklist = Vec::new();
        self.move_children(&mut worklist);
        while let Some(mut value) = worklist.pop() {
            value.move_children(&mut worklist);
        }
    }
}

impl Clone for JsValue {
    fn clone(&self) -> Self {
        match self {
            JsValue::Array(items) => nested(|| JsValue::Array(items.clone())),
            JsValue::Object(object) => nested(|| JsValue::Object(object.clone())),
            leaf => return leaf.clone_leaf(),
        }
        .unwrap_or_else(|| self.clone_iterative())
    }
}

impl JsValue {
    fn clone_iterative(&self) -> Self {
        enum Frame<'a> {
            Array(Vec<JsValue>, std::slice::Iter<'a, JsValue>),
            Object(JsObject, ObjectIter<'a>, JsString),
        }
        fn descend<'a>(frames: &mut Vec<Frame<'a>>, mut source: &'a JsValue) -> JsValue {
            loop {
                match source {
                    JsValue::Array(items) => {
                        let mut rest = items.iter();
                        match rest.next() {
                            Some(first) => {
                                frames.push(Frame::Array(Vec::with_capacity(items.len()), rest));
                                source = first;
                            }
                            None => return JsValue::Array(Vec::new()),
                        }
                    }
                    JsValue::Object(object) => {
                        let mut members = object.object_iter();
                        match members.next() {
                            Some((key, first)) => {
                                frames.push(Frame::Object(JsObject::new(), members, key.clone()));
                                source = first;
                            }
                            None => return JsValue::Object(JsObject::new()),
                        }
                    }
                    leaf => return leaf.clone_leaf(),
                }
            }
        }
        let mut frames = Vec::new();
        let mut value = descend(&mut frames, self);
        loop {
            match frames.pop() {
                None => return value,
                Some(Frame::Array(mut out, mut rest)) => {
                    out.push(value);
                    match rest.next() {
                        Some(next) => {
                            frames.push(Frame::Array(out, rest));
                            value = descend(&mut frames, next);
                        }
                        None => value = JsValue::Array(out),
                    }
                }
                Some(Frame::Object(mut out, mut members, key)) => {
                    out.insert(key, value);
                    match members.next() {
                        Some((key, next)) => {
                            frames.push(Frame::Object(out, members, key.clone()));
                            value = descend(&mut frames, next);
                        }
                        None => value = JsValue::Object(out),
                    }
                }
            }
        }
    }
}

impl PartialEq for JsValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (JsValue::Array(x), JsValue::Array(y)) => nested(|| x == y),
            (JsValue::Object(x), JsValue::Object(y)) => nested(|| x == y),
            _ => return leaf_eq(self, other),
        }
        .unwrap_or_else(|| eq_iterative(self, other))
    }
}

/// Derived `PartialEq` for every pair that is not two arrays or two objects.
fn leaf_eq(a: &JsValue, b: &JsValue) -> bool {
    match (a, b) {
        (JsValue::Undefined, JsValue::Undefined) | (JsValue::Null, JsValue::Null) => true,
        (JsValue::Bool(x), JsValue::Bool(y)) => x == y,
        (JsValue::Number(x), JsValue::Number(y)) => x == y,
        (JsValue::String(x), JsValue::String(y)) => x == y,
        (JsValue::Bytes(x), JsValue::Bytes(y)) => x == y,
        (JsValue::BigInt(x), JsValue::BigInt(y)) => x == y,
        (JsValue::Date(x), JsValue::Date(y)) => x == y,
        (JsValue::ExtData(k1, d1), JsValue::ExtData(k2, d2)) => k1 == k2 && d1 == d2,
        _ => false,
    }
}

fn eq_iterative<'a>(a: &'a JsValue, b: &'a JsValue) -> bool {
    let mut worklist = vec![(a, b)];
    while let Some((a, b)) = worklist.pop() {
        match (a, b) {
            (JsValue::Array(x), JsValue::Array(y)) => {
                if x.len() != y.len() {
                    return false;
                }
                worklist.extend(x.iter().zip(y.iter()));
            }
            (JsValue::Object(x), JsValue::Object(y)) => {
                if x.len() != y.len() {
                    return false;
                }
                for ((ka, va), (kb, vb)) in x.iter().zip(y.iter()) {
                    if ka != kb {
                        return false;
                    }
                    worklist.push((va, vb));
                }
            }
            _ if leaf_eq(a, b) => {}
            _ => return false,
        }
    }
    true
}

impl fmt::Debug for JsValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        enum Work<'a> {
            Value(&'a JsValue),
            Key(&'a JsString),
            Str(&'static str),
        }
        let mut worklist = vec![Work::Value(self)];
        while let Some(work) = worklist.pop() {
            match work {
                Work::Str(s) => f.write_str(s)?,
                Work::Key(k) => write!(f, "{k:?}")?,
                Work::Value(value) => match value {
                    JsValue::Undefined => f.write_str("Undefined")?,
                    JsValue::Null => f.write_str("Null")?,
                    JsValue::Bool(b) => write!(f, "Bool({b})")?,
                    JsValue::Number(x) => write!(f, "Number({x:?})")?,
                    JsValue::String(s) => write!(f, "String({s:?})")?,
                    JsValue::Array(items) => {
                        f.write_str("Array([")?;
                        worklist.push(Work::Str("])"));
                        for (i, item) in items.iter().enumerate().rev() {
                            worklist.push(Work::Value(item));
                            if i > 0 {
                                worklist.push(Work::Str(", "));
                            }
                        }
                    }
                    JsValue::Object(object) => {
                        f.write_str("Object({")?;
                        worklist.push(Work::Str("})"));
                        let mut first = true;
                        for (k, v) in object.iter().rev() {
                            if !first {
                                worklist.push(Work::Str(", "));
                            }
                            first = false;
                            worklist.push(Work::Value(v));
                            worklist.push(Work::Str(": "));
                            worklist.push(Work::Key(k));
                        }
                    }
                    JsValue::Bytes(bytes) => write!(f, "Bytes({bytes:?})")?,
                    JsValue::BigInt(b) => write!(f, "BigInt({b:?})")?,
                    JsValue::Date(t) => write!(f, "Date({t:?})")?,
                    JsValue::ExtData(kind, data) => write!(f, "ExtData({kind:?}, {data:?})")?,
                },
            }
        }
        Ok(())
    }
}

/// `JsObject::iter` as a named type, so `Clone` and the JSON writer can hold one per pending
/// object frame.
pub(crate) struct ObjectIter<'a> {
    indexed: std::collections::btree_map::Values<'a, u32, (JsString, JsValue)>,
    named: indexmap::map::Iter<'a, JsString, JsValue>,
}

impl<'a> Iterator for ObjectIter<'a> {
    type Item = (&'a JsString, &'a JsValue);

    fn next(&mut self) -> Option<Self::Item> {
        self.indexed
            .next()
            .map(|(k, v)| (k, v))
            .or_else(|| self.named.next())
    }
}

impl JsObject<JsValue> {
    pub(crate) fn object_iter(&self) -> ObjectIter<'_> {
        ObjectIter {
            indexed: self.indexed.values(),
            named: self.named.iter(),
        }
    }
}

impl JsValue {
    pub fn string(s: &str) -> Self {
        JsValue::String(JsString::from(s))
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, JsValue::Undefined)
    }

    pub fn is_null(&self) -> bool {
        matches!(self, JsValue::Null)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            JsValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            JsValue::Number(x) => Some(*x),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&JsString> {
        match self {
            JsValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[JsValue]> {
        match self {
            JsValue::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_array_mut(&mut self) -> Option<&mut Vec<JsValue>> {
        match self {
            JsValue::Array(items) => Some(items),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&JsObject> {
        match self {
            JsValue::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut JsObject> {
        match self {
            JsValue::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&JsValue> {
        self.as_object()?.get(key)
    }

    pub fn type_of(&self) -> &'static str {
        match self {
            JsValue::Undefined => "undefined",
            JsValue::Null
            | JsValue::Array(_)
            | JsValue::Object(_)
            | JsValue::Bytes(_)
            | JsValue::Date(_)
            | JsValue::ExtData(..) => "object",
            JsValue::Bool(_) => "boolean",
            JsValue::Number(_) => "number",
            JsValue::String(_) => "string",
            JsValue::BigInt(_) => "bigint",
        }
    }
}

impl From<bool> for JsValue {
    fn from(b: bool) -> Self {
        JsValue::Bool(b)
    }
}

impl From<f64> for JsValue {
    fn from(x: f64) -> Self {
        JsValue::Number(x)
    }
}

impl From<&str> for JsValue {
    fn from(s: &str) -> Self {
        JsValue::string(s)
    }
}

impl From<String> for JsValue {
    fn from(s: String) -> Self {
        JsValue::String(JsString::from(s))
    }
}

impl From<JsString> for JsValue {
    fn from(s: JsString) -> Self {
        JsValue::String(s)
    }
}

impl From<Vec<JsValue>> for JsValue {
    fn from(items: Vec<JsValue>) -> Self {
        JsValue::Array(items)
    }
}

impl From<JsObject> for JsValue {
    fn from(o: JsObject) -> Self {
        JsValue::Object(o)
    }
}

impl From<BigInt> for JsValue {
    fn from(b: BigInt) -> Self {
        JsValue::BigInt(b)
    }
}

/// The array index a property key denotes, if any: `"0"` or a decimal numeral without a
/// leading zero whose value is at most 2^32 - 2.
pub fn array_index(key: &[u16]) -> Option<u32> {
    if key.is_empty() || key.len() > 10 || (key.len() > 1 && key[0] == u16::from(b'0')) {
        return None;
    }
    let mut value: u64 = 0;
    for &u in key {
        if !(u16::from(b'0')..=u16::from(b'9')).contains(&u) {
            return None;
        }
        value = value * 10 + u64::from(u - u16::from(b'0'));
    }
    (value <= 4_294_967_294).then_some(value as u32)
}

/// The hasher of maps keyed by field and property names. Iteration order must never depend on it.
pub type KeyState = foldhash::fast::RandomState;

/// An ordinary JS object's own enumerable string-keyed properties, iterated in JS property
/// order. Assigning to an existing key keeps its position; removing a key and adding it again
/// moves it to the end (for non-index keys).
#[derive(Clone)]
pub struct JsObject<V = JsValue> {
    indexed: BTreeMap<u32, (JsString, V)>,
    named: IndexMap<JsString, V, KeyState>,
}

impl<V> Default for JsObject<V> {
    fn default() -> Self {
        JsObject {
            indexed: BTreeMap::new(),
            named: IndexMap::default(),
        }
    }
}

impl<V> JsObject<V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.indexed.len() + self.named.len()
    }

    pub fn has_index_keys(&self) -> bool {
        !self.indexed.is_empty()
    }

    pub fn is_empty(&self) -> bool {
        self.indexed.is_empty() && self.named.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.get_units(&key.encode_utf16().collect::<Vec<u16>>())
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut V> {
        self.get_units_mut(&key.encode_utf16().collect::<Vec<u16>>())
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    pub fn get_key(&self, key: &JsString) -> Option<&V> {
        self.get_units(key.units())
    }

    pub fn get_units(&self, key: &[u16]) -> Option<&V> {
        match array_index(key) {
            Some(i) => self.indexed.get(&i).map(|(_, v)| v),
            None => self.named.get(key),
        }
    }

    pub fn get_units_mut(&mut self, key: &[u16]) -> Option<&mut V> {
        match array_index(key) {
            Some(i) => self.indexed.get_mut(&i).map(|(_, v)| v),
            None => self.named.get_mut(key),
        }
    }

    /// `obj[key] = value`. Returns the previous value.
    pub fn insert(&mut self, key: impl Into<JsString>, value: V) -> Option<V> {
        let key = key.into();
        match array_index(key.units()) {
            Some(i) => self.indexed.insert(i, (key, value)).map(|(_, v)| v),
            None => self.named.insert(key, value),
        }
    }

    /// `delete obj[key]`. Returns the removed value.
    pub fn remove(&mut self, key: &str) -> Option<V> {
        self.remove_units(&key.encode_utf16().collect::<Vec<u16>>())
    }

    pub fn remove_key(&mut self, key: &JsString) -> Option<V> {
        self.remove_units(key.units())
    }

    pub fn remove_units(&mut self, key: &[u16]) -> Option<V> {
        match array_index(key) {
            Some(i) => self.indexed.remove(&i).map(|(_, v)| v),
            None => self.named.shift_remove(key),
        }
    }

    pub fn clear(&mut self) {
        self.indexed.clear();
        self.named.clear();
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&JsString, &V)> + '_ {
        self.indexed
            .values()
            .map(|(k, v)| (k, v))
            .chain(self.named.iter())
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&JsString, &mut V)> + '_ {
        self.indexed
            .values_mut()
            .map(|(k, v)| (&*k, v))
            .chain(self.named.iter_mut())
    }

    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &JsString> + '_ {
        self.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &V> + '_ {
        self.iter().map(|(_, v)| v)
    }

    pub fn retain(&mut self, mut f: impl FnMut(&JsString, &mut V) -> bool) {
        self.indexed.retain(|_, (k, v)| f(k, v));
        self.named.retain(|k, v| f(k, v));
    }

    pub fn map_values<W>(&self, mut f: impl FnMut(&V) -> W) -> JsObject<W> {
        JsObject {
            indexed: self
                .indexed
                .iter()
                .map(|(i, (k, v))| (*i, (k.clone(), f(v))))
                .collect(),
            named: self.named.iter().map(|(k, v)| (k.clone(), f(v))).collect(),
        }
    }
}

impl<V> IntoIterator for JsObject<V> {
    type Item = (JsString, V);
    type IntoIter = std::iter::Chain<
        std::collections::btree_map::IntoValues<u32, (JsString, V)>,
        indexmap::map::IntoIter<JsString, V>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.indexed.into_values().chain(self.named)
    }
}

impl<K: Into<JsString>, V> FromIterator<(K, V)> for JsObject<V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut object = JsObject::new();
        object.extend(iter);
        object
    }
}

impl<K: Into<JsString>, V> Extend<(K, V)> for JsObject<V> {
    fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

impl<V: PartialEq> PartialEq for JsObject<V> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().eq(other.iter())
    }
}

impl<V: fmt::Debug> fmt::Debug for JsObject<V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

/// Conversion of a Rust value into the JS value upstream would hold, so key order and the
/// omission of `undefined` members are spelled out by each implementation.
pub trait ToJs {
    fn to_js(&self) -> JsValue;
}

impl ToJs for JsValue {
    fn to_js(&self) -> JsValue {
        self.clone()
    }
}

impl<V: ToJs> ToJs for JsObject<V> {
    fn to_js(&self) -> JsValue {
        JsValue::Object(self.map_values(ToJs::to_js))
    }
}

impl ToJs for bool {
    fn to_js(&self) -> JsValue {
        JsValue::Bool(*self)
    }
}

macro_rules! number_to_js {
    ($($t:ty),*) => {$(
        impl ToJs for $t {
            fn to_js(&self) -> JsValue {
                JsValue::Number(*self as f64)
            }
        }
    )*};
}

number_to_js!(f64, f32, i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);

impl ToJs for str {
    fn to_js(&self) -> JsValue {
        JsValue::string(self)
    }
}

impl ToJs for String {
    fn to_js(&self) -> JsValue {
        JsValue::string(self)
    }
}

impl ToJs for JsString {
    fn to_js(&self) -> JsValue {
        JsValue::String(self.clone())
    }
}

impl ToJs for BigInt {
    fn to_js(&self) -> JsValue {
        JsValue::BigInt(self.clone())
    }
}

impl<T: ToJs> ToJs for [T] {
    fn to_js(&self) -> JsValue {
        JsValue::Array(self.iter().map(ToJs::to_js).collect())
    }
}

impl<T: ToJs> ToJs for Vec<T> {
    fn to_js(&self) -> JsValue {
        self.as_slice().to_js()
    }
}

impl<T: ToJs, const N: usize> ToJs for [T; N] {
    fn to_js(&self) -> JsValue {
        self.as_slice().to_js()
    }
}

/// `None` is `undefined`, which `JSON.stringify` omits from objects.
impl<T: ToJs> ToJs for Option<T> {
    fn to_js(&self) -> JsValue {
        match self {
            Some(v) => v.to_js(),
            None => JsValue::Undefined,
        }
    }
}

impl<T: ToJs + ?Sized> ToJs for &T {
    fn to_js(&self) -> JsValue {
        (**self).to_js()
    }
}

impl<T: ToJs + ?Sized> ToJs for Box<T> {
    fn to_js(&self) -> JsValue {
        (**self).to_js()
    }
}

/// Builds a `JsValue::Object` from `key => value` pairs in source order; values go through
/// [`ToJs`].
#[macro_export]
macro_rules! js_object {
    ($($key:expr => $value:expr),* $(,)?) => {{
        #[allow(unused_mut)]
        let mut object = $crate::value::JsObject::new();
        $(object.insert($key, $crate::value::ToJs::to_js(&$value));)*
        $crate::value::JsValue::Object(object)
    }};
}
