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
}

fn node_error(syscall: &str, path: &str, e: &std::io::Error) -> JsError {
    JsError::new("Error", format!("{e} ({syscall} '{path}')"))
}

fn eacces(syscall: &str, path: &str) -> JsError {
    JsError::new(
        "Error",
        format!("EACCES: permission denied, {syscall} '{path}'"),
    )
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
}

/// An injected [`MemFs`] error, like a file without read or write permission.
#[derive(Clone, Copy, PartialEq)]
pub enum Failure {
    Read,
    Write,
}

/// An in-memory filesystem recording write calls in order, for the vector harness.
#[derive(Default)]
pub struct MemFs {
    files: RefCell<BTreeMap<String, Vec<u8>>>,
    failures: RefCell<BTreeMap<String, Failure>>,
    writes: RefCell<Vec<String>>,
}

impl MemFs {
    pub fn new() -> MemFs {
        MemFs::default()
    }

    pub fn writes(&self) -> Vec<String> {
        self.writes.borrow().clone()
    }

    pub fn take_writes(&self) -> Vec<String> {
        std::mem::take(&mut *self.writes.borrow_mut())
    }

    /// Seed a file without logging a write, like the oracle's `putFile`.
    pub fn put(&self, path: &str, data: &[u8]) {
        self.files
            .borrow_mut()
            .insert(path.to_owned(), data.to_vec());
    }

    pub fn set_failure(&self, path: &str, failure: Option<Failure>) {
        let mut failures = self.failures.borrow_mut();
        match failure {
            Some(f) => failures.insert(path.to_owned(), f),
            None => failures.remove(path),
        };
    }

    fn fails(&self, path: &str, failure: Failure) -> bool {
        self.failures.borrow().get(path) == Some(&failure)
    }

    pub fn remove(&self, path: &str) {
        self.files.borrow_mut().remove(path);
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
        if self.fails(path, Failure::Read) {
            return Err(eacces("open", path));
        }
        self.files.borrow().get(path).cloned().ok_or_else(|| {
            JsError::new(
                "Error",
                format!("ENOENT: no such file or directory, open '{path}'"),
            )
        })
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), JsError> {
        if self.fails(path, Failure::Write) {
            return Err(eacces("open", path));
        }
        self.files
            .borrow_mut()
            .insert(path.to_owned(), data.to_vec());
        self.writes.borrow_mut().push(path.to_owned());
        Ok(())
    }

    fn copy(&self, from: &str, to: &str) -> Result<(), JsError> {
        if self.fails(from, Failure::Read) || self.fails(to, Failure::Write) {
            return Err(eacces("copyfile", from));
        }
        let data = self.read(from)?;
        self.write(to, &data)
    }
}
