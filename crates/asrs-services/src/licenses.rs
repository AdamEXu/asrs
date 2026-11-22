//! `getLicenses.mjs`: the license list (`src/licenses.json`) and `ThirdPartyLicenses.txt` built
//! from a package-lock tree, its license files, SPDX texts and the `licenses` folder.

use asrs_js::string::JsString;

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

/// Runs the script once against `host`.
pub fn generate(host: &mut dyn LicenseHost) {
    let _ = host;
    todo!("P07: getLicenses.mjs")
}
