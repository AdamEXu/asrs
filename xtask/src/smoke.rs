use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use asrs_parity::Json;
use asrs_parity::json;
use asrs_parity::vectors;

use crate::bundle::{self, Bundle, ENTITLEMENTS, Os, at, contains, copy, copy_tree, run_tool};
use crate::{Result, workspace_root};

const USAGE: &str = "usage: cargo xtask smoke [--target <triple>] [--no-docs]

Checks the packages `cargo xtask bundle` wrote with the same arguments, installed in
target/smoke:

- its license files and assets are there, the documentation is not with --no-docs, and no file
  association names .hoot;
- on macOS, the code signature is valid, with the hardened runtime and exactly the entitlements of
  packaging/macos;
- the program reads a recorded frame's match timer, which needs the OCR's compiled code to run;
- it starts, opens a window, loads a log given as an argument and reports no errors (needs a
  display);
- it runs with an asrs-libm-lgpl built from the source in the package in place of its own, and
  refuses one with another ABI version and a missing one;
- on Windows, the installer registers the file types without becoming their default, and
  uninstalls; on Linux, install.sh does the same and the AppImage runs; on macOS, Launch Services
  does, with which the app is registered while this runs.";

const CLIP: &str = "banner-lower-2025";
const START_TIMEOUT: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(250);
/// How long a started program must go on running without an error.
const SETTLE: Duration = Duration::from_secs(5);
const LOG: &str = "parity/scenarios/inputs/drive.wpilog";
const LIBRARY_SOURCE: &str = "asrs-libm-lgpl";

const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/\
                          LaunchServices.framework/Support/lsregister";
const HANDLERS: &str = r#"
ObjC.import("AppKit");
function run(argv) {
  const workspace = $.NSWorkspace.sharedWorkspace;
  const url = $.NSURL.fileURLWithPath(argv[0]);
  const first = workspace.URLForApplicationToOpenURL(url);
  const all = workspace.URLsForApplicationsToOpenURL(url);
  const paths = first.isNil() ? [] : [ObjC.unwrap(first.path)];
  for (let i = 0; i < all.count; i++) paths.push(ObjC.unwrap(all.objectAtIndex(i).path));
  return paths.join("\n");
}"#;

/// Other programs' files, of the type asrs names for CSV files on macOS.
const TEXT_TYPES: &[&str] = &["csv", "tsv"];

/// Prints the program Windows opens CSV files with.
const CSV_PROGRAM: &str = r#"
$query = Add-Type -PassThru -Namespace Asrs -Name Assoc -MemberDefinition @'
[DllImport("shlwapi.dll", CharSet = CharSet.Unicode)]
public static extern uint AssocQueryString(
    uint flags, uint what, string type, string extra, System.Text.StringBuilder text, ref uint length);
'@
$text = New-Object System.Text.StringBuilder 1024
[uint32]$length = 1024
[void]$query::AssocQueryString(0, 2, '.csv', [NullString]::Value, $text, [ref]$length)
$text.ToString()
"#;

/// Prints what of asrs and its file types the current user and the machine have registered.
const WINDOWS_TRACES: &str = r#"
foreach ($root in 'HKCU:\Software\Classes', 'HKLM:\Software\Classes') {
  foreach ($ext in 'wpilog', 'wpilogxz', 'rlog', 'revlog', 'csv', 'dslog', 'dsevents', 'hoot') {
    foreach ($key in "$root\.$ext", "$root\.$ext\OpenWithProgids", "$root\asrs.$ext") {
      if (Test-Path $key) { "$key $((Get-Item $key).Property -join ',')" }
    }
  }
  foreach ($key in "$root\Applications\asrs.exe", "$root\AppUserModelId\AdamXu.asrs") {
    if (Test-Path $key) { $key }
  }
}
foreach ($root in 'HKCU:\Software', 'HKLM:\Software', 'HKLM:\Software\WOW6432Node') {
  $key = "$root\Microsoft\Windows\CurrentVersion\Uninstall\net.adamxu.asrs_is1"
  if (Test-Path $key) { $key }
}
foreach ($menu in 'Programs', 'CommonPrograms') {
  $link = Join-Path ([Environment]::GetFolderPath($menu)) 'asrs.lnk'
  if (Test-Path $link) { $link }
}
"#;

