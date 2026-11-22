//! `src/main/electron/assetLoader.ts`: the user, automatic and bundled asset folders, the
//! README written into the default user folder, and `loadAssets`, which main runs at startup,
//! every [`ASSET_RELOAD_PERIOD_MS`], and after the downloader or the custom-folder menu item
//! changes something, sending the result to every window as `set-assets`.

use asrs_js::sort::{array_sort, code_unit_cmp};
use asrs_js::string::{JsError, JsString, decode_uri_component, encode_uri_component};
use asrs_js::value::JsValue;
use asrs_model::assets::{
    AdvantageScopeAssets, Parsed, TypeError, filter_and_sort_assets, parse_field_2d,
    parse_field_3d, parse_joystick, parse_robot,
};
use asrs_model::json_file;

use crate::fs::Fs;
use crate::path::PathStyle;

/// The `setInterval` period of main's periodic `loadAssets` and `sendAssets`.
pub const ASSET_RELOAD_PERIOD_MS: f64 = 5000.0;

pub const USER_ASSETS_README: &str = "This folder contains extra assets for the 2D field, 3D field, and joystick views. For more details, see the \"Custom Fields/Robots/Joysticks\" page in the AdvantageScope documentation (available through the documentation tab in the app or the URL below).\n\nhttps://docs.advantagescope.org/more-features/custom-assets";

/// `ElectronConstants`' asset folders and the preferences file they depend on.
#[derive(Clone, Debug, PartialEq)]
pub struct AssetFolders {
    pub style: PathStyle,
    pub prefs_filename: JsString,
    pub auto: JsString,
    pub default_user: JsString,
    pub bundled: JsString,
}

impl AssetFolders {
    pub fn new(style: PathStyle, user_data: &str, bundled: &str) -> AssetFolders {
        let user_data = JsString::from(user_data);
        let join = |name: &str| style.join(&[&user_data, &JsString::from(name)]);
        AssetFolders {
            style,
            prefs_filename: join("prefs.json"),
            auto: join("autoAssets"),
            default_user: join("userAssets"),
            bundled: JsString::from(bundled),
        }
    }
}

/// `fs` takes a JS string path as UTF-8, with lone surrogates replaced.
fn os(path: &JsString) -> String {
    path.to_string_lossy()
}

fn type_error(e: TypeError) -> JsError {
    JsError::new("TypeError", e.message.to_string_lossy())
}

/// `getUserAssetsPath()`: the default folder when `prefs.userAssetsFolder` is `null`, otherwise
/// whatever value the preferences hold there (`undefined` when absent).
pub fn get_user_assets_path(fs: &dyn Fs, folders: &AssetFolders) -> Result<JsValue, JsError> {
    let file = os(&folders.prefs_filename);
    let prefs = fs
        .read(&file)
        .and_then(|bytes| json_file::parse(&bytes))
        .map_err(|e| JsError::new(e.name, format!("{file}: {}", e.message)))?;
    let folder = match &prefs {
        JsValue::Null => {
            return Err(JsError::type_error(
                "Cannot read properties of null (reading 'userAssetsFolder')",
            ));
        }
        JsValue::Object(o) => o.get("userAssetsFolder").cloned(),
        _ => None,
    };
    Ok(match folder {
        Some(JsValue::Null) => JsValue::String(folders.default_user.clone()),
        Some(value) => value,
        None => JsValue::Undefined,
    })
}

/// `createAssetFolders()`.
pub fn create_asset_folders(fs: &dyn Fs, folders: &AssetFolders) -> Result<(), JsError> {
    for folder in [&folders.auto, &folders.default_user] {
        if !fs.exists(&os(folder)) {
            fs.mkdir(&os(folder))?;
        }
    }
    let readme = folders
        .style
        .join(&[&folders.default_user, &JsString::from("README.txt")]);
    fs.write(&os(&readme), USER_ASSETS_README.as_bytes())
}

