//! The file system of the asset download loop: upstream's names and archives decide the paths,
//! and nothing outside the automatic assets folder is written or removed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use asrs_js::string::JsError;
use asrs_services::fs::{DirEntry, Fs, ReadError, RealFs};
use asrs_services::path::PathStyle;

pub(super) struct Confined {
    root: String,
    stopped: Arc<AtomicBool>,
}

/// Whether an operation follows a symlink at the end of its path.
#[derive(Clone, Copy, PartialEq)]
enum Last {
    Followed,
    Kept,
}

impl Confined {
    pub(super) fn new(root: &str, stopped: Arc<AtomicBool>) -> Confined {
        Confined {
            root: normalize(root),
            stopped,
        }
    }

    /// `path` normalized, as it was checked.
    fn allow(&self, path: &str, last: Last) -> Result<String, JsError> {
        let refused = |why: &str| Err(JsError::new("Error", format!("{why}: {path}")));
        if self.stopped.load(Ordering::SeqCst) {
            return refused("the app is quitting");
        }
        let normalized = normalize(path);
        let path_in_root = Path::new(&normalized)
            .strip_prefix(&self.root)
            .is_ok_and(|rest| {
                let names: Vec<_> = rest.iter().map(|name| name.to_string_lossy()).collect();
                let named = |name: &str| !cfg!(windows) || is_plain_windows_name(name);
                !names.is_empty() && names.iter().all(|name| named(name))
            });
        if !path_in_root || !self.resolves_inside(Path::new(&normalized), last) {
            return refused("outside the automatic assets folder");
        }
        Ok(normalized)
    }

    /// Whether the deepest part of `path` that exists is in the folder with its symlinks resolved.
    fn resolves_inside(&self, path: &Path, last: Last) -> bool {
        let Ok(root) = std::fs::canonicalize(&self.root) else {
            return true;
        };
        let start = match last {
            Last::Followed => Some(path),
            Last::Kept => path.parent(),
        };
        let exists = |path: &&Path| std::fs::symlink_metadata(path).is_ok();
        start
            .and_then(|start| start.ancestors().find(exists))
            .and_then(|found| std::fs::canonicalize(found).ok())
            .is_some_and(|found: PathBuf| found.starts_with(&root))
    }
}

/// Whether Windows takes the name for a file of that name: not a device, a drive, a stream, or a
/// name whose end it drops.
fn is_plain_windows_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or_default().trim_end();
    let stem = stem.to_uppercase();
    let device = match stem.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$" => true,
        stem => ["COM", "LPT"].iter().any(|port| {
            let mut number = stem.strip_prefix(port).unwrap_or_default().chars();
            let digit = number.next();
            digit.is_some_and(|c| c.is_ascii_digit() || "\u{b9}\u{b2}\u{b3}".contains(c))
                && number.next().is_none()
        }),
    };
    !device && !name.contains(':') && !name.ends_with(['.', ' '])
}

fn normalize(path: &str) -> String {
    PathStyle::host().normalize(&path.into()).to_string_lossy()
}