const LICENSE_FILES: &[&str] = &[
    "LICENSE",
    "THIRD_PARTY_NOTICES.md",
    "THIRD_PARTY_LICENSES.txt",
    "licenses.json",
    "licenses/LGPL-2.1.txt",
    "asrs-libm-lgpl/LICENSE",
    "asrs-libm-lgpl/README.md",
    "asrs-libm-lgpl/Cargo.toml",
    "asrs-libm-lgpl/src/lib.rs",
    "asrs-libm-lgpl/src/glibc.rs",
    "asrs-libm-lgpl/src/sincostab.rs",
];

pub fn run(args: &[String]) -> Result {
    let bundle = Bundle::parse(args, USAGE)?;
    for package in bundle.packages() {
        if !package.is_file() {
            return Err(format!(
                "{} does not exist; run `cargo xtask bundle` with the same arguments",
                package.display()
            ));
        }
    }
    let dir = workspace_root().join("target").join("smoke");
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(at(&dir))?;
    }
    fs::create_dir_all(&dir).map_err(at(&dir))?;
    let program = install(&bundle, &dir)?;
    let frame = Frame::recorded(&dir)?;
    let smoke = Smoke {
        bundle,
        dir,
        program,
        frame,
    };
    step("files", smoke.files())?;
    if smoke.bundle.os == Os::Macos {
        step("signature", smoke.signature())?;
    }
    step("self-test", smoke.self_test())?;
    step("start", smoke.start())?;
    step("library", smoke.library())?;
    match smoke.bundle.os {
        Os::Macos => step("associations", smoke.associations())?,
        Os::Windows => step("installer", smoke.installer())?,
        Os::Linux => {
            step("install.sh", smoke.install_script())?;
            step("AppImage", smoke.appimage())?;
        }
    }
    eprintln!("smoke passed");
    Ok(())
}

fn step(name: &str, result: Result) -> Result {
    match &result {
        Ok(()) => eprintln!("ok   {name}"),
        Err(_) => eprintln!("FAIL {name}"),
    }
    result.map_err(|e| format!("{name}: {e}"))
}

/// Unpacks the first package and returns the app bundle or the program's folder.
fn install(bundle: &Bundle, dir: &Path) -> Result<PathBuf> {
    let packages = bundle.packages();
    let package = &packages[0];
    match bundle.os {
        Os::Macos => {
            let volume = dir.join("volume");
            fs::create_dir_all(&volume).map_err(at(&volume))?;
            run_tool(
                Command::new("hdiutil")
                    .args(["attach", "-quiet", "-nobrowse", "-readonly", "-mountpoint"])
                    .arg(&volume)
                    .arg(package),
            )?;
            let app = dir.join("asrs.app");
            let copied = run_tool(Command::new("ditto").arg(volume.join("asrs.app")).arg(&app));
            run_tool(
                Command::new("hdiutil")
                    .args(["detach", "-quiet", "-force"])
                    .arg(&volume),
            )?;
            copied?;
            Ok(app)
        }
        Os::Windows | Os::Linux => {
            let tar = match bundle.os {
                Os::Windows => bundle::windows_tar(),
                _ => PathBuf::from("tar"),
            };
            run_tool(Command::new(tar).arg("-xf").arg(package).arg("-C").arg(dir))?;
            Ok(dir.join(bundle.file_stem()))
        }
    }
}

/// A frame of a `video/timer-rects` vector in which upstream's OCR reads a time.
struct Frame {
    image: PathBuf,
    rectangle: [f64; 4],
    text: String,
}

