use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::particle_pipeline::{
    ParticleFrame2D, ParticleFrame3D, ParticleInstance2D, ParticleInstance3D, ParticlePipelines,
};
use super::{ImageRef, RenderItem, SceneShaderState};

use crate::libs::renderable::{self, render as r3d};

static VSYNC_ON: AtomicBool = AtomicBool::new(false);
static PRESENT_MODE_DIRTY: AtomicBool = AtomicBool::new(false);

fn clamp_scissor(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    fb_w: u32,
    fb_h: u32,
) -> Option<(u32, u32, u32, u32)> {
    let mut x = x;
    let mut y = y;
    let mut w = w;
    let mut h = h;
    if x < 0.0 {
        w += x;
        x = 0.0;
    }
    if y < 0.0 {
        h += y;
        y = 0.0;
    }
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let fb_w_f = fb_w as f32;
    let fb_h_f = fb_h as f32;
    if x >= fb_w_f || y >= fb_h_f {
        return None;
    }
    let xi = x as u32;
    let yi = y as u32;
    let max_w = fb_w.saturating_sub(xi);
    let max_h = fb_h.saturating_sub(yi);
    let wi = (w as u32).min(max_w);
    let hi = (h as u32).min(max_h);
    if wi == 0 || hi == 0 {
        return None;
    }
    Some((xi, yi, wi, hi))
}

pub fn set_vsync(on: bool) {
    let prev = VSYNC_ON.swap(on, Ordering::Relaxed);
    if prev != on {
        PRESENT_MODE_DIRTY.store(true, Ordering::Relaxed);
    }
}

pub fn get_vsync() -> bool {
    VSYNC_ON.load(Ordering::Relaxed)
}

pub fn take_present_mode_dirty() -> bool {
    PRESENT_MODE_DIRTY.swap(false, Ordering::Relaxed)
}

static POWER_MODE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn set_power_mode_quality() {
    POWER_MODE.store(0, Ordering::Relaxed);
}

pub fn set_power_mode_performance() {
    POWER_MODE.store(1, Ordering::Relaxed);
}

pub fn set_power_mode_auto() {
    POWER_MODE.store(2, Ordering::Relaxed);
}

pub fn power_mode_label() -> &'static str {
    match POWER_MODE.load(Ordering::Relaxed) {
        0 => "Quality",
        1 => "Performance",
        _ => "Auto",
    }
}

pub fn power_preference() -> wgpu::PowerPreference {
    match POWER_MODE.load(Ordering::Relaxed) {
        0 => wgpu::PowerPreference::HighPerformance,
        1 => wgpu::PowerPreference::LowPower,
        _ => wgpu::PowerPreference::default(),
    }
}

fn vsync_preference() -> bool {
    VSYNC_ON.load(Ordering::Relaxed)
}

fn pick_present_mode(
    available: &[wgpu::PresentMode],
    vsync: bool,
) -> wgpu::PresentMode {
    let prefer_order: &[wgpu::PresentMode] = if vsync {
        &[
            wgpu::PresentMode::Fifo,
            wgpu::PresentMode::FifoRelaxed,
            wgpu::PresentMode::Mailbox,
            wgpu::PresentMode::Immediate,
        ]
    } else {
        &[
            wgpu::PresentMode::Immediate,
            wgpu::PresentMode::Mailbox,
            wgpu::PresentMode::FifoRelaxed,
            wgpu::PresentMode::Fifo,
        ]
    };
    for want in prefer_order {
        if available.iter().any(|m| m == want) {
            return *want;
        }
    }
    available.first().copied().unwrap_or(wgpu::PresentMode::Fifo)
}

#[derive(Clone, Debug)]
pub struct GpuMeta {
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    pub backend: String,
    pub driver: String,
    pub driver_info: String,
    pub device_type: String,
    pub max_texture_size: u32,
    pub max_buffer_size: u64,
    pub max_bind_groups: u32,
    pub max_vertex_buffers: u32,
    pub max_compute_workgroup_size: [u32; 3],
}

pub static GPU_META: OnceLock<GpuMeta> = OnceLock::new();

pub static GPU_DEVICE: OnceLock<Arc<wgpu::Device>> = OnceLock::new();
pub static GPU_QUEUE: OnceLock<Arc<wgpu::Queue>> = OnceLock::new();

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct UniData {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
    pub resolution: [f32; 2],
    pub time: f32,
    pub shape: u32,
    pub params: [[f32; 4]; 4],
}

pub const FRAGMENT_PRELUDE: &str = r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct RuzitUni {
    pos: vec2<f32>,
    size: vec2<f32>,
    color: vec4<f32>,
    resolution: vec2<f32>,
    time: f32,
    shape: u32,
    params: array<vec4<f32>, 4>,
};

@group(0) @binding(0) var<uniform> U: RuzitUni;

fn p(idx: u32) -> f32 {
    let v = U.params[idx >> 2u];
    let c = idx & 3u;
    if (c == 0u) { return v.x; }
    if (c == 1u) { return v.y; }
    if (c == 2u) { return v.z; }
    return v.w;
}

@group(0) @binding(1) var IMG: texture_2d<f32>;
@group(0) @binding(2) var IMG_SAMP: sampler;

fn ruzit_inside_shape(uv: vec2<f32>, shape: u32) -> bool {
    if (shape == 0u) {
        return true;
    }
    if (shape == 1u) {
        let d = uv - vec2<f32>(0.5);
        return dot(d, d) <= 0.25;
    }
    if (shape == 2u) {
        let dx = abs(uv.x - 0.5);
        return dx <= uv.y * 0.5 + 0.0001;
    }
    return true;
}

fn ruzit_apply_alpha(color: vec3<f32>) -> vec4<f32> {
    return vec4<f32>(color, U.color.a);
}
"#;

const VERTEX_WGSL: &str = r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct RuzitUni {
    pos: vec2<f32>,
    size: vec2<f32>,
    color: vec4<f32>,
    resolution: vec2<f32>,
    time: f32,
    shape: u32,
    params: array<vec4<f32>, 4>,
};

@group(0) @binding(0) var<uniform> U: RuzitUni;

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 1.0),
    );
    let q = quad[vid];
    let world = U.pos + q * U.size;
    let x = (world.x / U.resolution.x) * 2.0 - 1.0;
    let y = 1.0 - (world.y / U.resolution.y) * 2.0;
    var out: VsOut;
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = q;
    return out;
}
"#;

const FULLSCREEN_VERTEX_WGSL: &str = r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