impl Fs for Confined {
    fn exists(&self, path: &str) -> bool {
        RealFs.exists(path)
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, JsError> {
        RealFs.read(path)
    }

    fn write(&self, path: &str, data: &[u8]) -> Result<(), JsError> {
        RealFs.write(&self.allow(path, Last::Followed)?, data)
    }

    fn copy(&self, from: &str, to: &str) -> Result<(), JsError> {
        RealFs.copy(from, &self.allow(to, Last::Followed)?)
    }

    fn read_file(&self, path: &str) -> Result<Vec<u8>, ReadError> {
        RealFs.read_file(path)
    }

    fn remove(&self, path: &str) {
        if let Ok(path) = self.allow(path, Last::Kept) {
            RealFs.remove(&path);
        }
    }

    fn read_dir(&self, path: &str) -> Result<Vec<DirEntry>, JsError> {
        RealFs.read_dir(path)
    }

    fn mkdir(&self, path: &str) -> Result<(), JsError> {
        RealFs.mkdir(&self.allow(path, Last::Followed)?)
    }

    fn mkdir_all(&self, path: &str) -> Result<(), JsError> {
        RealFs.mkdir_all(&self.allow(path, Last::Followed)?)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), JsError> {
        let from = self.allow(from, Last::Kept)?;
        RealFs.rename(&from, &self.allow(to, Last::Kept)?)
    }

    fn remove_all(&self, path: &str) -> Result<(), JsError> {
        RealFs.remove_all(&self.allow(path, Last::Kept)?)
    }

    fn realpath(&self, path: &str) -> Result<String, JsError> {
        RealFs.realpath(path)
    }

    fn readlink(&self, path: &str) -> Result<String, JsError> {
        RealFs.readlink(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Temp(PathBuf);

    impl Temp {
        fn new(name: &str) -> Temp {
            let dir = std::env::temp_dir().join(format!("asrs-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("autoAssets/Robot_A")).expect("temp");
            fs::create_dir_all(dir.join("other")).expect("temp");
            fs::write(dir.join("other/file"), "kept").expect("temp");
            Temp(dir)
        }

        fn path(&self, rest: &str) -> String {
            self.0.join(rest).to_string_lossy().into_owned()
        }

        fn fs(&self, stopped: bool) -> Confined {
            Confined::new(&self.path("autoAssets"), Arc::new(stopped.into()))
        }

        fn other(&self) -> Vec<(String, Vec<u8>)> {
            let entries = fs::read_dir(self.0.join("other")).expect("other");
            entries
                .map(|e| e.expect("other"))
                .map(|e| {
                    let data = fs::read(e.path()).expect("a file");
                    (e.file_name().to_string_lossy().into_owned(), data)
                })
                .collect()
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn refuses_everything(fs: &Confined, temp: &Temp, path: &str) {
        let inside = temp.path("autoAssets/Robot_A");
        assert!(fs.write(path, b"x").is_err(), "write {path}");
        assert!(fs.copy(&inside, path).is_err(), "copy {path}");
        assert!(fs.mkdir(path).is_err(), "mkdir {path}");
        assert!(fs.mkdir_all(path).is_err(), "mkdir_all {path}");
        assert!(fs.rename(&inside, path).is_err(), "rename to {path}");
        assert!(fs.rename(path, &inside).is_err(), "rename from {path}");
        assert!(fs.remove_all(path).is_err(), "remove_all {path}");
        fs.remove(path);
    }

    #[test]
    fn nothing_outside_the_folder_changes() {
        let temp = Temp::new("confined-outside");
        let fs = temp.fs(false);
        for path in [
            "autoAssets",
            "autoAssets/.",
            "autoAssets/Robot_A/..",
            "autoAssets/../other/file",
            "autoAssets/../other/new",
            "autoAssets/../new",
            "autoAssets2",
            "autoAssets_download/x",
            "other/file",
            "",
        ] {
            refuses_everything(&fs, &temp, &temp.path(path));
        }
        refuses_everything(&fs, &temp, "autoAssets/relative");
        assert_eq!(temp.other(), [("file".to_owned(), b"kept".to_vec())]);
        assert!(temp.0.join("autoAssets/Robot_A").is_dir());
        assert!(!temp.0.join("autoAssets2").exists());
    }

    #[test]
    fn the_folder_itself_is_writable() {
        let temp = Temp::new("confined-inside");
        let fs = temp.fs(false);
        let file = temp.path("autoAssets/Robot_B_download/sub/../deep/file");
        let deep = temp.path("autoAssets/Robot_B_download/deep");
        fs.mkdir_all(&deep).expect("mkdir_all");
        fs.mkdir_all(&temp.path("autoAssets/Robot_B_download/sub"))
            .expect("mkdir_all");
        fs.write(&file, b"data").expect("write");
        let (from, to) = (
            temp.path("autoAssets/Robot_B_download"),
            temp.path("autoAssets/Robot_B"),
        );
        fs.rename(&from, &to).expect("rename");
        assert_eq!(
            fs.read(&temp.path("autoAssets/Robot_B/deep/file"))
                .expect("read"),
            b"data"
        );
        fs.remove_all(&to).expect("remove_all");
        assert!(!fs.exists(&to));
    }

    #[test]
    fn a_missing_folder_is_created() {
        let temp = Temp::new("confined-missing");
        fs::remove_dir_all(temp.0.join("autoAssets")).expect("remove");
        let fs = temp.fs(false);
        let dir = temp.path("autoAssets/Robot_A_download");
        fs.mkdir_all(&dir).expect("mkdir_all");
        assert!(Path::new(&dir).is_dir());
    }

    #[test]
    fn nothing_changes_once_stopped() {
        let temp = Temp::new("confined-stopped");
        let fs = temp.fs(true);
        refuses_everything(&fs, &temp, &temp.path("autoAssets/Robot_B"));
        assert!(!temp.0.join("autoAssets/Robot_B").exists());
    }

    #[test]
    fn windows_devices_streams_and_dropped_endings_are_not_plain_names() {
        for name in [
            "NUL",
            "nul",
            "Con",
            "AUX.txt",
            "nul .tar.gz",
            "PRN.",
            "COM1",
            "lpt9.json",
            "com0",
            "COM\u{b9}",
            "LPT\u{b3}.x",
            "CONIN$",
            "conout$",
            "C:",
            "C:evil",
            "file:stream",
            "config.json::$DATA",
            "x.",
            "x ",
            "...",
            ".. ",
            "x. .",
        ] {
            assert!(!is_plain_windows_name(name), "{name:?}");
        }
        for name in [
            "Robot_A",
            "Robot_A_download",
            "config.json",
            "NULL",
            "COM",
            "COM10",
            "LPTX",
            "COM1x",
            "a.nul",
            ".hidden",
            "CONsole",
            "Field3d_2025 Reefscape",
            "x..y",
        ] {
            assert!(is_plain_windows_name(name), "{name:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_names_that_are_not_files_are_refused() {
        let temp = Temp::new("confined-windows");
        let fs = temp.fs(false);
        for name in [
            "NUL",
            "Robot_A/COM1",
            "Robot_A/file:stream",
            "C:evil",
            "Robot_B. ",
        ] {
            refuses_everything(&fs, &temp, &temp.path(&format!("autoAssets/{name}")));
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_lead_nowhere() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new("confined-symlink");
        let auto = temp.0.join("autoAssets");
        symlink(temp.0.join("other"), auto.join("dir")).expect("symlink");
        symlink(temp.0.join("other/file"), auto.join("to_file")).expect("symlink");
        symlink(temp.0.join("other/new"), auto.join("dangling")).expect("symlink");
        let fs = temp.fs(false);
        for path in ["dir/file", "dir/new", "dir/sub/new"] {
            refuses_everything(&fs, &temp, &temp.path(&format!("autoAssets/{path}")));
        }
        for link in ["to_file", "dangling"] {
            let path = temp.path(&format!("autoAssets/{link}"));
            assert!(fs.write(&path, b"x").is_err(), "{link}");
            fs.remove_all(&path).expect("the link itself");
        }
        fs.remove_all(&temp.path("autoAssets/dir"))
            .expect("the link itself");
        assert_eq!(temp.other(), [("file".to_owned(), b"kept".to_vec())]);
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_folder_is_still_the_folder() {
        use std::os::unix::fs::symlink;
        let temp = Temp::new("confined-linked");
        let auto = temp.0.join("autoAssets");
        fs::rename(&auto, temp.0.join("moved")).expect("rename");
        symlink(temp.0.join("moved"), &auto).expect("symlink");
        let fs = temp.fs(false);
        fs.mkdir_all(&temp.path("autoAssets/Robot_B"))
            .expect("mkdir_all");
        fs.write(&temp.path("autoAssets/Robot_B/file"), b"x")
            .expect("write");
        assert!(temp.0.join("moved/Robot_B/file").is_file());
    }
}