impl Frame {
    fn recorded(dir: &Path) -> Result<Frame> {
        let bits = |json: &Json| {
            let hex = json.get("n")?.as_str()?.to_string_lossy();
            u64::from_str_radix(&hex, 16).ok().map(f64::from_bits)
        };
        for record in vectors::load("video/timer-rects")? {
            let field = |key: &str| record.field(key);
            let text = |json: Option<&Json>| json?.as_str().map(|s| s.to_string_lossy());
            if record.string("clip").as_deref() != Some(CLIP) {
                continue;
            }
            let Some(expected) = text(field("fresh").and_then(|fresh| fresh.get("text"))) else {
                continue;
            };
            if !expected.contains(':') || field("edit") != Some(&Json::Object(Vec::new())) {
                continue;
            }
            let rectangle: Option<Vec<f64>> = field("rectangle")
                .and_then(Json::as_array)
                .and_then(|sides| sides.iter().map(bits).collect());
            let rectangle = rectangle.and_then(|sides| <[f64; 4]>::try_from(sides).ok());
            let frame = field("frame").and_then(Json::as_f64);
            let (Some(rectangle), Some(frame)) = (rectangle, frame) else {
                return Err(format!("line {}: not a rectangle record", record.line));
            };
            let image = dir.join("frame.jpg");
            fs::write(&image, clip_frame(frame)?).map_err(at(&image))?;
            return Ok(Frame {
                image,
                rectangle,
                text: expected,
            });
        }
        Err(format!("no record of {CLIP} reads a time"))
    }

    fn args(&self) -> Vec<String> {
        let image = self.image.display().to_string();
        [image]
            .into_iter()
            .chain(self.rectangle.iter().map(f64::to_string))
            .collect()
    }
}

/// The JPEG of a frame in the clip (format in `oracle/tools/timerClips.mjs`).
fn clip_frame(frame: f64) -> Result<Vec<u8>> {
    let path = workspace_root().join(format!("parity/specs/video/timer/{CLIP}.clip"));
    let data = fs::read(&path).map_err(at(&path))?;
    let bad = || format!("{}: not a clip", path.display());
    let size = data.first_chunk::<4>().ok_or_else(bad)?;
    let end = 4 + u32::from_le_bytes(*size) as usize;
    let header = data.get(4..end).ok_or_else(bad)?;
    let header = json::parse(&String::from_utf8_lossy(header)).map_err(|e| e.to_string())?;
    let mut offset = end;
    for entry in header
        .get("frames")
        .and_then(Json::as_array)
        .ok_or_else(bad)?
    {
        let Some([number, length]) = entry.as_array() else {
            return Err(bad());
        };
        let length = length.as_f64().ok_or_else(bad)? as usize;
        if number.as_f64() == Some(frame) {
            let image = data.get(offset..offset + length).ok_or_else(bad)?;
            return Ok(image.to_vec());
        }
        offset += length;
    }
    Err(format!("{}: no frame {frame}", path.display()))
}

struct Smoke {
    bundle: Bundle,
    dir: PathBuf,
    program: PathBuf,
    frame: Frame,
}

impl Smoke {
    fn exe(&self) -> PathBuf {
        self.bundle
            .exe_dir(&self.program)
            .join(self.bundle.exe_name())
    }

    fn library_path(&self) -> PathBuf {
        self.bundle
            .exe_dir(&self.program)
            .join(self.bundle.library_name())
    }

    fn files(&self) -> Result {
        let resources = self.bundle.resources_dir(&self.program);
        for name in LICENSE_FILES {
            if !resources.join(name).is_file() {
                return Err(format!("{name} is missing"));
            }
        }
        let docs = resources.join("docs/build/index.html").is_file();
        if docs != self.bundle.docs {
            return Err(format!("documentation in the package: {docs}"));
        }
        if !self.bundle.docs && resources.join("docs").exists() {
            return Err("part of the documentation is in the package".to_owned());
        }
        let registration = match self.bundle.os {
            Os::Macos => self.program.join("Contents/Info.plist"),
            Os::Windows => self.bundle.out.join("installer.iss"),
            Os::Linux => self.program.join("share/applications/asrs.desktop"),
        };
        let text = fs::read(&registration).map_err(at(&registration))?;
        if contains(&text, b"hoot") {
            return Err(format!("{} names .hoot", registration.display()));
        }
        if !contains(&text, b"wpilog") {
            return Err(format!("{} has no file types", registration.display()));
        }
        Ok(())
    }

