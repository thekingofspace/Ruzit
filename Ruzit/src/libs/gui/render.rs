//! GPU renderer for GUI primitives. Each primitive draws as a unit quad with
//! a per-draw uniform buffer. The default fragment shader colors the quad and
//! discards pixels outside the shape mask. User shaders attached via
//! `:AttachShader` get compiled into their own render pipeline (cached by
//! shader id) and used in place of the default for that primitive.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use super::{ImageRef, RenderItem, SceneShaderState};

use crate::libs::renderable::{
    self,
    render as r3d, // 3D pipeline / matrix helpers live in libs/renderable/render.rs
};

/// Shared by every fragment shader (built-in and user-supplied). Keep this in
/// sync with `RuzitUni` in WGSL — bytemuck on the Rust side, `@group(0)
/// @binding(0)` on the WGSL side.
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

// Linear-index getter for the params block. Hides the `vec4 → channel`
// math so user shaders can write `let rate = p(0u);` after declaring
// `// @ruzit param rate`.
fn p(idx: u32) -> f32 {
    let v = U.params[idx >> 2u];
    let c = idx & 3u;
    if (c == 0u) { return v.x; }
    if (c == 1u) { return v.y; }
    if (c == 2u) { return v.z; }
    return v.w;
}

// IMG / IMG_SAMP are bound for every primitive. For shape primitives the
// engine binds a 1×1 white texture so `textureSample(IMG, IMG_SAMP, uv)`
// returns (1,1,1,1) — a no-op multiplier. For Shape::Image primitives this
// is the actual asset, so user shaders can sample the image at `in.uv`.
@group(0) @binding(1) var IMG: texture_2d<f32>;
@group(0) @binding(2) var IMG_SAMP: sampler;

// Built-in shape mask. Returns true if the pixel at `uv` ∈ [0,1]² lies inside
// the primitive's geometry. Shape::Image (3) is a full quad — alpha clipping
// is the texture's job.
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

// Common helper for user shaders that want to honor the engine's
// transparency value without re-implementing the alpha math.
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
    // Two triangles forming a unit quad in [0,1]².
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
    // Pixel space (origin top-left, y down) → clip space.
    let x = (world.x / U.resolution.x) * 2.0 - 1.0;
    let y = 1.0 - (world.y / U.resolution.y) * 2.0;
    var out: VsOut;
    out.clip = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = q;
    return out;
}
"#;

/// Fullscreen quad vertex shader. Used by skybox + post-effect passes. uv
/// runs (0,0) at top-left to (1,1) at bottom-right so user fragment shaders
/// can compose the same way as for primitives.
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
    // Top-left origin, matches the primitive uv convention.
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
    // Shape primitives bind a 1×1 white texture so `tex` is (1,1,1,1) and
    // the result reduces to U.color. Image primitives bind the asset, so
    // U.color acts as a tint + alpha multiplier.
    let tex = textureSample(IMG, IMG_SAMP, in.uv);
    return tex * U.color;
}
"#;

/// All wgpu state lives here. Owned by the window module; recreated when the
/// window opens / resizes.
pub struct GpuState {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    pub size: (u32, u32),

    bind_group_layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    /// Static vertex shader module — every pipeline shares this.
    vs_module: wgpu::ShaderModule,
    /// Default solid-color pipeline used when no shader is attached.
    default_pipeline: wgpu::RenderPipeline,

    /// Shader-id → compiled fragment pipeline. Built lazily the first time a
    /// given shader is rendered, kept across frames.
    pipelines: HashMap<u64, wgpu::RenderPipeline>,
    /// Per-primitive uniform buffers, grown on demand. Bind groups are built
    /// per-frame because the texture binding varies per primitive.
    uniform_buffers: Vec<wgpu::Buffer>,

    /// Sampler shared by all primitives. Linear filter both ways — image
    /// scaling stays smooth.
    sampler: wgpu::Sampler,
    /// 1×1 white texture used by every shape primitive so the fragment
    /// shader's `textureSample` returns (1,1,1,1) — a no-op multiplier.
    white_view: wgpu::TextureView,
    /// image-asset id → uploaded texture view. Built lazily on first use.
    image_textures: HashMap<u64, wgpu::TextureView>,

