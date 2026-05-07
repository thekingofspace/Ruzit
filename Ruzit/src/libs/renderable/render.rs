
use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct FrameUniform3D {
    pub view_proj: [[f32; 4]; 4],
    pub light_dir: [f32; 3],
    pub time: f32,
    pub camera_pos: [f32; 3],
    pub frame_index: u32,
    pub sun_color: [f32; 3],
    pub _pad0: f32,
    pub ambient: [f32; 3],
    pub _pad1: f32,
    pub viewport: [f32; 2],
    pub _pad2: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct InstanceUniform3D {
    pub model: [[f32; 4]; 4],
    pub color: [f32; 4],
    pub params: [[f32; 4]; 4],
    pub flags: [u32; 4],
}

pub const VERTEX_ATTRS: &[wgpu::VertexAttribute] = &[
    
    wgpu::VertexAttribute {
        offset: 0,
        shader_location: 0,
        format: wgpu::VertexFormat::Float32x3,
    },
    
    wgpu::VertexAttribute {
        offset: 12,
        shader_location: 1,
        format: wgpu::VertexFormat::Float32x3,
    },
    
    wgpu::VertexAttribute {
        offset: 24,
        shader_location: 2,
        format: wgpu::VertexFormat::Float32x2,
    },
];

pub fn vertex_buffer_layout() -> wgpu::VertexBufferLayout<'static> {
    wgpu::VertexBufferLayout {
        array_stride: 32,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: VERTEX_ATTRS,
    }
}

pub const FRAGMENT_PRELUDE_3D: &str = r#"
struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct Frame {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    time: f32,
    camera_pos: vec3<f32>,
    frame_index: u32,
    sun_color: vec3<f32>,
    _pad0: f32,
    ambient: vec3<f32>,
    _pad1: f32,
    viewport: vec2<f32>,
    _pad2: vec2<f32>,
};

struct Instance {
    model: mat4x4<f32>,
    color: vec4<f32>,
    params: array<vec4<f32>, 4>,
    // Render-hint flags. Each is 0 or 1.
    cast_shadow: u32,
    receive_shadow: u32,
    _flag2: u32,
    _flag3: u32,
};

@group(0) @binding(0) var<uniform> F: Frame;
@group(0) @binding(1) var<uniform> I: Instance;
@group(0) @binding(2) var IMG: texture_2d<f32>;
@group(0) @binding(3) var IMG_SAMP: sampler;

// Optional read-only storage buffer. The default binding is a 1-float
// stub; populate via Lua's GPU.NewBuffer + GPU.SetBuffer to ship up to
// max_storage_buffer_binding_size floats (typically >= 128 MB).
// Useful for LUTs, particle data, custom per-vertex/per-instance lookups.
//
//     let r = SDATA[0u];
//     let g = SDATA[1u];
//     let b = SDATA[2u];
@group(0) @binding(4) var<storage, read> SDATA: array<f32>;

// Read one of your declared `// @ruzit param` floats (slot order = decl
// order, 0-based, packed across the four vec4s of I.params).
fn p(idx: u32) -> f32 {
    let v = I.params[idx >> 2u];
    let c = idx & 3u;
    if (c == 0u) { return v.x; }
    if (c == 1u) { return v.y; }
    if (c == 2u) { return v.z; }
    return v.w;
}

// ─── Engine-provided WGSL helpers (3D) ───────────────────────────────────
// All available unconditionally — call from any user fragment shader.

// View direction at a world-space point: normalized vector FROM the pixel
// TOWARD the camera. Useful for fresnel + specular.
fn view_dir(world_pos: vec3<f32>) -> vec3<f32> {
    return normalize(F.camera_pos - world_pos);
}

// Build a cheap orthonormal tangent frame from a world-space normal.
// Returns mat3x3 with columns (T, B, N). Approximation — not derived from
// UV partial derivatives, so it's stable across the surface but won't
// align perfectly with a baked normal map. Plenty for stylized work.
fn tangent_basis(n: vec3<f32>) -> mat3x3<f32> {
    let nn = normalize(n);
    var ref_axis = vec3<f32>(0.0, 1.0, 0.0);
    if (abs(nn.y) > 0.99) { ref_axis = vec3<f32>(1.0, 0.0, 0.0); }
    let t = normalize(cross(ref_axis, nn));
    let b = cross(nn, t);
    return mat3x3<f32>(t, b, nn);
}