    fn signature(&self) -> Result {
        run_tool(
            Command::new("plutil")
                .args(["-lint", "-s"])
                .arg(self.program.join("Contents/Info.plist")),
        )?;
        run_tool(
            Command::new("codesign")
                .args(["--verify", "--strict", "--deep"])
                .arg(&self.program),
        )?;
        let shown = Command::new("codesign")
            .args(["--display", "--verbose=2"])
            .arg(&self.program)
            .output()
            .map_err(|e| format!("codesign: {e}"))?;
        let details = String::from_utf8_lossy(&shown.stderr).into_owned();
        if !details.contains("runtime)") {
            return Err(format!("no hardened runtime:\n{details}"));
        }
        let signed = Command::new("codesign")
            .args(["--display", "--xml", "--entitlements", "-"])
            .arg(&self.program)
            .output()
            .map_err(|e| format!("codesign: {e}"))?;
        let expected = workspace_root().join(ENTITLEMENTS);
        let expected = fs::read_to_string(&expected).map_err(at(&expected))?;
        let (signed, expected) = (
            plist_keys(&String::from_utf8_lossy(&signed.stdout)),
            plist_keys(&expected),
        );
        if signed != expected || expected.is_empty() {
            return Err(format!(
                "entitlements are {signed:?}, expected {expected:?}"
            ));
        }
        Ok(())
    }

    fn self_test(&self) -> Result {
        let report = self_test(&self.exe(), &self.frame, &[])?;
        self.check_report(&report, &self.library_path())?;
        eprint!("{report}");
        Ok(())
    }

    fn check_report(&self, report: &str, library: &Path) -> Result {
        let value = |name: &str| {
            report
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{name}: ")))
                .ok_or_else(|| format!("no {name} in:\n{report}"))
        };
        let text = format!("{:?}", self.frame.text);
        if value("text")? != text {
            return Err(format!("expected the text {text} in:\n{report}"));
        }
        let name = library.file_name().unwrap_or_default().to_string_lossy();
        if !value("library")?.contains(&*name) {
            return Err(format!("not the package's library:\n{report}"));
        }
        if (value("documentation")? != "none") != self.bundle.docs {
            return Err(format!("wrong documentation:\n{report}"));
        }
        value("assets")?;
        value("textures")?;
        value("licenses")?;
        Ok(())
    }

    /// Runs the program on a log and waits for its state file, which it saves once a window is
    /// open.
    fn start(&self) -> Result {
        let home = self.dir.join("home");
        let log = workspace_root().join(LOG);
        let mut app = App::start(&self.exe(), &[log.as_path()], &home, &self.dir)?;
        let state = app.wait(|app| window_state(&app.user_data))?;
        let title = log.file_name().unwrap_or_default().to_string_lossy();
        app.wait(|app| app.titled(&title))?;
        thread::sleep(SETTLE);
        app.check()?;
        eprintln!("     window {state}");
        Ok(())
    }

    /// Launch Services lists the app for each file type, as the default only where no other app
    /// opens the type, and a file opened through it starts the app.
    fn associations(&self) -> Result {
        let launch_services =
            |option: &str| run_tool(Command::new(LSREGISTER).arg(option).arg(&self.program));
        let folder = self.dir.join("files");
        fs::create_dir_all(&folder).map_err(at(&folder))?;
        let file = |extension: &str| folder.join(format!("file.{extension}"));
        for extension in bundle::extensions().chain(["hoot", "tsv"]) {
            fs::write(file(extension), "").map_err(at(&folder))?;
        }
        launch_services("-u")?;
        thread::sleep(SETTLE);
        let defaults = || -> Result<Vec<Option<PathBuf>>> {
            TEXT_TYPES
                .iter()
                .map(|extension| Ok(handlers(&file(extension))?.into_iter().next()))
                .collect()
        };
        let before = defaults()?;
        launch_services("-f")?;
        thread::sleep(SETTLE);
        let checked = bundle::extensions()
            .try_for_each(|extension| {
                let found = handlers(&file(extension))?;
                if !found.contains(&self.program) {
                    return Err(format!(".{extension} is not for asrs: {found:?}"));
                }
                Ok(())
            })
            .and_then(|()| match defaults()? {
                now if now == before && before.iter().all(Option::is_some) => Ok(()),
                now => Err(format!(
                    "the defaults for {TEXT_TYPES:?} went from {before:?} to {now:?}"
                )),
            })
            .and_then(|()| match handlers(&file("hoot"))? {
                found if found.contains(&self.program) => Err(".hoot is for asrs".to_owned()),
                _ => Ok(()),
            })
            .and_then(|()| self.open());
        launch_services("-u")?;
        checked
    }

