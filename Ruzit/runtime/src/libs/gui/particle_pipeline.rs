use std::collections::HashMap;
use std::num::NonZeroU64;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct ParticleFrame3D {
    pub view_proj: [[f32; 4]; 4],
    pub camera_right: [f32; 4],
    pub camera_up: [f32; 4],
    pub viewport: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct ParticleInstance3D {
    pub position_size: [f32; 4],
    pub color: [f32; 4],
    pub rotation_face: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct ParticleFrame2D {
    pub resolution: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct ParticleInstance2D {
    pub pos_size_rot: [f32; 4],
    pub color: [f32; 4],
}

pub const PARTICLE_3D_PRELUDE: &str = r#"
struct ParticleFrame {
    view_proj: mat4x4<f32>,
    camera_right: vec4<f32>,
    camera_up: vec4<f32>,
    viewport: vec4<f32>,
};

struct ParticleInstance {
    position_size: vec4<f32>,
    color: vec4<f32>,
    rotation_face: vec4<f32>,
};

@group(0) @binding(0) var<uniform> F: ParticleFrame;
@group(0) @binding(1) var<storage, read> P: array<ParticleInstance>;
@group(0) @binding(2) var IMG: texture_2d<f32>;
@group(0) @binding(3) var IMG_SAMP: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) life_t: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> VsOut {
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(-0.5, -0.5),
        vec2<f32>( 0.5, -0.5),
        vec2<f32>(-0.5,  0.5),
        vec2<f32>( 0.5, -0.5),
        vec2<f32>( 0.5,  0.5),
        vec2<f32>(-0.5,  0.5),
    );
    let p = P[iid];
    let center = p.position_size.xyz;
    let size = p.position_size.w;
    let rot = p.rotation_face.x;
    let face = p.rotation_face.y > 0.5;

    let corner = quad[vid];
    let cs = cos(rot);
    let sn = sin(rot);
    let rotated = vec2<f32>(corner.x * cs - corner.y * sn, corner.x * sn + corner.y * cs);

    var world: vec3<f32>;
    if (face) {
        let right = F.camera_right.xyz;
        let up = F.camera_up.xyz;
        world = center + (right * rotated.x + up * rotated.y) * size;
    } else {
        world = center + vec3<f32>(rotated.x * size, rotated.y * size, 0.0);
    }

    var out: VsOut;
    out.clip = F.view_proj * vec4<f32>(world, 1.0);
    out.uv = corner + vec2<f32>(0.5, 0.5);
    out.color = p.color;
    out.life_t = p.rotation_face.z;
    return out;
}
"#;

pub const PARTICLE_3D_DEFAULT_FRAGMENT: &str = r#"
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(IMG, IMG_SAMP, in.uv);
    return vec4<f32>(tex.rgb * in.color.rgb, tex.a * in.color.a);
}
"#;

pub const PARTICLE_2D_PRELUDE: &str = r#"
struct ParticleFrame {
    resolution: vec4<f32>,
};

struct ParticleInstance {
    pos_size_rot: vec4<f32>,
    color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> F: ParticleFrame;
@group(0) @binding(1) var<storage, read> P: array<ParticleInstance>;
@group(0) @binding(2) var IMG: texture_2d<f32>;
@group(0) @binding(3) var IMG_SAMP: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) life_t: f32,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> VsOut {
    var quad = array<vec2<f32>, 6>(
        vec2<f32>(-0.5, -0.5),
        vec2<f32>( 0.5, -0.5),
        vec2<f32>(-0.5,  0.5),
        vec2<f32>( 0.5, -0.5),
        vec2<f32>( 0.5,  0.5),
        vec2<f32>(-0.5,  0.5),
    );
    let p = P[iid];
    let center = p.pos_size_rot.xy;
    let size = p.pos_size_rot.z;
    let rot = p.pos_size_rot.w;

    let corner = quad[vid];
    let cs = cos(rot);
    let sn = sin(rot);
    let rotated = vec2<f32>(corner.x * cs - corner.y * sn, corner.x * sn + corner.y * cs);
    let pixel = center + rotated * size;
    let x = (pixel.x / F.resolution.x) * 2.0 - 1.0;
    let y = 1.0 - (pixel.y / F.resolution.y) * 2.0;

    var out: VsOut;
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = corner + vec2<f32>(0.5, 0.5);
    out.color = p.color;
    out.life_t = 0.0;
    return out;
}
"#;

