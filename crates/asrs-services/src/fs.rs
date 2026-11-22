//! The filesystem surface main's persistence code uses, behind a trait so tests can replay the
//! same calls against an in-memory store. Errors carry the `Error` name node's fs errors have.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{ErrorKind, Read};

use asrs_js::string::{JsError, JsString};

use crate::path::PathStyle;

/// Where `fs.open` followed by `fs.readFile(fd)` failed.
#[derive(Debug)]
pub enum ReadError {
    Open(JsError),
    Read(JsError),
}

/// A `fs.Dirent` from `readdirSync(dir, { withFileTypes: true })`: `is_dir` is
/// `isDirectory()`, which is false for a symlink to a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub is_dir: bool,
}

pub trait Fs {
    /// `fs.existsSync`.
    fn exists(&self, path: &str) -> bool;

    /// `fs.readFileSync`.
    fn read(&self, path: &str) -> Result<Vec<u8>, JsError>;

    /// `fs.writeFileSync`.
    fn write(&self, path: &str, data: &[u8]) -> Result<(), JsError>;

    /// `fs.copyFileSync`.
    fn copy(&self, from: &str, to: &str) -> Result<(), JsError>;

    /// `fs.open(path, "r")`, then `fs.readFile` on the descriptor.
    fn read_file(&self, path: &str) -> Result<Vec<u8>, ReadError>;

    /// `fs.rmSync(path, { force: true })`.
    fn remove(&self, path: &str);

    /// `fs.readdirSync(path, { withFileTypes: true })`, in no particular order.
    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, JsError>;

    /// `fs.mkdirSync(path)`.
    fn mkdir(&self, path: &str) -> Result<(), JsError>;

    /// `fs.mkdirSync(path, { recursive: true })`: every missing ancestor too.
    fn mkdir_all(&self, path: &str) -> Result<(), JsError> {
        Err(enosys("mkdir", path))
    }

    /// `fs.renameSync(from, to)` for a file or a whole directory tree.
    fn rename(&self, from: &str, to: &str) -> Result<(), JsError> {
        let _ = to;
        Err(enosys("rename", from))
    }

    /// `fs.rmSync(path, { recursive: true })`: a file or a whole directory tree; a missing path
    /// throws.
    fn remove_all(&self, path: &str) -> Result<(), JsError> {
        Err(enosys("rm", path))
    }
}

fn enosys(syscall: &str, path: &str) -> JsError {
    JsError::new(
        "Error",
        format!("ENOSYS: function not implemented, {syscall} '{path}'"),
    )
}

/// libuv's code and description for the errors file opens and writes commonly hit.
fn uv_error(e: &std::io::Error) -> Option<(&'static str, &'static str)> {
    Some(match e.kind() {
        ErrorKind::NotFound => ("ENOENT", "no such file or directory"),
        ErrorKind::PermissionDenied if cfg!(windows) => ("EPERM", "operation not permitted"),
        ErrorKind::PermissionDenied => ("EACCES", "permission denied"),
        ErrorKind::AlreadyExists => ("EEXIST", "file already exists"),
        ErrorKind::IsADirectory => ("EISDIR", "illegal operation on a directory"),
        ErrorKind::NotADirectory => ("ENOTDIR", "not a directory"),
        ErrorKind::StorageFull => ("ENOSPC", "no space left on device"),
        ErrorKind::ReadOnlyFilesystem => ("EROFS", "read-only file system"),
        ErrorKind::InvalidFilename => ("ENAMETOOLONG", "name too long"),
        ErrorKind::ResourceBusy => ("EBUSY", "resource busy or locked"),
        _ => return None,
    })
}

/// The message node gives an fs error: `CODE: description, syscall 'path'`.
fn node_error(syscall: &str, path: &str, e: &std::io::Error) -> JsError {
    match uv_error(e) {
        Some((code, description)) => JsError::new(
            "Error",
            format!("{code}: {description}, {syscall} '{path}'"),
        ),
        None => JsError::new("Error", format!("{e} ({syscall} '{path}')")),
    }
}