/// `encodePath`: every component through `encodeURIComponent` except a first one ending in
/// `:`, which is kept as is whatever it contains.
fn encode_path(style: PathStyle, path: &JsString) -> Result<JsString, JsError> {
    let sep = JsString::from_units(vec![style.sep()]);
    let mut out = JsString::new();
    for (index, component) in path.split(Some(&sep), None).into_iter().enumerate() {
        if index > 0 {
            out.push_str(&sep);
        }
        if index == 0 && component.units().last() == Some(&u16::from(b':')) {
            out.push_str(&component);
        } else {
            out.push_str(&encode_uri_component(&component).map_err(|e| e.into_error())?);
        }
    }
    Ok(out)
}

fn decode(path: &JsString) -> Result<JsString, JsError> {
    decode_uri_component(path).map_err(|e| e.into_error())
}

/// `decodeURIComponent(config.path).slice(0, -4) + "_" + index + ".glb"` exists for every part.
fn parts_exist(fs: &dyn Fs, path: &JsString, count: usize) -> Result<bool, JsError> {
    for index in 0..count {
        let decoded = decode(path)?;
        let units = decoded.units();
        let mut part = JsString::from_units(units[..units.len().saturating_sub(4)].to_vec());
        part.push_str(&JsString::from(format!("_{index}.glb")));
        if !fs.exists(&os(&part)) {
            return Ok(false);
        }
    }
    Ok(true)
}

enum Kind {
    Field2d,
    Field3d,
    Robot,
    Joystick,
    Unknown,
}

fn kind_of(name: &JsString) -> Kind {
    let starts = |prefix: &str| name.starts_with(&JsString::from(prefix), None);
    if starts("Field2d_") {
        Kind::Field2d
    } else if starts("Field3d_") {
        Kind::Field3d
    } else if starts("Robot_") {
        Kind::Robot
    } else if starts("Joystick_") {
        Kind::Joystick
    } else {
        Kind::Unknown
    }
}

struct Loader<'a> {
    fs: &'a dyn Fs,
    style: PathStyle,
    assets: AdvantageScopeAssets,
}

