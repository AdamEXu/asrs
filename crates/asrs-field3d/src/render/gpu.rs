use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::adapter::Opened;
use super::frame::{Draw, DrawKind, Frame};
use super::scene::{Filter, Geometry, LineSegmentsGeometry, MinFilter, Side, Texture};
use crate::three::{ArrayData, BufferAttribute, Group};

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
/// Ids of the multisampled aux pass, packed into the bytes of a colour: `R32Uint` cannot be
/// multisampled.
const SAMPLE_ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DRAW_STRIDE: u64 = 256;
const MAX_HEMI: usize = 4;
const MAX_POINT: usize = 8;
const MAX_SPOT: usize = 8;
const MAX_SPOT_SHADOWS: usize = 4;

/// `LineSegmentsGeometry`'s instanced quad: two cap rows around the segment's two ends.
const LINE_QUAD_POSITIONS: [f32; 24] = [
    -1.0, 2.0, 0.0, 1.0, 2.0, 0.0, -1.0, 1.0, 0.0, 1.0, 1.0, 0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0,
    -1.0, -1.0, 0.0, 1.0, -1.0, 0.0,
];
const LINE_QUAD_UVS: [f32; 16] = [
    -1.0, 2.0, 1.0, 2.0, -1.0, 1.0, 1.0, 1.0, -1.0, -1.0, 1.0, -1.0, -1.0, -2.0, 1.0, -2.0,
];
const LINE_QUAD_INDEX: [u16; 18] = [0, 2, 1, 2, 3, 1, 2, 4, 3, 4, 5, 3, 4, 6, 5, 6, 7, 5];

