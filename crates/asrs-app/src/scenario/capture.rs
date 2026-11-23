//! The files one port capture writes, in the layout and keys of the Electron driver's
//! (`oracle/src/appMain.ts`), so `cargo xtask pixel` reads both sides with one loader.

use std::path::Path;

use asrs_js::value::{JsObject, JsValue};
use asrs_ui::capture::Capture;
use asrs_ui::parity::{Recorder, Rect};
use asrs_ui::platform::dialog::{FileFilter, MessageBoxType, Shown};

use super::{Action, COLUMNS, PATCH, Page, Scenario, Theme, patch_colours};

/// `parity/probes.json`: for each upstream selector a scenario probes or masks, the parity ids
/// of the port elements it matches.
pub struct Probes(Vec<(String, Vec<String>)>);

impl Probes {
    pub fn load(path: &Path) -> Result<Probes, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let root =
            asrs_js::json::parse_str(&text).map_err(|e| format!("{}: {e:?}", path.display()))?;
        let object = root
            .as_object()
            .ok_or_else(|| format!("{}: not an object", path.display()))?;
        let mut map = Vec::new();
        for key in object.keys() {
            let selector = key.to_string_lossy();
            if selector == "about" {
                continue;
            }
            let ids = object
                .get_key(key)
                .and_then(JsValue::as_array)
                .and_then(|ids| {
                    ids.iter()
                        .map(|id| id.as_str().map(|s| s.to_string_lossy()))
                        .collect::<Option<Vec<_>>>()
                })
                .ok_or_else(|| {
                    format!("{}: {selector} is not a list of parity ids", path.display())
                })?;
            map.push((selector, ids));
        }
        Ok(Probes(map))
    }

    fn ids(&self, selector: &str) -> Result<&[String], String> {
        self.0
            .iter()
            .find(|(s, _)| s == selector)
            .map(|(_, ids)| ids.as_slice())
            .ok_or_else(|| format!("{selector} has no parity ids in parity/probes.json"))
    }

    /// The painted boxes `selector` matches, in paint order.
    fn rects(&self, recorder: &Recorder, selector: &str) -> Result<Vec<Rect>, String> {
        let ids = self.ids(selector)?;
        Ok(recorder
            .boxes
            .iter()
            .filter(|(id, _)| ids.iter().any(|i| i == id.as_ref()))
            .map(|(_, r)| *r)
            .collect())
    }
}

pub struct Shot {
    pub image: Capture,
    pub recorder: Recorder,
    pub viewport: (f64, f64),
}

impl Shot {
    pub fn same_as(&self, other: &Shot) -> bool {
        self.image == other.image && self.recorder == other.recorder
    }
}

pub struct Context<'a> {
    pub scenario: &'a Scenario,
    pub scale: u32,
    pub theme: Theme,
    pub probes: &'a Probes,
}

pub fn kind(action: &Action) -> &'static str {
    match action {
        Action::Main { .. } => "main",
        Action::Open { .. } => "open",
        Action::Selection { .. } => "selection",
        Action::Wait { .. } => "wait",
        Action::Idle => "idle",
        Action::Mouse { .. } => "mouse",
        Action::Wheel(_) => "wheel",
        Action::Key { .. } => "key",
        Action::Dialog(_) => "dialog",
        Action::Menu { .. } => "menu",
        Action::AppMenu { .. } => "appMenu",
        Action::Prompt { .. } => "prompt",
        Action::Capture { .. } => "capture",
    }
}

pub fn write(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(rgba))
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn json(value: &JsValue) -> Result<String, String> {
    asrs_js::json::stringify_gap(value, " ")
        .map_err(|e| format!("{e:?}"))?
        .map(|text| text + "\n")
        .ok_or_else(|| "the record is not JSON".to_owned())
}

fn num(v: f64) -> JsValue {
    JsValue::Number(v)
}

fn rect(r: Rect) -> JsValue {
    JsValue::Array(r.iter().map(|v| num(*v)).collect())
}

fn rects(rs: &[Rect]) -> JsValue {
    JsValue::Array(rs.iter().map(|r| rect(*r)).collect())
}

