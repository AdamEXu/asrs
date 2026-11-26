//! Replays `parity/vectors/field3d/renderer` (oracle/src/genField3dRenderer.ts): each case drives
//! the port's `Field3dRenderer` through the recorded ops (renders, keys, mouse, camera and FOV
//! changes, pointer and wheel input, saved state, asset and preference changes, worker and
//! texture loads) and compares,
//! after every op, the whole scene graph, the camera, the controls, the DOM the renderer writes,
//! the render calls, main-process messages, load requests, the aspect ratio and the saved state
//! with what upstream's real renderer produced.

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use asrs_field3d::camera_controls::{ElementRect, PointerInput, WheelInput};
use asrs_field3d::glb::{self, LoadedField, OptimizedMesh};
use asrs_field3d::managers::{april_tag_texture_path, decode_png_texture};
use asrs_field3d::render::{
    Filter, LineBasicMaterial, LineMaterial, Materials, MinFilter, NodeKind, PhongMaterial, Side,
    Texture,
};
use asrs_field3d::renderer::{
    Field3dEnv, Field3dRenderer, Field3dRendererCommand, LoadRequest, TextureLoad,
};
use asrs_field3d::three::{BufferGeometry, ObjectId};
use asrs_js::string::{JsError, JsString};
use asrs_js::value::{JsObject, JsValue};
use asrs_model::assets::{
    AdvantageScopeAssets, Config3dField, Config3dRobot, Parsed, parse_field_3d, parse_robot,
};
use asrs_model::preferences::{
    CoordinateSystemPreference, Field3dBatteryMode, Field3dMode, Preferences,
};
use asrs_parity::vectors::{self, Record, check_records};
use asrs_parity::{Value, diff};

type Result<T> = std::result::Result<T, String>;

fn get<'a>(v: &'a Value, key: &str) -> Result<&'a Value> {
    v.get(key).ok_or_else(|| format!("missing {key}"))
}

fn items(v: &Value) -> Result<&[Value]> {
    v.as_array()
        .ok_or_else(|| format!("expected an array, got {}", v.kind()))
}

fn number(v: &Value) -> Result<f64> {
    v.as_f64()
        .ok_or_else(|| format!("expected a number, got {}", v.kind()))
}

fn boolean(v: &Value) -> Result<bool> {
    v.as_bool()
        .ok_or_else(|| format!("expected a bool, got {}", v.kind()))
}

fn text(v: &Value) -> Result<String> {
    v.as_str()
        .map(|s| JsString::from_units(s.units().to_vec()).to_string_lossy())
        .ok_or_else(|| format!("expected a string, got {}", v.kind()))
}

fn num(x: f64) -> Value {
    Value::number(x)
}

fn nums(xs: &[f64]) -> Value {
    Value::Array(xs.iter().map(|&x| num(x)).collect())
}

fn jstr(s: &JsString) -> Value {
    Value::String(s.units().to_vec().into())
}

fn to_js(v: &Value) -> Result<JsValue> {
    Ok(match v {
        Value::Undefined => JsValue::Undefined,
        Value::Null => JsValue::Null,
        Value::Bool(b) => JsValue::Bool(*b),
        Value::Number(_) => JsValue::Number(number(v)?),
        Value::String(s) => JsValue::String(JsString::from_units(s.units().to_vec())),
        Value::Array(items) => JsValue::Array(items.iter().map(to_js).collect::<Result<_>>()?),
        Value::Object(pairs) => {
            let mut o = JsObject::new();
            for (k, v) in pairs {
                o.insert(JsString::from_units(k.units().to_vec()), to_js(v)?);
            }
            JsValue::Object(o)
        }
        other => return Err(format!("no JS value for {}", other.kind())),
    })
}