struct RuzitUni {
    pos: vec2<f32>,
    size: vec2<f32>,
    color: vec4<f32>,
    resolution: vec2<f32>,
    time: f32,
    shape: u32,
    params: array<vec4<f32>, 4>,
};

@group(0) @binding(0) var<uniform> U: RuzitUni;

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0),
        vec2<f32>(1.0, -1.0),
        vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    let q = quad[vid];
    var out: VsOut;
    out.clip = vec4<f32>(q.x, q.y, 0.0, 1.0);
    out.uv = vec2<f32>((q.x + 1.0) * 0.5, 1.0 - (q.y + 1.0) * 0.5);
    return out;
}
"#;

const DEFAULT_FRAGMENT_WGSL: &str = r#"
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if (!ruzit_inside_shape(in.uv, U.shape)) {
        discard;
    }
    let tex = textureSample(IMG, IMG_SAMP, in.uv);
    return tex * U.color;
}
"#;

pub struct LiveTextureSlot {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub last_version: u64,
}

pub struct GpuState {
    pub device: Arc<wgpu::Device>,
    pub queue: Arc<wgpu::Queue>,
    pub adapter: wgpu::Adapter,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub size: (u32, u32),

    bind_group_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,

    vs_module: wgpu::ShaderModule,

    default_pipeline: wgpu::RenderPipeline,

    pipelines: HashMap<u64, wgpu::RenderPipeline>,

    uniform_buffers: Vec<wgpu::Buffer>,

    sampler: wgpu::Sampler,

    white_view: wgpu::TextureView,

    image_textures: HashMap<u64, wgpu::TextureView>,
    live_textures: HashMap<u64, LiveTextureSlot>,

    last_parts_version: u64,
    last_instance_count: usize,

    cached_2d_bind_groups: Vec<wgpu::BindGroup>,
    last_2d_item_count: usize,
    last_2d_image_ids: Vec<u64>,

    fullscreen_vs: wgpu::ShaderModule,

    skybox_pipelines: HashMap<u64, wgpu::RenderPipeline>,

    post_pipelines: HashMap<u64, wgpu::RenderPipeline>,

    skybox_uniform: wgpu::Buffer,
    post_uniform: wgpu::Buffer,

    skybox_bind_group: wgpu::BindGroup,

    post_bind_group: Option<wgpu::BindGroup>,

    scene_texture: Option<wgpu::Texture>,
    scene_view: Option<wgpu::TextureView>,
    scene_size: (u32, u32),

    depth_texture: Option<wgpu::Texture>,
    depth_view: Option<wgpu::TextureView>,
    depth_size: (u32, u32),

    bind_group_layout_3d: wgpu::BindGroupLayout,
    pipeline_layout_3d: wgpu::PipelineLayout,

    default_pipeline_3d: wgpu::RenderPipeline,

    pipelines_3d: HashMap<u64, wgpu::RenderPipeline>,

    frame_uniform: wgpu::Buffer,

    instance_buffer: wgpu::Buffer,
    instance_capacity_bytes: u64,
    instance_stride: u64,

    bind_group_3d_cache: HashMap<u64, wgpu::BindGroup>,
    seen_storage_version: u64,
    default_storage_buffer: wgpu::Buffer,

    cube_vertex: wgpu::Buffer,
    cube_index: wgpu::Buffer,
    cube_index_count: u32,
    sphere_vertex: wgpu::Buffer,
    sphere_index: wgpu::Buffer,
    sphere_index_count: u32,

    model_buffers: HashMap<u64, ModelBuffers>,

    particles: ParticlePipelines,
}

struct ModelBuffers {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    index_count: u32,
}