#[derive(Debug)]
pub enum RenderError {
    NoAdapter(String),
    Device(String),
    /// The device is gone: open another renderer.
    Lost(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::NoAdapter(e) => write!(f, "no graphics adapter: {e}"),
            RenderError::Device(e) => write!(f, "device request failed: {e}"),
            RenderError::Lost(e) => write!(f, "the graphics device was lost: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct HemiU {
    direction: [f32; 4],
    sky: [f32; 4],
    ground: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PointU {
    position: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SpotU {
    position: [f32; 4],
    direction: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SpotShadowU {
    matrix: [f32; 16],
    params: [f32; 4],
    map_size: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameU {
    projection: [f32; 16],
    counts: [u32; 4],
    clip_factors: [f32; 4],
    hemi: [HemiU; MAX_HEMI],
    point: [PointU; MAX_POINT],
    spot: [SpotU; MAX_SPOT],
    view: [f32; 16],
    shadow_counts: [u32; 4],
    spot_shadow: [SpotShadowU; MAX_SPOT_SHADOWS],
}

const FRAME_STRIDE: u64 = size_of::<FrameU>().div_ceil(256) as u64 * 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawU {
    model_view: [f32; 16],
    normal_matrix: [f32; 12],
    diffuse_opacity: [f32; 4],
    emissive: [f32; 4],
    specular_shininess: [f32; 4],
    flags: [u32; 4],
    extra: [u32; 4],
    model: [f32; 16],
}

const _: () = assert!(size_of::<DrawU>() as u64 <= DRAW_STRIDE);

fn pad3(v: [f32; 3], w: f32) -> [f32; 4] {
    [v[0], v[1], v[2], w]
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Program {
    Phong,
    Depth,
    FatLine,
    Lines,
    LineStrip,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PipeKey {
    program: Program,
    cull: bool,
    front_cw: bool,
    blend: bool,
    depth_test: bool,
    depth_write: bool,
    instanced: bool,
    float_color: bool,
    aux: bool,
    samples: u32,
    strip_index_format: Option<wgpu::IndexFormat>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Color,
    Aux,
    AuxSamples,
    Shadow,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Primitives {
    Triangles,
    Lines,
    LineStrip,
}

struct GpuGeometry {
    position: wgpu::Buffer,
    normal: wgpu::Buffer,
    color: wgpu::Buffer,
    float_color: bool,
    uv: wgpu::Buffer,
    index: Option<(wgpu::Buffer, wgpu::IndexFormat, u32)>,
    vertex_count: u32,
    color_components: u32,
}

struct LineQuad {
    position: wgpu::Buffer,
    uv: wgpu::Buffer,
    index: wgpu::Buffer,
}

struct GpuTexture {
    bind_group: wgpu::BindGroup,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Timings {
    pub encode_ms: f64,
    pub gpu_and_map_ms: f64,
    pub copy_ms: f64,
}

/// Per-pixel object ids and depth from a single-sample pass over the same draws, for the edge
/// mask of the 3D pixel gate. A pixel holds `(object id + 1) * 2 + front facing`; 0 is background.
/// `mixed` marks the pixels whose samples hold more than one id when the same draws are rendered
/// with the frame's sample count.
pub struct AuxBuffers {
    pub width: u32,
    pub height: u32,
    pub opaque_ids: Vec<u32>,
    pub transparent_ids: Vec<u32>,
    pub depth: Vec<f32>,
    pub mixed: Vec<bool>,
}

struct Targets {
    width: u32,
    height: u32,
    samples: u32,
    msaa: Option<wgpu::Texture>,
    resolve: wgpu::Texture,
    depth: wgpu::Texture,
    readback: wgpu::Buffer,
}

/// One RGBA8 layer per shadow-casting spot light (`WebGLRenderTarget` with nearest filtering),
/// the depth buffer they share, and the per-light frame uniforms of the depth pass.
struct ShadowMaps {
    size: [u32; 2],
    layers: u32,
    texture: wgpu::Texture,
    depth: wgpu::Texture,
    frame_buffer: wgpu::Buffer,
    frame_groups: Vec<wgpu::BindGroup>,
}

/// The wgpu half of `WebGLRenderer`: its own device, an offscreen `Bgra8Unorm` target (MSAA 4x
/// when antialiased) and a readback of every frame, with the shadow maps rendered first when
/// the frame has shadow passes. Geometry and textures are uploaded once per key and dropped
/// when a frame no longer uses them.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    lost: Arc<Mutex<Option<String>>>,
    adapter_info: wgpu::AdapterInfo,
    provoking_last: bool,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    depth_layout: wgpu::PipelineLayout,
    pipelines: HashMap<PipeKey, wgpu::RenderPipeline>,
    frame_layout: wgpu::BindGroupLayout,
    depth_frame_layout: wgpu::BindGroupLayout,
    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    shadow_sampler: wgpu::Sampler,
    blank_shadow_maps: wgpu::Texture,
    draw_layout: wgpu::BindGroupLayout,
    draw_buffer: wgpu::Buffer,
    draw_group: wgpu::BindGroup,
    draw_capacity: u64,
    texture_layout: wgpu::BindGroupLayout,
    blank_texture: wgpu::BindGroup,
    instance_buffer: Option<wgpu::Buffer>,
    line_quad: LineQuad,
    geometries: HashMap<(u64, Primitives), GpuGeometry>,
    line_geometries: HashMap<u64, (wgpu::Buffer, u32)>,
    textures: HashMap<u64, GpuTexture>,
    shadow_maps: Option<ShadowMaps>,
    targets: Option<Targets>,
}

impl Renderer {
    pub fn new() -> Result<Self, RenderError> {
        let Opened {
            info: adapter_info,
            device,
            queue,
            gl_clip_space,
            lost,
        } = super::adapter::open()?;
        let source = [
            include_str!("phong.wgsl"),
            match adapter_info.backend {
                wgpu::Backend::Vulkan => include_str!("vulkan.wgsl"),
                wgpu::Backend::Dx12 => include_str!("dx12.wgsl"),
                _ => include_str!("metal.wgsl"),
            },
            &format!("const GL_CLIP_SPACE = {gl_clip_space};"),
            include_str!("shadow.wgsl"),
            include_str!("lines.wgsl"),
        ]
        .join("\n");
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("field3d"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let uniform = |dynamic| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                uniform(false),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
            ],
        });
        let depth_frame_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[uniform(false)],
            });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[uniform(true)],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[
                Some(&frame_layout),
                Some(&draw_layout),
                Some(&texture_layout),
            ],
            immediate_size: 0,
        });
        let depth_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&depth_frame_layout), Some(&draw_layout)],
            immediate_size: 0,
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: size_of::<FrameU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let blank_shadow_maps = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let frame_group = make_frame_group(
            &device,
            &frame_layout,
            &frame_buffer,
            &blank_shadow_maps,
            &shadow_sampler,
        );
        let (draw_buffer, draw_group) = make_draw_buffer(&device, &draw_layout, 64);
        let blank = Texture::new(1, 1, vec![255u8; 4].into());
        let blank_texture = upload_texture(&device, &queue, &texture_layout, &blank).bind_group;
        let init = |contents: &[u8], usage| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents,
                usage,
            })
        };
        let provoking_last = adapter_info.backend == wgpu::Backend::Vulkan;
        let quad_index: Vec<u16> = if provoking_last {
            let indices = LINE_QUAD_INDEX.map(u32::from);
            provoking_first(Primitives::Triangles, &indices)
                .iter()
                .map(|&i| i as u16)
                .collect()
        } else {
            LINE_QUAD_INDEX.to_vec()
        };
        let line_quad = LineQuad {
            position: init(
                bytemuck::cast_slice(&LINE_QUAD_POSITIONS),
                wgpu::BufferUsages::VERTEX,
            ),
            uv: init(
                bytemuck::cast_slice(&LINE_QUAD_UVS),
                wgpu::BufferUsages::VERTEX,
            ),
            index: init(bytemuck::cast_slice(&quad_index), wgpu::BufferUsages::INDEX),
        };
        Ok(Renderer {
            device,
            queue,
            lost,
            provoking_last,
            adapter_info,
            shader,
            layout,
            depth_layout,
            pipelines: HashMap::new(),
            frame_layout,
            depth_frame_layout,
            frame_buffer,
            frame_group,
            shadow_sampler,
            blank_shadow_maps,
            draw_layout,
            draw_buffer,
            draw_group,
            draw_capacity: 64,
            texture_layout,
            blank_texture,
            instance_buffer: None,
            line_quad,
            geometries: HashMap::new(),
            line_geometries: HashMap::new(),
            textures: HashMap::new(),
            shadow_maps: None,
            targets: None,
        })
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    /// Loses the device, as a driver reset would.
    #[cfg(feature = "parity")]
    pub fn lose_device(&self) {
        self.device.destroy();
    }

    /// The bytes of a buffer whose mapping was requested before the last poll.
    fn mapped(&self, buffer: &wgpu::Buffer) -> Result<wgpu::BufferView, RenderError> {
        let lost = || self.lost.lock().ok().and_then(|l| l.clone());
        buffer.slice(..).get_mapped_range().map_err(|e| {
            RenderError::Lost(lost().unwrap_or_else(|| format!("no frame to read back ({e})")))
        })
    }

    fn pipeline(&mut self, key: PipeKey) -> &wgpu::RenderPipeline {
        let device = &self.device;
        let shader = &self.shader;
        let layout = if key.program == Program::Depth {
            &self.depth_layout
        } else {
            &self.layout
        };
        self.pipelines.entry(key).or_insert_with(|| {
            const POSITION: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];
            const NORMAL: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x3];
            const COLOR_U8: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![2 => Unorm8x4];
            const COLOR_F32: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![2 => Float32x4];
            const UV: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![3 => Float32x2];
            const INSTANCE: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
                4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4
            ];
            const SEGMENT: [wgpu::VertexAttribute; 2] =
                wgpu::vertex_attr_array![8 => Float32x3, 9 => Float32x3];
            let vertex = |stride, attributes: &'static [wgpu::VertexAttribute]| {
                Some(wgpu::VertexBufferLayout {
                    array_stride: stride,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes,
                })
            };
            let instance = |stride, attributes: &'static [wgpu::VertexAttribute]| {
                Some(wgpu::VertexBufferLayout {
                    array_stride: stride,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes,
                })
            };
            let buffers = if key.program == Program::FatLine {
                vec![
                    vertex(12, &POSITION),
                    vertex(8, &UV),
                    instance(24, &SEGMENT),
                ]
            } else {
                let mut buffers = vec![
                    vertex(12, &POSITION),
                    vertex(12, &NORMAL),
                    if key.float_color {
                        vertex(16, &COLOR_F32)
                    } else {
                        vertex(4, &COLOR_U8)
                    },
                    vertex(8, &UV),
                ];
                if key.instanced {
                    buffers.push(instance(64, &INSTANCE));
                }
                buffers
            };
            let blend = key.blend.then_some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
            });
            let targets: Vec<Option<wgpu::ColorTargetState>> = if key.aux {
                let mask = |on: bool| {
                    if on {
                        wgpu::ColorWrites::ALL
                    } else {
                        wgpu::ColorWrites::empty()
                    }
                };
                let format = if key.samples > 1 {
                    SAMPLE_ID_FORMAT
                } else {
                    ID_FORMAT
                };
                vec![
                    Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: mask(!key.blend),
                    }),
                    Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: mask(key.blend),
                    }),
                ]
            } else {
                vec![Some(wgpu::ColorTargetState {
                    format: if key.program == Program::Depth {
                        SHADOW_FORMAT
                    } else {
                        COLOR_FORMAT
                    },
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })]
            };
            let fragment = |color, aux, aux_samples| match (key.aux, key.samples) {
                (false, _) => color,
                (true, 1) => aux,
                (true, _) => aux_samples,
            };
            let (vertex_entry, fragment_entry) = match key.program {
                Program::Phong => (
                    if key.instanced {
                        "vs_instanced"
                    } else {
                        "vs_mesh"
                    },
                    fragment("fs", "fs_aux", "fs_aux_samples"),
                ),
                Program::Depth => (
                    if key.instanced {
                        "vs_depth_instanced"
                    } else {
                        "vs_depth"
                    },
                    "fs_depth",
                ),
                Program::FatLine => (
                    "vs_fat_line",
                    fragment("fs_fat_line", "fs_fat_line_aux", "fs_fat_line_aux_samples"),
                ),
                Program::Lines | Program::LineStrip => (
                    "vs_basic",
                    fragment("fs_basic", "fs_basic_aux", "fs_basic_aux_samples"),
                ),
            };
            let topology = match key.program {
                Program::Lines => wgpu::PrimitiveTopology::LineList,
                Program::LineStrip => wgpu::PrimitiveTopology::LineStrip,
                _ => wgpu::PrimitiveTopology::TriangleList,
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(vertex_entry),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(fragment_entry),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    front_face: if key.front_cw {
                        wgpu::FrontFace::Ccw
                    } else {
                        wgpu::FrontFace::Cw
                    },
                    cull_mode: key.cull.then_some(wgpu::Face::Back),
                    strip_index_format: key.strip_index_format,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(key.depth_test && key.depth_write),
                    depth_compare: Some(if key.depth_test {
                        wgpu::CompareFunction::LessEqual
                    } else {
                        wgpu::CompareFunction::Always
                    }),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: key.samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        })
    }

    fn ensure_targets(&mut self, width: u32, height: u32, samples: u32) {
        if let Some(t) = &self.targets
            && t.width == width
            && t.height == height
            && t.samples == samples
        {
            return;
        }
        let resolve = self.texture(
            width,
            height,
            COLOR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        );
        let msaa = (samples > 1).then(|| {
            self.texture(
                width,
                height,
                COLOR_FORMAT,
                samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
        });
        let depth = self.texture(
            width,
            height,
            DEPTH_FORMAT,
            samples,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded_row(width) * height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.targets = Some(Targets {
            width,
            height,
            samples,
            msaa,
            resolve,
            depth,
            readback,
        });
    }

    /// Shadow maps share one size, as upstream's 2048 x 2048 for every spot light; a frame whose
    /// lights disagree uses the first light's size for all.
    fn ensure_shadow_maps(&mut self, frame: &Frame) {
        let Some(first) = frame.shadows.first() else {
            if self.shadow_maps.take().is_some() {
                self.frame_group = make_frame_group(
                    &self.device,
                    &self.frame_layout,
                    &self.frame_buffer,
                    &self.blank_shadow_maps,
                    &self.shadow_sampler,
                );
            }
            return;
        };
        let size = first.map_size.map(|v| v.max(1));
        let layers = frame.shadows.len().min(MAX_SPOT_SHADOWS) as u32;
        if let Some(m) = &self.shadow_maps
            && m.size == size
            && m.layers == layers
        {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow maps"),
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: SHADOW_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth = self.texture(
            size[0],
            size[1],
            DEPTH_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let frame_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: FRAME_STRIDE * u64::from(layers),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_groups = (0..u64::from(layers))
            .map(|layer| {
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.depth_frame_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &frame_buffer,
                            offset: layer * FRAME_STRIDE,
                            size: wgpu::BufferSize::new(size_of::<FrameU>() as u64),
                        }),
                    }],
                })
            })
            .collect();
        self.frame_group = make_frame_group(
            &self.device,
            &self.frame_layout,
            &self.frame_buffer,
            &texture,
            &self.shadow_sampler,
        );
        self.shadow_maps = Some(ShadowMaps {
            size,
            layers,
            texture,
            depth,
            frame_buffer,
            frame_groups,
        });
    }

    fn texture(
        &self,
        width: u32,
        height: u32,
        format: wgpu::TextureFormat,
        samples: u32,
        usage: wgpu::TextureUsages,
    ) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    }

    fn sync_resources(&mut self, frame: &Frame) {
        let mut used_geometries = HashSet::new();
        let mut used_lines = HashSet::new();
        let mut used_textures = HashSet::new();
        for draw in all_draws(frame) {
            match &draw.kind {
                DrawKind::Mesh {
                    geometry, material, ..
                } => {
                    self.sync_geometry(geometry, Primitives::Triangles, &mut used_geometries);
                    if let Some(texture) = &material.map {
                        used_textures.insert(texture.key());
                        if !self.textures.contains_key(&texture.key()) {
                            let gpu = upload_texture(
                                &self.device,
                                &self.queue,
                                &self.texture_layout,
                                texture,
                            );
                            self.textures.insert(texture.key(), gpu);
                        }
                    }
                }
                DrawKind::Line {
                    geometry, segments, ..
                } => {
                    self.sync_geometry(geometry, line_primitives(*segments), &mut used_geometries);
                }
                DrawKind::FatLine { geometry, .. } => {
                    used_lines.insert(geometry.key());
                    if !self.line_geometries.contains_key(&geometry.key()) {
                        let gpu = upload_segments(&self.device, geometry);
                        self.line_geometries.insert(geometry.key(), gpu);
                    }
                }
            }
        }
        self.geometries.retain(|k, _| used_geometries.contains(k));
        self.line_geometries.retain(|k, _| used_lines.contains(k));
        self.textures.retain(|k, _| used_textures.contains(k));
    }

    fn sync_geometry(
        &mut self,
        geometry: &Geometry,
        primitives: Primitives,
        used: &mut HashSet<(u64, Primitives)>,
    ) {
        let key = (geometry.key(), primitives);
        used.insert(key);
        if !self.geometries.contains_key(&key) {
            let gpu = upload_geometry(
                &self.device,
                geometry,
                self.provoking_last.then_some(primitives),
            );
            self.geometries.insert(key, gpu);
        }
    }

    fn frame_uniform(&self, frame: &Frame) -> FrameU {
        let lights = &frame.lights;
        let mut frame_u = FrameU::zeroed();
        frame_u.projection = frame.projection;
        frame_u.view = frame.view;
        frame_u.clip_factors = [-1.0, 1.0, 0.0, 0.0];
        frame_u.counts = [
            lights.hemisphere.len().min(MAX_HEMI) as u32,
            lights.point.len().min(MAX_POINT) as u32,
            lights.spot.len().min(MAX_SPOT) as u32,
            // opaque() in phong.wgsl XORs with this word, so it has to stay 0.
            0,
        ];
        let shadows = lights
            .spot_shadow
            .len()
            .min(frame.shadows.len())
            .min(MAX_SPOT_SHADOWS);
        frame_u.shadow_counts = [shadows as u32, 0, 0, 0];
        for (slot, l) in frame_u.hemi.iter_mut().zip(&lights.hemisphere) {
            *slot = HemiU {
                direction: pad3(l.direction, 0.0),
                sky: pad3(l.sky_color, 0.0),
                ground: pad3(l.ground_color, 0.0),
            };
        }
        for (slot, l) in frame_u.point.iter_mut().zip(&lights.point) {
            *slot = PointU {
                position: pad3(l.position, 1.0),
                color: pad3(l.color, 0.0),
                params: [l.distance, l.decay, 0.0, 0.0],
            };
        }
        for (slot, l) in frame_u.spot.iter_mut().zip(&lights.spot) {
            *slot = SpotU {
                position: pad3(l.position, 1.0),
                direction: pad3(l.direction, 0.0),
                color: pad3(l.color, 0.0),
                params: [l.distance, l.decay, l.cone_cos, l.penumbra_cos],
            };
        }
        for (slot, s) in frame_u.spot_shadow.iter_mut().zip(&lights.spot_shadow) {
            *slot = SpotShadowU {
                matrix: s.matrix,
                params: [s.intensity, s.bias, s.normal_bias, s.radius],
                map_size: [s.map_size[0], s.map_size[1], 0.0, 0.0],
            };
        }
        frame_u
    }

    /// Writes the frame uniforms, the shadow passes' frame uniforms, and one draw uniform per
    /// draw of [`all_draws`], returning each draw's offset into the instance buffer.
    fn write_uniforms(&mut self, frame: &Frame) -> Vec<Option<u64>> {
        let frame_u = self.frame_uniform(frame);
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame_u));
        if let Some(maps) = &self.shadow_maps {
            for (layer, pass) in frame.shadows.iter().take(maps.layers as usize).enumerate() {
                let mut shadow_u = frame_u;
                shadow_u.projection = pass.projection;
                self.queue.write_buffer(
                    &maps.frame_buffer,
                    layer as u64 * FRAME_STRIDE,
                    bytemuck::bytes_of(&shadow_u),
                );
            }
        }

        let draws: Vec<&Draw> = all_draws(frame).collect();
        let count = draws.len() as u64;
        if count > self.draw_capacity {
            self.draw_capacity = count.next_power_of_two();
            let (buffer, group) =
                make_draw_buffer(&self.device, &self.draw_layout, self.draw_capacity);
            self.draw_buffer = buffer;
            self.draw_group = group;
        }
        let mut data = vec![0u8; (count * DRAW_STRIDE) as usize];
        let mut instances: Vec<[f32; 16]> = Vec::new();
        let mut instance_offsets = Vec::with_capacity(draws.len());
        for (n, draw) in draws.iter().enumerate() {
            let d = draw_uniform(draw, &self.geometries);
            let o = n * DRAW_STRIDE as usize;
            data[o..o + size_of::<DrawU>()].copy_from_slice(bytemuck::bytes_of(&d));
            let matrices = match &draw.kind {
                DrawKind::Mesh { instances, .. } => instances.as_ref(),
                _ => None,
            };
            instance_offsets.push(matrices.map(|m| {
                let offset = (instances.len() * 64) as u64;
                instances.extend_from_slice(m);
                offset
            }));
        }
        self.queue.write_buffer(&self.draw_buffer, 0, &data);
        if !instances.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&instances);
            let too_small = self
                .instance_buffer
                .as_ref()
                .is_none_or(|b| b.size() < bytes.len() as u64);
            if too_small {
                self.instance_buffer = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("instances"),
                    size: (bytes.len() as u64).next_power_of_two(),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            if let Some(buffer) = &self.instance_buffer {
                self.queue.write_buffer(buffer, 0, bytes);
            }
        }
        instance_offsets
    }

    fn pipe_key(&self, draw: &Draw, pass: Pass, samples: u32) -> PipeKey {
        let kind = &draw.kind;
        let (program, instanced, float_color) = match kind {
            DrawKind::Mesh {
                geometry,
                instances,
                ..
            } => (
                if pass == Pass::Shadow {
                    Program::Depth
                } else {
                    Program::Phong
                },
                instances.is_some(),
                self.geometries[&(geometry.key(), Primitives::Triangles)].float_color,
            ),
            DrawKind::FatLine { .. } => (Program::FatLine, false, false),
            DrawKind::Line {
                geometry, segments, ..
            } => (
                if *segments || self.provoking_last {
                    Program::Lines
                } else {
                    Program::LineStrip
                },
                false,
                self.geometries[&(geometry.key(), line_primitives(*segments))].float_color,
            ),
        };
        let is_line = matches!(program, Program::Lines | Program::LineStrip);
        let shadow = pass == Pass::Shadow;
        let strip_index_format = match kind {
            DrawKind::Line { geometry, .. } if program == Program::LineStrip => self.geometries
                [&(geometry.key(), Primitives::LineStrip)]
                .index
                .as_ref()
                .map(|(_, format, _)| *format),
            _ => None,
        };
        PipeKey {
            program,
            cull: !is_line && draw.side != Side::Double,
            front_cw: (draw.side == Side::Back) != draw.front_face_cw,
            blend: !shadow && kind.transparent(),
            depth_test: shadow || kind.depth_test(),
            depth_write: shadow || kind.depth_write(),
            instanced,
            float_color,
            aux: matches!(pass, Pass::Aux | Pass::AuxSamples),
            samples: match pass {
                Pass::Color | Pass::AuxSamples => samples,
                Pass::Aux | Pass::Shadow => 1,
            },
            strip_index_format,
        }
    }

    /// Encodes `draws` (which start at index `first` of [`all_draws`]) into one render pass.
    #[allow(clippy::too_many_arguments)]
    fn encode_draws(
        &mut self,
        draws: &[Draw],
        first: usize,
        samples: u32,
        instance_offsets: &[Option<u64>],
        pass_kind: Pass,
        viewport: Option<[u32; 2]>,
        frame_group: Option<&wgpu::BindGroup>,
        pass_desc: &wgpu::RenderPassDescriptor<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let keys: Vec<PipeKey> = draws
            .iter()
            .map(|d| self.pipe_key(d, pass_kind, samples))
            .collect();
        for &key in &keys {
            self.pipeline(key);
        }
        let mut pass = encoder.begin_render_pass(pass_desc);
        if let Some([width, height]) = viewport {
            pass.set_viewport(0.0, 0.0, width as f32, height as f32, 0.0, 1.0);
        }
        pass.set_bind_group(0, frame_group.unwrap_or(&self.frame_group), &[]);
        for (n, draw) in draws.iter().enumerate() {
            let index = first + n;
            pass.set_pipeline(&self.pipelines[&keys[n]]);
            pass.set_bind_group(1, &self.draw_group, &[(index as u64 * DRAW_STRIDE) as u32]);
            match &draw.kind {
                DrawKind::Mesh {
                    geometry,
                    material,
                    instances,
                    group,
                } => {
                    if instances.as_ref().is_some_and(|m| m.is_empty()) {
                        continue;
                    }
                    if pass_kind != Pass::Shadow {
                        let texture = material
                            .map
                            .as_ref()
                            .map_or(&self.blank_texture, |t| &self.textures[&t.key()].bind_group);
                        pass.set_bind_group(2, texture, &[]);
                    }
                    let geometry = &self.geometries[&(geometry.key(), Primitives::Triangles)];
                    let mut instance_count = 1;
                    if let (Some(offset), Some(matrices), Some(buffer)) =
                        (instance_offsets[index], instances, &self.instance_buffer)
                    {
                        pass.set_vertex_buffer(
                            4,
                            buffer.slice(offset..offset + matrices.len() as u64 * 64),
                        );
                        instance_count = matrices.len() as u32;
                    }
                    draw_geometry(&mut pass, geometry, *group, instance_count);
                }
                DrawKind::Line {
                    geometry, segments, ..
                } => {
                    pass.set_bind_group(2, &self.blank_texture, &[]);
                    let key = (geometry.key(), line_primitives(*segments));
                    draw_geometry(&mut pass, &self.geometries[&key], None, 1);
                }
                DrawKind::FatLine { geometry, .. } => {
                    let (segments, count) = &self.line_geometries[&geometry.key()];
                    if *count == 0 {
                        continue;
                    }
                    pass.set_bind_group(2, &self.blank_texture, &[]);
                    pass.set_vertex_buffer(0, self.line_quad.position.slice(..));
                    pass.set_vertex_buffer(1, self.line_quad.uv.slice(..));
                    pass.set_vertex_buffer(2, segments.slice(..));
                    pass.set_index_buffer(
                        self.line_quad.index.slice(..),
                        wgpu::IndexFormat::Uint16,
                    );
                    pass.draw_indexed(0..LINE_QUAD_INDEX.len() as u32, 0, 0..*count);
                }
            }
        }
    }

    /// `WebGLShadowMap.render`: each light's layer cleared to white with a cleared depth buffer,
    /// then its casters with `MeshDepthMaterial`.
    fn encode_shadow_passes(
        &mut self,
        frame: &Frame,
        instance_offsets: &[Option<u64>],
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let Some(maps) = self.shadow_maps.take() else {
            return;
        };
        let depth_view = maps.depth.create_view(&Default::default());
        let mut first = frame.draws.len();
        for (layer, pass) in frame.shadows.iter().enumerate() {
            if layer as u32 >= maps.layers {
                break;
            }
            let view = maps.texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: layer as u32,
                array_layer_count: Some(1),
                ..Default::default()
            });
            let color_attachments = [Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                    store: wgpu::StoreOp::Store,
                },
            })];
            let desc = wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &color_attachments,
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            };
            self.encode_draws(
                &pass.draws,
                first,
                1,
                instance_offsets,
                Pass::Shadow,
                None,
                Some(&maps.frame_groups[layer]),
                &desc,
                encoder,
            );
            first += pass.draws.len();
        }
        self.shadow_maps = Some(maps);
    }

    /// Renders `frame` and writes its pixels to `out` as tightly packed BGRA rows, top row
    /// first, which is `gl.readPixels` flipped to image order.
    pub fn render(&mut self, frame: &Frame, out: &mut Vec<u8>) -> Result<Timings, RenderError> {
        let t0 = Instant::now();
        let FrameOptionsSize {
            width,
            height,
            samples,
        } = frame_size(frame);
        self.ensure_targets(width, height, samples);
        self.ensure_shadow_maps(frame);
        self.sync_resources(frame);
        let instance_offsets = self.write_uniforms(frame);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode_shadow_passes(frame, &instance_offsets, &mut encoder);
        let targets = self.targets.take().expect("targets");
        let resolve_view = targets.resolve.create_view(&Default::default());
        let msaa_view = targets
            .msaa
            .as_ref()
            .map(|t| t.create_view(&Default::default()));
        let depth_view = targets.depth.create_view(&Default::default());
        let [r, g, b, a] = frame.clear;
        {
            let (view, resolve_target) = match &msaa_view {
                Some(m) => (m, Some(&resolve_view)),
                None => (&resolve_view, None),
            };
            let color_attachments = [Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                    store: if msaa_view.is_some() {
                        wgpu::StoreOp::Discard
                    } else {
                        wgpu::StoreOp::Store
                    },
                },
            })];
            let desc = wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &color_attachments,
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            };
            self.encode_draws(
                &frame.draws,
                0,
                samples,
                &instance_offsets,
                Pass::Color,
                Some(gl_viewport(frame)),
                None,
                &desc,
                &mut encoder,
            );
        }
        let padded = padded_row(width);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &targets.resolve,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &targets.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            extent(width, height),
        );
        self.queue.submit([encoder.finish()]);
        let t1 = Instant::now();
        targets
            .readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let t2 = Instant::now();
        {
            let data = self.mapped(&targets.readback)?;
            copy_rows(
                &data,
                padded as usize,
                (width * 4) as usize,
                height as usize,
                out,
            );
        }
        targets.readback.unmap();
        self.targets = Some(targets);
        let t3 = Instant::now();
        Ok(Timings {
            encode_ms: (t1 - t0).as_secs_f64() * 1e3,
            gpu_and_map_ms: (t2 - t1).as_secs_f64() * 1e3,
            copy_ms: (t3 - t2).as_secs_f64() * 1e3,
        })
    }

    /// The RGBA8 texels of the last frame's shadow map for `layer`, row 0 first (GL's bottom
    /// row, as `readRenderTargetPixels` returns it).
    pub fn shadow_map(&mut self, layer: u32) -> Option<(u32, u32, Vec<u8>)> {
        let maps = self.shadow_maps.as_ref()?;
        if layer >= maps.layers {
            return None;
        }
        let [width, height] = maps.size;
        let padded = padded_row(width);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded * height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &maps.texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            extent(width, height),
        );
        self.queue.submit([encoder.finish()]);
        buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        {
            let data = self.mapped(&buffer).expect("mapped shadow map");
            for row in data.chunks(padded as usize).take(height as usize) {
                out.extend_from_slice(&row[..(width * 4) as usize]);
            }
        }
        buffer.unmap();
        Some((width, height, out))
    }

    /// The id and depth passes behind the edge mask, single-sample and, for a multisampled frame,
    /// with the frame's samples. Opaque draws write the first id target and depth; transparent
    /// draws only the second id target, depth tested.
    pub fn render_aux(&mut self, frame: &Frame) -> AuxBuffers {
        let FrameOptionsSize {
            width,
            height,
            samples,
        } = frame_size(frame);
        self.ensure_shadow_maps(frame);
        self.sync_resources(frame);
        let instance_offsets = self.write_uniforms(frame);
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let opaque = self.texture(width, height, ID_FORMAT, 1, usage);
        let transparent = self.texture(width, height, ID_FORMAT, 1, usage);
        let depth = self.texture(width, height, DEPTH_FORMAT, 1, usage);
        let mixed = self.texture(width, height, ID_FORMAT, 1, usage);
        let mut encoder = self.device.create_command_encoder(&Default::default());
        self.encode_aux(
            frame,
            &instance_offsets,
            Pass::Aux,
            1,
            [&opaque, &transparent, &depth],
            &mut encoder,
        );
        if samples > 1 {
            let usage =
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            let ids =
                [(); 2].map(|()| self.texture(width, height, SAMPLE_ID_FORMAT, samples, usage));
            let sample_depth = self.texture(
                width,
                height,
                DEPTH_FORMAT,
                samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            self.encode_aux(
                frame,
                &instance_offsets,
                Pass::AuxSamples,
                samples,
                [&ids[0], &ids[1], &sample_depth],
                &mut encoder,
            );
            super::mixed::encode(&self.device, &mut encoder, &ids, &mixed);
        }
        let padded = padded_row(width);
        let buffers: Vec<wgpu::Buffer> = (0..4)
            .map(|_| {
                self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: u64::from(padded * height),
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        for (texture, aspect, buffer) in [
            (&opaque, wgpu::TextureAspect::All, &buffers[0]),
            (&transparent, wgpu::TextureAspect::All, &buffers[1]),
            (&depth, wgpu::TextureAspect::DepthOnly, &buffers[2]),
            (&mixed, wgpu::TextureAspect::All, &buffers[3]),
        ] {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded),
                        rows_per_image: Some(height),
                    },
                },
                extent(width, height),
            );
        }
        self.queue.submit([encoder.finish()]);
        for b in &buffers {
            b.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        }
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let read = |b: &wgpu::Buffer| {
            let mut bytes = Vec::new();
            {
                let data = self.mapped(b).expect("mapped aux readback");
                copy_rows(
                    &data,
                    padded as usize,
                    (width * 4) as usize,
                    height as usize,
                    &mut bytes,
                );
            }
            b.unmap();
            bytes
        };
        let u32s = |bytes: Vec<u8>| -> Vec<u32> {
            bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|c| u32::from_ne_bytes(*c))
                .collect()
        };
        let opaque_ids = u32s(read(&buffers[0]));
        let transparent_ids = u32s(read(&buffers[1]));
        let depth = read(&buffers[2])
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_ne_bytes(*c))
            .collect();
        let mixed = u32s(read(&buffers[3])).iter().map(|&v| v != 0).collect();
        AuxBuffers {
            width,
            height,
            opaque_ids,
            transparent_ids,
            depth,
            mixed,
        }
    }

    fn encode_aux(
        &mut self,
        frame: &Frame,
        instance_offsets: &[Option<u64>],
        pass: Pass,
        samples: u32,
        [opaque, transparent, depth]: [&wgpu::Texture; 3],
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let views = [opaque, transparent].map(|t| t.create_view(&Default::default()));
        let depth_view = depth.create_view(&Default::default());
        let color_attachments = views.each_ref().map(|view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })
        });
        let desc = wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &color_attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        };
        self.encode_draws(
            &frame.draws,
            0,
            samples,
            instance_offsets,
            pass,
            Some(gl_viewport(frame)),
            None,
            &desc,
            encoder,
        );
    }
}