fn from_js(v: &JsValue) -> Result<Value> {
    Ok(match v {
        JsValue::Undefined => Value::Undefined,
        JsValue::Null => Value::Null,
        JsValue::Bool(b) => Value::Bool(*b),
        JsValue::Number(x) => num(*x),
        JsValue::String(s) => jstr(s),
        JsValue::Array(items) => Value::Array(items.iter().map(from_js).collect::<Result<_>>()?),
        JsValue::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| Ok((k.units().to_vec().into(), from_js(v)?)))
                .collect::<Result<_>>()?,
        ),
        other => return Err(format!("unexpected JS value {other:?}")),
    })
}

fn fnv1a64(text: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for unit in text.encode_utf16() {
        h ^= u64::from(unit);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}")
}

fn float32_digest(xs: impl IntoIterator<Item = f32>) -> Value {
    let array = Value::TypedArray(
        "Float32Array".to_owned(),
        xs.into_iter().map(|x| num(f64::from(x))).collect(),
    );
    Value::string(&fnv1a64(&array.encode()))
}

fn geometry_digest(g: &BufferGeometry) -> Value {
    let array = |a: &asrs_field3d::three::ArrayData| {
        if let asrs_field3d::three::ArrayData::Uint8(bytes) = a {
            return Value::Bytes(bytes.clone());
        }
        let values = (0..a.len()).map(|i| num(a.get(i))).collect();
        Value::TypedArray(a.kind().name().to_owned(), values)
    };
    let tree = Value::Array(vec![
        g.index.as_ref().map_or(Value::Null, |i| array(&i.array)),
        Value::Array(
            g.attributes()
                .map(|(name, a)| {
                    Value::Array(vec![
                        Value::string(name),
                        num(a.item_size as f64),
                        Value::Bool(a.normalized),
                        array(&a.array),
                    ])
                })
                .collect(),
        ),
        Value::Array(
            g.groups
                .iter()
                .map(|g| nums(&[g.start as f64, g.count as f64, g.material_index as f64]))
                .collect(),
        ),
    ]);
    Value::string(&fnv1a64(&tree.encode()))
}

fn min_filter(f: MinFilter) -> f64 {
    match f {
        MinFilter::Nearest => 1003.0,
        MinFilter::NearestMipmapNearest => 1004.0,
        MinFilter::NearestMipmapLinear => 1005.0,
        MinFilter::Linear => 1006.0,
        MinFilter::LinearMipmapNearest => 1007.0,
        MinFilter::LinearMipmapLinear => 1008.0,
    }
}

fn mag_filter(f: Filter) -> f64 {
    match f {
        Filter::Nearest => 1003.0,
        Filter::Linear => 1006.0,
    }
}

struct Asset {
    config_text: String,
    field_id: Option<String>,
    never: bool,
}

struct Case {
    assets: HashMap<String, Asset>,
    files: HashMap<String, Vec<u8>>,
    asset_sets: HashMap<String, Arc<AdvantageScopeAssets>>,
    window_assets: Option<Arc<AdvantageScopeAssets>>,
    preferences: Option<Arc<Preferences>>,
    is_battery: bool,
    darwin: bool,
    env: Value,
    clock: f64,
    renderer: Field3dRenderer,
    pending: Vec<LoadRequest>,
    textures: Vec<TextureLoad>,
    texture_urls: HashMap<u64, JsString>,
    field_results: HashMap<(String, String), Option<LoadedField>>,
    robot_results: HashMap<(String, String), Option<Vec<Vec<OptimizedMesh>>>>,
    seen: HashSet<String>,
    messages: Vec<Value>,
    requests: Vec<Value>,
    requested_textures: Vec<Value>,
}

fn preferences(v: &Value) -> Result<Option<Arc<Preferences>>> {
    if matches!(v, Value::Undefined | Value::Null) {
        return Ok(None);
    }
    let mode = |s: String| -> Result<Field3dMode> {
        Field3dMode::from_js_str(&JsString::from(s.as_str())).ok_or(format!("mode {s}"))
    };
    let battery = text(get(v, "field3dModeBattery")?)?;
    let system = text(get(v, "coordinateSystem")?)?;
    Ok(Some(Arc::new(Preferences {
        field3d_mode_ac: mode(text(get(v, "field3dModeAc")?)?)?,
        field3d_mode_battery: Field3dBatteryMode::from_js_str(&JsString::from(battery.as_str()))
            .ok_or("battery mode")?,
        field3d_antialiasing: boolean(get(v, "field3dAntialiasing")?)?,
        coordinate_system: CoordinateSystemPreference::from_js_str(&JsString::from(
            system.as_str(),
        ))
        .ok_or("coordinate system")?,
        ..Preferences::default()
    })))
}

