// Ported from three.js r168 (MIT, Copyright 2010-2024 three.js authors). See THIRD_PARTY_NOTICES.md.

//! The wgpu port of the `WebGLRenderer` path upstream's 3D field uses: three's render lists
//! and sorts, `MeshPhongMaterial` in WGSL with the sRGB OETF in the shader over a `Bgra8Unorm`
//! target, instancing, two-pass transparent double-sided draws, `PCFSoftShadowMap` spot-light
//! shadows, `Line2`/`LineMaterial` fat lines, `Line`, `LineSegments` and `GridHelper`, readback,
//! and the render thread that owns the device.

mod edges;
mod frame;
mod gpu;
mod scene;
mod thread;

pub use edges::edge_mask;
pub use frame::{
    Draw, DrawKind, Frame, FrameOptions, HemisphereUniform, Lights, PointUniform, ShadowPass,
    SpotShadowUniform, SpotUniform, build_frame,
};
pub use gpu::{AuxBuffers, COLOR_FORMAT, RenderError, Renderer, Timings};
pub use scene::{
    FatLine, Filter, Geometry, HemisphereLight, Instances, LightShadow, Line, LineBasicMaterial,
    LineMaterial, LineSegmentsGeometry, Mesh, MinFilter, NodeKind, PhongMaterial, PointLight,
    Scene, SceneNode, Side, SpotLight, Texture,
};
pub use thread::{RenderThread, RenderedFrame};

/// BGRA to RGBA in place.
pub fn swap_red_blue(pixels: &mut [u8]) {
    for p in pixels.as_chunks_mut::<4>().0.iter_mut() {
        p.swap(0, 2);
    }
}