// Schlick approximation of Fresnel reflectance. Pass cos_theta = dot(N, V)
// (use max(dot(N, V), 0.0)) and the surface's reflectance at normal
// incidence (F0). For dielectrics, F0 ≈ vec3(0.04); for metals it's the
// metal's albedo.
fn fresnel_schlick(cos_theta: f32, F0: vec3<f32>) -> vec3<f32> {
    let m = clamp(1.0 - cos_theta, 0.0, 1.0);
    let m2 = m * m;
    return F0 + (vec3<f32>(1.0) - F0) * (m2 * m2 * m);
}

// Project a world-space point to 0..1 screen-space UV. Returns (uv, z)
// where uv has top-left origin and z is the linear-ish view-space depth.
// Useful for billboards, world-anchored UI overlays in fragment shaders,
// or hand-rolled effects that need the screen position.
fn to_screen_uv(world_pos: vec3<f32>) -> vec3<f32> {
    let clip = F.view_proj * vec4<f32>(world_pos, 1.0);
    if (clip.w <= 0.0) { return vec3<f32>(-1.0, -1.0, -1.0); }
    let ndc = clip.xyz / clip.w;
    let uv = vec2<f32>((ndc.x + 1.0) * 0.5, 1.0 - (ndc.y + 1.0) * 0.5);
    return vec3<f32>(uv, ndc.z);
}

// Convert a depth-buffer value (0..1, non-linear) to linear view-space
// distance. Useful when sampling depth in a post-effect pass.
fn linearize_depth(d: f32, near: f32, far: f32) -> f32 {
    let z = d * 2.0 - 1.0;
    return (2.0 * near * far) / (far + near - z * (far - near));
}

// sRGB ↔ linear. The swap-chain format is sRGB so the compositor handles
// the gamma curve, but if you're authoring colors in linear space and
// want to push them as sRGB (or vice-versa), these are the standard
// approximations.
fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let cutoff = step(c, vec3<f32>(0.04045));
    let lower = c / 12.92;
    let upper = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return mix(upper, lower, cutoff);
}
fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let cutoff = step(c, vec3<f32>(0.0031308));
    let lower = c * 12.92;
    let upper = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return mix(upper, lower, cutoff);
}

// Cheap stable hash of three integer-ish components → vec3<f32> in [0,1].
// Use it for stylized noise patterns or seeding from world position
// (multiply pos by a scale first). Not cryptographic, deterministic per
// inputs.
fn hash3(p: vec3<f32>) -> vec3<f32> {
    var q = vec3<f32>(
        dot(p, vec3<f32>(127.1, 311.7, 74.7)),
        dot(p, vec3<f32>(269.5, 183.3, 246.1)),
        dot(p, vec3<f32>(113.5, 271.9, 124.6))
    );
    q = fract(sin(q) * 43758.5453);
    return q;
}
"#;

pub const VERTEX_WGSL_3D: &str = r#"
struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_pos: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

struct Frame {
    view_proj: mat4x4<f32>,
    light_dir: vec3<f32>,
    time: f32,
    camera_pos: vec3<f32>,
    frame_index: u32,
    sun_color: vec3<f32>,
    _pad0: f32,
    ambient: vec3<f32>,
    _pad1: f32,
    viewport: vec2<f32>,
    _pad2: vec2<f32>,
};

struct Instance {
    model: mat4x4<f32>,
    color: vec4<f32>,
    params: array<vec4<f32>, 4>,
    // Render-hint flags. Each is 0 or 1.
    cast_shadow: u32,
    receive_shadow: u32,
    _flag2: u32,
    _flag3: u32,
};

@group(0) @binding(0) var<uniform> F: Frame;
@group(0) @binding(1) var<uniform> I: Instance;

@vertex
fn vs_main(in: VsIn) -> VsOut {
    let world = I.model * vec4<f32>(in.position, 1.0);
    let world_n = (I.model * vec4<f32>(in.normal, 0.0)).xyz;
    var out: VsOut;
    out.clip = F.view_proj * world;
    out.world_pos = world.xyz;
    out.world_normal = normalize(world_n);
    out.uv = in.uv;
    return out;
}
"#;