fn workspace_root() -> std::path::PathBuf {
    vectors::workspace_root()
}

fn mode_name(mode: Field3dMode) -> &'static str {
    mode.as_str()
}

impl Case {
    fn new(record: &Record) -> Result<Case> {
        let setup = record.value("setup").map_err(|e| format!("{e}"))?;
        let asset_list = record.value("assets").map_err(|e| format!("{e}"))?;
        let mut assets = HashMap::new();
        let mut files = HashMap::new();
        for asset in items(&asset_list)? {
            let id = text(get(asset, "id")?)?;
            for file in items(get(asset, "files")?)? {
                let pair = items(file)?;
                let name = text(&pair[0])?;
                let bundled = text(get(&pair[1], "bundled")?)?;
                let bytes = std::fs::read(
                    workspace_root()
                        .join("reference/AdvantageScope")
                        .join(&bundled),
                )
                .map_err(|e| format!("{bundled}: {e}"))?;
                files.insert(format!("{id}/{name}"), bytes);
            }
            let field_id = match get(asset, "fieldId")? {
                Value::Null => None,
                v => Some(text(v)?),
            };
            assets.insert(
                id,
                Asset {
                    config_text: text(get(asset, "config")?)?,
                    field_id,
                    never: boolean(get(asset, "never")?)?,
                },
            );
        }
        let robot = |id: &str| -> Result<Config3dRobot> {
            let asset = &assets[id];
            let raw = asrs_js::json::parse_str(&asset.config_text).map_err(|e| format!("{e:?}"))?;
            let Ok(Parsed::Config(mut config)) = parse_robot(&raw) else {
                return Err(format!("robot {id} does not parse"));
            };
            config.path = JsString::from(format!("{id}/model.glb").as_str());
            Ok(config)
        };
        let field = |id: &str| -> Result<Config3dField> {
            let asset = &assets[id];
            let raw = asrs_js::json::parse_str(&asset.config_text).map_err(|e| format!("{e:?}"))?;
            let Ok(Parsed::Config(mut config)) = parse_field_3d(&raw) else {
                return Err(format!("field {id} does not parse"));
            };
            config.path = JsString::from(format!("{id}/model.glb").as_str());
            config.id = JsString::from(asset.field_id.as_deref().ok_or("field without id")?);
            Ok(config)
        };
        let mut asset_sets = HashMap::new();
        let Value::Object(sets) = get(&setup, "assetSets")? else {
            return Err("assetSets is not an object".to_owned());
        };
        for (key, set) in sets {
            let ids = |k: &str| -> Result<Vec<String>> {
                items(get(set, k)?)?.iter().map(text).collect()
            };
            let set = AdvantageScopeAssets {
                field3ds: ids("field3ds")?
                    .iter()
                    .map(|id| field(id))
                    .collect::<Result<_>>()?,
                robots: ids("robots")?
                    .iter()
                    .map(|id| robot(id))
                    .collect::<Result<_>>()?,
                ..AdvantageScopeAssets::default()
            };
            asset_sets.insert(
                JsString::from_units(key.units().to_vec()).to_string_lossy(),
                Arc::new(set),
            );
        }
        let window_assets = match get(&setup, "assets")? {
            Value::Null => None,
            key => Some(asset_sets[&text(key)?].clone()),
        };
        let prefs = preferences(get(&setup, "preferences")?)?;
        let is_battery = boolean(get(&setup, "isBattery")?)?;
        let darwin = text(get(&setup, "platform")?)? == "darwin";
        let env = get(&setup, "env")?.clone();
        let clock = number(get(&setup, "now")?)?;
        let renderer = Field3dRenderer::new(prefs.as_deref(), is_battery, true);
        Ok(Case {
            assets,
            files,
            asset_sets,
            window_assets,
            preferences: prefs,
            is_battery,
            darwin,
            env,
            clock,
            renderer,
            pending: Vec::new(),
            textures: Vec::new(),
            texture_urls: HashMap::new(),
            field_results: HashMap::new(),
            robot_results: HashMap::new(),
            seen: HashSet::new(),
            messages: Vec::new(),
            requests: Vec::new(),
            requested_textures: Vec::new(),
        })
    }