    // -------- Scene-wide shaders (skybox + post-effect) --------
    /// Vertex shader for fullscreen quads — separate from `vs_module` because
    /// the math differs (clip-space corners, full-screen uv).
    fullscreen_vs: wgpu::ShaderModule,
    /// Cached skybox pipelines (drawn in the main pass — built with the
    /// no-op depth state so they're attachment-compatible).
    skybox_pipelines: HashMap<u64, wgpu::RenderPipeline>,
    /// Cached post-effect pipelines. Drawn in a separate pass against the
    /// surface with no depth attachment — built without depth_stencil.
    post_pipelines: HashMap<u64, wgpu::RenderPipeline>,
    /// Separate uniform buffer + per-pass bind groups for skybox vs post.
    skybox_uniform: wgpu::Buffer,
    post_uniform: wgpu::Buffer,
    /// Bind group with `IMG = white_view`, used by the skybox pass.
    skybox_bind_group: wgpu::BindGroup,
    /// Lazily-rebuilt bind group with `IMG = scene_view`. Rebuild whenever
    /// the scene texture is recreated (resize / first use).
    post_bind_group: Option<wgpu::BindGroup>,
    /// Offscreen render target used when a post-effect is active. The
    /// primitive pass writes here instead of straight to the surface; the
    /// post-effect pass then samples this and writes to the surface.
    scene_texture: Option<wgpu::Texture>,
    scene_view: Option<wgpu::TextureView>,
    scene_size: (u32, u32),

    // -------- 3D rendering --------
    depth_texture: Option<wgpu::Texture>,
    depth_view: Option<wgpu::TextureView>,
    depth_size: (u32, u32),
    /// Bind group layout for 3D draws: per-frame uniform + per-instance
    /// uniform + texture + sampler. Different from the 2D layout so we
    /// keep them as separate pipelines.
    bind_group_layout_3d: wgpu::BindGroupLayout,
    pipeline_layout_3d: wgpu::PipelineLayout,
    /// Default 3D pipeline (Lambert + texture * color). The vertex module is
    /// only needed at construction — user pipelines compile their own.
    default_pipeline_3d: wgpu::RenderPipeline,
    /// User-shader 3D pipelines, keyed by shader id.
    pipelines_3d: HashMap<u64, wgpu::RenderPipeline>,
    /// Per-frame uniform buffer (view+proj+light+time).
    frame_uniform: wgpu::Buffer,
    /// Per-instance uniform buffers (model+color+params), grown on demand.
    instance_uniforms: Vec<wgpu::Buffer>,
    /// Built-in cube + sphere vertex/index buffers.
    cube_vertex: wgpu::Buffer,
    cube_index: wgpu::Buffer,
    cube_index_count: u32,
    sphere_vertex: wgpu::Buffer,
    sphere_index: wgpu::Buffer,
    sphere_index_count: u32,
    /// Per-model GPU mesh cache, keyed by ModelAsset id.
    model_buffers: HashMap<u64, ModelBuffers>,
}

struct ModelBuffers {
    vertex: wgpu::Buffer,
    index: wgpu::Buffer,
    index_count: u32,
}

