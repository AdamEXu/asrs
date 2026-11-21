//! The filesystem surface main's persistence code uses, behind a trait so tests can replay the
//! same calls against an in-memory store. Errors carry the `Error` name node's fs errors have.

use std::cell::RefCell;
use std::collections::BTreeMap;

use asrs_js::string::JsError;

pub trait Fs {
    /// `fs.existsSync`.
    fn exists(&self, path: &str) -> bool;

    /// `fs.readFileSync`.
    fn read(&self, path: &str) -> Result<Vec<u8>, JsError>;

    /// `fs.writeFileSync`.
    fn write(&self, path: &str, data: &[u8]) -> Result<(), JsError>;

    /// `fs.copyFileSync`.
    fn copy(&self, from: &str, to: &str) -> Result<(), JsError>;

    /// `fs.rmSync(path)`, used by test environments only.
    fn remove(&self, _path: &str) -> Result<(), JsError> {
        unimplemented!("remove is only used by test filesystems")
    }
}

fn node_error(syscall: &str, path: &str, e: &std::io::Error) -> JsError {
    JsError::new("Error", format!("{e} ({syscall} '{path}')"))
}

/// `fs` against the real filesystem.
pub struct RealFs;

impl Fs for RealFs {
    fn exists(&self, path: &str) -> bool {
        std::fs::exists(path).unwrap_or(false)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, JsError> {
        std::fs::read(path).map_err(|e| node_error("open", path, &e))
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), JsError> {
        std::fs::write(path, data).map_err(|e| node_error("open", path, &e))
    }

    fn copy(&self, from: &str, to: &str) -> Result<(), JsError> {
        std::fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| node_error("copyfile", from, &e))
    }

    fn remove(&self, path: &str) -> Result<(), JsError> {
        std::fs::remove_file(path).map_err(|e| node_error("rm", path, &e))
    }
}

/// An in-memory filesystem recording write calls in order, for the vector harness.
#[derive(Default)]
pub struct MemFs {
    files: RefCell<BTreeMap<String, Vec<u8>>>,
    writes: RefCell<Vec<String>>,
}

impl MemFs {
    pub fn new() -> MemFs {
        MemFs::default()
    }

    pub fn writes(&self) -> Vec<String> {
        self.writes.borrow().clone()
    }

    /// `(name, bytes)` pairs in name order, like the oracle's dump.
    pub fn dump(&self) -> Vec<(String, Vec<u8>)> {
        self.files
            .borrow()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

impl Fs for MemFs {
    fn exists(&self, path: &str) -> bool {
        self.files.borrow().contains_key(path)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, JsError> {
        self.files.borrow().get(path).cloned().ok_or_else(|| {
            JsError::new(
                "Error",
                format!("ENOENT: no such file or directory, open '{path}'"),
            )
        })
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), JsError> {
        self.files
            .borrow_mut()
            .insert(path.to_owned(), data.to_vec());
        self.writes.borrow_mut().push(path.to_owned());
        Ok(())
    }

    fn copy(&self, from: &str, to: &str) -> Result<(), JsError> {
        let data = self.read(from)?;
        self.write(to, &data)
    }

    fn remove(&self, path: &str) -> Result<(), JsError> {
        self.files.borrow_mut().remove(path);
        Ok(())
    }
}
