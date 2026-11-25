use std::sync::{Arc, Mutex};

use super::gpu::RenderError;

/// Set to `swiftshader` or `warp`, the device is that software rasterizer or an error.
#[cfg(feature = "parity")]
const VARIABLE: &str = "ASRS_FIELD3D_ADAPTER";

pub(super) struct Opened {
    pub info: wgpu::AdapterInfo,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Whether clip space is GL's: depth from -w to w, y from the first row up.
    pub gl_clip_space: bool,
    /// Why the device was lost, once it is.
    pub lost: Arc<Mutex<Option<String>>>,
}

/// DX12 on Windows, as upstream's WebGL runs on D3D there, and Vulkan only where DX12 has no
/// adapter at all. Vulkan on Linux, and the GL backend gpui compiles in only where no Vulkan
/// device opens: its adapters never come before a Vulkan one.
fn backend_sets() -> Vec<wgpu::Backends> {
    match wgpu::Backends::from_env() {
        Some(backends) => vec![backends],
        None if cfg!(windows) => vec![wgpu::Backends::DX12, wgpu::Backends::VULKAN],
        None if cfg!(target_os = "linux") => vec![wgpu::Backends::VULKAN, wgpu::Backends::GL],
        None => vec![wgpu::Backends::default()],
    }
}

fn adapters(backends: wgpu::Backends) -> Vec<wgpu::Adapter> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
        backends,
        ..wgpu::InstanceDescriptor::new_without_display_handle_from_env()
    });
    pollster::block_on(instance.enumerate_adapters(backends))
}

/// The first adapter whose device opens, software rasterizers last.
pub(super) fn open() -> Result<Opened, RenderError> {
    #[cfg(feature = "parity")]
    if let Ok(wanted) = std::env::var(VARIABLE)
        && !wanted.is_empty()
    {
        return open_software(&wanted);
    }
    let mut failure = RenderError::NoAdapter("none found".to_owned());
    for backends in backend_sets() {
        let mut adapters = adapters(backends);
        adapters.sort_by_key(|a| a.get_info().device_type == wgpu::DeviceType::Cpu);
        for adapter in adapters {
            match open_device(&adapter) {
                Ok(opened) => return Ok(opened),
                Err(e) => {
                    let name = adapter.get_info().name;
                    failure = RenderError::Device(format!("{name}: {e}"));
                }
            }
        }
    }
    Err(failure)
}

#[cfg(feature = "parity")]
fn open_software(wanted: &str) -> Result<Opened, RenderError> {
    let (backends, name) = match wanted {
        "swiftshader" => (wgpu::Backends::VULKAN, "SwiftShader Device"),
        "warp" => (wgpu::Backends::DX12, "Microsoft Basic Render Driver"),
        other => {
            return Err(RenderError::NoAdapter(format!(
                "{VARIABLE} is swiftshader or warp, not {other:?}"
            )));
        }
    };
    let adapters = adapters(backends);
    let found: Vec<String> = adapters.iter().map(|a| a.get_info().name).collect();
    match adapters
        .iter()
        .find(|a| a.get_info().name.starts_with(name))
    {
        Some(adapter) => open_device(adapter),
        None => Err(RenderError::NoAdapter(format!(
            "{VARIABLE}={wanted}: no {name} among {found:?}"
        ))),
    }
}

fn open_device(adapter: &wgpu::Adapter) -> Result<Opened, RenderError> {
    let desc = wgpu::DeviceDescriptor {
        label: Some("field3d"),
        ..Default::default()
    };
    #[cfg(any(target_os = "linux", windows))]
    let gl_clip = super::gl_clip::open(adapter, &desc);
    #[cfg(not(any(target_os = "linux", windows)))]
    let gl_clip: Option<(wgpu::Device, wgpu::Queue)> = None;
    let gl_clip_space = gl_clip.is_some();
    let (device, queue) = match gl_clip {
        Some(opened) => opened,
        None => pollster::block_on(adapter.request_device(&desc))
            .map_err(|e| RenderError::Device(e.to_string()))?,
    };
    let lost = Arc::new(Mutex::new(None));
    let seen = lost.clone();
    device.set_device_lost_callback(move |reason, message| {
        if let Ok(mut lost) = seen.lock() {
            lost.get_or_insert(format!("{reason:?}: {message}"));
        }
    });
    Ok(Opened {
        info: adapter.get_info(),
        device,
        queue,
        gl_clip_space,
        lost,
    })
}
