use std::path::PathBuf;
use std::process::Command;

use crate::Result;
use crate::oracle::Paths;

pub(crate) const MODES: [&str; 3] = ["native", "swiftshader", "d3d11-warp"];
pub(crate) const VARIABLE: &str = "ASRS_GL";

pub(crate) fn parse(mode: &str) -> Result<&'static str> {
    MODES
        .into_iter()
        .find(|m| *m == mode)
        .ok_or_else(|| format!("the GL mode is one of {}, not {mode:?}", MODES.join(", ")))
}

/// The rasterizer 3D references are captured with on this machine.
pub(crate) fn default_mode() -> Result<&'static str> {
    match std::env::var(VARIABLE) {
        Ok(mode) if !mode.is_empty() => parse(&mode).map_err(|e| format!("{VARIABLE}: {e}")),
        _ if cfg!(target_os = "macos") => Ok("native"),
        _ => Ok("swiftshader"),
    }
}

/// Never for canvas or DOM captures: `--use-angle` also turns on GPU compositing and the GPU
/// canvas.
pub(crate) fn electron_switches(mode: &str) -> &'static [&'static str] {
    match mode {
        "swiftshader" => &["--enable-unsafe-swiftshader"],
        "d3d11-warp" => &["--enable-unsafe-swiftshader", "--use-angle=d3d11-warp"],
        _ => &[],
    }
}

/// Has the port's Vulkan device load the SwiftShader that Electron's WebGL runs on. Two different
/// software rasterizers do not agree at edges, so a machine without it is refused.
pub(crate) fn pair_port(paths: &Paths, mode: &str, command: &mut Command) -> Result {
    if cfg!(target_os = "linux") && mode == "swiftshader" {
        command.env("VK_ICD_FILENAMES", swiftshader_icd(paths)?);
    }
    Ok(())
}

fn swiftshader_icd(paths: &Paths) -> Result<PathBuf> {
    let icd = paths.electron()?.with_file_name("vk_swiftshader_icd.json");
    if icd.exists() {
        Ok(icd)
    } else {
        Err(format!("{} is missing", icd.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_warp_pairing_replaces_angles_backend() {
        assert_eq!(electron_switches("native"), [""; 0]);
        for mode in MODES {
            let switches = electron_switches(mode);
            assert_eq!(
                switches.iter().any(|s| s.starts_with("--use-angle")),
                mode == "d3d11-warp",
                "{mode}"
            );
            assert_eq!(
                switches.contains(&"--enable-unsafe-swiftshader"),
                mode != "native"
            );
        }
        assert!(parse("swiftshader").is_ok());
        assert!(parse("lavapipe").is_err());
    }
}