/// The `process.platform` name the Electron driver records, so both runs pick the same masks.
fn platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

/// A file the run left in `userData/`, as the Electron driver's `collectFiles` lists it.
pub struct WrittenFile {
    pub file: String,
    pub bytes: usize,
    pub seeded: bool,
    pub changed: bool,
}

/// Copies what the port's main wrote to `<dir>/userData` into `<dir>/files`, with the driver's
/// filter: top-level JSON files and the asset folders. `seeds` are the files the run seeded,
/// with the bytes it seeded them with.
pub fn collect_files(dir: &Path, seeds: &[(String, Vec<u8>)]) -> Result<Vec<WrittenFile>, String> {
    fn visit(
        root: &Path,
        rel: &Path,
        out_root: &Path,
        seeds: &[(String, Vec<u8>)],
        written: &mut Vec<WrittenFile>,
    ) -> Result<(), String> {
        let full = root.join(rel);
        let mut entries: Vec<_> = std::fs::read_dir(&full)
            .map_err(|e| format!("{}: {e}", full.display()))?
            .filter_map(Result::ok)
            .collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = rel.join(&name);
            let top = rel.as_os_str().is_empty();
            let is_dir = entry.file_type().map_err(|e| e.to_string())?.is_dir();
            if top && is_dir && !["userAssets", "autoAssets"].contains(&name.as_str()) {
                continue;
            }
            if top && !is_dir && !name.ends_with(".json") {
                continue;
            }
            if is_dir {
                visit(root, &child, out_root, seeds, written)?;
                continue;
            }
            let bytes =
                std::fs::read(entry.path()).map_err(|e| format!("{}: {e}", child.display()))?;
            let dest = out_root.join(&child);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            std::fs::write(&dest, &bytes).map_err(|e| format!("{}: {e}", dest.display()))?;
            let file = child
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            let seed = seeds.iter().find(|(name, _)| *name == file);
            written.push(WrittenFile {
                changed: seed.is_none_or(|(_, seed)| *seed != bytes),
                bytes: bytes.len(),
                seeded: seed.is_some(),
                file,
            });
        }
        Ok(())
    }
    let user_data = dir.join("userData");
    let mut written = Vec::new();
    if user_data.is_dir() {
        visit(
            &user_data,
            Path::new(""),
            &dir.join("files"),
            seeds,
            &mut written,
        )?;
    }
    Ok(written)
}

/// A dialog the port showed, as the Electron driver records one. Options that Electron treats
/// like an absent one when empty are left out when empty; `icon` and the `properties` the port's
/// platform dialogs have no switch for are not recorded.
pub fn dialog_record(shown: &Shown) -> JsValue {
    let list =
        |items: &[String]| JsValue::Array(items.iter().map(|s| JsValue::string(s)).collect());
    let mut options = JsObject::new();
    let mut text = |key: &str, value: &str| {
        if !value.is_empty() {
            options.insert(key, JsValue::string(value));
        }
    };
    let kind = match shown {
        Shown::Message(o) => {
            let kind = match o.kind {
                MessageBoxType::None => "",
                MessageBoxType::Info => "info",
                MessageBoxType::Warning => "warning",
                MessageBoxType::Error => "error",
                MessageBoxType::Question => "question",
            };
            text("type", kind);
            text("title", &o.title);
            text("message", &o.message);
            text("detail", &o.detail);
            text("checkboxLabel", &o.checkbox_label);
            if !o.buttons.is_empty() {
                options.insert("buttons", list(&o.buttons));
            }
            let mut index = |key: &str, value: Option<usize>| {
                if let Some(value) = value {
                    options.insert(key, JsValue::Number(value as f64));
                }
            };
            index("defaultId", o.default_id);
            index("cancelId", o.cancel_id);
            if o.checkbox_checked {
                options.insert("checkboxChecked", JsValue::Bool(true));
            }
            "showMessageBox"
        }
        Shown::Open(o) => {
            text("title", &o.title);
            text("message", &o.message);
            text("buttonLabel", &o.button_label);
            let properties = [
                (o.open_file, "openFile"),
                (o.open_directory, "openDirectory"),
                (o.multi_selections, "multiSelections"),
                (o.create_directory, "createDirectory"),
            ];
            panel_options(
                &mut options,
                o.default_path.as_deref(),
                &o.filters,
                &properties,
            );
            "showOpenDialog"
        }
        Shown::Save(o) => {
            text("title", &o.title);
            text("message", &o.message);
            text("buttonLabel", &o.button_label);
            let properties = [(o.create_directory, "createDirectory")];
            panel_options(
                &mut options,
                o.default_path.as_deref(),
                &o.filters,
                &properties,
            );
            "showSaveDialog"
        }
    };
    asrs_js::ops::object([
        ("kind", JsValue::string(kind)),
        ("options", JsValue::Object(options)),
    ])
}

