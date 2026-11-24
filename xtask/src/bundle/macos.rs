use std::fmt::Write;
use std::fs;
use std::path::Path;
use std::process::Command;

use asrs_model::app_identity::BUNDLE_ID;

use super::associations::{ASSOCIATIONS, CSV};
use super::{Bundle, MACOS_MINIMUM, NAME, VERSION, at, copy, run_tool, write};
use crate::{Result, workspace_root};

pub const ENTITLEMENTS: &str = "packaging/macos/entitlements.plist";
/// Where the license files are in the mounted image, for a reader who does not open the app.
const DMG_LICENSES: &[&str] = &["LICENSE", "THIRD_PARTY_NOTICES.md"];
/// Even as an alternate, the only app that names CSV files becomes their default, ahead of every
/// text editor, which name text. An app that names this type, which a CSV file also has, does not.
const CSV_PARENT: &str = "public.delimited-values-text";
/// hdiutil fails now and then while the system still scans a fresh volume ("Resource busy").
const DMG_ATTEMPTS: u32 = 5;

pub fn package(bundle: &Bundle) -> Result {
    let root = workspace_root();
    let app = bundle.stage();
    let contents = app.join("Contents");
    copy(
        &root.join("packaging/icons/asrs.icns"),
        &contents.join("Resources/asrs.icns"),
    )?;
    write(&contents.join("Info.plist"), &info_plist())?;
    write(&contents.join("PkgInfo"), "APPL????")?;
    sign(&bundle.exe_dir(&app).join(bundle.library_name()), &root)?;
    sign(&app, &root)?;
    dmg(bundle, &app)
}

fn sign(path: &Path, root: &Path) -> Result {
    run_tool(
        Command::new("codesign")
            .args(["--force", "--sign", "-", "--options", "runtime"])
            .arg("--entitlements")
            .arg(root.join(ENTITLEMENTS))
            .arg(path),
    )
}

fn dmg(bundle: &Bundle, app: &Path) -> Result {
    let folder = bundle.out.join("dmg");
    run_tool(Command::new("ditto").arg(app).arg(folder.join("asrs.app")))?;
    for name in DMG_LICENSES {
        copy(&bundle.resources_dir(app).join(name), &folder.join(name))?;
    }
    let link = folder.join("Applications");
    run_tool(Command::new("ln").args(["-s", "/Applications"]).arg(&link))?;
    let image = bundle.out.join(format!("{}.dmg", bundle.file_stem()));
    let mut attempt = 1;
    loop {
        let made = run_tool(
            Command::new("hdiutil")
                .args(["create", "-quiet", "-ov", "-fs", "HFS+", "-format", "UDZO"])
                .args(["-volname", &format!("{NAME} {VERSION}"), "-srcfolder"])
                .arg(&folder)
                .arg(&image),
        );
        match made {
            Err(e) if attempt == DMG_ATTEMPTS => return Err(e),
            Err(e) => eprintln!("{e}, trying again"),
            Ok(()) => break,
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
        attempt += 1;
    }
    fs::remove_dir_all(&folder).map_err(at(&folder))
}

fn info_plist() -> String {
    let mut documents = String::new();
    for a in ASSOCIATIONS {
        let files = if a.mime == CSV {
            format!(
                "<key>LSItemContentTypes</key>
      <array>
        <string>{CSV_PARENT}</string>
      </array>"
            )
        } else {
            format!(
                "<key>CFBundleTypeExtensions</key>
      <array>
        <string>{}</string>
      </array>
      <key>CFBundleTypeMIMETypes</key>
      <array>
        <string>{}</string>
      </array>",
                a.extension, a.mime
            )
        };
        let _ = write!(
            documents,
            "    <dict>
      <key>CFBundleTypeName</key>
      <string>{}</string>
      {files}
      <key>CFBundleTypeRole</key>
      <string>Viewer</string>
      <key>LSHandlerRank</key>
      <string>Alternate</string>
    </dict>
",
            a.name
        );
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">
<plist version=\"1.0\">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleLocalizations</key>
  <array>
    <string>en</string>
  </array>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleIdentifier</key>
  <string>{BUNDLE_ID}</string>
  <key>CFBundleName</key>
  <string>{NAME}</string>
  <key>CFBundleDisplayName</key>
  <string>{NAME}</string>
  <key>CFBundleExecutable</key>
  <string>{NAME}</string>
  <key>CFBundleIconFile</key>
  <string>asrs</string>
  <key>CFBundleShortVersionString</key>
  <string>{VERSION}</string>
  <key>CFBundleVersion</key>
  <string>{VERSION}</string>
  <key>CFBundleDocumentTypes</key>
  <array>
{documents}  </array>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.utilities</string>
  <key>LSMinimumSystemVersion</key>
  <string>{MACOS_MINIMUM}</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSHumanReadableCopyright</key>
  <string>Copyright \u{a9} 2026 Adam Xu. Copyright \u{a9} 2021-2026 Littleton Robotics.</string>
  <key>NSLocalNetworkUsageDescription</key>
  <string>asrs uses the local network to communicate with robots.</string>
</dict>
</plist>
"
    )
}