pub const PARTICLE_2D_DEFAULT_FRAGMENT: &str = r#"
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let tex = textureSample(IMG, IMG_SAMP, in.uv);
    return vec4<f32>(tex.rgb * in.color.rgb, tex.a * in.color.a);
}
"#;

pub struct ParticlePipelines {
    pub bind_group_layout_3d: wgpu::BindGroupLayout,
    pub bind_group_layout_2d: wgpu::BindGroupLayout,
    pub pipeline_layout_3d: wgpu::PipelineLayout,
    pub pipeline_layout_2d: wgpu::PipelineLayout,

    pub default_pipeline_3d: wgpu::RenderPipeline,
    pub default_pipeline_2d: wgpu::RenderPipeline,

    pub pipelines_3d: HashMap<u64, wgpu::RenderPipeline>,
    pub pipelines_2d: HashMap<u64, wgpu::RenderPipeline>,

    pub frame_3d_buffer: wgpu::Buffer,
    pub frame_2d_buffer: wgpu::Buffer,

    pub instance_3d_buffer: wgpu::Buffer,
    pub instance_3d_capacity: u64,
    pub instance_2d_buffer: wgpu::Buffer,
    pub instance_2d_capacity: u64,

    pub bind_cache_3d: HashMap<u64, wgpu::BindGroup>,
    pub bind_cache_2d: HashMap<u64, wgpu::BindGroup>,

    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
}