    fn env_number(&self, key: &str) -> Result<f64> {
        number(get(&self.env, key)?)
    }

    fn env(&self) -> Result<Field3dEnv> {
        let width = self.env_number("width")?;
        Ok(Field3dEnv {
            assets: self.window_assets.clone(),
            preferences: self.preferences.clone(),
            is_battery: self.is_battery,
            client_width: width,
            client_height: self.env_number("height")?,
            rect_width: width,
            parent_size: Some([
                self.env_number("parentWidth")?,
                self.env_number("parentHeight")?,
            ]),
            device_pixel_ratio: self.env_number("dpr")?,
            is_dark: boolean(get(&self.env, "dark")?)?,
            reduced_motion: boolean(get(&self.env, "reducedMotion")?)?,
            performance_now: self.clock,
            date_now: self.clock,
        })
    }

    fn collect_requests(&mut self) {
        for request in self.renderer.take_load_requests() {
            let (script, name, is_ftc, options) = match &request {
                LoadRequest::Field {
                    config, options, ..
                } => ("../bundles/loadField.js", &config.id, Value::Null, options),
                LoadRequest::Robot { request, .. } => (
                    "../bundles/loadRobot.js",
                    &request.config.name,
                    Value::Bool(request.is_ftc),
                    &request.options,
                ),
            };
            self.requests.push(Value::Array(vec![
                Value::string(script),
                jstr(name),
                is_ftc,
                Value::string(mode_name(options.mode)),
                nums(&options.material_specular),
                num(options.material_shininess),
            ]));
            self.pending.push(request);
        }
        for texture in self.renderer.take_texture_requests() {
            self.requested_textures.push(jstr(&texture.url));
            self.textures.push(texture);
        }
    }

    fn asset_id(path: &JsString) -> Result<String> {
        path.to_string_lossy()
            .split('/')
            .next()
            .map(str::to_owned)
            .ok_or_else(|| "a load for no asset".to_owned())
    }

    fn check_never(&self, id: &str, loads: bool) -> Result<()> {
        match (loads, self.assets[id].never) {
            (true, true) => Err(format!("{id} loads; upstream never answers")),
            (false, false) => Err(format!("{id} fails to load; upstream answers")),
            _ => Ok(()),
        }
    }

    fn answer_loads(&mut self) -> Result<()> {
        let files = &self.files;
        let fetch = |url: &JsString| files.get(&url.to_string_lossy()).cloned();
        let mut still = Vec::new();
        for request in std::mem::take(&mut self.pending) {
            match &request {
                LoadRequest::Field {
                    ticket,
                    config,
                    options,
                } => {
                    let id = Self::asset_id(&config.path)?;
                    let key = (id.clone(), mode_name(options.mode).to_owned());
                    if !self.field_results.contains_key(&key) {
                        let loaded =
                            glb::with_load_stack(|| glb::load_field(config, options, &fetch));
                        self.field_results.insert(key.clone(), loaded.ok());
                    }
                    let result = &self.field_results[&key];
                    self.check_never(&id, result.is_some())?;
                    match result {
                        Some(field) => self.renderer.field_loaded(*ticket, field),
                        None => still.push(request.clone()),
                    }
                }
                LoadRequest::Robot {
                    ticket,
                    request: robot,
                } => {
                    let id = Self::asset_id(&robot.config.path)?;
                    let key = (id.clone(), mode_name(robot.options.mode).to_owned());
                    if !self.robot_results.contains_key(&key) {
                        let loaded = glb::with_load_stack(|| {
                            glb::load_robot(&robot.config, robot.is_ftc, &robot.options, &fetch)
                        });
                        self.robot_results.insert(key.clone(), loaded.ok());
                    }
                    let result = &self.robot_results[&key];
                    self.check_never(&id, result.is_some())?;
                    match result {
                        Some(meshes) => self.renderer.robot_loaded(*ticket, meshes),
                        None => still.push(request.clone()),
                    }
                }
            }
        }
        self.pending = still;
        Ok(())
    }

