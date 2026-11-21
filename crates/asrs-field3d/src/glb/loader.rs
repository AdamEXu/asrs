// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

use std::collections::HashMap;
use std::sync::Arc;

use asrs_js::math;
use asrs_js::string::JsString;
use asrs_js::string::encoding::TextDecoder;
use asrs_js::value::JsValue;

use super::{MeshMaterial, ObjectKind, SceneGraph, SceneObject, SourceMaterial, to_number, truthy};
use crate::three::{
    ArrayData, ArrayKind, BufferAttribute, BufferGeometry, Matrix4, ObjectId, Quaternion, Vector3,
};

/// Why `GLTFLoader` rejected a file (upstream's worker then never answers), or a glTF feature
/// the port does not model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GltfError {
    Rejected(String),
    Unmodeled(String),
}

impl std::fmt::Display for GltfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GltfError::Rejected(m) => write!(f, "GLTFLoader rejects the file: {m}"),
            GltfError::Unmodeled(m) => write!(f, "not modeled by the port: {m}"),
        }
    }
}

type Result<T> = std::result::Result<T, GltfError>;

fn reject<T>(message: impl Into<String>) -> Result<T> {
    Err(GltfError::Rejected(message.into()))
}

fn unmodeled<T>(message: impl Into<String>) -> Result<T> {
    Err(GltfError::Unmodeled(message.into()))
}

/// `gltf.scene` and the graph holding it.
#[derive(Debug)]
pub struct GltfScene {
    pub graph: SceneGraph,
    pub scene: ObjectId,
}

const OBJECT_PROTOTYPE_KEYS: &[&str] = &[
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

fn is_js_whitespace(u: u16) -> bool {
    matches!(
        u,
        0x09..=0x0d | 0x20 | 0xa0 | 0x1680 | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000 | 0xfeff
    )
}

/// `PropertyBinding.sanitizeNodeName`: whitespace becomes `_`, then `[`, `]`, `.`, `:` and `/`
/// are removed.
pub fn sanitize_node_name(name: &JsString) -> JsString {
    let units = name
        .units()
        .iter()
        .map(|&u| {
            if is_js_whitespace(u) {
                u16::from(b'_')
            } else {
                u
            }
        })
        .filter(|&u| !matches!(u, 0x5b | 0x5d | 0x2e | 0x3a | 0x2f))
        .collect();
    JsString::from_units(units)
}

/// `GLTFParser.createUniqueName` over its `nodeNamesUsed = {}`. The `in` test sees
/// `Object.prototype`, so a name such as `constructor` is "taken" from the start and `++` on the
/// inherited function (or on the prototype itself for `__proto__`) gives `NaN`.
#[derive(Default)]
struct NodeNames {
    used: HashMap<Vec<u16>, f64>,
}

impl NodeNames {
    fn unique(&mut self, original: &JsString) -> JsString {
        let sanitized = sanitize_node_name(original);
        let key = sanitized.units().to_vec();
        let next = match self.used.get(&key) {
            Some(&count) => count + 1.0,
            None if OBJECT_PROTOTYPE_KEYS
                .iter()
                .any(|k| k.encode_utf16().eq(key.iter().copied())) =>
            {
                f64::NAN
            }
            None => {
                self.used.insert(key, 0.0);
                return sanitized;
            }
        };
        if sanitized.units() != "__proto__".encode_utf16().collect::<Vec<_>>().as_slice() {
            self.used.insert(key, next);
        }
        let mut out = sanitized;
        out.push_str(&JsString::from(format!("_{}", number_text(next))));
        out
    }
}

fn number_text(n: f64) -> String {
    if n.is_nan() {
        "NaN".to_owned()
    } else {
        format!("{}", n as i64)
    }
}

fn name_arg(value: Option<&JsValue>) -> Result<JsString> {
    match value {
        Some(JsValue::String(s)) => Ok(s.clone()),
        _ => reject("TypeError: name.replace is not a function"),
    }
}

fn prop<'a>(value: &'a JsValue, key: &str) -> Option<&'a JsValue> {
    match value {
        JsValue::Object(_) => value.get(key),
        _ => None,
    }
}

fn defined(value: Option<&JsValue>) -> Option<&JsValue> {
    value.filter(|v| !v.is_undefined())
}

/// `array[index]` for a JSON value used as an index.
fn element<'a>(array: Option<&'a JsValue>, index: &JsValue) -> Option<&'a JsValue> {
    let i = index_of(index)?;
    match array? {
        JsValue::Array(items) => items.get(i),
        object @ JsValue::Object(_) => object.get(&format!("{i}")),
        _ => None,
    }
}

fn index_of(value: &JsValue) -> Option<usize> {
    match value {
        JsValue::Number(n) if *n >= 0.0 && n.fract() == 0.0 && *n < 4294967295.0 => {
            Some(*n as usize)
        }
        JsValue::String(s) => {
            let n = asrs_js::number::to_number(s.units());
            let i = index_of(&JsValue::Number(n))?;
            (format!("{i}") == s.to_string_lossy()).then_some(i)
        }
        _ => None,
    }
}

fn require<'a>(value: Option<&'a JsValue>, what: &str) -> Result<&'a JsValue> {
    match value {
        Some(v @ (JsValue::Object(_) | JsValue::Array(_))) => Ok(v),
        _ => reject(format!("TypeError: {what} is undefined")),
    }
}

fn items(value: Option<&JsValue>) -> &[JsValue] {
    match value {
        Some(JsValue::Array(items)) => items,
        _ => &[],
    }
}

fn number_elem(value: Option<&JsValue>, i: usize) -> f64 {
    match value {
        Some(JsValue::Array(items)) => to_number(items.get(i)),
        Some(JsValue::String(s)) => to_number(
            s.units()
                .get(i)
                .map(|&u| JsValue::String(JsString::from_units(vec![u])))
                .as_ref(),
        ),
        _ => f64::NAN,
    }
}

/// `meshDef.primitives` as `loadMesh` and `loadGeometries` walk it: `for (i < primitives.length)`.
fn primitives_of(mesh: &JsValue) -> Result<Vec<JsValue>> {
    match prop(mesh, "primitives") {
        Some(JsValue::Array(p)) => Ok(p.clone()),
        None | Some(JsValue::Undefined | JsValue::Null) => {
            reject("TypeError: Cannot read properties of undefined (reading 'length')")
        }
        Some(JsValue::String(s)) if !s.is_empty() => {
            reject("TypeError: Cannot convert undefined or null to object")
        }
        Some(object @ JsValue::Object(_)) if object.get("length").is_some() => {
            unmodeled("mesh primitives as an array-like object")
        }
        Some(_) => Ok(Vec::new()),
    }
}

