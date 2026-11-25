use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use asrs_model::app_identity::{APP_USER_MODEL_ID, BUNDLE_ID};

use super::associations::{ASSOCIATIONS, CSV};
use super::{Bundle, NAME, VERSION, run_tool, write};
use crate::version::Version;
use crate::{Result, workspace_root};

pub fn package(bundle: &Bundle) -> Result {
    let stage = bundle.stage();
    let zip = bundle.out.join(format!("{}.zip", bundle.file_stem()));
    run_tool(
        Command::new(system_tar())
            .arg("-a")
            .arg("-cf")
            .arg(&zip)
            .arg("-C")
            .arg(&bundle.out)
            .arg(bundle.file_stem()),
    )?;
    let script = bundle.out.join("installer.iss");
    write(&script, &installer(bundle, &stage, &Version::current()?))?;
    run_tool(Command::new(inno_setup()?).arg("/Q").arg(&script))
}

/// Git for Windows puts a GNU tar without zip support first on the PATH.
pub fn system_tar() -> PathBuf {
    let root = std::env::var_os("SystemRoot").map_or_else(|| "C:/Windows".into(), PathBuf::from);
    root.join("System32/tar.exe")
}

fn inno_setup() -> Result<PathBuf> {
    ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"]
        .into_iter()
        .filter_map(std::env::var_os)
        .flat_map(|dir| {
            let dir = PathBuf::from(dir);
            [dir.join("Inno Setup 6"), dir.join("Programs/Inno Setup 6")]
        })
        .map(|dir| dir.join("ISCC.exe"))
        .find(|exe| exe.is_file())
        .ok_or_else(|| "Inno Setup 6 (https://jrsoftware.org) is not installed".to_owned())
}

/// Installs for the current user and adds asrs to the "Open with" list of each extension. The
/// extension's own default value, which names the default program, is never written. CSV files
/// often have no default, and the only program in OpenWithProgids then becomes it, so for them
/// asrs is only among the programs that support the type.
fn installer(bundle: &Bundle, stage: &Path, version: &Version) -> String {
    let architecture = match bundle.arch {
        "arm64" => "arm64",
        _ => "x64compatible",
    };
    let icon = workspace_root().join("packaging/icons/asrs.ico");
    let mut registry = String::new();
    for a in ASSOCIATIONS {
        let (ext, name) = (a.extension, a.name);
        let _ = writeln!(
            registry,
            "Root: HKCU; Subkey: \"Software\\Classes\\Applications\\asrs.exe\\SupportedTypes\"; ValueType: string; ValueName: \".{ext}\"; ValueData: \"\""
        );
        if a.mime == CSV {
            continue;
        }
        let class = format!("Software\\Classes\\asrs.{ext}");
        let _ = write!(
            registry,
            "Root: HKCU; Subkey: \"{class}\"; ValueType: string; ValueData: \"{name}\"; Flags: uninsdeletekey
Root: HKCU; Subkey: \"{class}\\DefaultIcon\"; ValueType: string; ValueData: \"{{app}}\\asrs.exe,0\"
Root: HKCU; Subkey: \"{class}\\shell\\open\\command\"; ValueType: string; ValueData: \"\"\"{{app}}\\asrs.exe\"\" \"\"%1\"\"\"
Root: HKCU; Subkey: \"Software\\Classes\\.{ext}\"; Flags: uninsdeletekeyifempty
Root: HKCU; Subkey: \"Software\\Classes\\.{ext}\\OpenWithProgids\"; ValueType: string; ValueName: \"asrs.{ext}\"; ValueData: \"\"; Flags: uninsdeletevalue uninsdeletekeyifempty
"
        );
    }
    format!(
        "[Setup]
AppId={BUNDLE_ID}
AppName={NAME}
AppVersion={VERSION}
AppPublisher=Adam Xu
AppPublisherURL=https://github.com/AdamEXu/asrs
VersionInfoVersion={windows}
VersionInfoTextVersion={VERSION}
VersionInfoProductTextVersion={VERSION}
DefaultDirName={{autopf}}\\{NAME}
PrivilegesRequired=lowest
DisableProgramGroupPage=yes
ChangesAssociations=yes
ArchitecturesAllowed={architecture}
ArchitecturesInstallIn64BitMode={architecture}
SetupIconFile={icon}
UninstallDisplayIcon={{app}}\\asrs.exe
OutputDir={out}
OutputBaseFilename={stem}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern

[Files]
Source: \"{stage}\\*\"; DestDir: \"{{app}}\"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: \"{{autoprograms}}\\{NAME}\"; Filename: \"{{app}}\\asrs.exe\"; AppUserModelID: \"{APP_USER_MODEL_ID}\"

[Registry]
Root: HKCU; Subkey: \"Software\\Classes\\Applications\\asrs.exe\"; Flags: uninsdeletekey
Root: HKCU; Subkey: \"Software\\Classes\\Applications\\asrs.exe\\shell\\open\\command\"; ValueType: string; ValueData: \"\"\"{{app}}\\asrs.exe\"\" \"\"%1\"\"\"
{registry}
[Run]
Filename: \"{{app}}\\asrs.exe\"; Description: \"{{cm:LaunchProgram,{NAME}}}\"; Flags: nowait postinstall skipifsilent
",
        windows = version.windows(),
        icon = icon.display(),
        out = bundle.out.display(),
        stem = bundle.file_stem(),
        stage = stage.display(),
    )
}