    fn load_textures(&mut self) -> Result<()> {
        for request in std::mem::take(&mut self.textures) {
            let Some(path) = april_tag_texture_path(
                &workspace_root().join("reference/AdvantageScope"),
                &request.url,
            ) else {
                continue;
            };
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let texture: Texture = decode_png_texture(&bytes)
                .ok_or_else(|| format!("{} does not decode", path.display()))?;
            self.texture_urls.insert(texture.key(), request.url.clone());
            self.renderer.texture_loaded(request.ticket, texture);
        }
        Ok(())
    }

    fn apply(&mut self, op: &Value) -> Result<std::result::Result<(), JsError>> {
        let op = items(op)?;
        let name = text(&op[0])?;
        let mut outcome = Ok(());
        match name.as_str() {
            "env" => {
                let Value::Object(update) = &op[1] else {
                    return Err("env update is not an object".to_owned());
                };
                let Value::Object(env) = &mut self.env else {
                    return Err("env is not an object".to_owned());
                };
                for (key, value) in update {
                    match env.iter_mut().find(|(k, _)| k == key) {
                        Some(slot) => slot.1 = value.clone(),
                        None => return Err("unknown env key".to_owned()),
                    }
                }
            }
            "advance" => {
                self.clock += number(&op[1])?;
                self.renderer.dom_mut().run_timers(self.clock);
            }
            "render" => {
                let command = Field3dRendererCommand::from_js(&to_js(&op[1])?)
                    .map_err(|e| format!("command: {e:?}"))?;
                let env = self.env()?;
                outcome = self.renderer.render(&command, &env);
            }
            "key" => {
                let code = text(&op[2])?;
                let opts = &op[3];
                let flag = |k: &str| opts.get(k).and_then(Value::as_bool).unwrap_or(false);
                let editable = opts
                    .get("editable")
                    .is_some_and(|v| !matches!(v, Value::Undefined));
                if text(&op[1])? == "down" {
                    let modifier = if self.darwin {
                        flag("meta")
                    } else {
                        flag("ctrl")
                    };
                    let height = self.env_number("containerHeight")?;
                    self.renderer.key_down(&code, modifier, editable, height);
                } else {
                    self.renderer.key_up(&code, editable);
                }
            }
            "mouse" => {
                let (button, x, y) = (number(&op[2])?, number(&op[3])?, number(&op[4])?);
                if text(&op[1])? == "down" {
                    self.renderer.mouse_down(button, x, y);
                } else if let Some(data) =
                    self.renderer.mouse_up(x, y, self.window_assets.as_deref())
                {
                    self.messages.push(Value::Array(vec![
                        Value::string("ask-3d-camera"),
                        from_js(&data)?,
                    ]));
                }
            }
            "pointer" => {
                let event = PointerInput {
                    pointer_id: number(&op[2])? as i32,
                    buttons: number(&op[3])? as u32,
                    client_x: number(&op[4])?,
                    client_y: number(&op[5])?,
                };
                let rect = ElementRect {
                    x: 0.0,
                    y: 0.0,
                    width: self.env_number("width")?,
                    height: self.env_number("height")?,
                };
                if let Some(implementation) = self.renderer.implementation_mut() {
                    let controls = implementation.controls_mut();
                    match text(&op[1])?.as_str() {
                        "down" => controls.pointer_down(event, rect),
                        "move" => controls.pointer_move(event),
                        _ => controls.pointer_up(event),
                    }
                }
            }
            "wheel" => {
                if let Some(implementation) = self.renderer.implementation_mut() {
                    implementation.controls_mut().wheel(WheelInput {
                        delta_y: number(&op[1])?,
                        delta_mode: number(&op[2])? as u32,
                    });
                }
            }
            "camera" => self.renderer.set_3d_camera(number(&op[1])?),
            "fov" => self.renderer.set_fov(number(&op[1])?),
            "restore" => self.renderer.restore_state(&to_js(&op[1])?),
            "assets" => {
                self.window_assets = match &op[1] {
                    Value::Null => None,
                    key => Some(self.asset_sets[&text(key)?].clone()),
                };
            }
            "preferences" => self.preferences = preferences(&op[1])?,
            "battery" => self.is_battery = boolean(&op[1])?,
            "load" => self.answer_loads()?,
            "loadTextures" => self.load_textures()?,
            other => return Err(format!("unknown op {other}")),
        }
        self.collect_requests();
        Ok(outcome)
    }