fn enoent(syscall: &str, path: &str) -> JsError {
    JsError::new(
        "Error",
        format!("ENOENT: no such file or directory, {syscall} '{path}'"),
    )
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

    fn read_file(&self, path: &str) -> Result<Vec<u8>, ReadError> {
        let mut file =
            std::fs::File::open(path).map_err(|e| ReadError::Open(node_error("open", path, &e)))?;
        let mut data = Vec::new();
        file.read_to_end(&mut data)
            .map_err(|e| ReadError::Read(node_error("read", path, &e)))?;
        Ok(data)
    }

    fn remove(&self, path: &str) {
        let _ = std::fs::remove_file(path);
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, JsError> {
        let err = |e: std::io::Error| node_error("scandir", path, &e);
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(path).map_err(err)? {
            let entry = entry.map_err(err)?;
            entries.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: entry.file_type().map_err(err)?.is_dir(),
            });
        }
        Ok(entries)
    }

    fn mkdir(&self, path: &str) -> Result<(), JsError> {
        std::fs::create_dir(path).map_err(|e| node_error("mkdir", path, &e))
    }

    fn mkdir_all(&self, path: &str) -> Result<(), JsError> {
        std::fs::create_dir_all(path).map_err(|e| node_error("mkdir", path, &e))
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), JsError> {
        std::fs::rename(from, to)
            .map_err(|e| node_error("rename", &format!("{from}' -> '{to}"), &e))
    }

    fn remove_all(&self, path: &str) -> Result<(), JsError> {
        match std::fs::symlink_metadata(path) {
            Err(e) => Err(node_error("lstat", path, &e)),
            Ok(meta) if meta.is_dir() => {
                std::fs::remove_dir_all(path).map_err(|e| node_error("rm", path, &e))
            }
            Ok(_) => std::fs::remove_file(path).map_err(|e| node_error("rm", path, &e)),
        }
    }
}

/// An injected [`MemFs`] error, like a file without read or write permission.
#[derive(Clone, Copy, PartialEq)]
pub enum Failure {
    Read,
    Write,
    /// Opens, but reading the descriptor fails, like a directory.
    ReadOpened,
}

/// An in-memory filesystem recording write calls in order, for the vector harness. Like the
/// oracle's fake, directories exist only when put or made and are keyed by their normalized path.
#[derive(Default)]
pub struct MemFs {
    files: RefCell<BTreeMap<String, Vec<u8>>>,
    dirs: RefCell<BTreeSet<String>>,
    failures: RefCell<BTreeMap<String, Failure>>,
    missing_dirs: RefCell<BTreeSet<String>>,
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

    /// Seed a directory, like the oracle's `putDir`.
    pub fn put_dir(&self, path: &str) {
        self.dirs.borrow_mut().insert(dir_key(path));
    }

    pub fn dirs(&self) -> Vec<String> {
        self.dirs.borrow().iter().cloned().collect()
    }

    fn is_dir(&self, path: &str) -> bool {
        self.dirs.borrow().contains(&dir_key(path))
    }

    pub fn set_failure(&self, path: &str, failure: Option<Failure>) {
        let mut failures = self.failures.borrow_mut();
        match failure {
            Some(f) => failures.insert(path.to_owned(), f),
            None => failures.remove(path),
        };
    }

    /// Writes to a file directly inside `dir` fail with ENOENT, as if it did not exist.
    pub fn set_missing_dir(&self, dir: &str, missing: bool) {
        let mut dirs = self.missing_dirs.borrow_mut();
        if missing {
            dirs.insert(dir.to_owned());
        } else {
            dirs.remove(dir);
        }
    }

