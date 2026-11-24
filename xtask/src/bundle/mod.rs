mod associations;
mod linux;
mod macos;
mod windows;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Result, docs, gpui, libm, licenses, workspace_root};

pub use associations::extensions;
pub use macos::ENTITLEMENTS;
pub use windows::system_tar as windows_tar;

const USAGE: &str = "usage: cargo xtask bundle [--target <triple>] [--no-docs]

Builds the unsigned packages of this platform (or of <triple>) in
target/bundle/<triple or host>/, with --no-docs in target/bundle/<triple or host>-no-docs/:

  macOS    asrs.app, ad hoc signed with the hardened runtime, and a .dmg of it
  Windows  a .zip and an Inno Setup installer for the current user (needs Inno Setup 6)
  Linux    a .tar.gz with an install.sh for the current user, and an AppImage (needs mksquashfs)

Each has the asrs binary (a release build without test-support, built with
ASRS_LIBM_LGPL_SHIP=1, so it only loads the library next to it or from ASRS_LIBM_LGPL, and
ASRS_PACKAGED=1, so it runs as a packaged app), the asrs-libm-lgpl shared library, on Linux
asrs-webview (the only program that links WebKitGTK, and the only platform that uses it),
upstream's bundledAssets/ and www/textures/ (from assets/ or reference/AdvantageScope), the
documentation `cargo xtask docs` built (as docs/build, left out with --no-docs: it may not be
published, see issue 310) and the `cargo xtask licenses` bundle including the library's LGPL text
and source.
The version is the workspace's. Needs cargo-about.";

const WEBVIEW: &str = "asrs-webview";
/// What asrs reads from upstream's files at run time.
const UPSTREAM_DIRS: &[&str] = &["bundledAssets", "www/textures"];
const NAME: &str = "asrs";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// What upstream's Electron needs.
const MACOS_MINIMUM: &str = "12.0";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Macos,
    Windows,
    Linux,
}

/// What is being packaged and where.
pub struct Bundle {
    pub os: Os,
    pub arch: &'static str,
    pub docs: bool,
    target: Option<String>,
    /// `target/bundle/<triple or host>[-no-docs]`.
    pub out: PathBuf,
}

impl Bundle {
    pub fn parse(args: &[String], usage: &str) -> Result<Bundle> {
        let mut target = None;
        let mut docs = true;
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--target" => target = Some(args.next().ok_or(usage)?.clone()),
                "--no-docs" => docs = false,
                _ => return Err(usage.to_owned()),
            }
        }
        let triple = target.clone().unwrap_or_else(host_triple);
        let os = match &triple {
            t if t.contains("windows") => Os::Windows,
            t if t.contains("apple") => Os::Macos,
            t if t.contains("linux") => Os::Linux,
            t => return Err(format!("no packages for {t}")),
        };
        let arch = match triple.split('-').next() {
            Some("aarch64") => "arm64",
            Some("x86_64") => "x64",
            _ => return Err(format!("no packages for {triple}")),
        };
        let mut dir = target.clone().unwrap_or_else(|| "host".to_owned());
        if !docs {
            dir.push_str("-no-docs");
        }
        Ok(Bundle {
            os,
            arch,
            docs,
            target,
            out: workspace_root().join("target/bundle").join(dir),
        })
    }

    /// The packages' file name without an extension.
    pub fn file_stem(&self) -> String {
        let os = match self.os {
            Os::Macos => "macos",
            Os::Windows => "windows",
            Os::Linux => "linux",
        };
        let docs = if self.docs { "" } else { "-no-docs" };
        format!("{NAME}-{VERSION}-{os}-{}{docs}", self.arch)
    }

    /// `asrs.app` on macOS, the folder the packages are made from elsewhere.
    pub fn stage(&self) -> PathBuf {
        self.out.join(match self.os {
            Os::Macos => format!("{NAME}.app"),
            Os::Windows | Os::Linux => self.file_stem(),
        })
    }

    pub fn exe_dir(&self, stage: &Path) -> PathBuf {
        match self.os {
            Os::Macos => stage.join("Contents/MacOS"),
            Os::Windows | Os::Linux => stage.to_owned(),
        }
    }

    pub fn resources_dir(&self, stage: &Path) -> PathBuf {
        match self.os {
            Os::Macos => stage.join("Contents/Resources"),
            Os::Windows | Os::Linux => stage.to_owned(),
        }
    }

    pub fn exe_name(&self) -> &'static str {
        match self.os {
            Os::Windows => "asrs.exe",
            Os::Macos | Os::Linux => "asrs",
        }
    }

    pub fn library_name(&self) -> &'static str {
        libm::file_name(self.target.as_deref())
    }

    pub fn packages(&self) -> Vec<PathBuf> {
        let extensions: &[&str] = match self.os {
            Os::Macos => &[".dmg"],
            Os::Windows => &[".zip", "-setup.exe"],
            Os::Linux => &[".tar.gz", ".AppImage"],
        };
        extensions
            .iter()
            .map(|e| self.out.join(format!("{}{e}", self.file_stem())))
            .collect()
    }
}