    fn texture(&self, texture: Option<&Arc<Texture>>) -> Value {
        let Some(texture) = texture else {
            return Value::Null;
        };
        let mut out = match self.texture_urls.get(&texture.key()) {
            Some(url) => vec![Value::string("url"), jstr(url)],
            None => vec![Value::string("canvas")],
        };
        out.extend([
            num(min_filter(texture.min_filter)),
            num(mag_filter(texture.mag_filter)),
            Value::Bool(texture.flip_y),
            Value::Bool(texture.generate_mipmaps),
        ]);
        Value::Array(out)
    }

    fn phong(&self, m: &PhongMaterial) -> Value {
        let side = match m.side {
            Side::Front => 0.0,
            Side::Back => 1.0,
            Side::Double => 2.0,
        };
        Value::Array(vec![
            Value::string("MeshPhongMaterial"),
            nums(&m.color.to_array()),
            nums(&m.specular.to_array()),
            num(m.shininess),
            nums(&m.emissive.to_array()),
            num(m.opacity),
            Value::Bool(m.transparent),
            num(side),
            Value::Bool(m.depth_test),
            Value::Bool(m.depth_write),
            Value::Bool(m.vertex_colors),
            Value::Bool(m.visible),
            self.texture(m.map.as_ref()),
        ])
    }

    fn line_material(m: &LineMaterial) -> Value {
        Value::Array(vec![
            Value::string("LineMaterial"),
            nums(&m.color.to_array()),
            num(m.linewidth),
            num(m.opacity),
            Value::Bool(m.transparent),
            Value::Bool(m.depth_test),
            Value::Bool(m.depth_write),
            Value::Bool(m.visible),
        ])
    }

    fn line_basic_material(m: &LineBasicMaterial) -> Value {
        Value::Array(vec![
            Value::string("LineBasicMaterial"),
            nums(&m.color.to_array()),
            Value::Bool(m.vertex_colors),
            num(m.opacity),
            Value::Bool(m.transparent),
            Value::Bool(m.visible),
        ])
    }