impl Loader<'_> {
    fn succeeded(&mut self, name: &JsString) {
        if let Some(index) = self.assets.load_failures.iter().position(|n| n == name) {
            self.assets.load_failures.remove(index);
        }
    }

    fn asset_path(
        &self,
        parent: &JsString,
        name: &JsString,
        file: &str,
    ) -> Result<JsString, JsError> {
        let joined = self.style.join(&[parent, name, &JsString::from(file)]);
        encode_path(self.style, &joined)
    }

    fn exists_decoded(&self, path: &JsString) -> Result<bool, JsError> {
        Ok(self.fs.exists(&os(&decode(path)?)))
    }

    fn load_folder(&mut self, parent: &JsString) -> Result<(), JsError> {
        if !self.fs.exists(&os(parent)) {
            return Ok(());
        }
        let mut entries: Vec<(JsString, bool)> = self
            .fs
            .read_dir(&os(parent))?
            .into_iter()
            .map(|e| (JsString::from(e.name), e.is_dir))
            .collect();
        array_sort(&mut entries, |a, b| {
            code_unit_cmp(a.0.units(), b.0.units()).reverse()
        });
        for (name, is_dir) in entries {
            if !is_dir || name.starts_with(&JsString::from("."), None) {
                continue;
            }
            self.load_entry(parent, &name)?;
        }
        Ok(())
    }

    fn load_entry(&mut self, parent: &JsString, name: &JsString) -> Result<(), JsError> {
        self.assets.load_failures.push(name.clone());
        let kind = kind_of(name);
        let config_path = self
            .style
            .join(&[parent, name, &JsString::from("config.json")]);
        if !self.fs.exists(&os(&config_path)) {
            return Ok(());
        }
        let Ok(config_raw) = self
            .fs
            .read(&os(&config_path))
            .and_then(|bytes| json_file::parse(&bytes))
        else {
            return Ok(());
        };
        match kind {
            Kind::Field2d => match parse_field_2d(&config_raw).map_err(type_error)? {
                Parsed::Skip => self.succeeded(name),
                Parsed::Invalid => {}
                Parsed::Config(mut config) => {
                    config.path = self.asset_path(parent, name, "image.png")?;
                    if self.exists_decoded(&config.path)? {
                        self.assets.field2ds.push(config);
                        self.succeeded(name);
                    }
                }
            },
            Kind::Field3d => match parse_field_3d(&config_raw).map_err(type_error)? {
                Parsed::Skip => self.succeeded(name),
                Parsed::Invalid => {}
                Parsed::Config(mut config) => {
                    config.path = self.asset_path(parent, name, "model.glb")?;
                    if self.exists_decoded(&config.path)?
                        && parts_exist(self.fs, &config.path, config.game_pieces.len())?
                    {
                        self.assets.field3ds.push(config);
                        self.succeeded(name);
                    }
                }
            },
            Kind::Robot => {
                if let Parsed::Config(mut config) = parse_robot(&config_raw).map_err(type_error)? {
                    config.path = self.asset_path(parent, name, "model.glb")?;
                    if self.exists_decoded(&config.path)?
                        && parts_exist(self.fs, &config.path, config.components.len())?
                    {
                        self.assets.robots.push(config);
                        self.succeeded(name);
                    }
                }
            }
            Kind::Joystick => {
                if let Parsed::Config(mut config) =
                    parse_joystick(&config_raw).map_err(type_error)?
                {
                    config.path = self.asset_path(parent, name, "image.png")?;
                    if self.exists_decoded(&config.path)? {
                        self.assets.joysticks.push(config);
                        self.succeeded(name);
                    }
                }
            }
            Kind::Unknown => {}
        }
        Ok(())
    }
}

/// `loadAssets()`: user, automatic, then bundled folders (highest priority first), each folder's
/// entries in descending name order so newer versions win, then `filterAndSortAssets`.
pub fn load_assets(fs: &dyn Fs, folders: &AssetFolders) -> Result<AdvantageScopeAssets, JsError> {
    let user = get_user_assets_path(fs, folders)?;
    let mut loader = Loader {
        fs,
        style: folders.style,
        assets: AdvantageScopeAssets::default(),
    };
    if let JsValue::String(user) = &user {
        loader.load_folder(user)?;
    }
    loader.load_folder(&folders.auto)?;
    loader.load_folder(&folders.bundled)?;
    Ok(filter_and_sort_assets(loader.assets))
}

/// main's `advantageScopeAssets`: what every `set-assets` carries.
pub struct AssetService {
    pub folders: AssetFolders,
    assets: AdvantageScopeAssets,
}

impl AssetService {
    /// Empty lists until the first load, as main declares it.
    pub fn new(folders: AssetFolders) -> AssetService {
        AssetService {
            folders,
            assets: AdvantageScopeAssets::default(),
        }
    }

    pub fn assets(&self) -> &AdvantageScopeAssets {
        &self.assets
    }

    /// The `set-assets` payload.
    pub fn set_assets_data(&self) -> JsValue {
        self.assets.to_js()
    }

    /// The `ready` handler's `createAssetFolders()` then `loadAssets()`; a throw from either
    /// ends main's startup there.
    pub fn start(&mut self, fs: &dyn Fs) -> Result<(), JsError> {
        create_asset_folders(fs, &self.folders)?;
        self.reload(fs)
    }

    /// `advantageScopeAssets = await loadAssets()` from the reload interval, the downloader or
    /// the custom-folder menu item. A throw leaves the previous assets in place and the caller
    /// sends nothing.
    pub fn reload(&mut self, fs: &dyn Fs) -> Result<(), JsError> {
        self.assets = load_assets(fs, &self.folders)?;
        Ok(())
    }
}
