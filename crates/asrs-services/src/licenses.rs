//! `getLicenses.mjs`: the license list (`src/licenses.json`) and `ThirdPartyLicenses.txt` built
//! from a package-lock tree, its license files, SPDX texts and the `licenses` folder.

use asrs_js::collation;
use asrs_js::json;
use asrs_js::ops;
use asrs_js::sort::{array_sort, default_compare};
use asrs_js::string::{JsError, JsString, TextDecoder, encode_uri_component, response_text};
use asrs_js::value::JsValue;

/// How one SPDX `fetch` settled.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpdxReply {
    Rejected,
    Response { status: u16, body: Vec<u8> },
}

/// The script's working directory and the outside world. Paths are relative to the working
/// directory, joined with `path.posix.join` exactly as upstream joins them; the root is `"."`.
pub trait LicenseHost {
    /// `fs.existsSync(path)`.
    fn exists(&mut self, path: &str) -> bool;
    /// `fs.readdirSync(path)`: entry names in the order the directory lists them.
    fn read_dir(&mut self, path: &str) -> Vec<String>;
    /// `fs.statSync(path).isDirectory()`.
    fn is_dir(&mut self, path: &str) -> bool;
    /// `fs.readFileSync(path)`.
    fn read(&mut self, path: &str) -> Vec<u8>;
    /// `fetch(url)`.
    fn fetch(&mut self, url: &str) -> SpdxReply;
    /// `console.error(text)`.
    fn error(&mut self, text: &str);
    /// `console.log(text)`.
    fn log(&mut self, text: &str);
    /// `fs.writeFileSync(path, text)`.
    fn write(&mut self, path: &str, text: &JsString);
}

fn decode(bytes: &[u8]) -> JsString {
    TextDecoder::new(None, false, true)
        .expect("utf-8 is supported")
        .decode(bytes)
        .expect("lossy decoding cannot fail")
}

/// `path.posix.join(a, b)` for the two-segment joins the script makes.
fn join(a: &str, b: &str) -> String {
    if a.is_empty() || a == "." {
        b.to_owned()
    } else {
        format!("{a}/{b}")
    }
}

fn thrown(error: json::JsonError) -> JsError {
    JsError::new(error.name, error.message.to_string_lossy())
}

/// The order the list is saved in: `first`, then by name.
pub fn sort<T>(licenses: &mut Vec<(JsString, T)>, first: &JsString) {
    array_sort(licenses, |(a, _), (b, _)| {
        if a == first {
            return -1.0;
        }
        if b == first {
            return 1.0;
        }
        match collation::locale_compare(a, b) {
            std::cmp::Ordering::Less => -1.0,
            std::cmp::Ordering::Equal => 0.0,
            std::cmp::Ordering::Greater => 1.0,
        }
    });
}

/// The entries of a licenses.json.
pub fn parse(text: &JsString) -> Result<Vec<(JsString, JsString)>, JsError> {
    let list = json::parse(text).map_err(thrown)?;
    let entries = list
        .as_array()
        .ok_or_else(|| JsError::type_error("not a list"))?;
    entries
        .iter()
        .map(|entry| {
            let field = |key: &str| match &ops::get(entry, key)? {
                JsValue::String(text) => Ok(text.clone()),
                _ => Err(JsError::type_error(format!("{key} is not a string"))),
            };
            Ok((field("module")?, field("text")?))
        })
        .collect()
}

