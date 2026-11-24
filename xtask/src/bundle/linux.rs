use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::process::Command;

use flate2::Compression;
use flate2::write::GzEncoder;

use super::associations::{mime_package, mime_types};
use super::{Bundle, at, copy, copy_tree, run_tool, write};
use crate::{Result, oracle, sha256_hex, workspace_root};

const ICON_SIZES: &[u32] = &[16, 32, 48, 64, 128, 256, 512];
const RUNTIME_TAG: &str = "20251108";
/// The AppImage runtime's file name and hash for each architecture.
const RUNTIMES: &[(&str, &str, &str)] = &[
    (
        "x64",
        "runtime-x86_64",
        "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d",
    ),
    (
        "arm64",
        "runtime-aarch64",
        "00cbdfcf917cc6c0ff6d3347d59e0ca1f7f45a6df1a428a0d6d8a78664d87444",
    ),
];
const RUNTIME_NOTICE: &str = "AppImage runtime
================

The AppImage starts with the type 2 runtime, release 20251108 of
https://github.com/AppImage/type2-runtime (MIT), which has its source and build scripts. It
contains musl (MIT), libfuse (LGPL-2.1, see licenses/LGPL-2.1.txt), squashfuse (BSD-2-Clause),
zstd (BSD-3-Clause) and zlib (Zlib). It can be replaced: `asrs.AppImage --appimage-extract`
unpacks the app, and a runtime of your own followed by `mksquashfs squashfs-root` of it is an
AppImage again.
";

pub fn package(bundle: &Bundle) -> Result {
    let root = workspace_root();
    let stage = bundle.stage();
    let share = stage.join("share");
    integration(&share, "asrs")?;
    let installer = root.join("packaging/linux/install.sh");
    let text = fs::read_to_string(&installer).map_err(at(&installer))?;
    let installed = stage.join("install.sh");
    copy(&installer, &installed)?;
    write(&installed, &text.replace("@MIME_LIST@", &mime_types(" ")))?;

    let tarball = bundle.out.join(format!("{}.tar.gz", bundle.file_stem()));
    tar_gz(&stage, &bundle.file_stem(), &tarball)?;
    appimage(bundle, &stage)
}

/// The desktop entry, the icons and the file types, in an XDG data directory.
fn integration(share: &Path, exec: &str) -> Result {
    let packaging = workspace_root().join("packaging");
    let entry = packaging.join("linux/asrs.desktop");
    let entry = fs::read_to_string(&entry).map_err(at(&entry))?;
    write(
        &share.join("applications/asrs.desktop"),
        &entry
            .replace("@EXEC@", exec)
            .replace("@MIME_TYPES@", &(mime_types(";") + ";")),
    )?;
    write(
        &share.join("mime/packages/net.adamxu.asrs.xml"),
        &mime_package(),
    )?;
    for size in ICON_SIZES {
        copy(
            &packaging.join(format!("icons/{size}.png")),
            &share.join(format!("icons/hicolor/{size}x{size}/apps/asrs.png")),
        )?;
    }
    Ok(())
}

fn tar_gz(stage: &Path, name: &str, out: &Path) -> Result {
    let file = File::create(out).map_err(at(out))?;
    let mut tar = tar::Builder::new(GzEncoder::new(file, Compression::best()));
    tar.mode(tar::HeaderMode::Deterministic);
    tar.append_dir_all(name, stage).map_err(at(out))?;
    tar.into_inner()
        .and_then(GzEncoder::finish)
        .map(drop)
        .map_err(at(out))
}

fn appimage(bundle: &Bundle, stage: &Path) -> Result {
    let dir = bundle.out.join("AppDir");
    let program = dir.join("usr/lib/asrs");
    copy_tree(stage, &program)?;
    for part in ["share", "install.sh"] {
        let path = program.join(part);
        if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        }
        .map_err(at(&path))?;
    }
    write(
        &program.join("licenses/AppImage runtime.txt"),
        RUNTIME_NOTICE,
    )?;
    integration(&dir.join("usr/share"), "asrs")?;
    copy(
        &dir.join("usr/share/applications/asrs.desktop"),
        &dir.join("asrs.desktop"),
    )?;
    for icon in ["asrs.png", ".DirIcon"] {
        copy(
            &workspace_root().join("packaging/icons/256.png"),
            &dir.join(icon),
        )?;
    }
    run_tool(
        Command::new("ln")
            .args(["-s", "usr/lib/asrs/asrs"])
            .arg(dir.join("AppRun")),
    )?;

    let image = bundle.out.join("app.squashfs");
    run_tool(
        Command::new("mksquashfs")
            .arg(&dir)
            .arg(&image)
            .args(["-root-owned", "-noappend", "-comp", "zstd", "-no-progress"])
            .arg("-quiet"),
    )
    .map_err(|e| format!("{e}\n(needs squashfs-tools)"))?;
    let out = bundle.out.join(format!("{}.AppImage", bundle.file_stem()));
    let mut bytes = runtime(bundle.arch)?;
    bytes.extend(fs::read(&image).map_err(at(&image))?);
    let mut file = File::create(&out).map_err(at(&out))?;
    file.write_all(&bytes).map_err(at(&out))?;
    drop(file);
    run_tool(Command::new("chmod").arg("755").arg(&out))?;
    fs::remove_file(&image).map_err(at(&image))?;
    fs::remove_dir_all(&dir).map_err(at(&dir))
}

fn runtime(arch: &str) -> Result<Vec<u8>> {
    let (_, name, sha256) = RUNTIMES
        .iter()
        .find(|(a, ..)| *a == arch)
        .ok_or_else(|| format!("no AppImage runtime for {arch}"))?;
    let path = oracle::Paths::new()?
        .cache
        .join("appimage")
        .join(RUNTIME_TAG)
        .join(name);
    if !path.is_file() {
        let dir = path.parent().unwrap_or(&path);
        fs::create_dir_all(dir).map_err(at(dir))?;
        let url = format!(
            "https://github.com/AppImage/type2-runtime/releases/download/{RUNTIME_TAG}/{name}"
        );
        run_tool(
            Command::new("curl")
                .args(["-fsSL", "--retry", "3", "-o"])
                .arg(&path)
                .arg(url),
        )?;
    }
    let bytes = fs::read(&path).map_err(at(&path))?;
    if sha256_hex(&bytes) != *sha256 {
        return Err(format!("{}: sha256 is not {sha256}", path.display()));
    }
    Ok(bytes)
}
