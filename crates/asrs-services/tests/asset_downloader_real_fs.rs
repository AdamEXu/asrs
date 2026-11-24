#![cfg(unix)]
#![allow(clippy::disallowed_methods)]

//! `asset_downloader` on `RealFs`: a zip lands in its asset folder and stale entries go, and an
//! archive cannot write through a symlink left inside `<target>_download` (decompress's guards).

use std::path::{Path, PathBuf};

use asrs_js::time::NamedZone;
use asrs_services::asset_downloader::{
    AssetDownloader, Distribution, DownloadHost, FetchId, FetchOutcome, FetchRequest, HttpOutcome,
    HttpRequest, Rejected, RequestId,
};
use asrs_services::asset_loader::AssetFolders;
use asrs_services::fs::RealFs;
use asrs_services::path::PathStyle;

const ASSET_URL: &str = "https://github.com/o/r/releases/download/default-assets-v2/Field2d_X.zip";

struct Host {
    zip: Vec<u8>,
}

impl DownloadHost for Host {
    fn free_space(&mut self, _: &str) -> Result<f64, Rejected> {
        Ok(1e12)
    }

    fn fetch(&mut self, _: FetchId, _: &FetchRequest) -> FetchOutcome {
        let body = format!(
            r#"[{{"tag_name":"default-assets-v2","assets":[{{"name":"Field2d_X.zip","browser_download_url":"{ASSET_URL}"}}]}}]"#
        );
        FetchOutcome::Response {
            status: 200,
            body: Some(body.into_bytes()),
        }
    }

    fn request(&mut self, _: RequestId, request: &HttpRequest) -> HttpOutcome {
        assert_eq!(request.url, ASSET_URL);
        HttpOutcome::Response {
            status: 200,
            headers: vec![(
                "content-disposition".to_owned(),
                "attachment; filename=Field2d_X.zip".to_owned(),
            )],
            body: self.zip.clone(),
        }
    }

    fn random(&mut self) -> f64 {
        0.5
    }

    fn assets_updated(&mut self) -> Result<(), Rejected> {
        Ok(())
    }
}

fn stored_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let offset = out.len() as u32;
        let header = |sig: u32, central_dir: bool| {
            let mut h = sig.to_le_bytes().to_vec();
            if central_dir {
                h.extend(0x0314u16.to_le_bytes());
            }
            h.extend(20u16.to_le_bytes());
            h.extend([0u8; 4]);
            h.extend([0u8; 4]);
            h.extend(crc32(data).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((data.len() as u32).to_le_bytes());
            h.extend((name.len() as u16).to_le_bytes());
            h.extend(0u16.to_le_bytes());
            if central_dir {
                h.extend([0u8; 6]);
                let mode: u32 = if name.ends_with('/') {
                    0o040755
                } else {
                    0o100644
                };
                h.extend((mode << 16).to_le_bytes());
                h.extend(offset.to_le_bytes());
            }
            h.extend(name.as_bytes());
            h
        };
        let local = header(0x0403_4b50, false);
        central.extend(header(0x0201_4b50, true));
        out.extend(local);
        out.extend(*data);
    }
    let cd_offset = out.len() as u32;
    out.extend(&central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0u8; 4]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((central.len() as u32).to_le_bytes());
    out.extend(cd_offset.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> TempDir {
        let dir = std::env::temp_dir().join(format!("asrs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        TempDir(dir.canonicalize().expect("canonical temp dir"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn run(root: &Path, zip: Vec<u8>) -> AssetDownloader {
    let user_data = root.join("userData");
    let folders = AssetFolders::new(PathStyle::Posix, user_data.to_str().unwrap(), "/bundled");
    let mut downloader = AssetDownloader::new(&folders, Distribution::Frc6328);
    let zone = NamedZone::get("UTC").expect("zone");
    downloader.start(0.0, &RealFs, &mut Host { zip }, &zone);
    downloader
}

#[test]
fn zip_is_extracted_and_strays_removed() {
    let temp = TempDir::new("download-ok");
    let auto = temp.0.join("userData/autoAssets");
    std::fs::create_dir_all(auto.join("Old_Asset/sub")).unwrap();
    std::fs::write(auto.join("stray.txt"), b"x").unwrap();
    let downloader = run(
        &temp.0,
        stored_zip(&[("config.json", b"{}"), ("dir/", b""), ("dir/a.bin", b"abc")]),
    );
    assert_eq!(
        downloader.status(),
        "All assets downloaded. AdvantageScope will check for updates periodically."
    );
    let target = auto.join("Field2d_X");
    assert_eq!(std::fs::read(target.join("config.json")).unwrap(), b"{}");
    assert_eq!(std::fs::read(target.join("dir/a.bin")).unwrap(), b"abc");
    let mut names: Vec<String> = std::fs::read_dir(&auto)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["Field2d_X"]);
}

#[test]
fn archive_never_writes_through_a_symlink() {
    let temp = TempDir::new("download-symlink");
    let auto = temp.0.join("userData/autoAssets");
    let outside = temp.0.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let download = auto.join("Field2d_X_download");
    std::fs::create_dir_all(&download).unwrap();
    std::os::unix::fs::symlink(&outside, download.join("link")).unwrap();
    std::os::unix::fs::symlink(outside.join("file.txt"), download.join("file.txt")).unwrap();
    let downloader = run(
        &temp.0,
        stored_zip(&[("link/escape.txt", b"nope"), ("file.txt", b"nope")]),
    );
    assert_eq!(
        downloader.status(),
        "Cannot connect to GitHub. Trying again soon."
    );
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
    assert!(
        std::fs::symlink_metadata(download.join("link"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}