    fn tree(&mut self, id: ObjectId) -> Value {
        let scene = self
            .renderer
            .implementation()
            .expect("an implementation")
            .scene();
        let node = scene.node(id);
        let object = scene.graph.object(id);
        let (kind, cast, receive, extra) = match &node.kind {
            NodeKind::Object3D => ("Object3D", false, false, Value::Null),
            NodeKind::Group => ("Group", false, false, Value::Null),
            NodeKind::Mesh(mesh) => {
                let (array, materials) = match &mesh.material {
                    Materials::Single(m) => (false, vec![self.phong(m)]),
                    Materials::Array(ms) => (true, ms.iter().map(|m| self.phong(m)).collect()),
                };
                let instances = mesh.instances.as_ref().map_or(Value::Null, |i| {
                    Value::Array(vec![
                        num(i.count() as f64),
                        float32_digest(i.matrices.iter().flatten().copied()),
                    ])
                });
                (
                    "Mesh",
                    mesh.cast_shadow,
                    mesh.receive_shadow,
                    Value::Array(vec![
                        geometry_digest(&mesh.geometry.data()),
                        Value::Bool(array),
                        Value::Array(materials),
                        instances,
                    ]),
                )
            }
            NodeKind::FatLine(line) => {
                let segments = &line.geometry.segments;
                let digest = if segments.is_empty() {
                    Value::Null
                } else {
                    float32_digest(segments.iter().flatten().copied())
                };
                (
                    "Line2",
                    false,
                    false,
                    Value::Array(vec![digest, Self::line_material(&line.material)]),
                )
            }
            NodeKind::Line(line) => (
                if line.segments {
                    "LineSegments"
                } else {
                    "Line"
                },
                false,
                false,
                Value::Array(vec![
                    geometry_digest(&line.geometry.data()),
                    Self::line_basic_material(&line.material),
                ]),
            ),
            NodeKind::HemisphereLight(light) => (
                "HemisphereLight",
                false,
                false,
                Value::Array(vec![
                    nums(&light.color.to_array()),
                    nums(&light.ground_color.to_array()),
                    num(light.intensity),
                ]),
            ),
            NodeKind::PointLight(light) => (
                "PointLight",
                light.cast_shadow,
                false,
                Value::Array(vec![
                    nums(&light.color.to_array()),
                    num(light.intensity),
                    num(light.distance),
                    num(light.decay),
                ]),
            ),
            NodeKind::SpotLight(light) => (
                "SpotLight",
                light.cast_shadow,
                false,
                Value::Array(vec![
                    nums(&light.color.to_array()),
                    num(light.intensity),
                    num(light.distance),
                    num(light.angle),
                    num(light.penumbra),
                    num(light.decay),
                    Value::Bool(light.cast_shadow),
                    nums(&light.shadow.map_size),
                    num(light.shadow.bias),
                ]),
            ),
        };
        let header = vec![
            Value::string(kind),
            Value::Bool(node.visible),
            nums(&object.position.to_array()),
            nums(&object.quaternion().to_array()),
            nums(&object.scale.to_array()),
            num(node.render_order),
            Value::Bool(cast),
            Value::Bool(receive),
            Value::Bool(node.frustum_culled),
            extra,
        ];
        let children = scene.graph.node(id).children().to_vec();
        let children = Value::Array(children.into_iter().map(|c| self.tree(c)).collect());
        let mut node = header;
        node.push(children);
        let node = Value::Array(node);
        let digest = format!("#{}", fnv1a64(&node.encode()));
        if self.seen.contains(&digest) {
            return Value::string(&digest);
        }
        self.seen.insert(digest);
        node
    }