pub const DEFAULT_VS_3D: &str = r#"
@vertex
fn vs_main(in: VsIn) -> VsOut {
    let world = I.model * vec4<f32>(in.position, 1.0);
    let world_n = (I.model * vec4<f32>(in.normal, 0.0)).xyz;
    var out: VsOut;
    out.clip = F.view_proj * world;
    out.world_pos = world.xyz;
    out.world_normal = normalize(world_n);
    out.uv = in.uv;
    return out;
}
"#;

pub const DEFAULT_FRAGMENT_WGSL_3D: &str = r#"
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    // Half-Lambert avoids the harsh terminator of pure max(0, n·l).
    let nl = dot(n, -F.light_dir);
    let lambert = nl * 0.5 + 0.5;
    let ambient = 0.25;
    let lit = ambient + (1.0 - ambient) * lambert;
    let tex = textureSample(IMG, IMG_SAMP, in.uv);
    return vec4<f32>(I.color.rgb * tex.rgb * lit, I.color.a);
}
"#;

pub fn build_pipeline_3d(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    vs: &wgpu::ShaderModule,
    fs: &wgpu::ShaderModule,
    color_format: wgpu::TextureFormat,
    depth_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("Ruzit 3D pipeline"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: vs,
            entry_point: "vs_main",
            buffers: &[vertex_buffer_layout()],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: fs,
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
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

pub fn perspective_matrix(fov_deg: f32, aspect: f32, near: f32, far: f32) -> [[f32; 4]; 4] {
    let f = 1.0 / (fov_deg.to_radians() * 0.5).tan();
    let nf = 1.0 / (near - far);
    let a = far * nf;
    let b = far * near * nf;
    [
        [f / aspect, 0.0, 0.0, 0.0],
        [0.0, f, 0.0, 0.0],
        [0.0, 0.0, a, b],
        [0.0, 0.0, -1.0, 0.0],
    ]
}

pub fn euler_rotation_matrix(rot: [f32; 3]) -> [[f32; 4]; 4] {
    let (sx, cx) = rot[0].sin_cos();
    let (sy, cy) = rot[1].sin_cos();
    let (sz, cz) = rot[2].sin_cos();
    [
        [cy * cz, -cy * sz, sy, 0.0],
        [
            sx * sy * cz + cx * sz,
            -sx * sy * sz + cx * cz,
            -sx * cy,
            0.0,
        ],
        [
            -cx * sy * cz + sx * sz,
            cx * sy * sz + sx * cz,
            cx * cy,
            0.0,
        ],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub fn part_model_matrix(pos: [f32; 3], rot: [f32; 3], size: [f32; 3]) -> [[f32; 4]; 4] {
    let r = euler_rotation_matrix(rot);
    [
        [
            r[0][0] * size[0],
            r[0][1] * size[1],
            r[0][2] * size[2],
            pos[0],
        ],
        [
            r[1][0] * size[0],
            r[1][1] * size[1],
            r[1][2] * size[2],
            pos[1],
        ],
        [
            r[2][0] * size[0],
            r[2][1] * size[1],
            r[2][2] * size[2],
            pos[2],
        ],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub fn view_matrix(pos: [f32; 3], rot: [f32; 3]) -> [[f32; 4]; 4] {
    let r = euler_rotation_matrix(rot);
    
    let tx = -(r[0][0] * pos[0] + r[1][0] * pos[1] + r[2][0] * pos[2]);
    let ty = -(r[0][1] * pos[0] + r[1][1] * pos[1] + r[2][1] * pos[2]);
    let tz = -(r[0][2] * pos[0] + r[1][2] * pos[1] + r[2][2] * pos[2]);
    [
        [r[0][0], r[1][0], r[2][0], tx],
        [r[0][1], r[1][1], r[2][1], ty],
        [r[0][2], r[1][2], r[2][2], tz],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

pub fn mat4_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut r = [[0.0_f32; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            r[i][j] = a[i][0] * b[0][j]
                + a[i][1] * b[1][j]
                + a[i][2] * b[2][j]
                + a[i][3] * b[3][j];
        }
    }
    r
}

pub fn transpose4(m: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    [
        [m[0][0], m[1][0], m[2][0], m[3][0]],
        [m[0][1], m[1][1], m[2][1], m[3][1]],
        [m[0][2], m[1][2], m[2][2], m[3][2]],
        [m[0][3], m[1][3], m[2][3], m[3][3]],
    ]
}