/// Runs the script once against `host`. `Err` is the exception that ends upstream's script (an
/// unparseable or missing package-lock, a missing `packages` object, a `null` package.json), at
/// the point it is thrown; everything logged or written before it stays.
pub fn generate(host: &mut dyn LicenseHost) -> Result<(), JsError> {
    if !host.exists("package-lock.json") {
        return Err(JsError::new(
            "Error",
            "ENOENT: no such file or directory, open 'package-lock.json'",
        ));
    }
    let package_lock = json::parse(&decode(&host.read("package-lock.json"))).map_err(thrown)?;
    let packages = ops::get(&package_lock, "packages")?;
    let module_paths = ops::object_keys(&packages)?;

    let advantage_scope = JsString::from("AdvantageScope");
    let mut licenses: Vec<(JsString, JsValue)> = Vec::new();
    for module_path in module_paths {
        let is_root = module_path.is_empty();
        let module_name = if is_root {
            advantage_scope.clone()
        } else {
            module_path.replace_all(&JsString::from("node_modules/"), &JsString::new())?
        };
        let path = module_path.to_string_lossy();
        let dir = if is_root { "." } else { path.as_str() };
        if !is_root && !host.exists(&path) {
            continue;
        }

        let license_files: Vec<String> = host
            .read_dir(dir)
            .into_iter()
            .filter(|filename| {
                let name = JsString::from(filename.as_str());
                name.to_lower_case()
                    .starts_with(&JsString::from("license"), None)
                    && !name.ends_with(&JsString::from(".js"), None)
                    && !name.ends_with(&JsString::from(".json"), None)
                    && !name.includes(&JsString::from("header"), None)
                    && !host.is_dir(&join(dir, filename))
            })
            .collect();

        let mut license_text = JsValue::Null;
        if !license_files.is_empty() {
            let mut text = JsString::new();
            for (index, filename) in license_files.iter().enumerate() {
                if index > 0 {
                    text.push_str(&JsString::from("\n"));
                }
                text.push_str(&decode(&host.read(&join(&path, filename))));
            }
            license_text = JsValue::String(text);
        } else if host.exists(&join(&path, "package.json")) {
            let package =
                json::parse(&decode(&host.read(&join(&path, "package.json")))).map_err(thrown)?;
            let mut spdx_id = ops::get(&package, "license")?;
            if let JsValue::String(id) = &spdx_id {
                let mut id = id
                    .replace_all(&JsString::from("("), &JsString::new())?
                    .replace_all(&JsString::from(")"), &JsString::new())?
                    .trim();
                if id.includes(&JsString::from(" OR "), None) {
                    id = id.split(Some(&JsString::from(" OR ")), None)[0].trim();
                }
                spdx_id = JsValue::String(id);
            }
            let encoded = encode_uri_component(&ops::to_string(&spdx_id));
            let reply = match encoded {
                Ok(encoded) => Some(host.fetch(&format!(
                    "https://raw.githubusercontent.com/spdx/license-list-data/main/json/details/{}.json",
                    encoded.to_string_lossy()
                ))),
                Err(_) => None,
            };
            let Some(SpdxReply::Response { status, body }) = reply else {
                host.error(&format!(
                    "Failed to get license for \"{}\"",
                    module_name.to_string_lossy()
                ));
                continue;
            };
            if !(200..300).contains(&status) {
                host.error(&format!(
                    "Failed to get license for \"{}\"",
                    module_name.to_string_lossy()
                ));
                continue;
            }
            let spdx_license = json::parse(&response_text(&body))
                .ok()
                .and_then(|value| ops::get(&value, "licenseText").ok());
            match spdx_license {
                Some(text) => license_text = text,
                None => {
                    host.error(&format!(
                        "Failed to get license for \"{}\"",
                        module_name.to_string_lossy()
                    ));
                    continue;
                }
            }
        }
        if !ops::strict_equals(&license_text, &JsValue::Null) {
            licenses.push((module_name, license_text));
        }
    }

    if host.exists("licenses") {
        let mut names: Vec<String> = host
            .read_dir("licenses")
            .into_iter()
            .filter(|name| JsString::from(name.as_str()).ends_with(&JsString::from(".txt"), None))
            .collect();
        array_sort(&mut names, |a, b| {
            default_compare(&JsString::from(a.as_str()), &JsString::from(b.as_str()))
        });
        for name in names {
            let module_name = JsString::from(name.as_str()).slice(0.0, Some(-4.0));
            licenses.push((
                module_name,
                JsValue::String(decode(&host.read(&join("licenses", &name)))),
            ));
        }
    }

    sort(&mut licenses, &advantage_scope);

    let array: Vec<JsValue> = licenses
        .iter()
        .map(|(module, text)| {
            ops::object([
                ("module", JsValue::String(module.clone())),
                ("text", text.clone()),
            ])
        })
        .collect();
    let Some(text) = json::stringify(&JsValue::Array(array)).map_err(thrown)? else {
        unreachable!("an array always stringifies")
    };
    host.write("src/licenses.json", &JsString::from(text));

    let mut full_text = JsString::new();
    for (index, (module, text)) in licenses.iter().enumerate() {
        if index == 0 {
            continue;
        }
        if index > 1 {
            full_text.push_str(&JsString::from("\n"));
        }
        full_text.push_str(&JsString::from("---------- "));
        full_text.push_str(module);
        full_text.push_str(&JsString::from(" ----------\n\n"));
        full_text.push_str(&ops::to_string(text));
    }
    host.write("ThirdPartyLicenses.txt", &full_text);

    host.log(&format!("Saved {} licenses", licenses.len()));
    Ok(())
}
