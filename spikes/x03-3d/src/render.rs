use std::collections::HashMap;
use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use glam::{DMat4, DVec3, Mat4};
use wgpu::util::DeviceExt;

use crate::scene::{Scene, Side};

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Bgra8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const DRAW_STRIDE: u64 = 256;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FrameU {
    proj: [[f32; 4]; 4],
    view: [[f32; 4]; 4],
    hemi_sky: [f32; 4],
    hemi_ground: [f32; 4],
    hemi_dir: [f32; 4],
    point_pos: [f32; 4],
    point_color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DrawU {
    model_view: [[f32; 4]; 4],
    normal_matrix: [[f32; 4]; 4],
    color: [f32; 4],
    specular: [f32; 4],
    flags: [f32; 4],
}

struct GpuMesh {
    pos: wgpu::Buffer,
    nrm: wgpu::Buffer,
    col: wgpu::Buffer,
    idx: Option<(wgpu::Buffer, u32)>,
    count: u32,
    center: DVec3,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PipeKey {
    cull: u8,
    blend: bool,
    depth_write: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub position: DVec3,
    pub target: DVec3,
    pub fov: f64,
    pub dark: bool,
}

pub struct Timings {
    pub encode_ms: f64,
    pub gpu_and_map_ms: f64,
    pub copy_ms: f64,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub adapter_info: wgpu::AdapterInfo,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    pipes: HashMap<PipeKey, wgpu::RenderPipeline>,
    frame_buf: wgpu::Buffer,
    frame_bg: wgpu::BindGroup,
    draw_bgl: wgpu::BindGroupLayout,
    draw_buf: wgpu::Buffer,
    draw_bg: wgpu::BindGroup,
    draw_cap: u64,
    meshes: Vec<GpuMesh>,
    width: u32,
    height: u32,
    samples: u32,
    msaa: Option<wgpu::Texture>,
    resolve: Option<wgpu::Texture>,
    depth: Option<wgpu::Texture>,
    readback: Option<wgpu::Buffer>,
}

impl Renderer {
    pub fn new(samples: u32) -> anyhow::Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::None,
            force_fallback_adapter: false,
            compatible_surface: None,
            apply_limit_buckets: false,
        }))?;
        let adapter_info = adapter.get_info();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("field3d"),
            ..Default::default()
        }))?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("phong"),
            source: wgpu::ShaderSource::Wgsl(include_str!("phong.wgsl").into()),
        });
        let uniform_entry = |dynamic| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        let frame_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[uniform_entry(false)],
        });
        let draw_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[uniform_entry(true)],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&frame_bgl), Some(&draw_bgl)],
            immediate_size: 0,
        });
        let frame_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: std::mem::size_of::<FrameU>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &frame_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buf.as_entire_binding(),
            }],
        });
        let (draw_buf, draw_bg) = Self::make_draw_buf(&device, &draw_bgl, 64);
        Ok(Renderer {
            device,
            queue,
            adapter_info,
            shader,
            layout,
            pipes: HashMap::new(),
            frame_buf,
            frame_bg,
            draw_bgl,
            draw_buf,
            draw_bg,
            draw_cap: 64,
            meshes: Vec::new(),
            width: 0,
            height: 0,
            samples,
            msaa: None,
            resolve: None,
            depth: None,
            readback: None,
        })
    }

    fn make_draw_buf(device: &wgpu::Device, bgl: &wgpu::BindGroupLayout, cap: u64) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: cap * DRAW_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<DrawU>() as u64),
                }),
            }],
        });
        (buf, bg)
    }

    pub fn upload(&mut self, scene: &Scene) {
        self.meshes = scene
            .meshes
            .iter()
            .map(|m| {
                let init = |contents: &[u8], usage| {
                    self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: None,
                        contents,
                        usage,
                    })
                };
                let colors = m.colors.clone().unwrap_or_else(|| vec![[255; 4]; m.positions.len()]);
                GpuMesh {
                    pos: init(bytemuck::cast_slice(&m.positions), wgpu::BufferUsages::VERTEX),
                    nrm: init(bytemuck::cast_slice(&m.normals), wgpu::BufferUsages::VERTEX),
                    col: init(bytemuck::cast_slice(&colors), wgpu::BufferUsages::VERTEX),
                    idx: m
                        .indices
                        .as_ref()
                        .map(|i| (init(bytemuck::cast_slice(i), wgpu::BufferUsages::INDEX), i.len() as u32)),
                    count: m.positions.len() as u32,
                    center: m.bounding_center(),
                }
            })
            .collect();
    }

    fn pipeline(&mut self, key: PipeKey) -> &wgpu::RenderPipeline {
        let device = &self.device;
        let shader = &self.shader;
        let layout = &self.layout;
        let samples = self.samples;
        self.pipes.entry(key).or_insert_with(|| {
            let attr = |loc, fmt| wgpu::VertexAttribute {
                format: fmt,
                offset: 0,
                shader_location: loc,
            };
            let a0 = [attr(0, wgpu::VertexFormat::Float32x3)];
            let a1 = [attr(1, wgpu::VertexFormat::Float32x3)];
            let a2 = [attr(2, wgpu::VertexFormat::Unorm8x4)];
            let vb = |stride, attrs: &'static [wgpu::VertexAttribute]| wgpu::VertexBufferLayout {
                array_stride: stride,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: attrs,
            };
            let a0: &'static [_] = Box::leak(Box::new(a0));
            let a1: &'static [_] = Box::leak(Box::new(a1));
            let a2: &'static [_] = Box::leak(Box::new(a2));
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[Some(vb(12, a0)), Some(vb(12, a1)), Some(vb(4, a2))],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend: key.blend.then_some(wgpu::BlendState {
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
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: match key.cull {
                        1 => Some(wgpu::Face::Back),
                        2 => Some(wgpu::Face::Front),
                        _ => None,
                    },
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(key.depth_write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        })
    }

    fn ensure_targets(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height && self.resolve.is_some() {
            return;
        }
        self.width = width;
        self.height = height;
        let tex = |format, samples, usage| {
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
        };
        self.resolve = Some(tex(
            FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        ));
        self.msaa = (self.samples > 1).then(|| tex(FORMAT, self.samples, wgpu::TextureUsages::RENDER_ATTACHMENT));
        self.depth = Some(tex(DEPTH, self.samples, wgpu::TextureUsages::RENDER_ATTACHMENT));
        self.readback = Some(self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (self.padded_row() * height) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
    }

    fn padded_row(&self) -> u32 {
        (self.width * 4).div_ceil(256) * 256
    }

    pub fn render(&mut self, scene: &Scene, cam: &Camera, width: u32, height: u32, out: &mut Vec<u8>) -> Timings {
        let t0 = Instant::now();
        self.ensure_targets(width, height);
        let aspect = width as f64 / height as f64;
        let view = DMat4::look_at_rh(cam.position, cam.target, DVec3::Y);
        let proj = DMat4::perspective_rh(cam.fov.to_radians(), aspect, 0.15, 1000.0);
        let hemi_dir = view.transform_vector3(DVec3::Y).normalize();
        let point = view.transform_point3(scene.point_pos);
        let f = |v: DMat4| v.as_mat4().to_cols_array_2d();
        let frame = FrameU {
            proj: f(proj),
            view: f(view),
            hemi_sky: [scene.hemi_sky[0], scene.hemi_sky[1], scene.hemi_sky[2], 0.0],
            hemi_ground: [scene.hemi_ground[0], scene.hemi_ground[1], scene.hemi_ground[2], 0.0],
            hemi_dir: [hemi_dir.x as f32, hemi_dir.y as f32, hemi_dir.z as f32, 0.0],
            point_pos: [point.x as f32, point.y as f32, point.z as f32, 1.0],
            point_color: [scene.point_color[0], scene.point_color[1], scene.point_color[2], 0.0],
        };
        self.queue.write_buffer(&self.frame_buf, 0, bytemuck::bytes_of(&frame));

        let proj_view = proj * view;
        let mut opaque = Vec::new();
        let mut transparent = Vec::new();
        for (i, item) in scene.items.iter().enumerate() {
            let c = (item.model).transform_point3(self.meshes[item.mesh].center);
            let z = proj_view.project_point3(c).z;
            if item.material.transparent {
                transparent.push((i, z));
            } else {
                opaque.push((i, z));
            }
        }
        opaque.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
        transparent.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

        struct Call {
            item: usize,
            key: PipeKey,
            flip: bool,
            double: bool,
        }
        let mut calls = Vec::new();
        for &(i, _) in &opaque {
            let m = &scene.items[i].material;
            let key = PipeKey {
                cull: match m.side {
                    Side::Front => 1,
                    Side::Back => 2,
                    Side::Double => 0,
                },
                blend: false,
                depth_write: m.depth_write,
            };
            calls.push(Call {
                item: i,
                key,
                flip: m.side == Side::Back,
                double: m.side == Side::Double,
            });
        }
        for &(i, _) in &transparent {
            let m = &scene.items[i].material;
            let sides: &[Side] = if m.side == Side::Double { &[Side::Back, Side::Front] } else { std::slice::from_ref(&m.side) };
            for &side in sides {
                calls.push(Call {
                    item: i,
                    key: PipeKey {
                        cull: if side == Side::Front { 1 } else { 2 },
                        blend: true,
                        depth_write: m.depth_write,
                    },
                    flip: side == Side::Back,
                    double: false,
                });
            }
        }

        if calls.len() as u64 > self.draw_cap {
            self.draw_cap = (calls.len() as u64).next_power_of_two();
            let (b, g) = Self::make_draw_buf(&self.device, &self.draw_bgl, self.draw_cap);
            self.draw_buf = b;
            self.draw_bg = g;
        }
        let mut draw_data = vec![0u8; calls.len() * DRAW_STRIDE as usize];
        for (n, call) in calls.iter().enumerate() {
            let item = &scene.items[call.item];
            let mv = view * item.model;
            let nm = Mat4::from_mat3(glam::DMat3::from_mat4(mv).inverse().transpose().as_mat3());
            let m = &item.material;
            let d = DrawU {
                model_view: f(mv),
                normal_matrix: nm.to_cols_array_2d(),
                color: [m.color[0], m.color[1], m.color[2], m.opacity],
                specular: [m.specular[0], m.specular[1], m.specular[2], m.shininess],
                flags: [
                    m.vertex_colors as u8 as f32,
                    call.flip as u8 as f32,
                    call.double as u8 as f32,
                    0.0,
                ],
            };
            let o = n * DRAW_STRIDE as usize;
            draw_data[o..o + std::mem::size_of::<DrawU>()].copy_from_slice(bytemuck::bytes_of(&d));
        }
        self.queue.write_buffer(&self.draw_buf, 0, &draw_data);
        for call in &calls {
            self.pipeline(call.key);
        }

        let bg = if cam.dark { 0x22 as f64 / 255.0 } else { 1.0 };
        let resolve_view = self.resolve.as_ref().unwrap().create_view(&Default::default());
        let msaa_view = self.msaa.as_ref().map(|t| t.create_view(&Default::default()));
        let depth_view = self.depth.as_ref().unwrap().create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let (view, resolve_target) = match &msaa_view {
                Some(m) => (m, Some(&resolve_view)),
                None => (&resolve_view, None),
            };
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg,
                            g: bg,
                            b: bg,
                            a: 1.0,
                        }),
                        store: if msaa_view.is_some() { wgpu::StoreOp::Discard } else { wgpu::StoreOp::Store },
                    },
                })],
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
            });
            pass.set_bind_group(0, &self.frame_bg, &[]);
            for (n, call) in calls.iter().enumerate() {
                let pipe = &self.pipes[&call.key];
                pass.set_pipeline(pipe);
                pass.set_bind_group(1, &self.draw_bg, &[(n as u64 * DRAW_STRIDE) as u32]);
                let mesh = &self.meshes[scene.items[call.item].mesh];
                pass.set_vertex_buffer(0, mesh.pos.slice(..));
                pass.set_vertex_buffer(1, mesh.nrm.slice(..));
                pass.set_vertex_buffer(2, mesh.col.slice(..));
                match &mesh.idx {
                    Some((ib, n)) => {
                        pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                        pass.draw_indexed(0..*n, 0, 0..1);
                    }
                    None => pass.draw(0..mesh.count, 0..1),
                }
            }
        }
        let padded = self.padded_row();
        let readback = self.readback.as_ref().unwrap();
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: self.resolve.as_ref().unwrap(),
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        let t1 = Instant::now();
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.unwrap());
        self.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let t2 = Instant::now();
        {
            let data = slice.get_mapped_range().unwrap();
            let row = (width * 4) as usize;
            out.resize(row * height as usize, 0);
            for y in 0..height as usize {
                out[y * row..(y + 1) * row].copy_from_slice(&data[y * padded as usize..y * padded as usize + row]);
            }
        }
        readback.unmap();
        let t3 = Instant::now();
        Timings {
            encode_ms: (t1 - t0).as_secs_f64() * 1e3,
            gpu_and_map_ms: (t2 - t1).as_secs_f64() * 1e3,
            copy_ms: (t3 - t2).as_secs_f64() * 1e3,
        }
    }
}