struct FrameOptionsSize {
    width: u32,
    height: u32,
    samples: u32,
}

fn frame_size(frame: &Frame) -> FrameOptionsSize {
    FrameOptionsSize {
        width: frame.options.width.max(1),
        height: frame.options.height.max(1),
        samples: if frame.options.samples > 1 { 4 } else { 1 },
    }
}

fn gl_viewport(frame: &Frame) -> [u32; 2] {
    frame.options.gl_viewport.map(|v| v.max(1))
}

fn extent(width: u32, height: u32) -> wgpu::Extent3d {
    wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    }
}

fn padded_row(width: u32) -> u32 {
    (width * 4).div_ceil(256) * 256
}

fn copy_rows(data: &[u8], padded: usize, row: usize, height: usize, out: &mut Vec<u8>) {
    out.resize(row * height, 0);
    for y in 0..height {
        let src = (height - 1 - y) * padded;
        out[y * row..(y + 1) * row].copy_from_slice(&data[src..src + row]);
    }
}

fn make_draw_buffer(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    capacity: u64,
) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: capacity * DRAW_STRIDE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: wgpu::BufferSize::new(size_of::<DrawU>() as u64),
            }),
        }],
    });
    (buffer, group)
}

/// The main draws followed by every shadow pass's draws: the order of the draw uniforms.
fn all_draws(frame: &Frame) -> impl Iterator<Item = &Draw> {
    frame
        .draws
        .iter()
        .chain(frame.shadows.iter().flat_map(|s| &s.draws))
}