fn host_triple() -> String {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => format!("{arch}-apple-darwin"),
        "windows" => format!("{arch}-pc-windows-msvc"),
        os => format!("{arch}-unknown-{os}-gnu"),
    }
}

pub fn run(args: &[String]) -> Result {
    let bundle = Bundle::parse(args, USAGE)?;
    let root = workspace_root();
    let upstream = ["assets", "reference/AdvantageScope"]
        .map(|dir| root.join(dir))
        .into_iter()
        .find(|dir| UPSTREAM_DIRS.iter().all(|part| dir.join(part).is_dir()))
        .ok_or(
            "upstream's assets not found; link reference/AdvantageScope or run `cargo xtask assets`",
        )?;
    let documentation = docs::build_dir();
    if bundle.docs && !documentation.join("index.html").is_file() {
        return Err(
            "the documentation is not built; run `cargo xtask docs` or pass --no-docs".to_owned(),
        );
    }
    let release = build(&bundle)?;
    licenses::run()?;

    if bundle.out.exists() {
        fs::remove_dir_all(&bundle.out).map_err(at(&bundle.out))?;
    }
    let stage = bundle.stage();
    let (exe_dir, resources) = (bundle.exe_dir(&stage), bundle.resources_dir(&stage));
    fs::create_dir_all(&exe_dir).map_err(at(&exe_dir))?;
    copy_tree(&root.join("target/licenses"), &resources)?;
    let exe = exe_dir.join(bundle.exe_name());
    copy(&release.join(bundle.exe_name()), &exe)?;
    check_release(&bundle, &exe)?;
    let library = exe_dir.join(bundle.library_name());
    copy(&release.join(bundle.library_name()), &library)?;
    check_release(&bundle, &library)?;
    if bundle.os == Os::Linux {
        copy(&release.join(WEBVIEW), &exe_dir.join(WEBVIEW))?;
    }
    for dir in UPSTREAM_DIRS {
        copy_tree(&upstream.join(dir), &resources.join(dir))?;
    }
    if bundle.docs {
        copy_tree(&documentation, &resources.join("docs/build"))?;
    }
    match bundle.os {
        Os::Macos => macos::package(&bundle)?,
        Os::Windows => windows::package(&bundle)?,
        Os::Linux => linux::package(&bundle)?,
    }
    for package in bundle.packages() {
        let size = fs::metadata(&package).map_err(at(&package))?.len();
        eprintln!("wrote {} ({:.1} MB)", package.display(), size as f64 / 1e6);
    }
    Ok(())
}