/// The texture references the material extension plugins hand to `assignTexture`.
const MATERIAL_EXTENSION_TEXTURES: &[(&str, &[&str])] = &[
    (
        "KHR_materials_clearcoat",
        &[
            "clearcoatTexture",
            "clearcoatRoughnessTexture",
            "clearcoatNormalTexture",
        ],
    ),
    (
        "KHR_materials_iridescence",
        &["iridescenceTexture", "iridescenceThicknessTexture"],
    ),
    (
        "KHR_materials_sheen",
        &["sheenColorTexture", "sheenRoughnessTexture"],
    ),
    ("KHR_materials_transmission", &["transmissionTexture"]),
    ("KHR_materials_volume", &["thicknessTexture"]),
    (
        "KHR_materials_specular",
        &["specularTexture", "specularColorTexture"],
    ),
    ("EXT_materials_bump", &["bumpTexture"]),
    ("KHR_materials_anisotropy", &["anisotropyTexture"]),
];

/// `ToIndex`: `NaN` and `undefined` give 0; negatives and huge values throw.
fn to_index(value: f64) -> Result<usize> {
    let n = if value.is_nan() { 0.0 } else { value.trunc() };
    if !(0.0..=9007199254740991.0).contains(&n) {
        return reject("RangeError: invalid typed array length or offset");
    }
    Ok(n as usize)
}

fn decode_text(bytes: &[u8]) -> JsString {
    TextDecoder::new(None, false, false)
        .and_then(|d| d.decode(bytes))
        .unwrap_or_default()
}

const BINARY_HEADER_LENGTH: usize = 12;
const CHUNK_JSON: u32 = 0x4e4f534a;
const CHUNK_BIN: u32 = 0x004e4942;

fn read_u32(data: &[u8], at: usize) -> Result<u32> {
    match data.get(at..at + 4) {
        Some(b) => Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]])),
        None => reject("RangeError: Offset is outside the bounds of the DataView"),
    }
}

/// `GLTFBinaryExtension`: the JSON chunk text and the BIN chunk.
fn parse_glb(data: &[u8]) -> Result<(JsString, Option<Vec<u8>>)> {
    if data.len() < BINARY_HEADER_LENGTH {
        return reject("RangeError: Offset is outside the bounds of the DataView");
    }
    if read_u32(data, 4)? < 2 {
        return reject("THREE.GLTFLoader: Legacy binary file detected.");
    }
    let contents_length = f64::from(read_u32(data, 8)?) - BINARY_HEADER_LENGTH as f64;
    let view = &data[BINARY_HEADER_LENGTH..];
    let mut content = None;
    let mut body = None;
    let mut index = 0usize;
    while (index as f64) < contents_length {
        let length = read_u32(view, index)? as usize;
        index += 4;
        let kind = read_u32(view, index)?;
        index += 4;
        let start = BINARY_HEADER_LENGTH + index;
        if kind == CHUNK_JSON {
            match data.get(start..start + length) {
                Some(bytes) => content = Some(decode_text(bytes)),
                None => return reject("RangeError: Invalid typed array length"),
            }
        } else if kind == CHUNK_BIN {
            let from = start.min(data.len());
            body = Some(data[from..(start + length).min(data.len())].to_vec());
        }
        index += length;
    }
    match content {
        Some(content) => Ok((content, body)),
        None => reject("THREE.GLTFLoader: JSON content not found."),
    }
}

fn component_kind(component_type: Option<&JsValue>) -> Result<ArrayKind> {
    Ok(match to_number(component_type) {
        5120.0 => ArrayKind::Int8,
        5121.0 => ArrayKind::Uint8,
        5122.0 => ArrayKind::Int16,
        5123.0 => ArrayKind::Uint16,
        5125.0 => ArrayKind::Uint32,
        5126.0 => ArrayKind::Float32,
        _ => return reject("TypeError: TypedArray is not a constructor"),
    })
}

fn element_bytes(kind: ArrayKind) -> usize {
    match kind {
        ArrayKind::Int8 | ArrayKind::Uint8 => 1,
        ArrayKind::Int16 | ArrayKind::Uint16 => 2,
        ArrayKind::Float64 => 8,
        _ => 4,
    }
}

fn item_size(type_name: Option<&JsValue>) -> Result<usize> {
    let Some(JsValue::String(s)) = type_name else {
        return unmodeled("accessor without a known type");
    };
    Ok(match s.to_string_lossy().as_str() {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" | "MAT2" | "VEC4" | "MAT3" | "MAT4" => match s.to_string_lossy().as_str() {
            "VEC3" => 3,
            "VEC4" | "MAT2" => 4,
            "MAT3" => 9,
            _ => 16,
        },
        _ => return unmodeled("accessor without a known type"),
    })
}

/// `new TypedArray(buffer, byteOffset, length)`.
fn typed_array(
    buffer: &[u8],
    kind: ArrayKind,
    byte_offset: usize,
    length: usize,
) -> Result<ArrayData> {
    let width = element_bytes(kind);
    if !byte_offset.is_multiple_of(width) {
        return reject(
            "RangeError: start offset of TypedArray should be a multiple of its element size",
        );
    }
    let Some(bytes) = length
        .checked_mul(width)
        .and_then(|n| n.checked_add(byte_offset))
        .and_then(|end| buffer.get(byte_offset..end))
    else {
        return reject(format!("RangeError: Invalid typed array length: {length}"));
    };
    Ok(match kind {
        ArrayKind::Int8 => ArrayData::Int8(bytes.iter().map(|&b| b as i8).collect()),
        ArrayKind::Uint8 => ArrayData::Uint8(bytes.to_vec()),
        ArrayKind::Int16 => ArrayData::Int16(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| i16::from_le_bytes(*b))
                .collect(),
        ),
        ArrayKind::Uint16 => ArrayData::Uint16(
            bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes(*b))
                .collect(),
        ),
        ArrayKind::Uint32 => ArrayData::Uint32(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| u32::from_le_bytes(*b))
                .collect(),
        ),
        ArrayKind::Int32 => ArrayData::Int32(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| i32::from_le_bytes(*b))
                .collect(),
        ),
        ArrayKind::Float32 => ArrayData::Float32(
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect(),
        ),
        ArrayKind::Float64 => ArrayData::Float64(
            bytes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|b| f64::from_le_bytes(*b))
                .collect(),
        ),
    })
}

/// `ArrayBuffer.prototype.slice(start, end)`.
fn slice_buffer(buffer: &[u8], start: f64, end: f64) -> Vec<u8> {
    let len = buffer.len() as f64;
    let clamp = |x: f64| -> usize {
        let x = if x.is_nan() { 0.0 } else { x.trunc() };
        (if x < 0.0 {
            math::max2(len + x, 0.0)
        } else {
            math::min2(x, len)
        }) as usize
    };
    let (from, to) = (clamp(start), clamp(end));
    if to > from {
        buffer[from..to].to_vec()
    } else {
        Vec::new()
    }
}

fn decode_data_url(url: &str) -> Option<Vec<u8>> {
    let rest = url.get(5..)?;
    let comma = rest.find(',')?;
    let (meta, data) = (&rest[..comma], &rest[comma + 1..]);
    let bytes = percent_decode(data.as_bytes());
    if meta
        .to_ascii_lowercase()
        .trim_end_matches(' ')
        .ends_with(";base64")
    {
        base64_decode(&bytes)
    } else {
        Some(bytes)
    }
}

