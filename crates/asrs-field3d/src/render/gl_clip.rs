//! GL's clip space on a Vulkan device, as ANGLE sets it up: depth from -w to w
//! (`VK_EXT_depth_clip_control`) and a viewport that is not flipped. wgpu has neither, and mapping
//! both in the vertex shader rounds differently, which shows up as z-fighting on other pixels and
//! as shadow map texels one step off. So the device wgpu gets has two of its functions wrapped.

#![allow(unsafe_code)]

use std::ffi::CStr;
use std::sync::OnceLock;

use ash::vk;
use wgpu::hal::vulkan;

struct Real {
    create_graphics_pipelines: vk::PFN_vkCreateGraphicsPipelines,
    cmd_set_viewport: vk::PFN_vkCmdSetViewport,
    // wgpu unloads the Vulkan library with its last instance, which would leave the two dangling.
    _library: ash::Entry,
}

/// The loader's own entry points, which dispatch on their handle and so serve every device.
static REAL: OnceLock<Real> = OnceLock::new();

unsafe extern "system" fn create_graphics_pipelines(
    device: vk::Device,
    cache: vk::PipelineCache,
    count: u32,
    infos: *const vk::GraphicsPipelineCreateInfo<'_>,
    allocator: *const vk::AllocationCallbacks<'_>,
    pipelines: *mut vk::Pipeline,
) -> vk::Result {
    let infos = unsafe { std::slice::from_raw_parts(infos, count as usize) };
    let mut clip: Vec<_> = infos
        .iter()
        .map(|_| {
            vk::PipelineViewportDepthClipControlCreateInfoEXT::default().negative_one_to_one(true)
        })
        .collect();
    let viewports: Vec<_> = infos
        .iter()
        .zip(&mut clip)
        .map(|(info, clip)| {
            let mut viewport = unsafe { info.p_viewport_state.as_ref() }
                .copied()
                .unwrap_or_default();
            clip.p_next = viewport.p_next;
            viewport.p_next = std::ptr::from_ref(clip).cast();
            viewport
        })
        .collect();
    let infos: Vec<_> = infos
        .iter()
        .zip(&viewports)
        .map(|(info, viewport)| {
            let mut info = *info;
            if !info.p_viewport_state.is_null() {
                info.p_viewport_state = viewport;
            }
            info
        })
        .collect();
    let real = REAL.get().expect("set before any device has the hook");
    unsafe {
        (real.create_graphics_pipelines)(device, cache, count, infos.as_ptr(), allocator, pipelines)
    }
}

unsafe extern "system" fn cmd_set_viewport(
    commands: vk::CommandBuffer,
    first: u32,
    count: u32,
    viewports: *const vk::Viewport,
) {
    let viewports: Vec<_> = unsafe { std::slice::from_raw_parts(viewports, count as usize) }
        .iter()
        .map(|v| vk::Viewport {
            y: v.y + v.height,
            height: -v.height,
            ..*v
        })
        .collect();
    let real = REAL.get().expect("set before any device has the hook");
    unsafe { (real.cmd_set_viewport)(commands, first, count, viewports.as_ptr()) }
}

/// A device in GL's clip space, or None where the adapter is not Vulkan or lacks the extension.
pub(super) fn open(
    adapter: &wgpu::Adapter,
    desc: &wgpu::DeviceDescriptor<'_>,
) -> Option<(wgpu::Device, wgpu::Queue)> {
    const EXTENSION: &CStr = ash::ext::depth_clip_control::NAME;
    // wgpu checks these only in request_device, which create_device_from_hal skips.
    if !desc.required_limits.check_limits(&adapter.limits())
        || !adapter.features().contains(desc.required_features)
    {
        return None;
    }
    // SAFETY: the raw handles stay inside this function and are destroyed by nothing but the
    // device handed to wgpu, which is created from this adapter with the extensions and features
    // wgpu itself asks for plus one the adapter reports.
    let open = unsafe {
        let hal = adapter.as_hal::<vulkan::Api>()?;
        let shared = hal.shared_instance();
        let device = hal.physical_device_capabilities();
        if !device.supports_extension(EXTENSION)
            || device.properties().api_version < vk::API_VERSION_1_1
            || shared.instance_api_version() < vk::API_VERSION_1_1
        {
            return None;
        }
        let (entry, instance) = (shared.entry(), shared.raw_instance());
        let mut supported = vk::PhysicalDeviceDepthClipControlFeaturesEXT::default();
        let mut query = vk::PhysicalDeviceFeatures2::default().push_next(&mut supported);
        instance.get_physical_device_features2(hal.raw_physical_device(), &mut query);
        if supported.depth_clip_control == vk::FALSE {
            return None;
        }
        let load = |name: &CStr| {
            (entry.static_fn().get_instance_proc_addr)(instance.handle(), name.as_ptr())
        };
        let create = load(c"vkCreateGraphicsPipelines")?;
        let viewport = load(c"vkCmdSetViewport")?;
        REAL.get_or_init(|| Real {
            create_graphics_pipelines: std::mem::transmute::<
                unsafe extern "system" fn(),
                vk::PFN_vkCreateGraphicsPipelines,
            >(create),
            cmd_set_viewport: std::mem::transmute::<
                unsafe extern "system" fn(),
                vk::PFN_vkCmdSetViewport,
            >(viewport),
            _library: entry.clone(),
        });

        let mut extensions = hal.required_device_extensions(desc.required_features);
        extensions.push(EXTENSION);
        let mut features = hal.physical_device_features(&extensions, desc.required_features);
        let mut clip =
            vk::PhysicalDeviceDepthClipControlFeaturesEXT::default().depth_clip_control(true);
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(0)
            .queue_priorities(&[1.0])];
        let names: Vec<_> = extensions.iter().map(|e| e.as_ptr()).collect();
        let info = features.add_to_device_create(
            vk::DeviceCreateInfo::default()
                .queue_create_infos(&queues)
                .enabled_extension_names(&names)
                .push_next(&mut clip),
        );
        let raw = instance
            .create_device(hal.raw_physical_device(), &info, None)
            .ok()?;
        let mut core = raw.fp_v1_0().clone();
        core.create_graphics_pipelines = create_graphics_pipelines;
        core.cmd_set_viewport = cmd_set_viewport;
        let hooked = ash::Device::from_parts_1_3(
            raw.handle(),
            core,
            raw.fp_v1_1().clone(),
            raw.fp_v1_2().clone(),
            raw.fp_v1_3().clone(),
        );
        hal.device_from_raw(
            hooked,
            None,
            &extensions,
            desc.required_features,
            &desc.required_limits,
            &desc.memory_hints,
            0,
            0,
        )
        .ok()?
    };
    // SAFETY: `open` was made from this adapter with the features of `desc`.
    unsafe { adapter.create_device_from_hal::<vulkan::Api>(open, desc) }.ok()
}