impl GpuState {
    pub fn new(window: Arc<winit::window::Window>) -> Result<Self, String> {
        let size = window.inner_size();

        let backends = if cfg!(windows) {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::PRIMARY
        };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });

        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: power_preference(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or("no compatible GPU adapter found")?;

        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("Ruzit Device"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                memory_hints: wgpu::MemoryHints::Performance,
            },
            None,
        ))
        .map_err(|e| format!("request_device: {e}"))?;
        let instance_stride = {
            let raw = std::mem::size_of::<r3d::InstanceUniform3D>() as u64;
            let align = device.limits().min_uniform_buffer_offset_alignment as u64;
            if align == 0 {
                raw
            } else {
                ((raw + align - 1) / align) * align
            }
        };
        let instance_initial_capacity = instance_stride * 64;

        let device_arc = Arc::new(device);
        let queue_arc = Arc::new(queue);
        let _ = GPU_DEVICE.set(device_arc.clone());
        let _ = GPU_QUEUE.set(queue_arc.clone());
        let device = device_arc;
        let queue = queue_arc;
        let max_dim = device.limits().max_texture_dimension_2d;
        let limits = device.limits();
        let info = adapter.get_info();
        let _ = GPU_META.set(GpuMeta {
            name: info.name.clone(),
            vendor: info.vendor,
            device: info.device,
            backend: format!("{:?}", info.backend),
            driver: info.driver.clone(),
            driver_info: info.driver_info.clone(),
            device_type: format!("{:?}", info.device_type),
            max_texture_size: limits.max_texture_dimension_2d,
            max_buffer_size: limits.max_buffer_size,
            max_bind_groups: limits.max_bind_groups,
            max_vertex_buffers: limits.max_vertex_buffers,
            max_compute_workgroup_size: [
                limits.max_compute_workgroup_size_x,
                limits.max_compute_workgroup_size_y,
                limits.max_compute_workgroup_size_z,
            ],
        });
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or_else(|| caps.formats[0]);
        let chosen_present = pick_present_mode(&caps.present_modes, vsync_preference());
        eprintln!(
            "[Ruzit] swapchain present_mode = {:?}  (available: {:?})",
            chosen_present, caps.present_modes
        );
        eprintln!(
            "[Ruzit] adapter power preference = {} ({:?})",
            power_mode_label(),
            power_preference()
        );
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.clamp(1, max_dim),
            height: size.height.clamp(1, max_dim),
            present_mode: chosen_present,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Ruzit GUI bind group layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: NonZeroU64::new(std::mem::size_of::<UniData>() as u64),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ruzit GUI pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });

        let vs_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ruzit GUI vertex"),
            source: wgpu::ShaderSource::Wgsl(VERTEX_WGSL.into()),
        });
        let default_fs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ruzit GUI default fragment"),
            source: wgpu::ShaderSource::Wgsl(
                format!("{FRAGMENT_PRELUDE}\n{DEFAULT_FRAGMENT_WGSL}").into(),
            ),
        });
        let default_pipeline = build_pipeline(
            &device,
            &pipeline_layout,
            &vs_module,
            &default_fs,
            config.format,
            Some(wgpu::TextureFormat::Depth32Float),
        );

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Ruzit GUI sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let white_view = create_white_texture(&device, &queue);

        let fullscreen_vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ruzit GUI fullscreen vertex"),
            source: wgpu::ShaderSource::Wgsl(FULLSCREEN_VERTEX_WGSL.into()),
        });
        let skybox_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ruzit GUI skybox uni"),
            contents: bytemuck::bytes_of(&UniData::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let post_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ruzit GUI post uni"),
            contents: bytemuck::bytes_of(&UniData::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let skybox_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ruzit GUI skybox bind"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: skybox_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&white_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });

        let bind_group_layout_3d =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Ruzit 3D bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(
                                std::mem::size_of::<r3d::FrameUniform3D>() as u64,
                            ),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: true,
                            min_binding_size: NonZeroU64::new(std::mem::size_of::<
                                r3d::InstanceUniform3D,
                            >()
                                as u64),
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(4),
                        },
                        count: None,
                    },
                ],
            });
        let pipeline_layout_3d = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ruzit 3D pipeline layout"),
            bind_group_layouts: &[&bind_group_layout_3d],
            push_constant_ranges: &[],
        });
        let vs_module_3d = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ruzit 3D vertex"),
            source: wgpu::ShaderSource::Wgsl(r3d::VERTEX_WGSL_3D.into()),
        });
        let default_fs_3d = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Ruzit 3D default fragment"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    "{}\n{}",
                    r3d::FRAGMENT_PRELUDE_3D,
                    r3d::DEFAULT_FRAGMENT_WGSL_3D
                )
                .into(),
            ),
        });
        let default_pipeline_3d = r3d::build_pipeline_3d(
            &device,
            &pipeline_layout_3d,
            &vs_module_3d,
            &default_fs_3d,
            config.format,
            wgpu::TextureFormat::Depth32Float,
        );

        let frame_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ruzit 3D frame uni"),
            contents: bytemuck::bytes_of(&r3d::FrameUniform3D::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ruzit 3D inst (shared)"),
            size: instance_initial_capacity,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cube_mesh = renderable::mesh::cube();
        let cube_vertex = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ruzit cube vertices"),
            contents: bytemuck::cast_slice(&cube_mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let cube_index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ruzit cube indices"),
            contents: bytemuck::cast_slice(&cube_mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let cube_index_count = cube_mesh.indices.len() as u32;
        let sphere_mesh = renderable::mesh::sphere(16, 32);
        let sphere_vertex = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ruzit sphere vertices"),
            contents: bytemuck::cast_slice(&sphere_mesh.vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let sphere_index = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ruzit sphere indices"),
            contents: bytemuck::cast_slice(&sphere_mesh.indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        let sphere_index_count = sphere_mesh.indices.len() as u32;

        let default_storage_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ruzit 3D storage default"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let particles = ParticlePipelines::new(
            &device,
            config.format,
            wgpu::TextureFormat::Depth32Float,
        );

        Ok(Self {
            device,
            queue,
            adapter,
            surface,
            config,
            size: (size.width, size.height),
            bind_group_layout,
            pipeline_layout,
            vs_module,
            default_pipeline,
            pipelines: HashMap::new(),
            uniform_buffers: Vec::new(),
            sampler,
            white_view,
            image_textures: HashMap::new(),
            live_textures: HashMap::new(),
            last_parts_version: 0,
            last_instance_count: 0,
            cached_2d_bind_groups: Vec::new(),
            last_2d_item_count: 0,
            last_2d_image_ids: Vec::new(),
            fullscreen_vs,
            skybox_pipelines: HashMap::new(),
            post_pipelines: HashMap::new(),
            skybox_uniform,
            post_uniform,
            skybox_bind_group,
            post_bind_group: None,
            scene_texture: None,
            scene_view: None,
            scene_size: (0, 0),
            depth_texture: None,
            depth_view: None,
            depth_size: (0, 0),
            bind_group_layout_3d,
            pipeline_layout_3d,
            default_pipeline_3d,
            pipelines_3d: HashMap::new(),
            frame_uniform,
            instance_buffer,
            instance_capacity_bytes: instance_initial_capacity,
            instance_stride,
            bind_group_3d_cache: HashMap::new(),
            seen_storage_version: 0,
            default_storage_buffer,
            cube_vertex,
            cube_index,
            cube_index_count,
            sphere_vertex,
            sphere_index,
            sphere_index_count,
            model_buffers: HashMap::new(),
            particles,
        })
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        let max_dim = self.device.limits().max_texture_dimension_2d;
        let cw = w.min(max_dim);
        let ch = h.min(max_dim);
        self.config.width = cw;
        self.config.height = ch;
        self.size = (cw, ch);
        self.surface.configure(&self.device, &self.config);

        self.scene_texture = None;
        self.scene_view = None;
        self.post_bind_group = None;
        self.scene_size = (0, 0);
        self.depth_texture = None;
        self.depth_view = None;
        self.depth_size = (0, 0);
    }

    fn ensure_pipeline(&mut self, shader_id: u64, wgsl: &str) -> bool {
        if self.pipelines.contains_key(&shader_id) {
            return true;
        }
        let label = format!("Ruzit GUI user shader #{shader_id}");

        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&label),
                source: wgpu::ShaderSource::Wgsl(wgsl.to_string().into()),
            });
        if let Some(err) = pollster::block_on(self.device.pop_error_scope()) {
            eprintln!("[GUI] shader #{shader_id} compile failed: {err}");
            return false;
        }
        let pipeline = build_pipeline(
            &self.device,
            &self.pipeline_layout,
            &self.vs_module,
            &module,
            self.config.format,
            Some(wgpu::TextureFormat::Depth32Float),
        );
        self.pipelines.insert(shader_id, pipeline);
        true
    }

    fn ensure_uniform_buffers(&mut self, n: usize) {
        while self.uniform_buffers.len() < n {
            let buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("Ruzit GUI uni"),
                    contents: bytemuck::bytes_of(&UniData::zeroed()),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                });
            self.uniform_buffers.push(buffer);
        }
    }

    fn ensure_skybox_pipeline(&mut self, shader_id: u64, wgsl: &str) -> bool {
        if self.skybox_pipelines.contains_key(&shader_id) {
            return true;
        }
        let label = format!("Ruzit skybox shader #{shader_id}");
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&label),
                source: wgpu::ShaderSource::Wgsl(wgsl.to_string().into()),
            });
        if let Some(err) = pollster::block_on(self.device.pop_error_scope()) {
            eprintln!("[GUI] skybox shader #{shader_id} compile failed: {err}");
            return false;
        }

        let pipeline = build_pipeline(
            &self.device,
            &self.pipeline_layout,
            &self.fullscreen_vs,
            &module,
            self.config.format,
            Some(wgpu::TextureFormat::Depth32Float),
        );
        self.skybox_pipelines.insert(shader_id, pipeline);
        true
    }

    fn ensure_post_pipeline(&mut self, shader_id: u64, wgsl: &str) -> bool {
        if self.post_pipelines.contains_key(&shader_id) {
            return true;
        }
        let label = format!("Ruzit post shader #{shader_id}");
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&label),
                source: wgpu::ShaderSource::Wgsl(wgsl.to_string().into()),
            });
        if let Some(err) = pollster::block_on(self.device.pop_error_scope()) {
            eprintln!("[GUI] post shader #{shader_id} compile failed: {err}");
            return false;
        }

        let pipeline = build_pipeline(
            &self.device,
            &self.pipeline_layout,
            &self.fullscreen_vs,
            &module,
            self.config.format,
            None,
        );
        self.post_pipelines.insert(shader_id, pipeline);
        true
    }

    fn ensure_scene_target(&mut self, w: u32, h: u32) {
        let want = (w.max(1), h.max(1));
        if self.scene_size == want && self.scene_texture.is_some() {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ruzit scene target"),
            size: wgpu::Extent3d {
                width: want.0,
                height: want.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let post_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Ruzit GUI post bind"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: self.post_uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.scene_texture = Some(texture);
        self.scene_view = Some(view);
        self.post_bind_group = Some(post_bind_group);
        self.scene_size = want;
    }

    fn write_scene_uniform(&self, buffer: &wgpu::Buffer, state: &SceneShaderState, time: f32) {
        let res = [self.size.0 as f32, self.size.1 as f32];
        let mut params = [[0.0_f32; 4]; 4];
        {
            let p = state.params.lock().unwrap();
            for j in 0..16 {
                params[j / 4][j % 4] = p[j];
            }
        }
        let data = UniData {
            pos: [0.0, 0.0],
            size: res,
            color: [1.0, 1.0, 1.0, 1.0],
            resolution: res,
            time,
            shape: 0,
            params,
        };
        self.queue
            .write_buffer(buffer, 0, bytemuck::bytes_of(&data));
    }

    fn ensure_depth_target(&mut self, w: u32, h: u32) {
        let want = (w.max(1), h.max(1));
        if self.depth_size == want && self.depth_texture.is_some() {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ruzit 3D depth"),
            size: wgpu::Extent3d {
                width: want.0,
                height: want.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.depth_texture = Some(texture);
        self.depth_view = Some(view);
        self.depth_size = want;
    }

    fn ensure_pipeline_3d(&mut self, shader_id: u64, wgsl: &str) -> bool {
        if self.pipelines_3d.contains_key(&shader_id) {
            return true;
        }
        let label = format!("Ruzit 3D user shader #{shader_id}");
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(&label),
                source: wgpu::ShaderSource::Wgsl(wgsl.to_string().into()),
            });
        if let Some(err) = pollster::block_on(self.device.pop_error_scope()) {
            eprintln!("[Renderable] 3D shader #{shader_id} compile failed: {err}");
            return false;
        }

        let pipeline = r3d::build_pipeline_3d(
            &self.device,
            &self.pipeline_layout_3d,
            &module,
            &module,
            self.config.format,
            wgpu::TextureFormat::Depth32Float,
        );
        self.pipelines_3d.insert(shader_id, pipeline);
        true
    }

    fn ensure_model_buffers(&mut self, model: &renderable::ModelRef) {
        if self.model_buffers.contains_key(&model.id) {
            return;
        }
        let vertex = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Ruzit model vertices"),
                contents: bytemuck::cast_slice(model.vertices.as_slice()),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let index = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Ruzit model indices"),
                contents: bytemuck::cast_slice(model.indices.as_slice()),
                usage: wgpu::BufferUsages::INDEX,
            });
        self.model_buffers.insert(
            model.id,
            ModelBuffers {
                vertex,
                index,
                index_count: model.indices.len() as u32,
            },
        );
    }

    fn evict_unused_resources(&mut self, parts: &[renderable::PartRender], items: &[RenderItem]) {
        use std::collections::HashSet;
        let mut live_models: HashSet<u64> = HashSet::new();
        let mut live_textures: HashSet<u64> = HashSet::new();
        for part in parts {
            if let Some(model) = &part.model {
                live_models.insert(model.id);
            }
            if let Some(tex) = &part.texture {
                live_textures.insert(tex.id);
            }
        }
        for item in items {
            if let Some(img) = &item.image {
                live_textures.insert(img.id);
            }
        }
        self.model_buffers.retain(|id, _| live_models.contains(id));
        self.image_textures
            .retain(|id, _| live_textures.contains(id));
        self.live_textures
            .retain(|id, _| live_textures.contains(id));
        self.bind_group_3d_cache
            .retain(|key, _| *key == 0 || live_textures.contains(key));
    }

    fn ensure_instance_capacity(&mut self, count: usize) {
        let needed = (count as u64).max(1) * self.instance_stride;
        if needed <= self.instance_capacity_bytes {
            return;
        }
        let mut new_cap = self.instance_capacity_bytes.max(self.instance_stride);
        while new_cap < needed {
            new_cap = new_cap.saturating_mul(2);
        }
        self.instance_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ruzit 3D inst (shared)"),
            size: new_cap,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.instance_capacity_bytes = new_cap;
        self.bind_group_3d_cache.clear();
    }

    fn ensure_part_texture(&mut self, tex: &renderable::PartTextureRef) -> &wgpu::TextureView {
        if let Some(live_arc) = &tex.live {
            let entry = self.live_textures.entry(tex.id).or_insert_with(|| {
                let buf = live_arc.lock().unwrap();
                let w = buf.width.max(1);
                let h = buf.height.max(1);
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Ruzit DynImg texture"),
                    size: wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                LiveTextureSlot {
                    texture,
                    view,
                    width: w,
                    height: h,
                    last_version: u64::MAX,
                }
            });
            let buf = live_arc.lock().unwrap();
            if buf.version != entry.last_version
                && buf.width == entry.width
                && buf.height == entry.height
                && (buf.bytes.len() as u32) >= 4 * entry.width * entry.height
            {
                self.queue.write_texture(
                    wgpu::ImageCopyTexture {
                        texture: &entry.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &buf.bytes,
                    wgpu::ImageDataLayout {
                        offset: 0,
                        bytes_per_row: Some(4 * entry.width),
                        rows_per_image: Some(entry.height),
                    },
                    wgpu::Extent3d {
                        width: entry.width,
                        height: entry.height,
                        depth_or_array_layers: 1,
                    },
                );
                entry.last_version = buf.version;
            }
            return &self.live_textures.get(&tex.id).unwrap().view;
        }
        if !self.image_textures.contains_key(&tex.id) {
            let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Ruzit 3D texture"),
                size: wgpu::Extent3d {
                    width: tex.width.max(1),
                    height: tex.height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            self.queue.write_texture(
                wgpu::ImageCopyTexture {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &tex.data,
                wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * tex.width),
                    rows_per_image: Some(tex.height),
                },
                wgpu::Extent3d {
                    width: tex.width,
                    height: tex.height,
                    depth_or_array_layers: 1,
                },
            );
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            self.image_textures.insert(tex.id, view);
        }
        self.image_textures.get(&tex.id).unwrap()
    }

    fn ensure_image(&mut self, image: &ImageRef) {
        if self.image_textures.contains_key(&image.id) {
            return;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Ruzit GUI image"),
            size: wgpu::Extent3d {
                width: image.width.max(1),
                height: image.height.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,

            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &image.data,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * image.width),
                rows_per_image: Some(image.height),
            },
            wgpu::Extent3d {
                width: image.width,
                height: image.height,
                depth_or_array_layers: 1,
            },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.image_textures.insert(image.id, view);
    }

    pub fn render(&mut self, items: &[RenderItem], time: f32, clear: [f32; 3]) {
        if take_present_mode_dirty() {
            let caps = self.surface.get_capabilities(&self.adapter);
            self.config.present_mode =
                pick_present_mode(&caps.present_modes, vsync_preference());
            self.surface.configure(&self.device, &self.config);
        }
        let surface_texture = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    Ok(t) => t,
                    Err(e) => {
                        eprintln!("[GUI] surface acquire after reconfigure: {e}");
                        return;
                    }
                }
            }
            Err(e) => {
                eprintln!("[GUI] surface acquire: {e}");
                return;
            }
        };
        let surface_view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let parts_arc = renderable::snapshot();
        let parts: &[renderable::PartRender] = parts_arc.as_slice();
        let skybox = super::skybox_snapshot();
        let post_effect = super::post_effect_snapshot();

        if let Some(sb) = &skybox {
            self.ensure_skybox_pipeline(sb.id, &sb.wgsl);
        }
        if let Some(pe) = &post_effect {
            self.ensure_post_pipeline(pe.id, &pe.wgsl);
            self.ensure_scene_target(self.size.0, self.size.1);
        }
        self.ensure_depth_target(self.size.0, self.size.1);
        self.ensure_uniform_buffers(items.len());
        self.ensure_instance_capacity(parts.len());
        self.evict_unused_resources(&parts, items);
        for item in items {
            if let Some(sh) = &item.active_shader {
                self.ensure_pipeline(sh.id, &sh.wgsl);
            }
            if let Some(img) = &item.image {
                self.ensure_image(img);
            }
        }
        for part in parts {
            if let Some(sh) = &part.active_shader {
                self.ensure_pipeline_3d(sh.id, &sh.wgsl);
            }
            if let Some(model) = &part.model {
                self.ensure_model_buffers(model);
            }
            if let Some(tex) = &part.texture {
                let _ = self.ensure_part_texture(tex);
            }
        }

        let effect_3d = renderable::effect_volume_snapshot();
        let ui_effects = super::ui_effect_volume_snapshot();
        for v in &effect_3d {
            if let Some(tex) = &v.texture {
                let _ = self.ensure_part_texture(tex);
            }
        }
        for v in &ui_effects {
            if let Some(tex) = &v.texture {
                let _ = self.ensure_part_texture(tex);
            }
        }

        let res = [self.size.0 as f32, self.size.1 as f32];

        let cam = renderable::camera_snapshot();
        let aspect = if self.size.1 > 0 {
            self.size.0 as f32 / self.size.1 as f32
        } else {
            1.0
        };
        let view = r3d::view_matrix(
            [
                cam.cframe.position.x,
                cam.cframe.position.y,
                cam.cframe.position.z,
            ],
            [
                cam.cframe.rotation.x,
                cam.cframe.rotation.y,
                cam.cframe.rotation.z,
            ],
        );
        let proj = r3d::perspective_matrix(cam.fov_deg, aspect, cam.near, cam.far);
        let view_proj = r3d::mat4_mul(proj, view);

        for (i, item) in items.iter().enumerate() {
            let alpha = (1.0 - item.transparency).clamp(0.0, 1.0);
            let color = [item.color.r, item.color.g, item.color.b, alpha];
            let mut params = [[0.0_f32; 4]; 4];
            if let Some(sh) = &item.active_shader {
                let p = sh.params.lock().unwrap();
                for j in 0..16 {
                    params[j / 4][j % 4] = p[j];
                }
            }

            let (pos_out, size_out, alpha_out) = match &item.billboard_anchor {
                Some(b) => {
                    let w = b.world_pos;
                    let v = [
                        view_proj[0][0] * w.x
                            + view_proj[0][1] * w.y
                            + view_proj[0][2] * w.z
                            + view_proj[0][3],
                        view_proj[1][0] * w.x
                            + view_proj[1][1] * w.y
                            + view_proj[1][2] * w.z
                            + view_proj[1][3],
                        view_proj[2][0] * w.x
                            + view_proj[2][1] * w.y
                            + view_proj[2][2] * w.z
                            + view_proj[2][3],
                        view_proj[3][0] * w.x
                            + view_proj[3][1] * w.y
                            + view_proj[3][2] * w.z
                            + view_proj[3][3],
                    ];
                    if v[3] <= 0.001 {
                        ([0.0_f32, 0.0_f32], [0.0_f32, 0.0_f32], 0.0_f32)
                    } else {
                        let ndc_x = v[0] / v[3];
                        let ndc_y = v[1] / v[3];
                        let sx = (ndc_x * 0.5 + 0.5) * res[0];
                        let sy = (1.0 - (ndc_y * 0.5 + 0.5)) * res[1];
                        let scale = if b.scale_with_camera {
                            let f = (cam.fov_deg.to_radians() * 0.5).tan();
                            let one_unit_in_px = res[1] * 0.5 / (v[3] * f).max(1e-4);
                            one_unit_in_px
                        } else {
                            1.0
                        };
                        let canvas_w = b.canvas_size.x * scale;
                        let canvas_h = b.canvas_size.y * scale;
                        let canvas_x = sx - canvas_w * 0.5;
                        let canvas_y = sy - canvas_h * 0.5;
                        let p_out = [
                            canvas_x + item.position.x * scale,
                            canvas_y + item.position.y * scale,
                        ];
                        let s_out = [item.size.x * scale, item.size.y * scale];
                        (p_out, s_out, alpha)
                    }
                }
                None => ([item.position.x, item.position.y], [item.size.x, item.size.y], alpha),
            };
            let color_out = [color[0], color[1], color[2], alpha_out];

            let data = UniData {
                pos: pos_out,
                size: size_out,
                color: color_out,
                resolution: res,
                time,
                shape: item.shape.shape_id(),
                params,
            };
            self.queue
                .write_buffer(&self.uniform_buffers[i], 0, bytemuck::bytes_of(&data));
        }

        if let Some(sb) = &skybox {
            self.write_scene_uniform(&self.skybox_uniform, sb, time);
        }
        if let Some(pe) = &post_effect {
            self.write_scene_uniform(&self.post_uniform, pe, time);
        }
        let lighting = renderable::lighting_snapshot();
        let frame = r3d::FrameUniform3D {
            view_proj: r3d::transpose4(view_proj),
            light_dir: normalize3([
                lighting.sun_direction.x,
                lighting.sun_direction.y,
                lighting.sun_direction.z,
            ]),
            time,
            camera_pos: [
                cam.cframe.position.x,
                cam.cframe.position.y,
                cam.cframe.position.z,
            ],
            frame_index: lighting.frame_index,
            sun_color: [
                lighting.sun_color.r,
                lighting.sun_color.g,
                lighting.sun_color.b,
            ],
            shadow_enabled: {
                let cfg = crate::libs::gpu::shadow_config();
                if cfg.enabled { 1 } else { 0 }
            },
            ambient: [lighting.ambient.r, lighting.ambient.g, lighting.ambient.b],
            shadow_strength: {
                let cfg = crate::libs::gpu::shadow_config();
                if cfg.enabled { (cfg.quality as f32 / 4096.0).clamp(0.3, 0.85) } else { 0.0 }
            },
            viewport: [self.size.0 as f32, self.size.1 as f32],
            shadow_softness: {
                let cfg = crate::libs::gpu::shadow_config();
                let base = 0.05_f32;
                let pcf = (cfg.pcf as f32 - 1.0).max(0.0);
                base * (1.0 + pcf * 0.8)
            },
            shadow_distance: crate::libs::gpu::shadow_config().distance.max(0.0),
            shadow_pcf: crate::libs::gpu::shadow_config().pcf,
            shadow_bias: crate::libs::gpu::shadow_config().bias,
            shadow_quality: crate::libs::gpu::shadow_config().quality,
            _shadow_pad: 0.0,
        };
        self.queue
            .write_buffer(&self.frame_uniform, 0, bytemuck::bytes_of(&frame));

        let parts_version = renderable::parts_version();
        let parts_dirty = parts_version != self.last_parts_version
            || parts.len() != self.last_instance_count;
        let any_animated_shader = parts.iter().any(|p| p.active_shader.is_some());
        if !parts.is_empty() && (parts_dirty || any_animated_shader) {
            let stride = self.instance_stride as usize;
            let inst_size = std::mem::size_of::<r3d::InstanceUniform3D>();
            let total = parts.len() * stride;
            let mut staging = vec![0u8; total];
            for (i, part) in parts.iter().enumerate() {
                let model_mat = r3d::part_model_matrix(
                    [
                        part.cframe.position.x,
                        part.cframe.position.y,
                        part.cframe.position.z,
                    ],
                    [
                        part.cframe.rotation.x,
                        part.cframe.rotation.y,
                        part.cframe.rotation.z,
                    ],
                    [part.size.x, part.size.y, part.size.z],
                );
                let color = [part.color.r, part.color.g, part.color.b, 1.0];
                let mut params = [[0.0_f32; 4]; 4];
                if let Some(sh) = &part.active_shader {
                    let p = sh.params.lock().unwrap();
                    for j in 0..16 {
                        params[j / 4][j % 4] = p[j];
                    }
                }
                let inst = r3d::InstanceUniform3D {
                    model: r3d::transpose4(model_mat),
                    color,
                    params,
                    flags: [
                        if part.cast_shadow { 1 } else { 0 },
                        if part.receive_shadow { 1 } else { 0 },
                        if part.lit { 1 } else { 0 },
                        0,
                    ],
                };
                let offset = i * stride;
                staging[offset..offset + inst_size].copy_from_slice(bytemuck::bytes_of(&inst));
            }
            self.queue.write_buffer(&self.instance_buffer, 0, &staging);
        }
        self.last_parts_version = parts_version;
        self.last_instance_count = parts.len();

        let current_image_ids: Vec<u64> = items
            .iter()
            .map(|i| i.image.as_ref().map(|img| img.id).unwrap_or(0))
            .collect();
        let bind_groups_dirty = items.len() != self.last_2d_item_count
            || current_image_ids != self.last_2d_image_ids
            || self.cached_2d_bind_groups.len() != items.len();
        if bind_groups_dirty {
            self.cached_2d_bind_groups = items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    let texture_view = match &item.image {
                        Some(img) => self.image_textures.get(&img.id).unwrap_or(&self.white_view),
                        None => &self.white_view,
                    };
                    self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("Ruzit 2D bind"),
                        layout: &self.bind_group_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: self.uniform_buffers[i].as_entire_binding(),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(texture_view),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::Sampler(&self.sampler),
                            },
                        ],
                    })
                })
                .collect();
            self.last_2d_item_count = items.len();
            self.last_2d_image_ids = current_image_ids;
        }
        let bind_groups_2d = &self.cached_2d_bind_groups;

        let storage_version = crate::libs::gpu::current_storage_version();
        if storage_version != self.seen_storage_version {
            self.seen_storage_version = storage_version;
            self.bind_group_3d_cache.clear();
        }
        let active_storage = crate::libs::gpu::current_storage();

        let inst_bind_size = NonZeroU64::new(std::mem::size_of::<r3d::InstanceUniform3D>() as u64);
        for part in parts {
            let key = part.texture.as_ref().map(|t| t.id).unwrap_or(0);
            if self.bind_group_3d_cache.contains_key(&key) {
                continue;
            }
            let texture_view = match &part.texture {
                Some(tex) => self.image_textures.get(&tex.id).unwrap_or(&self.white_view),
                None => &self.white_view,
            };
            let storage_buffer: &wgpu::Buffer = active_storage
                .as_deref()
                .unwrap_or(&self.default_storage_buffer);
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Ruzit 3D bind"),
                layout: &self.bind_group_layout_3d,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.frame_uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.instance_buffer,
                            offset: 0,
                            size: inst_bind_size,
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(texture_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: storage_buffer.as_entire_binding(),
                    },
                ],
            });
            self.bind_group_3d_cache.insert(key, bg);
        }


        for v in &effect_3d {
            if let Some(sh) = &v.active_shader {
                self.particles.ensure_pipeline_3d(&self.device, sh.id, &sh.wgsl);
            }
        }
        for v in &ui_effects {
            if let Some(sh) = &v.active_shader {
                self.particles.ensure_pipeline_2d(&self.device, sh.id, &sh.wgsl);
            }
        }

        let mut spans_3d: Vec<(u64, u32, u32, Option<u64>)> = Vec::new();
        let mut packed_3d: Vec<ParticleInstance3D> = Vec::new();
        for v in &effect_3d {
            if v.particles.is_empty() {
                continue;
            }
            let base = packed_3d.len() as u32;
            for p in &v.particles {
                packed_3d.push(ParticleInstance3D {
                    position_size: [p.position.x, p.position.y, p.position.z, p.size],
                    color: [p.color.r, p.color.g, p.color.b, p.alpha],
                    rotation_face: [
                        p.rotation,
                        if v.face_camera { 1.0 } else { 0.0 },
                        p.life_t,
                        0.0,
                    ],
                });
            }
            let count = packed_3d.len() as u32 - base;
            let tex_key = v.texture.as_ref().map(|t| t.id).unwrap_or(0);
            let shader_id = v.active_shader.as_ref().map(|s| s.id);
            spans_3d.push((tex_key, base, count, shader_id));
        }
        if !packed_3d.is_empty() {
            self.particles
                .ensure_3d_capacity(&self.device, packed_3d.len());
            self.queue.write_buffer(
                &self.particles.instance_3d_buffer,
                0,
                bytemuck::cast_slice(&packed_3d),
            );
        }
        let cam_rot = r3d::euler_rotation_matrix([
            cam.cframe.rotation.x,
            cam.cframe.rotation.y,
            cam.cframe.rotation.z,
        ]);
        let particle_frame_3d = ParticleFrame3D {
            view_proj: r3d::transpose4(view_proj),
            camera_right: [cam_rot[0][0], cam_rot[0][1], cam_rot[0][2], 0.0],
            camera_up: [cam_rot[1][0], cam_rot[1][1], cam_rot[1][2], 0.0],
            viewport: [self.size.0 as f32, self.size.1 as f32, time, 0.0],
        };
        self.queue.write_buffer(
            &self.particles.frame_3d_buffer,
            0,
            bytemuck::bytes_of(&particle_frame_3d),
        );

        let mut spans_2d: Vec<(u64, u32, u32, Option<u64>, i32)> = Vec::new();
        let mut packed_2d: Vec<ParticleInstance2D> = Vec::new();
        for v in &ui_effects {
            if v.particles.is_empty() {
                continue;
            }
            let base = packed_2d.len() as u32;
            for p in &v.particles {
                packed_2d.push(ParticleInstance2D {
                    pos_size_rot: [p.position.x, p.position.y, p.size, p.rotation],
                    color: [p.color.r, p.color.g, p.color.b, p.alpha],
                });
            }
            let count = packed_2d.len() as u32 - base;
            let tex_key = v.texture.as_ref().map(|t| t.id).unwrap_or(0);
            let shader_id = v.active_shader.as_ref().map(|s| s.id);
            spans_2d.push((tex_key, base, count, shader_id, v.z_index));
        }
        if !packed_2d.is_empty() {
            self.particles
                .ensure_2d_capacity(&self.device, packed_2d.len());
            self.queue.write_buffer(
                &self.particles.instance_2d_buffer,
                0,
                bytemuck::cast_slice(&packed_2d),
            );
        }
        let particle_frame_2d = ParticleFrame2D {
            resolution: [self.size.0 as f32, self.size.1 as f32, time, 0.0],
        };
        self.queue.write_buffer(
            &self.particles.frame_2d_buffer,
            0,
            bytemuck::bytes_of(&particle_frame_2d),
        );


        for v in &effect_3d {
            if v.particles.is_empty() {
                continue;
            }
            let key = v.texture.as_ref().map(|t| t.id).unwrap_or(0);
            if self.particles.bind_cache_3d.contains_key(&key) {
                continue;
            }
            let view = match &v.texture {
                Some(tex) => self.image_textures.get(&tex.id).unwrap_or(&self.white_view),
                None => &self.white_view,
            };
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Ruzit particle 3D bind"),
                layout: &self.particles.bind_group_layout_3d,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.particles.frame_3d_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.particles.instance_3d_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.particles.bind_cache_3d.insert(key, bg);
        }
        for v in &ui_effects {
            if v.particles.is_empty() {
                continue;
            }
            let key = v.texture.as_ref().map(|t| t.id).unwrap_or(0);
            if self.particles.bind_cache_2d.contains_key(&key) {
                continue;
            }
            let view = match &v.texture {
                Some(tex) => self.image_textures.get(&tex.id).unwrap_or(&self.white_view),
                None => &self.white_view,
            };
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Ruzit particle 2D bind"),
                layout: &self.particles.bind_group_layout_2d,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.particles.frame_2d_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.particles.instance_2d_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
            self.particles.bind_cache_2d.insert(key, bg);
        }

        let main_target: &wgpu::TextureView = if post_effect.is_some() {
            self.scene_view
                .as_ref()
                .expect("ensure_scene_target should have allocated it")
        } else {
            &surface_view
        };
        let depth_view = self
            .depth_view
            .as_ref()
            .expect("ensure_depth_target should have allocated it");

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Ruzit unified encoder"),
            });

        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Ruzit main pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: main_target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: clear[0] as f64,
                            g: clear[1] as f64,
                            b: clear[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            if let Some(sb) = &skybox {
                if let Some(pipeline) = self.skybox_pipelines.get(&sb.id) {
                    rpass.set_pipeline(pipeline);
                    rpass.set_bind_group(0, &self.skybox_bind_group, &[]);
                    rpass.draw(0..6, 0..1);
                }
            }

            for (i, part) in parts.iter().enumerate() {
                let pipeline = match &part.active_shader {
                    Some(sh) => self
                        .pipelines_3d
                        .get(&sh.id)
                        .unwrap_or(&self.default_pipeline_3d),
                    None => &self.default_pipeline_3d,
                };
                rpass.set_pipeline(pipeline);
                let key = part.texture.as_ref().map(|t| t.id).unwrap_or(0);
                let Some(bind_group) = self.bind_group_3d_cache.get(&key) else {
                    continue;
                };
                let dyn_offset = (i as u64 * self.instance_stride) as u32;
                rpass.set_bind_group(0, bind_group, &[dyn_offset]);

                let (vbuf, ibuf, idx_count) = match part.shape {
                    renderable::PartShape::Cube => {
                        (&self.cube_vertex, &self.cube_index, self.cube_index_count)
                    }
                    renderable::PartShape::Sphere => (
                        &self.sphere_vertex,
                        &self.sphere_index,
                        self.sphere_index_count,
                    ),
                    renderable::PartShape::Model => match &part.model {
                        Some(m) => match self.model_buffers.get(&m.id) {
                            Some(mb) => (&mb.vertex, &mb.index, mb.index_count),
                            None => continue,
                        },
                        None => continue,
                    },
                };
                rpass.set_vertex_buffer(0, vbuf.slice(..));
                rpass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                rpass.draw_indexed(0..idx_count, 0, 0..1);
            }


            if !spans_3d.is_empty() {
                for (tex_key, base, count, shader_id) in &spans_3d {
                    let pipeline = match shader_id {
                        Some(id) => self
                            .particles
                            .pipelines_3d
                            .get(id)
                            .unwrap_or(&self.particles.default_pipeline_3d),
                        None => &self.particles.default_pipeline_3d,
                    };
                    if let Some(bg) = self.particles.bind_cache_3d.get(tex_key) {
                        rpass.set_pipeline(pipeline);
                        rpass.set_bind_group(0, bg, &[]);
                        rpass.draw(0..6, *base..*base + *count);
                    }
                }
            }

            let win_w = self.config.width;
            let win_h = self.config.height;
            let full_scissor = (0u32, 0u32, win_w, win_h);
            let mut current_scissor = full_scissor;
            rpass.set_scissor_rect(0, 0, win_w, win_h);

            let mut draw_seq_2d: Vec<(i32, usize, bool)> =
                Vec::with_capacity(items.len() + spans_2d.len());
            for (i, item) in items.iter().enumerate() {
                draw_seq_2d.push((item.z_index, i, false));
            }
            for (i, span) in spans_2d.iter().enumerate() {
                draw_seq_2d.push((span.4, i, true));
            }
            draw_seq_2d.sort_by_key(|e| e.0);

            for (_, idx, is_particle) in &draw_seq_2d {
                if *is_particle {
                    let (tex_key, base, count, shader_id, _) = &spans_2d[*idx];
                    if current_scissor != full_scissor {
                        rpass.set_scissor_rect(0, 0, win_w, win_h);
                        current_scissor = full_scissor;
                    }
                    let pipeline = match shader_id {
                        Some(id) => self
                            .particles
                            .pipelines_2d
                            .get(id)
                            .unwrap_or(&self.particles.default_pipeline_2d),
                        None => &self.particles.default_pipeline_2d,
                    };
                    if let Some(bg) = self.particles.bind_cache_2d.get(tex_key) {
                        rpass.set_pipeline(pipeline);
                        rpass.set_bind_group(0, bg, &[]);
                        rpass.draw(0..6, *base..*base + *count);
                    }
                } else {
                    let i = *idx;
                    let item = &items[i];
                    let want = match item.clip_rect {
                        Some((x, y, w, h)) => clamp_scissor(x, y, w, h, win_w, win_h),
                        None => Some(full_scissor),
                    };
                    let scissor = match want {
                        Some(s) => s,
                        None => continue,
                    };
                    if scissor != current_scissor {
                        rpass.set_scissor_rect(scissor.0, scissor.1, scissor.2, scissor.3);
                        current_scissor = scissor;
                    }
                    let pipeline = match &item.active_shader {
                        Some(sh) => self.pipelines.get(&sh.id).unwrap_or(&self.default_pipeline),
                        None => &self.default_pipeline,
                    };
                    rpass.set_pipeline(pipeline);
                    rpass.set_bind_group(0, &bind_groups_2d[i], &[]);
                    rpass.draw(0..6, 0..1);
                }
            }
        }

        if let Some(pe) = &post_effect {
            if let (Some(pipeline), Some(post_bind)) = (
                self.post_pipelines.get(&pe.id),
                self.post_bind_group.as_ref(),
            ) {
                let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Ruzit post-effect pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &surface_view,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.0,
                                g: 0.0,
                                b: 0.0,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                rpass.set_pipeline(pipeline);
                rpass.set_bind_group(0, post_bind, &[]);
                rpass.draw(0..6, 0..1);
            }
        }

        let submit_start = std::time::Instant::now();
        self.queue.submit([encoder.finish()]);
        crate::libs::debug::record_gpu_submit(submit_start.elapsed().as_nanos() as u64);
        surface_texture.present();
    }
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-6 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, -1.0, 0.0]
    }
}

fn create_white_texture(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Ruzit GUI white"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::ImageCopyTexture {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[0xFFu8, 0xFF, 0xFF, 0xFF],
        wgpu::ImageDataLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

fn build_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    vs: &wgpu::ShaderModule,
    fs: &wgpu::ShaderModule,
    format: wgpu::TextureFormat,
    depth_format: Option<wgpu::TextureFormat>,
) -> wgpu::RenderPipeline {
    let depth_stencil = depth_format.map(|fmt| wgpu::DepthStencilState {
        format: fmt,
        depth_write_enabled: false,
        depth_compare: wgpu::CompareFunction::Always,
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Ruzit GUI pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: vs,
            entry_point: "vs_main",
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: fs,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::SrcAlpha,
                        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: wgpu::BlendComponent::OVER,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: wgpu::PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}