fn make_frame_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
    shadow_maps: &wgpu::Texture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let view = shadow_maps.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

/// `renderBufferDirect`'s range: the whole index or position count, narrowed to the group.
fn draw_range(group: Option<Group>, count: u32) -> Option<std::ops::Range<u32>> {
    let (start, end) = match group {
        Some(g) => (g.start, g.start.saturating_add(g.count)),
        None => (0, usize::MAX),
    };
    let end = end.min(count as usize);
    (start <= end).then_some(start as u32..end as u32)
}

fn draw_geometry(
    pass: &mut wgpu::RenderPass<'_>,
    geometry: &GpuGeometry,
    group: Option<Group>,
    instance_count: u32,
) {
    pass.set_vertex_buffer(0, geometry.position.slice(..));
    pass.set_vertex_buffer(1, geometry.normal.slice(..));
    pass.set_vertex_buffer(2, geometry.color.slice(..));
    pass.set_vertex_buffer(3, geometry.uv.slice(..));
    match &geometry.index {
        Some((buffer, format, count)) => {
            if let Some(range) = draw_range(group, *count) {
                pass.set_index_buffer(buffer.slice(..), *format);
                pass.draw_indexed(range, 0, 0..instance_count);
            }
        }
        None => {
            if let Some(range) = draw_range(group, geometry.vertex_count) {
                pass.draw(range, 0..instance_count);
            }
        }
    }
}