    fn open(&self) -> Result {
        let home = self.dir.join("open-home");
        let data = home.join("Library/Application Support");
        fs::create_dir_all(&data).map_err(at(&data))?;
        run_tool(
            Command::new("open")
                .args(["-n", "-g", "-a"])
                .arg(&self.program)
                .arg("--env")
                .arg(format!("HOME={}", home.display()))
                .arg(workspace_root().join(LOG)),
        )?;
        let deadline = Instant::now() + START_TIMEOUT;
        let user_data = data.join("asrs");
        while window_state(&user_data).is_none() && Instant::now() < deadline {
            thread::sleep(POLL);
        }
        let opened = window_state(&user_data);
        let running = Command::new("pgrep")
            .args(["-f", "-x"])
            .arg(self.exe())
            .output()
            .map_err(|e| format!("pgrep: {e}"))?;
        for pid in String::from_utf8_lossy(&running.stdout).lines() {
            run_tool(Command::new("kill").arg(pid))?;
        }
        opened
            .map(drop)
            .ok_or_else(|| "opening a file did not open a window".to_owned())
    }

    fn library(&self) -> Result {
        let shipped = fs::read(self.library_path()).map_err(at(&self.library_path()))?;
        let source = self
            .bundle
            .resources_dir(&self.program)
            .join(LIBRARY_SOURCE);

        let own = self.rebuild(&source, "own", |text| text)?;
        copy(&own, &self.library_path())?;
        let report = self_test(&self.exe(), &self.frame, &[])?;
        self.check_report(&report, &self.library_path())?;
        if fs::read(self.library_path()).is_ok_and(|bytes| bytes == shipped) {
            eprintln!("     the library built from the package's source is the shipped one");
        }

        let other = self.rebuild(&source, "other", |text| {
            text.replace("ABI_VERSION: u32 = 1;", "ABI_VERSION: u32 = 2;")
        })?;
        copy(&other, &self.library_path())?;
        self.refuses("ABI version 2", &[])?;

        fs::remove_file(self.library_path()).map_err(at(&self.library_path()))?;
        self.refuses("was not found", &[])?;
        let env = [("ASRS_LIBM_LGPL", own.as_path())];
        let report = self_test(&self.exe(), &self.frame, &env)?;
        self.check_report(&report, &own)?;
        fs::write(self.library_path(), shipped).map_err(at(&self.library_path()))
    }

    fn refuses(&self, reason: &str, env: &[(&str, &Path)]) -> Result {
        match self_test(&self.exe(), &self.frame, env) {
            Err(e) if e.contains(reason) => Ok(()),
            Err(e) => Err(format!("expected {reason:?}, got {e}")),
            Ok(report) => Err(format!("expected {reason:?}, but it ran:\n{report}")),
        }
    }

    /// Follows the package's README: `cargo build --release` of its library source, after `edit`.
    fn rebuild(
        &self,
        source: &Path,
        name: &str,
        edit: impl Fn(String) -> String,
    ) -> Result<PathBuf> {
        let dir = self.dir.join(format!("library-{name}"));
        copy_tree(source, &dir)?;
        let lib = dir.join("src/lib.rs");
        let text = fs::read_to_string(&lib).map_err(at(&lib))?;
        let edited = edit(text.clone());
        if name == "other" && edited == text {
            return Err("the library's source no longer has the ABI version to edit".to_owned());
        }
        fs::write(&lib, edited).map_err(at(&lib))?;
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
        run_tool(
            Command::new(cargo)
                .args(["build", "--release", "--quiet", "--target-dir"])
                .arg(dir.join("target"))
                .arg("--manifest-path")
                .arg(dir.join("Cargo.toml"))
                .env_remove("RUSTFLAGS")
                .current_dir(&dir),
        )?;
        Ok(dir.join("target/release").join(self.bundle.library_name()))
    }