impl ParticlePipelines {
    pub fn new(
        device: &wgpu::Device,
        color_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        let bind_group_layout_3d =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Ruzit particle 3D bind layout"),
                entries: &particle_bind_entries(
                    std::mem::size_of::<ParticleFrame3D>() as u64,
                    std::mem::size_of::<ParticleInstance3D>() as u64,
                ),
            });
        let bind_group_layout_2d =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Ruzit particle 2D bind layout"),
                entries: &particle_bind_entries(
                    std::mem::size_of::<ParticleFrame2D>() as u64,
                    std::mem::size_of::<ParticleInstance2D>() as u64,
                ),
            });

        let pipeline_layout_3d = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ruzit particle 3D layout"),
            bind_group_layouts: &[&bind_group_layout_3d],
            push_constant_ranges: &[],
        });
        let pipeline_layout_2d = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Ruzit particle 2D layout"),
            bind_group_layouts: &[&bind_group_layout_2d],
            push_constant_ranges: &[],
        });

        let default_pipeline_3d = build_particle_pipeline_3d(
            device,
            &pipeline_layout_3d,
            PARTICLE_3D_DEFAULT_FRAGMENT,
            color_format,
            depth_format,
        );
        let default_pipeline_2d = build_particle_pipeline_2d(
            device,
            &pipeline_layout_2d,
            PARTICLE_2D_DEFAULT_FRAGMENT,
            color_format,
            depth_format,
        );

        let frame_3d_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ruzit particle 3D frame"),
            contents: bytemuck::bytes_of(&ParticleFrame3D::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let frame_2d_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ruzit particle 2D frame"),
            contents: bytemuck::bytes_of(&ParticleFrame2D::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });

        let initial_3d = (std::mem::size_of::<ParticleInstance3D>() as u64) * 64;
        let initial_2d = (std::mem::size_of::<ParticleInstance2D>() as u64) * 64;
        let instance_3d_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ruzit particle 3D instances"),
            size: initial_3d,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instance_2d_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Ruzit particle 2D instances"),
            size: initial_2d,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            bind_group_layout_3d,
            bind_group_layout_2d,
            pipeline_layout_3d,
            pipeline_layout_2d,
            default_pipeline_3d,
            default_pipeline_2d,
            pipelines_3d: HashMap::new(),
            pipelines_2d: HashMap::new(),
            frame_3d_buffer,
            frame_2d_buffer,
            instance_3d_buffer,
            instance_3d_capacity: initial_3d,
            instance_2d_buffer,
            instance_2d_capacity: initial_2d,
            bind_cache_3d: HashMap::new(),
            bind_cache_2d: HashMap::new(),
            color_format,
            depth_format,
        }
    }

    pub fn ensure_3d_capacity(&mut self, device: &wgpu::Device, particle_count: usize) {
        let needed = (particle_count.max(1) * std::mem::size_of::<ParticleInstance3D>()) as u64;
        if needed > self.instance_3d_capacity {
            let new_cap = needed.next_power_of_two().max(64);
            self.instance_3d_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Ruzit particle 3D instances"),
                size: new_cap,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_3d_capacity = new_cap;
            self.bind_cache_3d.clear();
        }
    }

    pub fn ensure_2d_capacity(&mut self, device: &wgpu::Device, particle_count: usize) {
        let needed = (particle_count.max(1) * std::mem::size_of::<ParticleInstance2D>()) as u64;
        if needed > self.instance_2d_capacity {
            let new_cap = needed.next_power_of_two().max(64);
            self.instance_2d_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Ruzit particle 2D instances"),
                size: new_cap,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instance_2d_capacity = new_cap;
            self.bind_cache_2d.clear();
        }
    }

    pub fn ensure_pipeline_3d(&mut self, device: &wgpu::Device, id: u64, user_wgsl: &str) {
        if self.pipelines_3d.contains_key(&id) {
            return;
        }
        let pipeline = build_particle_pipeline_3d(
            device,
            &self.pipeline_layout_3d,
            user_wgsl,
            self.color_format,
            self.depth_format,
        );
        self.pipelines_3d.insert(id, pipeline);
    }

    pub fn ensure_pipeline_2d(&mut self, device: &wgpu::Device, id: u64, user_wgsl: &str) {
        if self.pipelines_2d.contains_key(&id) {
            return;
        }
        let pipeline = build_particle_pipeline_2d(
            device,
            &self.pipeline_layout_2d,
            user_wgsl,
            self.color_format,
            self.depth_format,
        );
        self.pipelines_2d.insert(id, pipeline);
    }
}

fn particle_bind_entries(frame_size: u64, instance_size: u64) -> [wgpu::BindGroupLayoutEntry; 4] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(frame_size),
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: NonZeroU64::new(instance_size),
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
    ]
}

fn build_particle_pipeline_3d(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    fragment_wgsl: &str,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let full = format!("{}\n{}", PARTICLE_3D_PRELUDE, fragment_wgsl);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Ruzit particle 3D shader"),
        source: wgpu::ShaderSource::Wgsl(full.into()),
    });
    build_particle_pipeline_inner(device, layout, &module, color_format, depth_format, true)
}

fn build_particle_pipeline_2d(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    fragment_wgsl: &str,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let full = format!("{}\n{}", PARTICLE_2D_PRELUDE, fragment_wgsl);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("Ruzit particle 2D shader"),
        source: wgpu::ShaderSource::Wgsl(full.into()),
    });
    build_particle_pipeline_inner(device, layout, &module, color_format, depth_format, false)
}

fn build_particle_pipeline_inner(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
    depth_test: bool,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Ruzit particle pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: "vs_main",
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: "fs_main",
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
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
        depth_stencil: Some(wgpu::DepthStencilState {
            format: depth_format,
            depth_write_enabled: false,
            depth_compare: if depth_test {
                wgpu::CompareFunction::Less
            } else {
                wgpu::CompareFunction::Always
            },
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}
