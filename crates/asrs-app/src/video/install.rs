//! What [`install_ffmpeg`](asrs_services::video::install_ffmpeg) does to the disk, on a thread of
//! its own: antivirus scans what it opens and renames.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use asrs_js::string::{JsError, JsString};
use asrs_services::video::InstallFiles;
use sha2::{Digest, Sha256};

/// Whether the app is quitting.
pub struct Files(pub Arc<AtomicBool>);

fn io_error(error: io::Error) -> JsError {
    JsError::new("Error", error.to_string())
}

impl InstallFiles for Files {
    fn read_file(&mut self, path: &str) -> Result<Vec<u8>, JsError> {
        std::fs::read(path).map_err(io_error)
    }

    fn unlink(&mut self, path: &str) -> Result<(), JsError> {
        std::fs::remove_file(path).map_err(io_error)
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), JsError> {
        std::fs::rename(from, to).map_err(io_error)
    }

    fn chmod(&mut self, path: &str, mode: u32) -> Result<(), JsError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).map_err(io_error)
        }
        #[cfg(not(unix))]
        {
            let _ = mode;
            std::fs::metadata(path).map(drop).map_err(io_error)
        }
    }

    fn sha256_hex(&mut self, data: &[u8]) -> String {
        let digest = Sha256::digest(data);
        digest.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    fn warn(&mut self, text: JsString) {
        eprintln!("asrs: {}", text.to_string_lossy());
    }

    fn cancelled(&mut self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use asrs_services::video::{FfmpegInstall, install_ffmpeg};
    use std::fs;
    use std::path::PathBuf;

    const ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    struct Case {
        dir: PathBuf,
        install: FfmpegInstall,
    }

    impl Case {
        fn new(name: &str, data: &str) -> Case {
            let dir = std::env::temp_dir().join(format!("asrs-{name}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("dir");
            let path = |name: &str| dir.join(name).to_string_lossy().into_owned();
            let install = FfmpegInstall {
                download: path("ffmpeg-download"),
                ffmpeg: path("ffmpeg"),
                sha256: ABC,
            };
            fs::write(&install.download, data).expect("download");
            Case { dir, install }
        }

        fn run(&self, quitting: bool) -> (bool, Vec<String>) {
            let mut files = Files(Arc::new(AtomicBool::new(quitting)));
            let ok = install_ffmpeg(&self.install, &mut files);
            let names = fs::read_dir(&self.dir).expect("dir").map(|entry| {
                let name = entry.expect("entry").file_name();
                name.to_string_lossy().into_owned()
            });
            (ok, names.collect())
        }
    }

    impl Drop for Case {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn a_verified_download_becomes_the_program() {
        let case = Case::new("install", "abc");
        assert_eq!(case.run(false), (true, vec!["ffmpeg".to_owned()]));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&case.install.ffmpeg).expect("ffmpeg");
            assert_eq!(mode.permissions().mode() & 0o777, 0o755);
        }
        assert_eq!(case.run(false), (false, vec!["ffmpeg".to_owned()]));
    }

    #[test]
    fn a_wrong_hash_deletes_the_download() {
        let case = Case::new("install-hash", "abd");
        assert_eq!(case.run(false), (false, Vec::new()));
    }

    #[test]
    fn quitting_leaves_the_files_alone() {
        for data in ["abc", "abd"] {
            let case = Case::new("install-quit", data);
            assert_eq!(case.run(true), (false, vec!["ffmpeg-download".to_owned()]));
        }
    }
}