    fn installer(&self) -> Result {
        let setup = &self.bundle.packages()[1];
        let installed = self.dir.join("installed");
        let csv = powershell(CSV_PROGRAM)?;
        let traces = powershell(WINDOWS_TRACES)?;
        run_tool(
            Command::new(setup)
                .args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"])
                .arg(format!("/DIR={}", installed.display())),
        )?;
        let exe = installed.join("asrs.exe");
        let report = self_test(&exe, &self.frame, &[])?;
        self.check_report(&report, &installed.join(self.bundle.library_name()))?;
        let registered = powershell(
            "$c = 'HKCU:\\Software\\Classes'
             $progids = (Get-Item \"$c\\.wpilog\\OpenWithProgids\").Property
             if ($progids -notcontains 'asrs.wpilog') { throw 'not an Open with program' }
             if ((Get-Item \"$c\\.wpilog\").GetValue('')) { throw 'became the default' }
             $supported = (Get-Item \"$c\\Applications\\asrs.exe\\SupportedTypes\").Property
             if ($supported -notcontains '.csv') { throw '.csv is not supported' }
             if (Test-Path \"$c\\.hoot\") { throw '.hoot is registered' }
             (Get-Item \"$c\\asrs.wpilog\\shell\\open\\command\").GetValue('')
             $lnk = Join-Path ([Environment]::GetFolderPath('Programs')) 'asrs.lnk'
             $folder = (New-Object -ComObject Shell.Application).Namespace((Split-Path $lnk))
             $folder.ParseName('asrs.lnk').ExtendedProperty('System.AppUserModel.ID')",
        )?;
        if powershell(WINDOWS_TRACES)?.trim() == traces.trim() {
            return Err("the installer left no traces to look for".to_owned());
        }
        let now = powershell(CSV_PROGRAM)?;
        if now != csv {
            return Err(format!("the default for .csv went from {csv:?} to {now:?}"));
        }
        let expected = format!("\"{}\" \"%1\"", exe.display());
        if !registered.contains(&expected) || !registered.contains("AdamXu.asrs") {
            return Err(format!("registered {registered:?}, expected {expected}"));
        }
        let uninstaller = installed.join("unins000.exe");
        run_tool(Command::new(&uninstaller).args([
            "/VERYSILENT",
            "/SUPPRESSMSGBOXES",
            "/NORESTART",
        ]))?;
        let deadline = Instant::now() + START_TIMEOUT;
        while uninstaller.exists() {
            if Instant::now() > deadline {
                return Err("the uninstaller did not finish".to_owned());
            }
            thread::sleep(POLL);
        }
        let deadline = Instant::now() + START_TIMEOUT;
        while installed.exists() {
            if Instant::now() > deadline {
                return Err(format!("the uninstaller left {}", installed.display()));
            }
            thread::sleep(POLL);
        }
        let left = powershell(WINDOWS_TRACES)?;
        if left.trim() == traces.trim() {
            Ok(())
        } else {
            Err(format!(
                "before installing: {traces:?}, after uninstalling: {left:?}"
            ))
        }
    }

    fn install_script(&self) -> Result {
        let data = self.dir.join("data");
        let config = self.dir.join("config");
        let script = self.program.join("install.sh");
        let run = |args: &[&str]| {
            run_tool(
                Command::new(&script)
                    .args(args)
                    .env("XDG_DATA_HOME", &data)
                    .env("XDG_CONFIG_HOME", &config),
            )
        };
        let other = data.join("applications/other.desktop");
        bundle::write(
            &other,
            "[Desktop Entry]\nType=Application\nName=other\nExec=true %f\n\
             MimeType=application/x-rlog;\n",
        )?;
        run(&[])?;
        let entry = data.join("applications/asrs.desktop");
        let text = fs::read_to_string(&entry).map_err(at(&entry))?;
        let exec = format!("Exec=\"{}\" %F", self.exe().display());
        if !text.contains(&exec) {
            return Err(format!("no {exec} in {text}"));
        }
        if text.contains("text/csv") {
            return Err("the menu entry can become the default for CSV files".to_owned());
        }
        run_tool(Command::new("desktop-file-validate").arg(&entry))?;
        let log = workspace_root().join(LOG);
        let gio = |args: &[&str]| -> Result<String> {
            let output = Command::new("gio")
                .args(args)
                .env("XDG_DATA_HOME", &data)
                .env("XDG_CONFIG_HOME", &config)
                .output()
                .map_err(|e| format!("gio: {e}"))?;
            Ok(String::from_utf8_lossy(&output.stdout).into_owned())
        };
        let log = log.display().to_string();
        let info = gio(&["info", "-a", "standard::content-type", &log])?;
        if !info.contains("application/x-wpilog") {
            return Err(format!("the log's type is {info}"));
        }
        let handlers = gio(&["mime", "application/x-wpilog"])?;
        if !handlers.contains("asrs.desktop") {
            return Err(format!("no asrs among the handlers: {handlers}"));
        }
        let handlers = gio(&["mime", "application/x-rlog"])?;
        let default = handlers.lines().next().unwrap_or_default();
        if !default.ends_with("other.desktop") || !handlers.contains("asrs.desktop") {
            return Err(format!("asrs took another program's file type: {handlers}"));
        }
        run(&["--uninstall"])?;
        let left = named(&data, &["asrs", "x-wpilog"]);
        if !left.is_empty() {
            return Err(format!("--uninstall left {left:?}"));
        }
        self.install_script_path()
    }

    /// The menu entry starts a program in a folder whose name needs every escape the entry has.
    fn install_script_path(&self) -> Result {
        let folder = self.dir.join("a \"b\" `c` $d \\e &|'");
        copy_tree(&self.program.join("share"), &folder.join("share"))?;
        let script = folder.join("install.sh");
        copy(&self.program.join("install.sh"), &script)?;
        let program = folder.join("asrs");
        copy(&script, &program)?;
        bundle::write(&program, "#!/bin/sh\n: >\"${0%/*}/started\"\n")?;
        let data = self.dir.join("data-path");
        let env = |command: &mut Command| {
            run_tool(
                command
                    .env("XDG_DATA_HOME", &data)
                    .env("XDG_CONFIG_HOME", self.dir.join("config-path")),
            )
        };
        env(&mut Command::new(&script))?;
        let entry = data.join("applications/asrs.desktop");
        run_tool(Command::new("desktop-file-validate").arg(&entry))?;
        env(Command::new("gio").arg("launch").arg(&entry))?;
        let started = folder.join("started");
        let deadline = Instant::now() + SETTLE;
        while !started.exists() {
            if Instant::now() > deadline {
                return Err(format!("{} starts nothing", entry.display()));
            }
            thread::sleep(POLL);
        }
        env(Command::new(&script).arg("--uninstall"))?;

        let percent = self.dir.join("a%f");
        fs::rename(&folder, &percent).map_err(at(&folder))?;
        if env(&mut Command::new(percent.join("install.sh"))).is_ok() || entry.exists() {
            return Err("install.sh wrote a menu entry that starts nothing".to_owned());
        }
        Ok(())
    }

    fn appimage(&self) -> Result {
        let image = &self.bundle.packages()[1];
        let extract = [("APPIMAGE_EXTRACT_AND_RUN", Path::new("1"))];
        let report = self_test(image, &self.frame, &extract)?;
        self.check_report(&report, &self.library_path())
    }
}

/// The files under `dir` with one of `parts` in their name.
fn named(dir: &Path, parts: &[&str]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            found.extend(named(&path, parts));
        } else if parts.iter().any(|part| name.contains(part)) {
            found.push(path);
        }
    }
    found
}

fn plist_keys(text: &str) -> Vec<String> {
    let mut keys: Vec<String> = text
        .split("<key>")
        .skip(1)
        .filter_map(|rest| Some(rest.split_once("</key>")?.0.to_owned()))
        .collect();
    keys.sort();
    keys
}

fn self_test(exe: &Path, frame: &Frame, env: &[(&str, &Path)]) -> Result<String> {
    let output = Command::new(exe)
        .arg("--self-test")
        .args(frame.args())
        .envs(env.iter().copied())
        .output()
        .map_err(|e| format!("{}: {e}", exe.display()))?;
    let Output {
        status,
        stdout,
        stderr,
    } = output;
    let report = String::from_utf8_lossy(&stdout).into_owned();
    if status.success() {
        Ok(report)
    } else {
        let stderr = String::from_utf8_lossy(&stderr);
        Err(format!("self-test failed ({status}):\n{report}{stderr}"))
    }
}

fn powershell(script: &str) -> Result<String> {
    let script = format!("$ErrorActionPreference = 'Stop'\n{script}");
    let output = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).into_owned())
    }
}

/// The first hub window's bounds in the state file.
fn window_state(user_data: &Path) -> Option<String> {
    let entries = fs::read_dir(user_data).ok()?;
    let state = entries.filter_map(std::result::Result::ok).find(|entry| {
        let name = entry.file_name();
        name.to_string_lossy().starts_with("state-")
    })?;
    let text = fs::read_to_string(state.path()).ok()?;
    let json = json::parse(&text).ok()?;
    let hub = json.get("hubs")?.as_array()?.first()?;
    let side = |key: &str| hub.get(key).and_then(Json::as_f64);
    let (width, height) = (side("width")?, side("height")?);
    (width > 0.0 && height > 0.0).then(|| format!("{width}x{height}"))
}

/// The apps Launch Services offers for a file, the default first.
fn handlers(file: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("osascript")
        .args(["-l", "JavaScript", "-e", HANDLERS])
        .arg(file)
        .output()
        .map_err(|e| format!("osascript: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let lines = String::from_utf8_lossy(&output.stdout);
    Ok(lines
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// A running asrs with a home of its own. Dropping it ends the process.
struct App {
    child: Child,
    user_data: PathBuf,
    stderr: PathBuf,
}

impl App {
    fn start(exe: &Path, files: &[&Path], home: &Path, dir: &Path) -> Result<App> {
        let (data, variable) = if cfg!(target_os = "macos") {
            (home.join("Library/Application Support"), "HOME")
        } else if cfg!(windows) {
            (home.to_owned(), "APPDATA")
        } else {
            (home.to_owned(), "XDG_CONFIG_HOME")
        };
        fs::create_dir_all(&data).map_err(at(&data))?;
        let stderr = dir.join("stderr.txt");
        let file = File::create(&stderr).map_err(at(&stderr))?;
        let child = Command::new(exe)
            .args(files)
            .env(variable, home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(file)
            .spawn()
            .map_err(|e| format!("{}: {e}", exe.display()))?;
        Ok(App {
            child,
            user_data: data.join("asrs"),
            stderr,
        })
    }

    fn wait<T>(&mut self, ready: impl Fn(&App) -> Option<T>) -> Result<T> {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            self.check()?;
            if let Some(value) = ready(self) {
                return Ok(value);
            }
            if Instant::now() > deadline {
                let stderr = fs::read_to_string(&self.stderr).unwrap_or_default();
                return Err(format!(
                    "not ready after {START_TIMEOUT:?}; stderr:\n{stderr}"
                ));
            }
            thread::sleep(POLL);
        }
    }

    /// Fails when the process has ended or printed an error.
    fn check(&mut self) -> Result {
        let stderr = fs::read_to_string(&self.stderr).unwrap_or_default();
        if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("it ended ({status}):\n{stderr}"));
        }
        if stderr.lines().any(|line| line.starts_with("asrs: ")) || stderr.contains("panicked") {
            return Err(format!("it printed:\n{stderr}"));
        }
        Ok(())
    }

    /// Whether a window's title names the log. macOS shows other programs' titles only to
    /// programs allowed to record the screen.
    fn titled(&self, name: &str) -> Option<()> {
        let titles = if cfg!(windows) {
            let id = self.child.id();
            powershell(&format!("(Get-Process -Id {id}).MainWindowTitle")).ok()?
        } else if cfg!(target_os = "linux") {
            let output = Command::new("xwininfo")
                .args(["-root", "-tree"])
                .output()
                .ok()?;
            String::from_utf8_lossy(&output.stdout).into_owned()
        } else {
            return Some(());
        };
        titles.contains(name).then_some(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