impl GpuState {
    pub fn new(window: Arc<winit::window::Window>) -> Result<Self, String> {
        let size = window.inner_size();
        // On Windows, prefer DX12. The Vulkan path on this platform routes
        // through every third-party overlay's capture layer (Twitch Studio,
        // OBS, Overwolf, TikTok Live, etc.), which both spams the log on
        // every resize and adds an extra source of instability when the
        // swapchain is recreated. DX12 is the native path here and skips
        // those hooks. On non-Windows we let wgpu pick.
        let backends = if cfg!(windows) {
            wgpu::Backends::DX12
        } else {
            wgpu::Backends::PRIMARY
        };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends,
            ..Default::default()
        });

        // Surface borrows the window via raw-window-handle. We turn it into
        // 'static by stashing the Arc inside (the surface owns its own
        // reference to the handles via the Arc clone).
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("create_surface: {e}"))?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::default(),
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .ok_or("no compatible GPU adapter found")?;

        // `default()` allows 8192px textures and works on virtually any
        // current desktop GPU. Downlevel's 2048 cap was being hit on
        // hi-DPI / wide monitors after winit scaled the surface up.
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("Ruzit Device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        ))
        .map_err(|e| format!("request_device: {e}"))?;

        let max_dim = device.limits().max_texture_dimension_2d;
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or_else(|| caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.clamp(1, max_dim),
            height: size.height.clamp(1, max_dim),
            present_mode: caps
                .present_modes
                .iter()
                .copied()
                .find(|m| matches!(m, wgpu::PresentMode::Fifo))
                .unwrap_or(caps.present_modes[0]),
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Ruzit GUI bind group layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(
                                std::mem::size_of::<UniData>() as u64,
                            ),
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
            // Drawn in the main pass which has a depth attachment so 3D can
            // depth-test. 2D pipelines stay attachment-compatible by
            // declaring a no-op depth state (compare = Always, write = off).
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

        // Scene-shader plumbing: fullscreen vertex module + uniform buffers
        // for the two scene passes (skybox / post-effect). Skybox bind group
        // can be built up-front since IMG = white view; post-effect bind
        // group depends on the scene texture and is built later.
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

        // -------- 3D bind group layout, pipeline, default shader --------
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
                            has_dynamic_offset: false,
                            min_binding_size: NonZeroU64::new(
                                std::mem::size_of::<r3d::InstanceUniform3D>() as u64,
                            ),
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

        // Frame + cube + sphere mesh buffers up front. Models upload lazily.
        let frame_uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ruzit 3D frame uni"),
            contents: bytemuck::bytes_of(&r3d::FrameUniform3D::zeroed()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
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

        Ok(Self {
            device,
            queue,
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
            instance_uniforms: Vec::new(),
            cube_vertex,
            cube_index,
            cube_index_count,
            sphere_vertex,
            sphere_index,
            sphere_index_count,
            model_buffers: HashMap::new(),
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
        // Drop the scene + depth render targets — they'll be recreated at
        // the new size on the next frame that needs them.
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
        // Capture compile errors here so a broken shader fails the attach
        // visibly (printed once) and falls back to the default pipeline for
        // subsequent frames.
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
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
            let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
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
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&label),
            source: wgpu::ShaderSource::Wgsl(wgsl.to_string().into()),
        });
        if let Some(err) = pollster::block_on(self.device.pop_error_scope()) {
            eprintln!("[GUI] skybox shader #{shader_id} compile failed: {err}");
            return false;
        }
        // Same pass as 3D / 2D primitives, so the pipeline must declare a
        // (no-op) depth state to stay attachment-compatible.
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
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&label),
            source: wgpu::ShaderSource::Wgsl(wgsl.to_string().into()),
        });
        if let Some(err) = pollster::block_on(self.device.pop_error_scope()) {
            eprintln!("[GUI] post shader #{shader_id} compile failed: {err}");
            return false;
        }
        // Drawn in a depth-less pass against the surface.
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

    /// Lazily create / re-create the offscreen scene render target so the
    /// post-effect pass has something to sample. Format matches the swap
    /// chain so the blit is identity in linear space.
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
        // Rebuild the post-effect bind group whenever the scene texture is
        // recreated — it references the view by GPU handle.
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
            // Scene shaders ignore pos/size (uv is screen-space) but we set
            // them to a sensible fullscreen value so user shaders that touch
            // them still get something coherent.
            pos: [0.0, 0.0],
            size: res,
            color: [1.0, 1.0, 1.0, 1.0],
            resolution: res,
            time,
            shape: 0,
            params,
        };
        self.queue.write_buffer(buffer, 0, bytemuck::bytes_of(&data));
    }

    /// Lazily allocate / re-allocate the depth target. Format = Depth32Float
    /// for predictable precision across drivers.
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
        let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(&label),
            source: wgpu::ShaderSource::Wgsl(wgsl.to_string().into()),
        });
        if let Some(err) = pollster::block_on(self.device.pop_error_scope()) {
            eprintln!("[Renderable] 3D shader #{shader_id} compile failed: {err}");
            return false;
        }
        // User shaders compile to a single module containing both stages
        // (engine defaults filled in for whichever stage the user didn't
        // override). Pass the same module for vs and fs — wgpu picks the
        // entry points by name (`vs_main` / `fs_main`).
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
        let vertex = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Ruzit model vertices"),
            contents: bytemuck::cast_slice(model.vertices.as_slice()),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
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

    fn ensure_instance_buffers(&mut self, n: usize) {
        while self.instance_uniforms.len() < n {
            let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("Ruzit 3D inst uni"),
                contents: bytemuck::bytes_of(&r3d::InstanceUniform3D::zeroed()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            self.instance_uniforms.push(buf);
        }
    }

    /// Upload a part-texture (BaseModel.Texture) into the shared image cache
    /// so the texture binding is reused across frames + parts. Returns the
    /// view to bind.
    fn ensure_part_texture(&mut self, tex: &renderable::PartTextureRef) -> &wgpu::TextureView {
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
            // Rgba8UnormSrgb so the asset's sRGB pixels are linearized for
            // shading and re-encoded to match the swap chain's sRGB format.
            // Otherwise images look washed out.
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

    /// Render a frame.
    /// Resolution comes from the surface config so the user shader's clip-space
    /// math always matches the actual swap chain.
    pub fn render(&mut self, items: &[RenderItem], time: f32, clear: [f32; 3]) {
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

        // Snapshot 3D parts + scene shaders for this frame.
        let parts = renderable::snapshot();
        let skybox = super::skybox_snapshot();
        let post_effect = super::post_effect_snapshot();

        // ---- ensure compiled pipelines + GPU resources before the pass ----
        if let Some(sb) = &skybox {
            self.ensure_skybox_pipeline(sb.id, &sb.wgsl);
        }
        if let Some(pe) = &post_effect {
            self.ensure_post_pipeline(pe.id, &pe.wgsl);
            self.ensure_scene_target(self.size.0, self.size.1);
        }
        self.ensure_depth_target(self.size.0, self.size.1);
        self.ensure_uniform_buffers(items.len());
        self.ensure_instance_buffers(parts.len());
        for item in items {
            if let Some(sh) = &item.active_shader {
                self.ensure_pipeline(sh.id, &sh.wgsl);
            }
            if let Some(img) = &item.image {
                self.ensure_image(img);
            }
        }
        for part in &parts {
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

        // ---- 2D primitive uniforms ---------------------------------------
        let res = [self.size.0 as f32, self.size.1 as f32];
        for (i, item) in items.iter().enumerate() {
            let alpha = (1.0 - item.transparency).clamp(0.0, 1.0);
            let color = [
                (item.color.r as f32) / 255.0,
                (item.color.g as f32) / 255.0,
                (item.color.b as f32) / 255.0,
                alpha,
            ];
            let mut params = [[0.0_f32; 4]; 4];
            if let Some(sh) = &item.active_shader {
                let p = sh.params.lock().unwrap();
                for j in 0..16 {
                    params[j / 4][j % 4] = p[j];
                }
            }
            let data = UniData {
                pos: [item.position.x, item.position.y],
                size: [item.size.x, item.size.y],
                color,
                resolution: res,
                time,
                shape: item.shape.shape_id(),
                params,
            };
            self.queue
                .write_buffer(&self.uniform_buffers[i], 0, bytemuck::bytes_of(&data));
        }

        // ---- skybox / post-effect uniforms (separate buffers — the GPU
        // sees the buffer state at submit time, not encode time, so sharing
        // a single buffer across passes would clobber the first) ----------
        if let Some(sb) = &skybox {
            self.write_scene_uniform(&self.skybox_uniform, sb, time);
        }
        if let Some(pe) = &post_effect {
            self.write_scene_uniform(&self.post_uniform, pe, time);
        }

        // ---- 3D frame + per-instance uniforms ----------------------------
        let cam = renderable::camera_snapshot();
        let aspect = if self.size.1 > 0 {
            self.size.0 as f32 / self.size.1 as f32
        } else {
            1.0
        };
        let view = r3d::view_matrix(
            [cam.cframe.position.x, cam.cframe.position.y, cam.cframe.position.z],
            [cam.cframe.rotation.x, cam.cframe.rotation.y, cam.cframe.rotation.z],
        );
        let proj = r3d::perspective_matrix(cam.fov_deg, aspect, cam.near, cam.far);
        let view_proj = r3d::mat4_mul(proj, view);
        let frame = r3d::FrameUniform3D {
            // wgpu reads uniform mat4 as column-major; our math is row-major
            // — transpose at upload so the GPU sees the matrix as intended.
            view_proj: r3d::transpose4(view_proj),
            light_dir: normalize3([-0.4, -1.0, -0.3]),
            time,
            camera_pos: [cam.cframe.position.x, cam.cframe.position.y, cam.cframe.position.z],
            _pad: 0.0,
        };
        self.queue.write_buffer(&self.frame_uniform, 0, bytemuck::bytes_of(&frame));

        for (i, part) in parts.iter().enumerate() {
            let model_mat = r3d::part_model_matrix(
                [part.cframe.position.x, part.cframe.position.y, part.cframe.position.z],
                [part.cframe.rotation.x, part.cframe.rotation.y, part.cframe.rotation.z],
                [part.size.x, part.size.y, part.size.z],
            );
            let color = [
                (part.color.r as f32) / 255.0,
                (part.color.g as f32) / 255.0,
                (part.color.b as f32) / 255.0,
                1.0,
            ];
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
            };
            self.queue
                .write_buffer(&self.instance_uniforms[i], 0, bytemuck::bytes_of(&inst));
        }

        // ---- 2D bind groups (one per primitive) --------------------------
        let bind_groups_2d: Vec<wgpu::BindGroup> = items
            .iter()
            .enumerate()
            .map(|(i, item)| {
                let texture_view = match &item.image {
                    Some(img) => self
                        .image_textures
                        .get(&img.id)
                        .unwrap_or(&self.white_view),
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

        // ---- 3D bind groups (one per part). Texture is part.texture or
        // the shared white view for color-only parts. -----------------------
        let bind_groups_3d: Vec<wgpu::BindGroup> = parts
            .iter()
            .enumerate()
            .map(|(i, part)| {
                let texture_view = match &part.texture {
                    Some(tex) => self
                        .image_textures
                        .get(&tex.id)
                        .unwrap_or(&self.white_view),
                    None => &self.white_view,
                };
                self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("Ruzit 3D bind"),
                    layout: &self.bind_group_layout_3d,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: self.frame_uniform.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: self.instance_uniforms[i].as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::TextureView(texture_view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: wgpu::BindingResource::Sampler(&self.sampler),
                        },
                    ],
                })
            })
            .collect();

        // Color target for the main pass — scene texture if a post-effect
        // is active, otherwise the swap chain directly.
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

        // ===== pass 1: skybox → 3D parts → 2D primitives =====
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

            // Skybox draws first so 3D + 2D land on top.
            if let Some(sb) = &skybox {
                if let Some(pipeline) = self.skybox_pipelines.get(&sb.id) {
                    rpass.set_pipeline(pipeline);
                    rpass.set_bind_group(0, &self.skybox_bind_group, &[]);
                    rpass.draw(0..6, 0..1);
                }
            }

            // 3D parts — depth-tested. Pick mesh + pipeline per part.
            for (i, part) in parts.iter().enumerate() {
                let pipeline = match &part.active_shader {
                    Some(sh) => self
                        .pipelines_3d
                        .get(&sh.id)
                        .unwrap_or(&self.default_pipeline_3d),
                    None => &self.default_pipeline_3d,
                };
                rpass.set_pipeline(pipeline);
                rpass.set_bind_group(0, &bind_groups_3d[i], &[]);

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
                            None => continue, // upload failed; skip
                        },
                        None => continue,
                    },
                };
                rpass.set_vertex_buffer(0, vbuf.slice(..));
                rpass.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                rpass.draw_indexed(0..idx_count, 0, 0..1);
            }

            // 2D primitives last — drawn on top of the 3D scene.
            for (i, item) in items.iter().enumerate() {
                let pipeline = match &item.active_shader {
                    Some(sh) => self
                        .pipelines
                        .get(&sh.id)
                        .unwrap_or(&self.default_pipeline),
                    None => &self.default_pipeline,
                };
                rpass.set_pipeline(pipeline);
                rpass.set_bind_group(0, &bind_groups_2d[i], &[]);
                rpass.draw(0..6, 0..1);
            }
        }

        // ===== pass 2: post-effect → surface (if active) =====
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

        self.queue.submit([encoder.finish()]);
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

/// Build a 2D-style pipeline. `depth_format` controls whether the pipeline is
/// compatible with a depth attachment — pass `Some` for pipelines used in
/// the main pass (which has a depth target so 3D can write to it), and
/// `None` for pipelines used in a depth-less pass (post-effect → surface).
/// 2D pipelines never actually depth-test; they just declare a no-op state
/// (compare = Always, write = false) so they're attachment-compatible.
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