fn upload_segments(device: &wgpu::Device, geometry: &LineSegmentsGeometry) -> (wgpu::Buffer, u32) {
    let bytes: &[u8] = bytemuck::cast_slice(&geometry.segments);
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: if bytes.is_empty() { &[0; 24] } else { bytes },
        usage: wgpu::BufferUsages::VERTEX,
    });
    (buffer, geometry.segments.len() as u32)
}

fn draw_uniform(draw: &Draw, geometries: &HashMap<(u64, Primitives), GpuGeometry>) -> DrawU {
    let f = |v: f64| v as f32;
    let id = draw.object.0 as u32 + 1;
    let mut u = DrawU::zeroed();
    u.model_view = draw.model_view;
    u.model = draw.model;
    match &draw.kind {
        DrawKind::Mesh {
            geometry, material, ..
        } => {
            let m = material;
            let n = &draw.normal_matrix;
            let emissive = [
                f(m.emissive.r * m.emissive_intensity),
                f(m.emissive.g * m.emissive_intensity),
                f(m.emissive.b * m.emissive_intensity),
            ];
            let shininess = asrs_js::math::max2(m.shininess, 1e-4);
            let color_components =
                geometries[&(geometry.key(), Primitives::Triangles)].color_components;
            u.normal_matrix = [
                n[0], n[1], n[2], 0.0, n[3], n[4], n[5], 0.0, n[6], n[7], n[8], 0.0,
            ];
            u.diffuse_opacity = [f(m.color.r), f(m.color.g), f(m.color.b), f(m.opacity)];
            u.emissive = pad3(emissive, 0.0);
            u.specular_shininess = [
                f(m.specular.r),
                f(m.specular.g),
                f(m.specular.b),
                f(shininess),
            ];
            u.flags = [
                if m.vertex_colors { color_components } else { 0 },
                u32::from(draw.side == Side::Back),
                u32::from(draw.side == Side::Double),
                u32::from(!m.transparent),
            ];
            u.extra = [
                u32::from(m.map.is_some()),
                id,
                u32::from(draw.receive_shadow),
                0,
            ];
        }
        DrawKind::FatLine {
            material,
            resolution,
            ..
        } => {
            let m = material;
            u.diffuse_opacity = [f(m.color.r), f(m.color.g), f(m.color.b), f(m.opacity)];
            u.specular_shininess = [f(m.linewidth), resolution[0], resolution[1], 0.0];
            u.flags = [0, 0, 0, u32::from(!m.transparent)];
            u.extra = [0, id, 0, 0];
        }
        DrawKind::Line {
            geometry,
            material,
            segments,
        } => {
            let m = material;
            let color_components =
                geometries[&(geometry.key(), line_primitives(*segments))].color_components;
            u.diffuse_opacity = [f(m.color.r), f(m.color.g), f(m.color.b), f(m.opacity)];
            u.flags = [
                if m.vertex_colors { color_components } else { 0 },
                0,
                0,
                u32::from(!m.transparent),
            ];
            u.extra = [0, id, 0, 0];
        }
    }
    u
}

