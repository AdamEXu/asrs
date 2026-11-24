//! The Licenses window's list. In a `parity` build it is upstream's own `src/licenses.json`, which
//! `cargo xtask oracle setup` generates. Otherwise it is asrs's, which `cargo xtask licenses`
//! writes in the same shape: asrs, the crates compiled into it and the notices of the projects
//! it is ported from. A build without that file lists the notices alone.

use std::path::{Path, PathBuf};

use asrs_js::string::JsString;
use asrs_services::licenses;
use asrs_tabs::info::License;
use asrs_ui::gpui::{App, Global};

use crate::main_process::{PACKAGED, resources_dir};

/// The list to show in place of the build's own.
pub struct LicensesFile(pub PathBuf);

impl Global for LicensesFile {}

type Entry = (JsString, JsString);

pub fn load(cx: &App) -> Result<Vec<License>, String> {
    let entries = match cx.try_global::<LicensesFile>() {
        Some(LicensesFile(file)) => read_list(file)?,
        None => build_list()?,
    };
    let license = |(module, text): &Entry| License::new(module, text);
    Ok(entries.iter().map(license).collect())
}

/// How many entries the list of this build has.
pub(crate) fn packaged_count() -> Result<usize, String> {
    build_list().map(|entries| entries.len())
}

fn read(path: &Path) -> Result<JsString, String> {
    std::fs::read_to_string(path)
        .map(JsString::from)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn read_list(path: &Path) -> Result<Vec<Entry>, String> {
    licenses::parse(&read(path)?)
        .map_err(|e| format!("{}: {}: {}", path.display(), e.name, e.message))
}

fn build_list() -> Result<Vec<Entry>, String> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if cfg!(feature = "parity") {
        return read_list(&workspace.join("target/oracle/upstream/src/licenses.json"));
    }
    let mut entries = if PACKAGED {
        read_list(&resources_dir().join("licenses.json"))?
    } else {
        match read_list(&workspace.join("target/licenses/licenses.json")) {
            Ok(entries) => entries,
            Err(_) => notices(&workspace)?,
        }
    };
    licenses::sort(&mut entries, &JsString::from(crate::IDENTITY.name));
    Ok(entries)
}

fn notices(workspace: &Path) -> Result<Vec<Entry>, String> {
    let mut entries = vec![(
        JsString::from(crate::IDENTITY.name),
        read(&workspace.join("LICENSE"))?,
    )];
    let dir = workspace.join("licenses");
    let files = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for file in files.filter_map(Result::ok) {
        let name = file.path().file_stem().unwrap_or_default().to_owned();
        entries.push((
            JsString::from(name.to_string_lossy().as_ref()),
            read(&file.path())?,
        ));
    }
    Ok(entries)
}