fn panel_options(
    options: &mut JsObject,
    default_path: Option<&Path>,
    filters: &[FileFilter],
    properties: &[(bool, &str)],
) {
    if let Some(path) = default_path {
        options.insert("defaultPath", JsValue::string(&path.to_string_lossy()));
    }
    if !filters.is_empty() {
        let filters = filters.iter().map(|f| {
            let extensions = f.extensions.iter().map(|e| JsValue::string(e)).collect();
            asrs_js::ops::object([
                ("name", JsValue::string(&f.name)),
                ("extensions", JsValue::Array(extensions)),
            ])
        });
        options.insert("filters", JsValue::Array(filters.collect()));
    }
    let on: Vec<JsValue> = properties
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| JsValue::string(name))
        .collect();
    if !on.is_empty() {
        options.insert("properties", JsValue::Array(on));
    }
}

pub fn run_record(
    scenario: &Scenario,
    scale: u32,
    theme: Theme,
    exceptions: &[String],
    files: &[WrittenFile],
    dialogs: &[JsValue],
) -> String {
    let record = asrs_js::ops::object([
        ("scenario", JsValue::string(&scenario.name)),
        ("scale", num(f64::from(scale))),
        ("theme", JsValue::string(theme.name())),
        (
            "window",
            asrs_js::ops::object([
                ("width", num(scenario.width)),
                ("height", num(scenario.height)),
            ]),
        ),
        ("platform", JsValue::string(platform())),
        ("arch", JsValue::string(std::env::consts::ARCH)),
        ("capture", JsValue::string("headless")),
        (
            "files",
            JsValue::Array(
                files
                    .iter()
                    .map(|f| {
                        asrs_js::ops::object([
                            ("file", JsValue::string(&f.file)),
                            ("bytes", num(f.bytes as f64)),
                            ("seeded", JsValue::Bool(f.seeded)),
                            ("changed", JsValue::Bool(f.changed)),
                        ])
                    })
                    .collect(),
            ),
        ),
        ("dialogs", JsValue::Array(dialogs.to_vec())),
        (
            "exceptions",
            JsValue::Array(exceptions.iter().map(|e| JsValue::string(e)).collect()),
        ),
    ]);
    json(&record).expect("the run record is plain JSON")
}