fn attribute_f32(attribute: Option<&BufferAttribute>, size: usize, count: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; count * size];
    if let Some(a) = attribute {
        for i in 0..count.min(a.count()) {
            for c in 0..size.min(a.item_size) {
                out[i * size + c] = a.get_component(i, c) as f32;
            }
        }
    }
    out
}

fn line_primitives(segments: bool) -> Primitives {
    if segments {
        Primitives::Lines
    } else {
        Primitives::LineStrip
    }
}

/// `indices` with the last vertex of every primitive first. ANGLE's Vulkan backend turns on GL's
/// last-vertex provoking convention, and SwiftShader starts a primitive's setup from its provoking
/// vertex, so its interpolation rounds differently for another order. Strips become segments.
fn provoking_first(primitives: Primitives, indices: &[u32]) -> Vec<u32> {
    match primitives {
        Primitives::Triangles => {
            let (triangles, rest) = indices.as_chunks::<3>();
            triangles
                .iter()
                .flat_map(|&[a, b, c]| [c, a, b])
                .chain(rest.iter().copied())
                .collect()
        }
        Primitives::Lines => {
            let (segments, rest) = indices.as_chunks::<2>();
            segments
                .iter()
                .flat_map(|&[a, b]| [b, a])
                .chain(rest.iter().copied())
                .collect()
        }
        Primitives::LineStrip => indices.windows(2).flat_map(|w| [w[1], w[0]]).collect(),
    }
}

