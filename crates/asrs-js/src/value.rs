//! JS values and ordinary objects with V8's own-property order: array-index keys (canonical
//! decimal integers up to 2^32 - 2) ascending, then every other key in insertion order.

use std::collections::BTreeMap;
use std::fmt;

use indexmap::IndexMap;

use crate::math::BigInt;
use crate::string::JsString;

#[derive(Clone, Debug, Default, PartialEq)]
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

/// An ordinary JS object's own enumerable string-keyed properties, iterated in JS property
/// order. Assigning to an existing key keeps its position; removing a key and adding it again
/// moves it to the end (for non-index keys).
#[derive(Clone)]
pub struct JsObject<V = JsValue> {
    indexed: BTreeMap<u32, (JsString, V)>,
    named: IndexMap<JsString, V>,
}

impl<V> Default for JsObject<V> {
    fn default() -> Self {
        JsObject {
            indexed: BTreeMap::new(),
            named: IndexMap::new(),
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