/// Builds the programs and returns the directory they are in.
fn build(bundle: &Bundle) -> Result<PathBuf> {
    let mut args = vec!["build", "--locked", "--release"];
    args.extend(["-p", "asrs-app", "-p", "asrs-libm-lgpl"]);
    if bundle.os == Os::Linux {
        args.extend(["-p", WEBVIEW]);
    }
    if let Some(target) = &bundle.target {
        args.extend(["--target", target]);
    }
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    eprintln!(
        "$ ASRS_LIBM_LGPL_SHIP=1 ASRS_PACKAGED=1 cargo {}",
        args.join(" ")
    );
    let mut command = Command::new(cargo);
    command
        .args(&args)
        .env("ASRS_LIBM_LGPL_SHIP", "1")
        .env("ASRS_PACKAGED", "1")
        .env("ASRS_BUILD_DATE", build_date()?)
        .env_remove("ASRS_LIBM_LGPL_DEV")
        .current_dir(workspace_root());
    if bundle.os == Os::Macos && std::env::var_os("MACOSX_DEPLOYMENT_TARGET").is_none() {
        command.env("MACOSX_DEPLOYMENT_TARGET", MACOS_MINIMUM);
    }
    if bundle.os == Os::Windows {
        // The Visual C++ runtime is not part of Windows.
        let flags = std::env::var("RUSTFLAGS").unwrap_or_default();
        command.env(
            "RUSTFLAGS",
            format!("{flags} -C target-feature=+crt-static"),
        );
    }
    let status = command
        .status()
        .map_err(|e| format!("failed to run cargo: {e}"))?;
    if !status.success() {
        return Err(format!("building asrs-app failed ({status})"));
    }
    let mut release = libm::target_dir();
    release.extend(&bundle.target);
    release.push("release");
    Ok(release)
}

/// The commit's time, as upstream's `new Date().toISOString()` prints it.
fn build_date() -> Result<String> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%cd"])
        .arg("--date=format-local:%Y-%m-%dT%H:%M:%S.000Z")
        .env("TZ", "UTC0")
        .current_dir(workspace_root())
        .output()
        .map_err(|e| format!("failed to run git: {e}"))?;
    let date = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() && !date.is_empty() {
        Ok(date)
    } else {
        Err("git log gave no commit date for ASRS_BUILD_DATE".to_owned())
    }
}

/// asrs must start on machines without WebKitGTK or the Visual C++ runtime, so it may not name
/// them as needed libraries, and it may not contain GPUI's test-support.
fn check_release(bundle: &Bundle, file: &Path) -> Result {
    let libraries: &[&str] = match bundle.os {
        Os::Linux => &["libwebkit2gtk-", "libgtk-3.so", "libjavascriptcoregtk-"],
        Os::Windows => &["VCRUNTIME140", "MSVCP140", "api-ms-win-crt-"],
        Os::Macos => &[],
    };
    let bytes = fs::read(file).map_err(at(file))?;
    let found = libraries
        .iter()
        .find(|library| contains(&bytes, library.as_bytes()));
    if let Some(library) = found {
        return Err(format!("{} links {library}", file.display()));
    }
    match gpui::scan(&[file.to_owned()])?.first() {
        Some(marker) => Err(format!("test-support code is in the package: {marker}")),
        None => Ok(()),
    }
}

pub fn contains(bytes: &[u8], part: &[u8]) -> bool {
    bytes.windows(part.len()).any(|window| window == part)
}

pub fn at(path: &Path) -> impl Fn(std::io::Error) -> String {
    let path = path.display().to_string();
    move |e| format!("{path}: {e}")
}

pub fn write(path: &Path, text: &str) -> Result {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(at(parent))?;
    }
    fs::write(path, text).map_err(at(path))
}

pub fn run_tool(command: &mut Command) -> Result {
    eprintln!("$ {command:?}");
    let status = command
        .status()
        .map_err(|e| format!("failed to run {:?}: {e}", command.get_program()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{:?} failed ({status})", command.get_program()))
    }
}

pub fn copy(src: &Path, dst: &Path) -> Result {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(at(parent))?;
    }
    fs::copy(src, dst).map(drop).map_err(at(src))
}

pub fn copy_tree(src: &Path, dst: &Path) -> Result {
    fs::create_dir_all(dst).map_err(at(dst))?;
    for entry in fs::read_dir(src).map_err(at(src))? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.file_name().is_some_and(|n| n == ".DS_Store") {
            continue;
        }
        let to = dst.join(path.file_name().unwrap_or_default());
        if path.is_dir() {
            copy_tree(&path, &to)?;
        } else {
            copy(&path, &to)?;
        }
    }
    Ok(())
}
