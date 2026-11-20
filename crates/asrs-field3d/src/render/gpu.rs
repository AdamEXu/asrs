use std::collections::{HashMap, HashSet};
use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::frame::{Draw, Frame};
use super::scene::{Filter, Geometry, MinFilter, Side, Texture};
use crate::three::{ArrayData, BufferAttribute};

pub const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const ID_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
const DRAW_STRIDE: u64 = 256;
const MAX_HEMI: usize = 4;
const MAX_POINT: usize = 8;
const MAX_SPOT: usize = 8;

#[derive(Debug)]
pub enum RenderError {
    NoAdapter(String),
    Device(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::NoAdapter(e) => write!(f, "no graphics adapter: {e}"),
            RenderError::Device(e) => write!(f, "device request failed: {e}"),
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
struct FrameU {
    projection: [f32; 16],
    counts: [u32; 4],
    clip_factors: [f32; 4],
    hemi: [HemiU; MAX_HEMI],
    point: [PointU; MAX_POINT],
    spot: [SpotU; MAX_SPOT],
}

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
}

fn pad3(v: [f32; 3], w: f32) -> [f32; 4] {
    [v[0], v[1], v[2], w]
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PipeKey {
    cull: bool,
    front_cw: bool,
    blend: bool,
    depth_test: bool,
    depth_write: bool,
    instanced: bool,
    float_color: bool,
    aux: bool,
    samples: u32,
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
pub struct AuxBuffers {
    pub width: u32,
    pub height: u32,
    pub opaque_ids: Vec<u32>,
    pub transparent_ids: Vec<u32>,
    pub depth: Vec<f32>,
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

/// The wgpu half of `WebGLRenderer`: its own device, an offscreen `Bgra8Unorm` target (MSAA 4x
/// when antialiased) and a readback of every frame. Geometry and textures are uploaded once per
/// key and dropped when a frame no longer uses them.
pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_info: wgpu::AdapterInfo,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    pipelines: HashMap<PipeKey, wgpu::RenderPipeline>,
    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    draw_layout: wgpu::BindGroupLayout,
    draw_buffer: wgpu::Buffer,
    draw_group: wgpu::BindGroup,
    draw_capacity: u64,
    texture_layout: wgpu::BindGroupLayout,
    blank_texture: wgpu::BindGroup,
    instance_buffer: Option<wgpu::Buffer>,
    geometries: HashMap<u64, GpuGeometry>,
    textures: HashMap<u64, GpuTexture>,
    targets: Option<Targets>,
}

impl Renderer {
    pub fn new() -> Result<Self, RenderError> {
        let instance = wgpu::Instance::new(
            wgpu::InstanceDescriptor::new_without_display_handle_from_env(),
        );
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::None,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))
        .map_err(|e| RenderError::NoAdapter(e.to_string()))?;
        let adapter_info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("field3d"),
            ..Default::default()
        }))
        .map_err(|e| RenderError::Device(e.to_string()))?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("phong"),
            source: wgpu::ShaderSource::Wgsl(include_str!("phong.wgsl").into()),
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
            bind_group_layouts: &[Some(&frame_layout), Some(&draw_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: size_of::<FrameU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
        });
        let (draw_buffer, draw_group) = make_draw_buffer(&device, &draw_layout, 64);
        let blank = Texture::new(1, 1, vec![255u8; 4].into());
        let blank_texture = upload_texture(&device, &queue, &texture_layout, &blank).bind_group;
        Ok(Renderer {
            device,
            queue,
            adapter_info,
            shader,
            layout,
            pipelines: HashMap::new(),
            frame_buffer,
            frame_group,
            draw_layout,
            draw_buffer,
            draw_group,
            draw_capacity: 64,
            texture_layout,
            blank_texture,
            instance_buffer: None,
            geometries: HashMap::new(),
            textures: HashMap::new(),
            targets: None,
        })
    }

    pub fn adapter_info(&self) -> &wgpu::AdapterInfo {
        &self.adapter_info
    }

    fn pipeline(&mut self, key: PipeKey) -> &wgpu::RenderPipeline {
        let device = &self.device;
        let shader = &self.shader;
        let layout = &self.layout;
        self.pipelines.entry(key).or_insert_with(|| {
            const POSITION: [wgpu::VertexAttribute; 1] =
                wgpu::vertex_attr_array![0 => Float32x3];
            const NORMAL: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x3];
            const COLOR_U8: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![2 => Unorm8x4];
            const COLOR_F32: [wgpu::VertexAttribute; 1] =
                wgpu::vertex_attr_array![2 => Float32x4];
            const UV: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![3 => Float32x2];
            const INSTANCE: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
                4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4
            ];
            let vertex = |stride, attributes: &'static [wgpu::VertexAttribute]| {
                Some(wgpu::VertexBufferLayout {
                    array_stride: stride,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes,
                })
            };
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
                buffers.push(Some(wgpu::VertexBufferLayout {
                    array_stride: 64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &INSTANCE,
                }));
            }
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
                vec![
                    Some(wgpu::ColorTargetState {
                        format: ID_FORMAT,
                        blend: None,
                        write_mask: mask(!key.blend),
                    }),
                    Some(wgpu::ColorTargetState {
                        format: ID_FORMAT,
                        blend: None,
                        write_mask: mask(key.blend),
                    }),
                ]
            } else {
                vec![Some(wgpu::ColorTargetState {
                    format: COLOR_FORMAT,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })]
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(if key.instanced { "vs_instanced" } else { "vs_mesh" }),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(if key.aux { "fs_aux" } else { "fs" }),
                    compilation_options: Default::default(),
                    targets: &targets,
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: if key.front_cw {
                        wgpu::FrontFace::Ccw
                    } else {
                        wgpu::FrontFace::Cw
                    },
                    cull_mode: key.cull.then_some(wgpu::Face::Back),
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
        let mut used_textures = HashSet::new();
        for draw in &frame.draws {
            let key = draw.geometry.key();
            used_geometries.insert(key);
            if !self.geometries.contains_key(&key) {
                let gpu = upload_geometry(&self.device, &draw.geometry);
                self.geometries.insert(key, gpu);
            }
            if let Some(texture) = &draw.material.map {
                used_textures.insert(texture.key());
                if !self.textures.contains_key(&texture.key()) {
                    let gpu =
                        upload_texture(&self.device, &self.queue, &self.texture_layout, texture);
                    self.textures.insert(texture.key(), gpu);
                }
            }
        }
        self.geometries.retain(|k, _| used_geometries.contains(k));
        self.textures.retain(|k, _| used_textures.contains(k));
    }

    fn write_uniforms(&mut self, frame: &Frame) -> Vec<Option<u64>> {
        let lights = &frame.lights;
        let mut frame_u = FrameU::zeroed();
        frame_u.projection = frame.projection;
        frame_u.clip_factors = [-1.0, 1.0, 0.0, 0.0];
        frame_u.counts = [
            lights.hemisphere.len().min(MAX_HEMI) as u32,
            lights.point.len().min(MAX_POINT) as u32,
            lights.spot.len().min(MAX_SPOT) as u32,
            0,
        ];
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
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame_u));

        let count = frame.draws.len() as u64;
        if count > self.draw_capacity {
            self.draw_capacity = count.next_power_of_two();
            let (buffer, group) = make_draw_buffer(&self.device, &self.draw_layout, self.draw_capacity);
            self.draw_buffer = buffer;
            self.draw_group = group;
        }
        let mut data = vec![0u8; (count * DRAW_STRIDE) as usize];
        let mut instances: Vec<[f32; 16]> = Vec::new();
        let mut instance_offsets = Vec::with_capacity(frame.draws.len());
        for (n, draw) in frame.draws.iter().enumerate() {
            let d = draw_uniform(draw, self.geometries[&draw.geometry.key()].color_components);
            let o = n * DRAW_STRIDE as usize;
            data[o..o + size_of::<DrawU>()].copy_from_slice(bytemuck::bytes_of(&d));
            instance_offsets.push(draw.instances.as_ref().map(|m| {
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

    fn pipe_key(&self, draw: &Draw, aux: bool, samples: u32) -> PipeKey {
        let m = &draw.material;
        PipeKey {
            cull: draw.side != Side::Double,
            front_cw: (draw.side == Side::Back) != draw.front_face_cw,
            blend: m.transparent,
            depth_test: m.depth_test,
            depth_write: m.depth_write,
            instanced: draw.instances.is_some(),
            float_color: self.geometries[&draw.geometry.key()].float_color,
            aux,
            samples: if aux { 1 } else { samples },
        }
    }

    fn encode_draws(
        &mut self,
        frame: &Frame,
        instance_offsets: &[Option<u64>],
        aux: bool,
        pass_desc: &wgpu::RenderPassDescriptor<'_>,
        encoder: &mut wgpu::CommandEncoder,
    ) {
        let samples = frame_size(frame).samples;
        let keys: Vec<PipeKey> = frame
            .draws
            .iter()
            .map(|d| self.pipe_key(d, aux, samples))
            .collect();
        for &key in &keys {
            self.pipeline(key);
        }
        let mut pass = encoder.begin_render_pass(pass_desc);
        pass.set_bind_group(0, &self.frame_group, &[]);
        for (n, draw) in frame.draws.iter().enumerate() {
            let geometry = &self.geometries[&draw.geometry.key()];
            pass.set_pipeline(&self.pipelines[&keys[n]]);
            pass.set_bind_group(1, &self.draw_group, &[(n as u64 * DRAW_STRIDE) as u32]);
            let texture = draw
                .material
                .map
                .as_ref()
                .map_or(&self.blank_texture, |t| &self.textures[&t.key()].bind_group);
            pass.set_bind_group(2, texture, &[]);
            pass.set_vertex_buffer(0, geometry.position.slice(..));
            pass.set_vertex_buffer(1, geometry.normal.slice(..));
            pass.set_vertex_buffer(2, geometry.color.slice(..));
            pass.set_vertex_buffer(3, geometry.uv.slice(..));
            let mut instance_count = 1;
            if let (Some(offset), Some(matrices), Some(buffer)) =
                (instance_offsets[n], &draw.instances, &self.instance_buffer)
            {
                if matrices.is_empty() {
                    continue;
                }
                pass.set_vertex_buffer(4, buffer.slice(offset..offset + matrices.len() as u64 * 64));
                instance_count = matrices.len() as u32;
            }
            match &geometry.index {
                Some((buffer, format, count)) => {
                    pass.set_index_buffer(buffer.slice(..), *format);
                    pass.draw_indexed(0..*count, 0, 0..instance_count);
                }
                None => pass.draw(0..geometry.vertex_count, 0..instance_count),
            }
        }
    }

    /// Renders `frame` and writes its pixels to `out` as tightly packed BGRA rows, top row
    /// first, which is `gl.readPixels` flipped to image order.
    pub fn render(&mut self, frame: &Frame, out: &mut Vec<u8>) -> Timings {
        let t0 = Instant::now();
        let FrameOptionsSize { width, height, samples } = frame_size(frame);
        self.ensure_targets(width, height, samples);
        self.sync_resources(frame);
        let instance_offsets = self.write_uniforms(frame);
        let targets = self.targets.take().expect("targets");
        let resolve_view = targets.resolve.create_view(&Default::default());
        let msaa_view = targets.msaa.as_ref().map(|t| t.create_view(&Default::default()));
        let depth_view = targets.depth.create_view(&Default::default());
        let [r, g, b, a] = frame.clear;
        let mut encoder = self.device.create_command_encoder(&Default::default());
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
            self.encode_draws(frame, &instance_offsets, false, &desc, &mut encoder);
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
        let slice = targets.readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let t2 = Instant::now();
        {
            let data = slice.get_mapped_range().expect("mapped readback");
            copy_rows(&data, padded as usize, (width * 4) as usize, height as usize, out);
        }
        targets.readback.unmap();
        self.targets = Some(targets);
        let t3 = Instant::now();
        Timings {
            encode_ms: (t1 - t0).as_secs_f64() * 1e3,
            gpu_and_map_ms: (t2 - t1).as_secs_f64() * 1e3,
            copy_ms: (t3 - t2).as_secs_f64() * 1e3,
        }
    }

    /// The single-sample id and depth pass behind the edge mask. Opaque draws write the first
    /// id target and depth; transparent draws only the second id target, depth tested.
    pub fn render_aux(&mut self, frame: &Frame) -> AuxBuffers {
        let FrameOptionsSize { width, height, .. } = frame_size(frame);
        self.sync_resources(frame);
        let instance_offsets = self.write_uniforms(frame);
        let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC;
        let opaque = self.texture(width, height, ID_FORMAT, 1, usage);
        let transparent = self.texture(width, height, ID_FORMAT, 1, usage);
        let depth = self.texture(width, height, DEPTH_FORMAT, 1, usage);
        let views = [&opaque, &transparent].map(|t| t.create_view(&Default::default()));
        let depth_view = depth.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        {
            let attachment = |view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })
            };
            let color_attachments = [attachment(&views[0]), attachment(&views[1])];
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
            self.encode_draws(frame, &instance_offsets, true, &desc, &mut encoder);
        }
        let padded = padded_row(width);
        let buffers: Vec<wgpu::Buffer> = (0..3)
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
                let data = b.slice(..).get_mapped_range().expect("mapped aux readback");
                copy_rows(&data, padded as usize, (width * 4) as usize, height as usize, &mut bytes);
            }
            b.unmap();
            bytes
        };
        let u32s = |bytes: Vec<u8>| -> Vec<u32> {
            bytes
                .chunks_exact(4)
                .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        };
        let opaque_ids = u32s(read(&buffers[0]));
        let transparent_ids = u32s(read(&buffers[1]));
        let depth = read(&buffers[2])
            .chunks_exact(4)
            .map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        AuxBuffers {
            width,
            height,
            opaque_ids,
            transparent_ids,
            depth,
        }
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