fn percent_decode(input: &[u8]) -> Vec<u8> {
    let hex = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%'
            && let (Some(&h), Some(&l)) = (input.get(i + 1), input.get(i + 2))
            && let (Some(h), Some(l)) = (hex(h), hex(l))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

/// WHATWG forgiving-base64 decode.
fn base64_decode(input: &[u8]) -> Option<Vec<u8>> {
    let mut data: Vec<u8> = input
        .iter()
        .copied()
        .filter(|b| !matches!(b, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r'))
        .collect();
    if data.len().is_multiple_of(4) && data.ends_with(b"==") {
        data.truncate(data.len() - 2);
    } else if data.len().is_multiple_of(4) && data.ends_with(b"=") {
        data.truncate(data.len() - 1);
    }
    if data.len() % 4 == 1 {
        return None;
    }
    let value = |b: u8| -> Option<u32> {
        Some(match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0;
    for &b in &data {
        buffer = (buffer << 6) | value(b)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// `LoaderUtils.resolveURL(url, path)` for the non-http paths the asset loader produces.
fn resolve_url(url: &JsValue, path: &JsString) -> JsString {
    let JsValue::String(url) = url else {
        return JsString::new();
    };
    let text = url.to_string_lossy();
    let lower = text.to_ascii_lowercase();
    if text.is_empty() {
        return JsString::new();
    }
    if lower.starts_with("http://") || lower.starts_with("https://") || text.starts_with("//") {
        return url.clone();
    }
    if (lower.starts_with("data:") && text.contains(',') && !text.contains('\n'))
        || lower.starts_with("blob:")
    {
        return url.clone();
    }
    let mut out = path.clone();
    out.push_str(url);
    out
}

fn extract_url_base(url: &JsString) -> JsString {
    match url.units().iter().rposition(|&u| u == u16::from(b'/')) {
        None => JsString::from("./"),
        Some(i) => JsString::from_units(url.units()[..=i].to_vec()),
    }
}

/// A mesh as `loadMesh` resolves it: one object per primitive, grouped when there are several.
struct MeshTemplate {
    primitives: Vec<SceneObject>,
}

struct CameraTemplate {
    object: SceneObject,
}

struct Parser<'a> {
    json: &'a JsValue,
    body: Option<Vec<u8>>,
    path: JsString,
    binary: bool,
    fetch: &'a dyn Fn(&JsString) -> Option<Vec<u8>>,
    unlit: bool,
    meshopt_required: bool,
    basisu_required: bool,
    names: NodeNames,
    buffers: HashMap<usize, Arc<Vec<u8>>>,
    buffer_views: HashMap<usize, Arc<Vec<u8>>>,
    accessors: HashMap<usize, BufferAttribute>,
    materials: HashMap<usize, Arc<SourceMaterial>>,
    default_material: Arc<SourceMaterial>,
    mesh_refs: HashMap<usize, usize>,
    mesh_uses: HashMap<usize, usize>,
    camera_refs: HashMap<usize, usize>,
    camera_uses: HashMap<usize, usize>,
    mesh_order: Vec<usize>,
    meshes: HashMap<usize, MeshTemplate>,
    cameras: HashMap<usize, CameraTemplate>,
    visited: Vec<(usize, JsString)>,
    node_seen: HashMap<usize, usize>,
}

/// Loads a `.glb` (or JSON glTF) file the way upstream's workers call `GLTFLoader.loadAsync`,
/// keeping what reaches `optimizeGeometries`: the object tree, names, transforms, triangle
/// geometry and the material color and transparency. `url` is the URL the file was loaded from;
/// external buffers resolve against it and go through `fetch`.
pub fn load_gltf(
    data: &[u8],
    url: &JsString,
    fetch: &dyn Fn(&JsString) -> Option<Vec<u8>>,
) -> Result<GltfScene> {
    if data.len() < 4 {
        return reject("RangeError: Invalid typed array length: 4");
    }
    let (text, body, binary) = if decode_text(&data[..4]) == JsString::from("glTF") {
        let (content, body) = parse_glb(data)?;
        (content, body, true)
    } else {
        (decode_text(data), None, false)
    };
    let json = asrs_js::json::parse(&text).map_err(|e| {
        GltfError::Rejected(format!("SyntaxError: {}", e.message.to_string_lossy()))
    })?;
    check_asset(&json)?;
    let used = items(prop(&json, "extensionsUsed"));
    let required = items(prop(&json, "extensionsRequired"));
    let named = |list: &[JsValue], name: &str| {
        list.iter()
            .any(|v| matches!(v, JsValue::String(s) if *s == JsString::from(name)))
    };
    if named(used, "KHR_draco_mesh_compression") {
        return reject("THREE.GLTFLoader: No DRACOLoader instance provided.");
    }
    let mut parser = Parser {
        json: &json,
        body: if binary { body } else { None },
        binary,
        path: extract_url_base(url),
        fetch,
        unlit: named(used, "KHR_materials_unlit"),
        meshopt_required: named(required, "EXT_meshopt_compression"),
        basisu_required: named(required, "KHR_texture_basisu"),
        names: NodeNames::default(),
        buffers: HashMap::new(),
        buffer_views: HashMap::new(),
        accessors: HashMap::new(),
        materials: HashMap::new(),
        default_material: Arc::new(SourceMaterial::white()),
        mesh_refs: HashMap::new(),
        mesh_uses: HashMap::new(),
        camera_refs: HashMap::new(),
        camera_uses: HashMap::new(),
        mesh_order: Vec::new(),
        meshes: HashMap::new(),
        cameras: HashMap::new(),
        visited: Vec::new(),
        node_seen: HashMap::new(),
    };
    parser.parse()
}

fn check_asset(json: &JsValue) -> Result<()> {
    let asset = match json {
        JsValue::Null | JsValue::Undefined => {
            return reject("TypeError: Cannot read properties of null");
        }
        JsValue::Object(_) => json.get("asset"),
        _ => None,
    };
    let Some(asset) = defined(asset) else {
        return reject("THREE.GLTFLoader: Unsupported asset. glTF versions >=2.0 are supported.");
    };
    let version = match asset {
        JsValue::Null => return reject("TypeError: Cannot read properties of null"),
        JsValue::Object(_) => asset.get("version"),
        _ => None,
    };
    let first = match version {
        None | Some(JsValue::Undefined | JsValue::Null) => {
            return reject("TypeError: Cannot read properties of undefined (reading '0')");
        }
        Some(JsValue::String(s)) => s
            .units()
            .first()
            .map(|&u| JsValue::String(JsString::from_units(vec![u]))),
        Some(JsValue::Array(items)) => items.first().cloned(),
        Some(_) => None,
    };
    if let Some(first) = first
        && !first.is_undefined()
        && to_number(Some(&first)) < 2.0
    {
        return reject("THREE.GLTFLoader: Unsupported asset. glTF versions >=2.0 are supported.");
    }
    Ok(())
}

impl Parser<'_> {
    fn defs(&self, key: &str) -> &[JsValue] {
        items(prop(self.json, key))
    }

    fn parse(&mut self) -> Result<GltfScene> {
        if !self.defs("skins").is_empty() {
            return unmodeled("skins");
        }
        let nodes = self.defs("nodes").to_vec();
        for node in &nodes {
            if let Some(mesh) = defined(prop(node, "mesh")).and_then(index_of) {
                *self.mesh_refs.entry(mesh).or_default() += 1;
            }
            if let Some(camera) = defined(prop(node, "camera")).and_then(index_of) {
                *self.camera_refs.entry(camera).or_default() += 1;
            }
        }

        let scenes = self.defs("scenes").to_vec();
        let mut scene_names = Vec::new();
        for scene in &scenes {
            let name = if truthy(prop(scene, "name")) {
                self.names.unique(&name_arg(prop(scene, "name"))?)
            } else {
                JsString::new()
            };
            scene_names.push(name);
            for id in items(prop(scene, "nodes")) {
                self.visit(id, 1)?;
            }
        }
        for animation in self.defs("animations").to_vec() {
            self.load_animation(&animation)?;
        }
        for i in 0..self.defs("cameras").len() {
            self.load_camera(i, false)?;
        }
        for mesh in self.mesh_order.clone() {
            let template = self.load_mesh(mesh)?;
            self.meshes.insert(mesh, template);
        }

        let mut graph = SceneGraph::new();
        let mut objects = HashMap::new();
        for (index, name) in self.visited.clone() {
            let id = self.build_node(&mut graph, index, name)?;
            objects.insert(index, id);
        }
        for (index, _) in self.visited.clone() {
            for child in items(prop(&nodes[index], "children")) {
                let child = objects[&index_of(child).unwrap_or(usize::MAX)];
                add_checked(&mut graph, objects[&index], child)?;
            }
        }
        let mut roots = Vec::new();
        for (scene, name) in scenes.iter().zip(scene_names) {
            let root = graph.create("", SceneObject::new(name, ObjectKind::Group));
            for id in items(prop(scene, "nodes")) {
                let child = objects[&index_of(id).unwrap_or(usize::MAX)];
                add_checked(&mut graph, root, child)?;
            }
            roots.push(root);
        }
        let chosen = prop(self.json, "scene");
        let index = if truthy(chosen) {
            chosen.and_then(index_of)
        } else {
            Some(0)
        };
        match index.and_then(|i| roots.get(i)) {
            Some(&scene) if hierarchy_depth(&graph, scene) > MAX_DEPTH + 1 => too_deep(),
            Some(&scene) => Ok(GltfScene { graph, scene }),
            None => reject("TypeError: gltf.scene is undefined"),
        }
    }

    fn node_def(&self, index: usize) -> Result<&JsValue> {
        require(self.defs("nodes").get(index), "nodeDef")
    }

    fn visit(&mut self, id: &JsValue, depth: usize) -> Result<()> {
        if depth > MAX_DEPTH {
            return too_deep();
        }
        let Some(index) = index_of(id) else {
            return reject("TypeError: nodeDef is undefined");
        };
        if self.node_seen.contains_key(&index) {
            return Ok(());
        }
        self.node_seen.insert(index, self.visited.len());
        let def = self.node_def(index)?.clone();
        let name = if truthy(prop(&def, "name")) {
            self.names.unique(&name_arg(prop(&def, "name"))?)
        } else {
            JsString::new()
        };
        self.visited.push((index, name));
        if let Some(extensions) = prop(&def, "extensions") {
            if let Some(gpu) = prop(extensions, "EXT_mesh_gpu_instancing")
                && truthy(Some(gpu))
                && defined(prop(&def, "mesh")).is_some()
            {
                return unmodeled("EXT_mesh_gpu_instancing");
            }
            if let Some(light) = prop(extensions, "KHR_lights_punctual")
                && defined(prop(light, "light")).is_some()
            {
                return unmodeled("KHR_lights_punctual");
            }
        }
        if let Some(mesh) = defined(prop(&def, "mesh")) {
            let Some(mesh) = index_of(mesh) else {
                return reject("TypeError: meshDef is undefined");
            };
            self.request_mesh(mesh)?;
        }
        if let Some(camera) = defined(prop(&def, "camera")) {
            let Some(camera) = index_of(camera) else {
                return reject("TypeError: cameraDef is undefined");
            };
            self.load_camera(camera, true)?;
        }
        if defined(prop(&def, "skin")).is_some() {
            return unmodeled("skins");
        }
        for child in items(prop(&def, "children")).to_vec() {
            self.visit(&child, depth + 1)?;
        }
        Ok(())
    }

    /// The synchronous part of `loadMesh`: material requests (which validate the material and
    /// its textures) and the geometry requests.
    fn request_mesh(&mut self, index: usize) -> Result<()> {
        if self.mesh_order.contains(&index) {
            return Ok(());
        }
        self.mesh_order.push(index);
        let def = require(self.defs("meshes").get(index), "meshDef")?.clone();
        for primitive in &primitives_of(&def)? {
            if let Some(material) = defined(prop(primitive, "material")) {
                self.load_material(material)?;
            }
        }
        Ok(())
    }

    /// `loadCamera`. Without parameters it returns `undefined`, and with an unknown type it
    /// makes no camera; only a node's `.then` on that, or naming it, then throws.
    fn load_camera(&mut self, index: usize, from_node: bool) -> Result<()> {
        if self.cameras.contains_key(&index) {
            return Ok(());
        }
        let def = match self.defs("cameras").get(index) {
            None | Some(JsValue::Null) => return reject("TypeError: cameraDef is undefined"),
            Some(def) => def.clone(),
        };
        let kind = prop(&def, "type");
        let params = match kind {
            Some(JsValue::String(s)) => prop(&def, &s.to_string_lossy()),
            _ => None,
        };
        if !truthy(params) {
            return if from_node {
                reject("TypeError: Cannot read properties of undefined (reading 'then')")
            } else {
                Ok(())
            };
        }
        let type_name = match kind {
            Some(JsValue::String(s)) if *s == JsString::from("perspective") => "PerspectiveCamera",
            Some(JsValue::String(s)) if *s == JsString::from("orthographic") => {
                "OrthographicCamera"
            }
            _ if from_node
                || truthy(prop(&def, "name"))
                || matches!(
                    prop(&def, "extras"),
                    Some(JsValue::Object(_) | JsValue::Array(_) | JsValue::Null)
                ) =>
            {
                return reject("TypeError: camera is undefined");
            }
            _ => return Ok(()),
        };
        let name = if truthy(prop(&def, "name")) {
            self.names.unique(&name_arg(prop(&def, "name"))?)
        } else {
            JsString::new()
        };
        self.cameras.insert(
            index,
            CameraTemplate {
                object: SceneObject::new(name, ObjectKind::Other(type_name)),
            },
        );
        Ok(())
    }

    fn load_material(&mut self, index_value: &JsValue) -> Result<Arc<SourceMaterial>> {
        let Some(index) = index_of(index_value) else {
            return reject("TypeError: materialDef is undefined");
        };
        if let Some(m) = self.materials.get(&index) {
            return Ok(m.clone());
        }
        let def = match self.defs("materials").get(index) {
            Some(d @ JsValue::Object(_)) => d.clone(),
            Some(JsValue::Null) | None => return reject("TypeError: materialDef is undefined"),
            Some(other) => other.clone(),
        };
        let extensions = prop(&def, "extensions").filter(|v| truthy(Some(v)));
        let is_unlit = extensions.is_some_and(|e| truthy(prop(e, "KHR_materials_unlit")));
        let pbr = prop(&def, "pbrMetallicRoughness").filter(|v| truthy(Some(v)));
        let mut material = SourceMaterial::white();
        if is_unlit && !self.unlit {
            return reject(
                "TypeError: Cannot read properties of undefined (reading 'getMaterialType')",
            );
        }
        if let Some(pbr) = pbr {
            if let Some(factor @ JsValue::Array(values)) = prop(pbr, "baseColorFactor") {
                material.color = Some([
                    number_elem(Some(factor), 0),
                    number_elem(Some(factor), 1),
                    number_elem(Some(factor), 2),
                ]);
                material.opacity = match values.get(3) {
                    None | Some(JsValue::Undefined) => 1.0,
                    value => to_number(value),
                };
            }
            self.check_texture(prop(pbr, "baseColorTexture"))?;
            if !is_unlit {
                self.check_texture(prop(pbr, "metallicRoughnessTexture"))?;
            }
        }
        let alpha_mode = prop(&def, "alphaMode").filter(|v| truthy(Some(v)));
        material.transparent =
            matches!(alpha_mode, Some(JsValue::String(s)) if *s == JsString::from("BLEND"));
        if !is_unlit {
            for key in ["normalTexture", "occlusionTexture", "emissiveTexture"] {
                self.check_texture(prop(&def, key))?;
            }
            if matches!(prop(&def, "emissiveFactor"), Some(JsValue::Null)) {
                return reject("TypeError: Cannot read properties of null (reading '0')");
            }
            for (name, textures) in MATERIAL_EXTENSION_TEXTURES {
                let Some(extension) = extensions
                    .and_then(|e| prop(e, name))
                    .filter(|v| truthy(Some(v)))
                else {
                    continue;
                };
                for key in *textures {
                    self.check_texture(prop(extension, key))?;
                }
                if *name == "KHR_materials_sheen"
                    && matches!(prop(extension, "sheenColorFactor"), Some(JsValue::Null))
                {
                    return reject("TypeError: Cannot read properties of null (reading '0')");
                }
            }
        }
        let material = Arc::new(material);
        self.materials.insert(index, material.clone());
        Ok(material)
    }

    /// The synchronous part of `assignTexture`, whose throws reject the load. A texture that
    /// then fails to load only becomes `null`, so the port never decodes texture images:
    /// `optimizeGeometries` replaces every material and drops UVs. The texture plugins run first:
    /// KTX2 needs a loader nobody sets, and the WebP and AVIF probes construct an `Image`, which
    /// a worker does not have, so their rejection reaches the load.
    fn check_texture(&self, map: Option<&JsValue>) -> Result<()> {
        let Some(map) = defined(map) else {
            return Ok(());
        };
        let texture = match (prop(map, "index"), prop(self.json, "textures")) {
            (Some(index), textures @ Some(_)) => element(textures, index),
            _ => None,
        };
        let Some(texture @ JsValue::Object(_)) = texture else {
            return reject("TypeError: textureDef is undefined");
        };
        let extensions = prop(texture, "extensions").filter(|v| truthy(Some(v)));
        let has = |name: &str| extensions.is_some_and(|e| truthy(prop(e, name)));
        if has("KHR_texture_basisu") && self.basisu_required {
            return reject(
                "THREE.GLTFLoader: setKTX2Loader must be called before loading KTX2 textures",
            );
        }
        if has("EXT_texture_webp") || has("EXT_texture_avif") {
            return reject("ReferenceError: Image is not defined");
        }
        let image = match (prop(texture, "source"), prop(self.json, "images")) {
            (Some(source), images @ Some(_)) => element(images, source),
            _ => None,
        };
        let Some(image @ JsValue::Object(_)) = image else {
            return reject("TypeError: sourceDef is undefined");
        };
        match (
            defined(prop(image, "bufferView")),
            defined(prop(image, "uri")),
        ) {
            (Some(view), _) => self.check_buffer_view(view),
            (None, Some(_)) => Ok(()),
            (None, None) => reject("THREE.GLTFLoader: Image is missing URI and bufferView"),
        }
    }

    /// The synchronous part of `loadBuffer`.
    fn check_buffer(&self, index: Option<&JsValue>) -> Result<()> {
        let index = index.and_then(index_of);
        let def = require(index.and_then(|i| self.defs("buffers").get(i)), "bufferDef")?;
        if let Some(t) = prop(def, "type")
            && truthy(Some(t))
            && *t != JsValue::string("arraybuffer")
        {
            return reject("THREE.GLTFLoader: buffer type is not supported.");
        }
        if prop(def, "uri").is_none_or(JsValue::is_undefined) && index == Some(0) && !self.binary {
            return reject("TypeError: Cannot read properties of undefined (reading 'body')");
        }
        Ok(())
    }

    /// The synchronous part of `getDependency('bufferView')`: the meshopt plugin, then
    /// `loadBufferView`.
    fn check_buffer_view(&self, index: &JsValue) -> Result<()> {
        let def = require(
            index_of(index).and_then(|i| self.defs("bufferViews").get(i)),
            "bufferViewDef",
        )?;
        if let Some(extensions) = prop(def, "extensions")
            && let Some(meshopt) = prop(extensions, "EXT_meshopt_compression")
            && truthy(Some(meshopt))
        {
            self.check_buffer(prop(meshopt, "buffer"))?;
            if self.meshopt_required {
                return reject(
                    "THREE.GLTFLoader: setMeshoptDecoder must be called before loading compressed files",
                );
            }
        }
        self.check_buffer(defined(prop(def, "buffer")))
    }

    fn load_buffer(&mut self, index: usize) -> Result<Arc<Vec<u8>>> {
        if let Some(b) = self.buffers.get(&index) {
            return Ok(b.clone());
        }
        let def = require(self.defs("buffers").get(index), "bufferDef")?.clone();
        if let Some(t) = prop(&def, "type")
            && truthy(Some(t))
            && *t != JsValue::string("arraybuffer")
        {
            return reject("THREE.GLTFLoader: buffer type is not supported.");
        }
        let uri = prop(&def, "uri").cloned().unwrap_or_default();
        let bytes = if uri.is_undefined() && index == 0 {
            if !self.binary {
                return reject("TypeError: Cannot read properties of undefined (reading 'body')");
            }
            match self.body.take() {
                Some(body) => body,
                None => {
                    return reject("TypeError: Cannot read properties of null (reading 'slice')");
                }
            }
        } else {
            let url = resolve_url(&uri, &self.path);
            let text = url.to_string_lossy();
            let loaded = if text.to_ascii_lowercase().starts_with("data:") {
                decode_data_url(&text)
            } else if text.is_empty() {
                None
            } else {
                (self.fetch)(&url)
            };
            match loaded {
                Some(bytes) => bytes,
                None => {
                    return reject(format!(
                        "THREE.GLTFLoader: Failed to load buffer \"{}\".",
                        uri.as_str()
                            .map(|s| s.to_string_lossy())
                            .unwrap_or_default()
                    ));
                }
            }
        };
        let bytes = Arc::new(bytes);
        self.buffers.insert(index, bytes.clone());
        Ok(bytes)
    }

    fn load_buffer_view(&mut self, index: &JsValue) -> Result<Arc<Vec<u8>>> {
        let Some(index) = index_of(index) else {
            return reject("TypeError: bufferViewDef is undefined");
        };
        if let Some(view) = self.buffer_views.get(&index) {
            return Ok(view.clone());
        }
        self.check_buffer_view(&JsValue::Number(index as f64))?;
        let def = require(self.defs("bufferViews").get(index), "bufferViewDef")?.clone();
        let Some(buffer) = defined(prop(&def, "buffer")).and_then(index_of) else {
            return reject("TypeError: bufferDef is undefined");
        };
        let buffer = self.load_buffer(buffer)?;
        let or_zero = |v: Option<&JsValue>| if truthy(v) { to_number(v) } else { 0.0 };
        let byte_length = or_zero(prop(&def, "byteLength"));
        let byte_offset = or_zero(prop(&def, "byteOffset"));
        let view = Arc::new(slice_buffer(
            &buffer,
            byte_offset,
            byte_offset + byte_length,
        ));
        self.buffer_views.insert(index, view.clone());
        Ok(view)
    }

    fn load_accessor(&mut self, index_value: &JsValue) -> Result<BufferAttribute> {
        let Some(index) = index_of(index_value) else {
            return reject("TypeError: accessorDef is undefined");
        };
        if let Some(a) = self.accessors.get(&index) {
            return Ok(a.clone());
        }
        let def = require(self.defs("accessors").get(index), "accessorDef")?.clone();
        let size = item_size(prop(&def, "type"))?;
        let kind = component_kind(prop(&def, "componentType"))?;
        let normalized = matches!(prop(&def, "normalized"), Some(JsValue::Bool(true)));
        let count = to_number(prop(&def, "count"));
        let buffer_view_index = defined(prop(&def, "bufferView"));
        let sparse = defined(prop(&def, "sparse"));
        let width = element_bytes(kind);

        let mut attribute = match buffer_view_index {
            None => BufferAttribute::new(
                ArrayData::zeroed(kind, to_index(count * size as f64)?),
                size,
                normalized,
            ),
            Some(view_index) => {
                let view = self.load_buffer_view(view_index)?;
                let byte_offset = if truthy(prop(&def, "byteOffset")) {
                    to_number(prop(&def, "byteOffset"))
                } else {
                    0.0
                };
                let stride = index_of(view_index)
                    .and_then(|i| self.defs("bufferViews").get(i))
                    .and_then(|v| prop(v, "byteStride"))
                    .map(|v| to_number(Some(v)))
                    .filter(|s| *s != 0.0 && !s.is_nan());
                match stride {
                    Some(stride) if stride != (width * size) as f64 => {
                        let slice = math::floor(byte_offset / stride);
                        let array = typed_array(
                            &view,
                            kind,
                            to_index(slice * stride)?,
                            to_index(count * stride / width as f64)?,
                        )?;
                        let stride_elements = stride / width as f64;
                        let offset = (byte_offset % stride) / width as f64;
                        if sparse.is_some() {
                            BufferAttribute::new(array, size, normalized)
                        } else {
                            deinterleave(
                                &array,
                                stride_elements,
                                offset,
                                size,
                                to_index(count)?,
                                normalized,
                            )?
                        }
                    }
                    _ => BufferAttribute::new(
                        typed_array(
                            &view,
                            kind,
                            to_index(byte_offset)?,
                            to_index(count * size as f64)?,
                        )?,
                        size,
                        normalized,
                    ),
                }
            }
        };

        if let Some(sparse) = sparse {
            let sparse = sparse.clone();
            let indices_def = prop(&sparse, "indices").cloned().unwrap_or_default();
            let values_def = prop(&sparse, "values").cloned().unwrap_or_default();
            let indices_view = self.load_buffer_view(
                prop(&indices_def, "bufferView").unwrap_or(&JsValue::Undefined),
            )?;
            let values_view = self
                .load_buffer_view(prop(&values_def, "bufferView").unwrap_or(&JsValue::Undefined))?;
            let sparse_count = to_number(prop(&sparse, "count"));
            let offset_of = |d: &JsValue| {
                if truthy(prop(d, "byteOffset")) {
                    to_number(prop(d, "byteOffset"))
                } else {
                    0.0
                }
            };
            let index_kind = component_kind(prop(&indices_def, "componentType"))?;
            let indices = typed_array(
                &indices_view,
                index_kind,
                to_index(offset_of(&indices_def))?,
                to_index(sparse_count)?,
            )?;
            let values = typed_array(
                &values_view,
                kind,
                to_index(offset_of(&values_def))?,
                to_index(sparse_count * size as f64)?,
            )?;
            if size >= 5 && !indices.is_empty() {
                return reject("THREE.GLTFLoader: Unsupported itemSize in sparse BufferAttribute.");
            }
            for i in 0..indices.len() {
                let target = indices.get(i) as usize;
                for c in 0..size {
                    if (target * size + c) < attribute.array.len() {
                        attribute.set_component(target, c, values.get(i * size + c));
                    }
                }
            }
        }
        self.accessors.insert(index, attribute.clone());
        Ok(attribute)
    }

    fn load_geometry(&mut self, primitive: &JsValue) -> Result<BufferGeometry> {
        if let Some(extensions) = prop(primitive, "extensions")
            && truthy(prop(extensions, "KHR_draco_mesh_compression"))
        {
            return reject(
                "TypeError: Cannot read properties of undefined (reading 'decodePrimitive')",
            );
        }
        if defined(prop(primitive, "targets")).is_some() {
            return unmodeled("morph targets");
        }
        let mut geometry = BufferGeometry::new();
        if let Some(JsValue::Object(attributes)) = prop(primitive, "attributes") {
            for (gltf_name, accessor) in attributes.iter() {
                let name = match gltf_name.to_string_lossy().as_str() {
                    "POSITION" => "position".to_owned(),
                    "NORMAL" => "normal".to_owned(),
                    "TANGENT" => "tangent".to_owned(),
                    "TEXCOORD_0" => "uv".to_owned(),
                    "TEXCOORD_1" => "uv1".to_owned(),
                    "TEXCOORD_2" => "uv2".to_owned(),
                    "TEXCOORD_3" => "uv3".to_owned(),
                    "COLOR_0" => "color".to_owned(),
                    "WEIGHTS_0" => "skinWeight".to_owned(),
                    "JOINTS_0" => "skinIndex".to_owned(),
                    _ => gltf_name.to_lower_case().to_string_lossy(),
                };
                let attribute = self.load_accessor(accessor)?;
                geometry.set_attribute(&name, attribute);
            }
        } else if matches!(
            prop(primitive, "attributes"),
            None | Some(JsValue::Undefined | JsValue::Null)
        ) {
            return reject("TypeError: Cannot convert undefined or null to object");
        } else {
            return unmodeled("primitive attributes that are not an object");
        }
        self.check_bounds(primitive)?;
        if let Some(indices) = defined(prop(primitive, "indices")) {
            geometry.index = Some(self.load_accessor(indices)?);
        }
        Ok(geometry)
    }

    /// `computeBounds`, which reads the POSITION accessor's `min` and `max` and throws on a null
    /// one or on a normalized component type `getNormalizedComponentScale` does not know.
    fn check_bounds(&self, primitive: &JsValue) -> Result<()> {
        let Some(position) = prop(primitive, "attributes")
            .and_then(|a| defined(prop(a, "POSITION")))
            .and_then(index_of)
        else {
            return Ok(());
        };
        let Some(accessor) = self.defs("accessors").get(position) else {
            return Ok(());
        };
        let (Some(min), Some(max)) = (
            defined(prop(accessor, "min")),
            defined(prop(accessor, "max")),
        ) else {
            return Ok(());
        };
        if matches!(min, JsValue::Null) || matches!(max, JsValue::Null) {
            return reject("TypeError: Cannot read properties of null (reading '0')");
        }
        if truthy(prop(accessor, "normalized"))
            && !matches!(
                component_kind(prop(accessor, "componentType")),
                Ok(ArrayKind::Int8 | ArrayKind::Uint8 | ArrayKind::Int16 | ArrayKind::Uint16)
            )
        {
            return reject("THREE.GLTFLoader: Unsupported normalized accessor component type.");
        }
        Ok(())
    }

    /// `loadAnimation`: its target nodes load (and take their names) in channel order, and the
    /// tracks it builds afterwards throw on an empty time accessor or a normalized output that
    /// `_getArrayFromAccessor` cannot scale.
    fn load_animation(&mut self, def: &JsValue) -> Result<()> {
        let channels = match prop(def, "channels") {
            _ if matches!(def, JsValue::Null) => {
                return reject("TypeError: Cannot read properties of null (reading 'name')");
            }
            Some(JsValue::Array(c)) => c.clone(),
            None | Some(JsValue::Undefined | JsValue::Null) => {
                return reject("TypeError: Cannot read properties of undefined (reading 'length')");
            }
            Some(_) => return unmodeled("animation channels that are not an array"),
        };
        let samplers = prop(def, "samplers");
        let parameters = defined(prop(def, "parameters"));
        let mut tracks = Vec::new();
        for channel in &channels {
            let sampler = match (channel, samplers) {
                (JsValue::Null, _) | (_, None | Some(JsValue::Undefined | JsValue::Null)) => None,
                (channel, samplers) => element(
                    samplers,
                    prop(channel, "sampler").unwrap_or(&JsValue::Undefined),
                ),
            };
            let target = prop(channel, "target");
            if matches!(target, None | Some(JsValue::Undefined | JsValue::Null))
                || matches!(sampler, None | Some(JsValue::Undefined | JsValue::Null))
                || matches!(parameters, Some(JsValue::Null))
            {
                return reject("TypeError: Cannot read properties of undefined");
            }
            let sampler = sampler.expect("checked above");
            let accessor = |key: &str| {
                let reference = prop(sampler, key).unwrap_or(&JsValue::Undefined);
                match parameters {
                    Some(parameters) => element(Some(parameters), reference)
                        .cloned()
                        .unwrap_or_default(),
                    None => reference.clone(),
                }
            };
            let (input, output) = (accessor("input"), accessor("output"));
            let Some(node) = target.and_then(|t| defined(prop(t, "node"))) else {
                continue;
            };
            self.visit(node, 1)?;
            tracks.push((input, output));
        }
        for (input, output) in tracks {
            let times = self.load_accessor(&input)?;
            let values = self.load_accessor(&output)?;
            if times.array.is_empty() {
                return reject("THREE.KeyframeTrack: no keyframes in track");
            }
            if values.normalized
                && !matches!(
                    values.array.kind(),
                    ArrayKind::Int8 | ArrayKind::Uint8 | ArrayKind::Int16 | ArrayKind::Uint16
                )
            {
                return reject("THREE.GLTFLoader: Unsupported normalized accessor component type.");
            }
        }
        Ok(())
    }

    fn load_mesh(&mut self, index: usize) -> Result<MeshTemplate> {
        let def = require(self.defs("meshes").get(index), "meshDef")?.clone();
        let primitives = primitives_of(&def)?;
        let mut materials = Vec::new();
        for primitive in &primitives {
            materials.push(match defined(prop(primitive, "material")) {
                Some(m) => self.load_material(m)?,
                None => self.default_material.clone(),
            });
        }
        let mut geometries = Vec::new();
        for primitive in &primitives {
            geometries.push(self.load_geometry(primitive)?);
        }
        let base_name = if truthy(prop(&def, "name")) {
            name_arg(prop(&def, "name"))?
        } else {
            JsString::from(format!("mesh_{index}"))
        };
        let mut objects = Vec::new();
        for ((primitive, geometry), material) in primitives.iter().zip(geometries).zip(materials) {
            let mode = defined(prop(primitive, "mode"));
            let kind = match mode.map(|m| match m {
                JsValue::Number(n) => *n,
                _ => f64::NAN,
            }) {
                None | Some(4.0) => Some("Mesh"),
                Some(5.0) => Some("strip"),
                Some(6.0) => Some("fan"),
                Some(1.0) => Some("LineSegments"),
                Some(3.0) => Some("Line"),
                Some(2.0) => Some("LineLoop"),
                Some(0.0) => Some("Points"),
                _ => None,
            };
            let object = match kind {
                None => return reject("THREE.GLTFLoader: Primitive mode unsupported"),
                Some(kind @ ("Mesh" | "strip" | "fan")) => {
                    let geometry = match kind {
                        "strip" => to_triangles_draw_mode(geometry, false),
                        "fan" => to_triangles_draw_mode(geometry, true),
                        _ => geometry,
                    };
                    ObjectKind::Mesh {
                        geometry: Arc::new(geometry),
                        material: MeshMaterial::Single(material),
                    }
                }
                Some(other) => ObjectKind::Other(other),
            };
            let name = self.names.unique(&base_name);
            objects.push(SceneObject::new(name, object));
        }
        Ok(MeshTemplate {
            primitives: objects,
        })
    }

    /// The object `_loadNodeShallow` resolves for a node, with `_getNodeRef`'s clones for meshes
    /// and cameras that several nodes reference.
    fn build_node(
        &mut self,
        graph: &mut SceneGraph,
        index: usize,
        name: JsString,
    ) -> Result<ObjectId> {
        let def = self.node_def(index)?.clone();
        let mut objects = Vec::new();
        if let Some(mesh) = defined(prop(&def, "mesh")).and_then(index_of) {
            let suffix = self.instance_suffix(true, mesh);
            let template = &self.meshes[&mesh];
            let id = if template.primitives.len() == 1 {
                graph.create("", template.primitives[0].clone())
            } else {
                let group = graph.create("", SceneObject::new(JsString::new(), ObjectKind::Group));
                for primitive in &template.primitives {
                    let child = graph.create("", primitive.clone());
                    graph.add(group, child);
                }
                group
            };
            if let Some(suffix) = suffix {
                graph.node_mut(id).data.name.push_str(&suffix);
            }
            objects.push(id);
        }
        if let Some(camera) = defined(prop(&def, "camera")).and_then(index_of) {
            let suffix = self.instance_suffix(false, camera);
            let id = graph.create("", self.cameras[&camera].object.clone());
            if let Some(suffix) = suffix {
                graph.node_mut(id).data.name.push_str(&suffix);
            }
            objects.push(id);
        }
        let node = match objects.as_slice() {
            [] => graph.create("", SceneObject::new(JsString::new(), ObjectKind::Object3D)),
            [single] => *single,
            many => {
                let group = graph.create("", SceneObject::new(JsString::new(), ObjectKind::Group));
                for &object in many {
                    graph.add(group, object);
                }
                group
            }
        };
        if truthy(prop(&def, "name")) {
            graph.node_mut(node).data.name = name;
        }
        let object = graph.object_mut(node);
        if let Some(matrix) = defined(prop(&def, "matrix")) {
            let mut e = [0.0; 16];
            for (i, slot) in e.iter_mut().enumerate() {
                *slot = number_elem(Some(matrix), i);
            }
            object.apply_matrix4(&Matrix4::from_array(e));
        } else {
            if let Some(t) = defined(prop(&def, "translation")) {
                object.position = Vector3::new(
                    number_elem(Some(t), 0),
                    number_elem(Some(t), 1),
                    number_elem(Some(t), 2),
                );
            }
            if let Some(r) = defined(prop(&def, "rotation")) {
                object.set_quaternion(Quaternion::new(
                    number_elem(Some(r), 0),
                    number_elem(Some(r), 1),
                    number_elem(Some(r), 2),
                    number_elem(Some(r), 3),
                ));
            }
            if let Some(s) = defined(prop(&def, "scale")) {
                object.scale = Vector3::new(
                    number_elem(Some(s), 0),
                    number_elem(Some(s), 1),
                    number_elem(Some(s), 2),
                );
            }
        }
        Ok(node)
    }

    fn instance_suffix(&mut self, mesh: bool, index: usize) -> Option<JsString> {
        let (refs, uses) = if mesh {
            (&self.mesh_refs, &mut self.mesh_uses)
        } else {
            (&self.camera_refs, &mut self.camera_uses)
        };
        if refs.get(&index).copied().unwrap_or(0) <= 1 {
            return None;
        }
        let n = uses.entry(index).or_default();
        let suffix = JsString::from(format!("_instance_{n}"));
        *n += 1;
        Some(suffix)
    }
}

/// Upstream recurses through the node tree (`loadNode`, `traverse`, `updateWorldMatrix`), and
/// its worker hits V8's stack limit somewhere past 4000 levels, which no asset comes near. The
/// port refuses deeper trees instead of modeling that limit, and so bounds its own recursion.
const MAX_DEPTH: usize = 4096;

fn too_deep<T>() -> Result<T> {
    unmodeled(format!("a node hierarchy deeper than {MAX_DEPTH} levels"))
}

fn hierarchy_depth(graph: &SceneGraph, root: ObjectId) -> usize {
    let mut deepest = 0;
    let mut stack = vec![(root, 1)];
    while let Some((id, depth)) = stack.pop() {
        deepest = deepest.max(depth);
        stack.extend(graph.node(id).children().iter().map(|&c| (c, depth + 1)));
    }
    deepest
}

fn add_checked(graph: &mut SceneGraph, parent: ObjectId, child: ObjectId) -> Result<()> {
    let mut cursor = Some(parent);
    while let Some(id) = cursor {
        if id == child {
            return reject("node hierarchy has a cycle");
        }
        cursor = graph.node(id).parent();
    }
    graph.add(parent, child);
    Ok(())
}

fn deinterleave(
    array: &ArrayData,
    stride: f64,
    offset: f64,
    size: usize,
    count: usize,
    normalized: bool,
) -> Result<BufferAttribute> {
    if stride.fract() != 0.0 || offset.fract() != 0.0 {
        return unmodeled("interleaved accessor not aligned to its component size");
    }
    let (stride, offset) = (stride as usize, offset as usize);
    let count = array.len().checked_div(stride).map_or(0, |n| count.min(n));
    let mut out = ArrayData::zeroed(array.kind(), count * size);
    for j in 0..count {
        for c in 0..size {
            let at = j * stride + offset + c;
            out.set(
                j * size + c,
                if at < array.len() {
                    array.get(at)
                } else {
                    f64::NAN
                },
            );
        }
    }
    let mut attribute = BufferAttribute::new(out, size, normalized);
    attribute.interleaved = true;
    Ok(attribute)
}

/// `BufferGeometryUtils.toTrianglesDrawMode` for `TriangleStripDrawMode` and
/// `TriangleFanDrawMode`.
fn to_triangles_draw_mode(mut geometry: BufferGeometry, fan: bool) -> BufferGeometry {
    let index = match &geometry.index {
        Some(index) => index.clone(),
        None => {
            let Some(position) = geometry.attribute("position") else {
                return geometry;
            };
            let indices: Vec<f64> = (0..position.count()).map(|i| i as f64).collect();
            geometry.set_index(&indices);
            geometry.index.clone().expect("set above")
        }
    };
    let triangles = index.count() as i64 - 2;
    let mut new_indices = Vec::new();
    for i in 0..triangles.max(0) as usize {
        if fan {
            new_indices.extend([index.get_x(0), index.get_x(i + 1), index.get_x(i + 2)]);
        } else if i % 2 == 0 {
            new_indices.extend([index.get_x(i), index.get_x(i + 1), index.get_x(i + 2)]);
        } else {
            new_indices.extend([index.get_x(i + 2), index.get_x(i + 1), index.get_x(i)]);
        }
    }
    let mut out = geometry.clone();
    out.set_index(&new_indices);
    out.groups.clear();
    out
}