    fn in_missing_dir(&self, path: &str) -> bool {
        let parent = path.rfind('/').map_or("", |i| &path[..i]);
        self.missing_dirs.borrow().contains(parent)
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

fn dir_key(path: &str) -> String {
    let normalized = PathStyle::Posix
        .normalize(&JsString::from(path))
        .to_string_lossy();
    let mut key = normalized.as_str();
    while key.len() > 1 && key.ends_with('/') {
        key = &key[..key.len() - 1];
    }
    key.to_owned()
}

/// `path.posix.dirname` of a normalized key.
fn parent_key(key: &str) -> &str {
    match key.rfind('/') {
        Some(0) => "/",
        Some(i) => &key[..i],
        None => ".",
    }
}

fn base_name(key: &str) -> &str {
    key.rfind('/').map_or(key, |i| &key[i + 1..])
}

impl Fs for MemFs {
    fn exists(&self, path: &str) -> bool {
        self.files.borrow().contains_key(path) || self.is_dir(path)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, JsError> {
        if self.fails(path, Failure::Read) {
            return Err(eacces("open", path));
        }
        self.files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| enoent("open", path))
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), JsError> {
        if self.fails(path, Failure::Write) {
            return Err(eacces("open", path));
        }
        if self.in_missing_dir(path) {
            return Err(enoent("open", path));
        }
        if self.is_dir(path) || path.ends_with('/') {
            return Err(JsError::new(
                "Error",
                format!("EISDIR: illegal operation on a directory, open '{path}'"),
            ));
        }
        let mut ancestor = path;
        while let Some(i) = ancestor.rfind('/') {
            ancestor = &ancestor[..i];
            if ancestor.is_empty() {
                break;
            }
            if self.files.borrow().contains_key(ancestor) {
                return Err(JsError::new(
                    "Error",
                    format!("ENOTDIR: not a directory, open '{path}'"),
                ));
            }
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

    fn read_file(&self, path: &str) -> Result<Vec<u8>, ReadError> {
        let data = self.read(path).map_err(ReadError::Open)?;
        if self.fails(path, Failure::ReadOpened) {
            return Err(ReadError::Read(JsError::new(
                "Error",
                "EISDIR: illegal operation on a directory, read",
            )));
        }
        Ok(data)
    }

    fn remove(&self, path: &str) {
        self.files.borrow_mut().remove(path);
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, JsError> {
        let key = dir_key(path);
        if !self.dirs.borrow().contains(&key) {
            if self.files.borrow().contains_key(path) {
                return Err(JsError::new(
                    "Error",
                    format!("ENOTDIR: not a directory, scandir '{path}'"),
                ));
            }
            return Err(enoent("scandir", path));
        }
        let mut children = BTreeMap::new();
        let mut visit = |entry: &str, is_dir: bool| {
            if entry != key && parent_key(entry) == key {
                children.insert(base_name(entry).to_owned(), is_dir);
            }
        };
        for entry in self.files.borrow().keys() {
            visit(entry, false);
        }
        for entry in self.dirs.borrow().iter() {
            visit(entry, true);
        }
        Ok(children
            .into_iter()
            .map(|(name, is_dir)| DirEntry { name, is_dir })
            .collect())
    }

    fn mkdir(&self, path: &str) -> Result<(), JsError> {
        let key = dir_key(path);
        if self.files.borrow().contains_key(path) || self.dirs.borrow().contains(&key) {
            return Err(JsError::new(
                "Error",
                format!("EEXIST: file already exists, mkdir '{path}'"),
            ));
        }
        let parent = parent_key(&key);
        if parent != "/" && parent != "." && !self.dirs.borrow().contains(parent) {
            return Err(enoent("mkdir", path));
        }
        self.dirs.borrow_mut().insert(key);
        Ok(())
    }

    fn mkdir_all(&self, path: &str) -> Result<(), JsError> {
        let key = dir_key(path);
        if self.dirs.borrow().contains(&key) {
            return Ok(());
        }
        if self.files.borrow().contains_key(&key) {
            return Err(JsError::new(
                "Error",
                format!("EEXIST: file already exists, mkdir '{path}'"),
            ));
        }
        let mut missing = Vec::new();
        let mut current = key.as_str();
        loop {
            let parent = parent_key(current);
            if parent == current {
                break;
            }
            if self.dirs.borrow().contains(parent) {
                break;
            }
            if self.files.borrow().contains_key(parent) {
                return Err(JsError::new(
                    "Error",
                    format!("ENOTDIR: not a directory, mkdir '{path}'"),
                ));
            }
            missing.push(parent.to_owned());
            current = parent;
        }
        let mut dirs = self.dirs.borrow_mut();
        for ancestor in missing.into_iter().rev() {
            dirs.insert(ancestor);
        }
        dirs.insert(key);
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), JsError> {
        let from_key = dir_key(from);
        let to_key = dir_key(to);
        let from_prefix = format!("{from_key}/");
        let to_prefix = format!("{to_key}/");
        let from_file_key = if self.files.borrow().contains_key(from) {
            from.to_owned()
        } else if self.files.borrow().contains_key(&from_key) {
            from_key.clone()
        } else {
            String::new()
        };
        let from_is_file = !from_file_key.is_empty();
        let from_is_dir = self.dirs.borrow().contains(&from_key);
        if !from_is_file && !from_is_dir {
            return Err(enoent("rename", from));
        }
        if from_key == to_key {
            return Ok(());
        }
        if from_is_dir && to_key.starts_with(&from_prefix) {
            return Err(JsError::new(
                "Error",
                format!("EINVAL: invalid argument, rename '{from}' -> '{to}'"),
            ));
        }
        let parent = parent_key(&to_key);
        if parent != "/" && parent != "." {
            if self.files.borrow().contains_key(parent) {
                return Err(JsError::new(
                    "Error",
                    format!("ENOTDIR: not a directory, rename '{from}' -> '{to}'"),
                ));
            }
            if !self.dirs.borrow().contains(parent) {
                return Err(enoent("rename", from));
            }
        }
        if from_is_file {
            if self.dirs.borrow().contains(&to_key) {
                return Err(JsError::new(
                    "Error",
                    format!("EISDIR: illegal operation on a directory, rename '{from}' -> '{to}'"),
                ));
            }
            let data = self
                .files
                .borrow_mut()
                .remove(&from_file_key)
                .unwrap_or_default();
            self.files.borrow_mut().insert(to_key, data);
            return Ok(());
        }
        if self.files.borrow().contains_key(to) || self.files.borrow().contains_key(&to_key) {
            return Err(JsError::new(
                "Error",
                format!("ENOTDIR: not a directory, rename '{from}' -> '{to}'"),
            ));
        }
        if self.dirs.borrow().contains(&to_key) {
            let occupied = self.dirs.borrow().iter().any(|k| k.starts_with(&to_prefix))
                || self
                    .files
                    .borrow()
                    .keys()
                    .any(|f| dir_key(f).starts_with(&to_prefix));
            if occupied {
                return Err(JsError::new(
                    "Error",
                    format!("ENOTEMPTY: directory not empty, rename '{from}' -> '{to}'"),
                ));
            }
            self.dirs.borrow_mut().remove(&to_key);
        }
        let moved_dirs: Vec<String> = self
            .dirs
            .borrow()
            .iter()
            .filter(|k| k.as_str() == from_key.as_str() || k.starts_with(&from_prefix))
            .cloned()
            .collect();
        let moved_files: Vec<(String, Vec<u8>)> = self
            .files
            .borrow()
            .iter()
            .filter(|(k, _)| dir_key(k).starts_with(&from_prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        {
            let mut dirs = self.dirs.borrow_mut();
            for old in moved_dirs {
                dirs.remove(&old);
                dirs.insert(format!("{to_key}{}", &old[from_key.len()..]));
            }
        }
        {
            let mut files = self.files.borrow_mut();
            for (old, data) in moved_files {
                files.remove(&old);
                let old_key = dir_key(&old);
                files.insert(format!("{to_key}{}", &old_key[from_key.len()..]), data);
            }
        }
        Ok(())
    }

    fn remove_all(&self, path: &str) -> Result<(), JsError> {
        let key = dir_key(path);
        if self.files.borrow_mut().remove(&key).is_some() {
            return Ok(());
        }
        if !self.dirs.borrow().contains(&key) {
            return Err(enoent("lstat", path));
        }
        let prefix = format!("{key}/");
        let gone_files: Vec<String> = self
            .files
            .borrow()
            .keys()
            .filter(|f| dir_key(f).starts_with(&prefix))
            .cloned()
            .collect();
        {
            let mut files = self.files.borrow_mut();
            for f in gone_files {
                files.remove(&f);
            }
        }
        let mut dirs = self.dirs.borrow_mut();
        let gone: Vec<String> = dirs
            .iter()
            .filter(|d| d.as_str() == key.as_str() || d.starts_with(&prefix))
            .cloned()
            .collect();
        for d in gone {
            dirs.remove(&d);
        }
        Ok(())
    }
}