    fn state(&mut self, outcome: std::result::Result<(), JsError>) -> Result<Value> {
        let mut fields: Vec<(&str, Value)> = Vec::new();
        if let Err(e) = outcome {
            fields.push((
                "throw",
                Value::string(&format!("{}: {}", e.name, e.message)),
            ));
        }
        let (scene, camera, controls) = match self.renderer.implementation() {
            None => (Value::Null, Value::Null, Value::Null),
            Some(implementation) => {
                let root = implementation.scene().root;
                let children = implementation.scene().graph.node(root).children().to_vec();
                let c = implementation.controls();
                let cam = c.camera();
                let camera = Value::Array(vec![
                    nums(&cam.object.position.to_array()),
                    nums(&cam.object.quaternion().to_array()),
                    num(cam.fov),
                    num(cam.aspect),
                    num(cam.zoom),
                    nums(&cam.projection_matrix.elements),
                ]);
                let controls = Value::Array(vec![
                    Value::Bool(c.enabled()),
                    num(c.min_distance),
                    nums(&c.get_position(false).to_array()),
                    nums(&c.get_target(false).to_array()),
                    nums(&c.get_position(true).to_array()),
                    nums(&c.get_target(true).to_array()),
                ]);
                let scene = Value::Array(children.into_iter().map(|id| self.tree(id)).collect());
                (scene, camera, controls)
            }
        };
        let dom = self.renderer.dom();
        let dom = Value::Array(vec![
            Value::Bool(dom.canvas_fixed),
            Value::Bool(dom.container_fixed),
            Value::string(&dom.canvas_width),
            Value::string(&dom.canvas_height),
            Value::Bool(dom.alert_hidden),
            Value::string(&dom.alert_html),
            Value::Bool(dom.spinner_visible),
            Value::Bool(dom.spinner_animating),
        ]);
        let renders = self
            .renderer
            .implementation_mut()
            .map_or(Vec::new(), |i| i.take_renders())
            .into_iter()
            .map(|r| {
                Value::Array(vec![
                    num(f64::from(r.width)),
                    num(f64::from(r.height)),
                    num(r.pixel_ratio),
                    num(r.background.get_hex()),
                ])
            })
            .collect();
        fields.push(("scene", scene));
        fields.push(("camera", camera));
        fields.push(("controls", controls));
        fields.push(("dom", dom));
        fields.push(("renders", Value::Array(renders)));
        fields.push(("messages", Value::Array(std::mem::take(&mut self.messages))));
        fields.push(("requests", Value::Array(std::mem::take(&mut self.requests))));
        fields.push((
            "textures",
            Value::Array(std::mem::take(&mut self.requested_textures)),
        ));
        fields.push((
            "aspect",
            self.renderer.get_aspect_ratio().map_or(Value::Null, num),
        ));
        fields.push(("saved", from_js(&self.renderer.save_state())?));
        let heatmaps = self
            .renderer
            .implementation()
            .map_or(Value::Null, |implementation| {
                Value::Array(
                    implementation
                        .heatmap_managers()
                        .map(|m| {
                            m.heatmap().get_data().map_or(Value::Null, |store| {
                                Value::object([
                                    ("min", num(store.min)),
                                    ("max", num(store.max)),
                                    (
                                        "data",
                                        Value::Array(
                                            store
                                                .data
                                                .iter()
                                                .map(|p| {
                                                    Value::object([
                                                        ("x", Value::string(&p.x)),
                                                        ("y", Value::string(&p.y)),
                                                        ("radius", num(p.radius)),
                                                        ("value", num(p.value)),
                                                    ])
                                                })
                                                .collect(),
                                        ),
                                    ),
                                ])
                            })
                        })
                        .collect(),
                )
            });
        fields.push(("heatmaps", heatmaps));
        Ok(Value::object(fields))
    }
}

fn check_case(record: &Record, compared: &mut usize) -> Result<()> {
    let ops = record.value("ops").map_err(|e| format!("{e}"))?;
    let expected = record.value("out").map_err(|e| format!("{e}"))?;
    let (ops, expected) = (items(&ops)?, items(&expected)?);
    if ops.len() != expected.len() {
        return Err(format!("{} ops but {} states", ops.len(), expected.len()));
    }
    let mut case = Case::new(record)?;
    for (i, (op, expected)) in ops.iter().zip(expected).enumerate() {
        let outcome = case.apply(op)?;
        let actual = case.state(outcome)?;
        if !vectors::values_match(expected, &actual) {
            let report =
                diff::report(expected, &actual).unwrap_or_else(|| "encodings differ".to_owned());
            return Err(format!("op {i} {}: {report}", op.encode()));
        }
        *compared += 1;
    }
    Ok(())
}

#[test]
fn renderer_matches_upstream() {
    let mut compared = 0;
    let mut recorded = 0;
    check_records("field3d/renderer", |record| {
        recorded += record
            .value("out")
            .ok()
            .and_then(|v| v.as_array().map(<[Value]>::len))
            .unwrap_or(0);
        check_case(record, &mut compared)
    });
    assert_eq!(compared, recorded, "every recorded state is compared");
}