fn upload_geometry(
    device: &wgpu::Device,
    geometry: &Geometry,
    provoking_last: Option<Primitives>,
) -> GpuGeometry {
    let g = &geometry.geometry;
    let position = g.attribute("position");
    let count = position.map_or(0, BufferAttribute::count);
    let init = |contents: &[u8], usage: wgpu::BufferUsages| {
        let contents: &[u8] = if contents.is_empty() {
            &[0; 16]
        } else {
            contents
        };
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents,
            usage,
        })
    };
    let vertex = wgpu::BufferUsages::VERTEX;
    let positions = attribute_f32(position, 3, count);
    let normals = attribute_f32(g.attribute("normal"), 3, count);
    let uvs = attribute_f32(g.attribute("uv"), 2, count);
    let color_attribute = g.attribute("color");
    let color_components = color_attribute.map_or(3, |a| a.item_size as u32);
    let (color, float_color) = match color_attribute {
        Some(BufferAttribute {
            array: ArrayData::Uint8(values),
            item_size,
            normalized: true,
            ..
        }) => {
            let mut bytes = vec![255u8; count * 4];
            for i in 0..count {
                for c in 0..(*item_size).min(4) {
                    bytes[i * 4 + c] = values[i * item_size + c];
                }
            }
            (init(&bytes, vertex), false)
        }
        Some(a) => {
            let mut values = attribute_f32(Some(a), 4, count);
            if a.item_size < 4 {
                for i in 0..count {
                    values[i * 4 + 3] = 1.0;
                }
            }
            (init(bytemuck::cast_slice(&values), vertex), true)
        }
        None => (init(&vec![255u8; count.max(1) * 4], vertex), false),
    };
    let index = if let Some(primitives) = provoking_last {
        let indices: Vec<u32> = match &g.index {
            Some(index) => (0..index.array.len())
                .map(|i| index.array.get(i) as u32)
                .collect(),
            None => (0..count as u32).collect(),
        };
        let indices = provoking_first(primitives, &indices);
        Some((
            init(bytemuck::cast_slice(&indices), wgpu::BufferUsages::INDEX),
            wgpu::IndexFormat::Uint32,
            indices.len() as u32,
        ))
    } else {
        g.index.as_ref().map(|index| {
            let usage = wgpu::BufferUsages::INDEX;
            match &index.array {
                ArrayData::Uint16(v) => {
                    let mut v = v.clone();
                    if v.len() % 2 == 1 {
                        v.push(0);
                    }
                    (
                        init(bytemuck::cast_slice(&v), usage),
                        wgpu::IndexFormat::Uint16,
                        index.array.len() as u32,
                    )
                }
                other => {
                    let v: Vec<u32> = (0..other.len()).map(|i| other.get(i) as u32).collect();
                    (
                        init(bytemuck::cast_slice(&v), usage),
                        wgpu::IndexFormat::Uint32,
                        v.len() as u32,
                    )
                }
            }
        })
    };
    GpuGeometry {
        position: init(bytemuck::cast_slice(&positions), vertex),
        normal: init(bytemuck::cast_slice(&normals), vertex),
        color,
        float_color,
        uv: init(bytemuck::cast_slice(&uvs), vertex),
        index,
        vertex_count: count as u32,
        color_components,
    }
}