fn draw_uniform(draw: &Draw, color_components: u32) -> DrawU {
    let m = &draw.material;
    let n = &draw.normal_matrix;
    let f = |v: f64| v as f32;
    let emissive = [
        f(m.emissive.r * m.emissive_intensity),
        f(m.emissive.g * m.emissive_intensity),
        f(m.emissive.b * m.emissive_intensity),
    ];
    let shininess = asrs_js::math::max2(m.shininess, 1e-4);
    DrawU {
        model_view: draw.model_view,
        normal_matrix: [n[0], n[1], n[2], 0.0, n[3], n[4], n[5], 0.0, n[6], n[7], n[8], 0.0],
        diffuse_opacity: [f(m.color.r), f(m.color.g), f(m.color.b), f(m.opacity)],
        emissive: pad3(emissive, 0.0),
        specular_shininess: [f(m.specular.r), f(m.specular.g), f(m.specular.b), f(shininess)],
        flags: [
            if m.vertex_colors { color_components } else { 0 },
            u32::from(draw.side == Side::Back),
            u32::from(draw.side == Side::Double),
            u32::from(!m.transparent),
        ],
        extra: [u32::from(m.map.is_some()), draw.object.0 as u32 + 1, 0, 0],
    }
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

fn upload_geometry(device: &wgpu::Device, geometry: &Geometry) -> GpuGeometry {
    let g = &geometry.geometry;
    let position = g.attribute("position");
    let count = position.map_or(0, BufferAttribute::count);
    let init = |contents: &[u8], usage: wgpu::BufferUsages| {
        let contents: &[u8] = if contents.is_empty() { &[0; 16] } else { contents };
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
    let index = g.index.as_ref().map(|index| {
        let usage = wgpu::BufferUsages::INDEX;
        match &index.array {
            ArrayData::Uint16(v) => {
                let mut v = v.clone();
                if v.len() % 2 == 1 {
                    v.push(0);
                }
                (init(bytemuck::cast_slice(&v), usage), wgpu::IndexFormat::Uint16, index.array.len() as u32)
            }
            other => {
                let v: Vec<u32> = (0..other.len()).map(|i| other.get(i) as u32).collect();
                (init(bytemuck::cast_slice(&v), usage), wgpu::IndexFormat::Uint32, v.len() as u32)
            }
        }
    });
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

/// Mip chain by 2x2 box filter with round-half-up, the reduction `generateMipmap` performs.
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
    while let Some((pw, ph, prev)) = levels.last()
        && (*pw > 1 || *ph > 1)
    {
        let (nw, nh) = ((*pw / 2).max(1), (*ph / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |sx: u32, sy: u32| {
                        let sx = sx.min(pw - 1);
                        let sy = sy.min(ph - 1);
                        u32::from(prev[((sy * pw + sx) * 4 + c) as usize])
                    };
                    let sum = at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1)
                        + at(2 * x + 1, 2 * y + 1);
                    next[((y * nw + x) * 4 + c) as usize] = ((sum + 2) / 4) as u8;
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