#[allow(clippy::too_many_arguments)]
pub fn write_capture(
    dir: &Path,
    name: &str,
    cx: Context<'_>,
    shot: &Shot,
    probes: &[String],
    masks: &[String],
    state: Option<&JsValue>,
    timeline: Option<[f64; 2]>,
) -> Result<(), String> {
    let recorder = &shot.recorder;
    let image = format!("{name}.png");
    write_png(
        &dir.join(&image),
        shot.image.width,
        shot.image.height,
        &shot.image.rgba,
    )?;

    let mut probed = JsObject::new();
    for selector in probes {
        let found = cx
            .probes
            .rects(recorder, selector)
            .map_err(|e| format!("capture {name}: {e}"))?;
        probed.insert(selector.as_str(), rects(&found));
    }
    let texts = recorder
        .texts
        .iter()
        .filter(|t| !t.text.trim().is_empty() && !t.rects.is_empty())
        .map(|t| {
            asrs_js::ops::object([
                ("text", JsValue::string(&t.text)),
                ("rects", rects(&t.rects)),
            ])
        })
        .collect();
    let mut mask_list: Vec<JsValue> = recorder
        .masks
        .iter()
        .map(|(reason, r)| {
            asrs_js::ops::object([("reason", JsValue::string(reason)), ("rect", rect(*r))])
        })
        .collect();
    for selector in masks {
        for r in cx
            .probes
            .rects(recorder, selector)
            .map_err(|e| format!("capture {name}: mask {e}"))?
        {
            mask_list.push(asrs_js::ops::object([
                ("reason", JsValue::string(selector)),
                ("rect", rect(r)),
            ]));
        }
    }

    let mut canvases = Vec::new();
    for (i, dump) in recorder.canvases.iter().enumerate() {
        let file = format!("{name}.canvas-{i}.png");
        write_png(&dir.join(&file), dump.width, dump.height, &dump.rgba)?;
        canvases.push(asrs_js::ops::object([
            ("file", JsValue::string(&file)),
            ("rect", rect(dump.rect)),
            ("width", num(f64::from(dump.width))),
            ("height", num(f64::from(dump.height))),
        ]));
    }
    let mut gl = Vec::new();
    for (i, dump) in recorder.gl.iter().enumerate() {
        let file = format!("{name}.gl-{i}.png");
        let edges = format!("{name}.gl-{i}.edges.png");
        write_png(&dir.join(&file), dump.width, dump.height, &dump.rgba)?;
        let mask: Vec<u8> = dump
            .edges
            .iter()
            .flat_map(|e| if *e { [255; 4] } else { [0, 0, 0, 255] })
            .collect();
        write_png(&dir.join(&edges), dump.width, dump.height, &mask)?;
        gl.push(asrs_js::ops::object([
            ("file", JsValue::string(&file)),
            ("edges", JsValue::string(&edges)),
            ("rect", rect(dump.rect)),
            ("width", num(f64::from(dump.width))),
            ("height", num(f64::from(dump.height))),
            ("mode", JsValue::string(dump.mode)),
        ]));
    }
    if cx.scenario.gl && gl.is_empty() {
        return Err(format!(
            "capture {name}: the scenario is a 3D scenario but no 3D view painted"
        ));
    }

    let scale = f64::from(cx.scale);
    let mut record = JsObject::new();
    record.insert("name", JsValue::string(name));
    record.insert("scale", num(scale));
    record.insert("theme", JsValue::string(cx.theme.name()));
    record.insert("width", num(f64::from(shot.image.width)));
    record.insert("height", num(f64::from(shot.image.height)));
    record.insert("image", JsValue::string(&image));
    record.insert("colourSpace", JsValue::string("srgb"));
    record.insert("devicePixelRatio", num(scale));
    record.insert(
        "viewport",
        JsValue::Array(vec![num(shot.viewport.0), num(shot.viewport.1)]),
    );
    record.insert("probes", JsValue::Object(probed));
    record.insert("texts", JsValue::Array(texts));
    record.insert("masks", JsValue::Array(mask_list));
    record.insert("canvases", JsValue::Array(canvases));
    record.insert("gl", JsValue::Array(gl));
    if let Some(state) = state {
        let file = format!("{name}.hub-state.json");
        let text = asrs_js::json::stringify(state)
            .map_err(|e| format!("capture {name}: HubState: {e:?}"))?
            .unwrap_or_default();
        write(&dir.join(&file), &text)?;
        record.insert("hubState", JsValue::string(&file));
    }
    if let Some([min, max]) = timeline {
        record.insert("timelineRange", JsValue::Array(vec![num(min), num(max)]));
    }
    if cx.scenario.page == Page::ColourPatch {
        let patches = patch_colours()
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let i = i as u32;
                asrs_js::ops::object([
                    (
                        "rgb",
                        JsValue::Array(c.iter().map(|v| num(f64::from(*v))).collect()),
                    ),
                    (
                        "rect",
                        rect([
                            f64::from((i % COLUMNS) * PATCH),
                            f64::from((i / COLUMNS) * PATCH),
                            f64::from(PATCH),
                            f64::from(PATCH),
                        ]),
                    ),
                ])
            })
            .collect();
        record.insert("patches", JsValue::Array(patches));
    }
    write(
        &dir.join(format!("{name}.json")),
        &json(&JsValue::Object(record))?,
    )
}