/// The mip chain `generateMipmap` builds on ANGLE's Metal backend: each level is a linear-filtered
/// blit of the one above, sampling at the destination texel centres with clamp to edge. Measured
/// against Electron's levels (weights quantized to 1/256 and rounded), even sizes reduce to the
/// exact 2x2 average; odd sizes, such as the 25x25 level of a 100x100 canvas, do not.
fn mip_chain(texture: &Texture) -> Vec<(u32, u32, Vec<u8>)> {
    let (w, h) = (texture.width, texture.height);
    let mut base = texture.rgba.to_vec();
    if texture.flip_y {
        let row = w as usize * 4;
        base = base.chunks_exact(row).rev().flatten().copied().collect();
    }
    let mut levels = vec![(w, h, base)];
    if !(texture.generate_mipmaps && texture.min_filter.uses_mipmaps()) {
        return levels;
    }
    let taps = |dst: u32, src: u32, i: u32| {
        let u = (i as f32 + 0.5) / dst as f32 * src as f32 - 0.5;
        let i0 = u.floor();
        let weight = ((u - i0) as f64 * 256.0 + 0.5).floor() / 256.0;
        let clamp = |v: f32| v.clamp(0.0, (src - 1) as f32) as u32;
        (clamp(i0), clamp(i0 + 1.0), weight)
    };
    while let Some((pw, ph, prev)) = levels.last()
        && (*pw > 1 || *ph > 1)
    {
        let (pw, ph) = (*pw, *ph);
        let (nw, nh) = ((pw / 2).max(1), (ph / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            let (y0, y1, fy) = taps(nh, ph, y);
            for x in 0..nw {
                let (x0, x1, fx) = taps(nw, pw, x);
                for c in 0..4 {
                    let at = |sx: u32, sy: u32| f64::from(prev[((sy * pw + sx) * 4 + c) as usize]);
                    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
                    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
                    let value = top * (1.0 - fy) + bottom * fy;
                    next[((y * nw + x) * 4 + c) as usize] = (value + 0.5).floor() as u8;
                }
            }
        }
        levels.push((nw, nh, next));
    }
    levels
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    texture: &Texture,
) -> GpuTexture {
    let levels = mip_chain(texture);
    let gpu = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: extent(texture.width, texture.height),
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, (w, h, data)) in levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &gpu,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(*h),
            },
            extent(*w, *h),
        );
    }
    let filter = |f: Filter| match f {
        Filter::Nearest => wgpu::FilterMode::Nearest,
        Filter::Linear => wgpu::FilterMode::Linear,
    };
    let (min, mip) = match texture.min_filter {
        MinFilter::Nearest | MinFilter::NearestMipmapNearest => (Filter::Nearest, Filter::Nearest),
        MinFilter::Linear | MinFilter::LinearMipmapNearest => (Filter::Linear, Filter::Nearest),
        MinFilter::NearestMipmapLinear => (Filter::Nearest, Filter::Linear),
        MinFilter::LinearMipmapLinear => (Filter::Linear, Filter::Linear),
    };
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: filter(texture.mag_filter),
        min_filter: filter(min),
        mipmap_filter: match mip {
            Filter::Nearest => wgpu::MipmapFilterMode::Nearest,
            Filter::Linear => wgpu::MipmapFilterMode::Linear,
        },
        lod_max_clamp: if levels.len() > 1 { 32.0 } else { 0.0 },
        ..Default::default()
    });
    let view = gpu.create_view(&Default::default());
    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    GpuTexture { bind_group }
}
