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

const ADAPTER_VARIABLE: &str = "ASRS_FIELD3D_ADAPTER";

/// The adapter a parity build of the port must use beside a reference of this mode, and the start
/// of its name.
pub(crate) fn port_adapter(mode: &str) -> Option<(&'static str, &'static str)> {
    match mode {
        "swiftshader" => Some(("swiftshader", "SwiftShader Device")),
        "d3d11-warp" => Some(("warp", "Microsoft Basic Render Driver")),
        _ => None,
    }
}

/// Has the port render on the rasterizer Electron's WebGL runs on, for SwiftShader the one in the
/// Electron dist. Two different software rasterizers do not agree at edges, so the port refuses
/// any other adapter.
pub(crate) fn pair_port(paths: &Paths, mode: &str, command: &mut Command) -> Result {
    command.env_remove(ADAPTER_VARIABLE);
    if let Some((adapter, _)) = port_adapter(mode) {
        command.env(ADAPTER_VARIABLE, adapter);
    }
    if mode == "swiftshader" {
        let icd = swiftshader_icd(paths)?;
        if cfg!(windows) {
            // The Vulkan loader in the Electron dist: a machine without a GPU driver has none.
            let path = std::env::var_os("PATH").unwrap_or_default();
            let dist = icd.parent().map(PathBuf::from).unwrap_or_default();
            let dirs = std::iter::once(dist).chain(std::env::split_paths(&path));
            let path = std::env::join_paths(dirs).map_err(|e| e.to_string())?;
            command.env("PATH", path);
        }
        command.env("VK_ICD_FILENAMES", icd);
    }
    Ok(())
}

/// What `pair_port` sets, for the host manifest.
pub(crate) fn port_environment(mode: &str) -> Vec<String> {
    let mut variables = Vec::new();
    if let Some((adapter, _)) = port_adapter(mode) {
        variables.push(format!("{ADAPTER_VARIABLE}={adapter}"));
    }
    if mode == "swiftshader" {
        variables.push("VK_ICD_FILENAMES=<electron dist>/vk_swiftshader_icd.json".to_owned());
    }
    variables
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
