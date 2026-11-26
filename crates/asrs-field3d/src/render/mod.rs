// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

//! The wgpu port of the `WebGLRenderer` path upstream's 3D field uses: three's render lists
//! and sorts, `MeshPhongMaterial` in WGSL with the sRGB OETF in the shader over a `Bgra8Unorm`
//! target, instancing, two-pass transparent double-sided draws, `PCFSoftShadowMap` spot-light
//! shadows, `Line2`/`LineMaterial` fat lines, `Line`, `LineSegments` and `GridHelper`, readback,
//! and the render thread that owns the device.

mod adapter;
mod edges;
mod frame;
#[cfg(any(target_os = "linux", windows))]
mod gl_clip;
mod gpu;
mod mixed;
mod scene;
mod spool;
mod thread;

pub use edges::edge_mask;
pub use frame::{
    Draw, DrawKind, Frame, FrameOptions, HemisphereUniform, Lights, PointUniform, ShadowPass,
    SpotShadowUniform, SpotUniform, build_frame,
};
pub use gpu::{AuxBuffers, COLOR_FORMAT, RenderError, Renderer, Timings};
pub use scene::{
    FatLine, Filter, Geometry, HemisphereLight, Instances, LightShadow, Line, LineBasicMaterial,
    LineMaterial, LineSegmentsGeometry, Materials, Mesh, MinFilter, NodeKind, PhongMaterial,
    PointLight, Scene, SceneNode, Side, SpotLight, Texture,
};
pub use thread::{RenderThread, RenderedFrame};

/// BGRA to RGBA in place.
pub fn swap_red_blue(pixels: &mut [u8]) {
    for p in pixels.as_chunks_mut::<4>().0.iter_mut() {
        p.swap(0, 2);
    }
}

/// Whether the 3D device has to live as long as the app. Vulkan loaders before 1.4.345 can
/// crash when one device is destroyed while another thread walks the device list, which GPUI's
/// own device does on every swapchain change (KhronosGroup/Vulkan-Loader@5ee27b30).
pub fn keep_device() -> bool {
    #[cfg(target_os = "linux")]
    {
        static KEEP: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *KEEP.get_or_init(|| loader_needs_device_kept(gl_clip::loader_version()))
    }
    #[cfg(not(target_os = "linux"))]
    false
}

/// Whether a Vulkan loader of `version` (`vkEnumerateInstanceVersion`) has the bug.
pub fn loader_needs_device_kept(version: Option<u32>) -> bool {
    const FIXED: u32 = (1 << 22) | (4 << 12) | 345;
    version.is_some_and(|v| v < FIXED)
}

#[cfg(test)]
mod tests {
    use super::loader_needs_device_kept;

    #[test]
    fn loaders_before_1_4_345_keep_the_device() {
        let version = |minor: u32, patch: u32| Some((1 << 22) | (minor << 12) | patch);
        assert!(loader_needs_device_kept(version(3, 204)));
        assert!(loader_needs_device_kept(version(3, 275)));
        assert!(loader_needs_device_kept(version(4, 344)));
        assert!(!loader_needs_device_kept(version(4, 345)));
        assert!(!loader_needs_device_kept(version(4, 363)));
        assert!(!loader_needs_device_kept(None));
    }
}
