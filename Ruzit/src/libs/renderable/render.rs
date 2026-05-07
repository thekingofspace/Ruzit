//! 3D rendering: vertex / fragment shaders, pipeline, mesh upload helpers.
//! Resources are owned by `gui::render::GpuState` (it's the unified GPU
//! container) — this file just provides the shader source, vertex layout,
//! and pipeline-build helper. The actual draw lives inline in GpuState's
//! render() so it can share the encoder + render pass with skybox / 2D /
//! post-effect.

use bytemuck::{Pod, Zeroable};

/// Per-frame uniform — view+projection matrix + a shared light direction
/// and the wall-clock time. WGSL `mat4x4` is 16-aligned 64 bytes; vec3 is
/// padded to 16 bytes.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct FrameUniform3D {
    pub view_proj: [[f32; 4]; 4],
    pub light_dir: [f32; 3],
    pub time: f32,
    pub camera_pos: [f32; 3],
    pub _pad: f32,
}

/// Per-instance uniform — the model matrix, base color (with transparency
/// in the alpha channel), and the same 16-float param block we use for 2D.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct InstanceUniform3D {
    pub model: [[f32; 4]; 4],
    pub color: [f32; 4],
    pub params: [[f32; 4]; 4],
}

pub const VERTEX_ATTRS: &[wgpu::VertexAttribute] = &[
    // position
    wgpu::VertexAttribute {
        offset: 0,
        shader_location: 0,
        format: wgpu::VertexFormat::Float32x3,
    },
    // normal
    wgpu::VertexAttribute {
        offset: 12,
        shader_location: 1,
        format: wgpu::VertexFormat::Float32x3,
    },
    // uv
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

/// Prelude for user 3D shaders (both vertex + fragment). Provides:
///   * `VsIn`                — vertex inputs (position, normal, uv)
///   * `VsOut`               — varyings (clip pos, world pos, world normal, uv)
///   * `Frame` (`F`)         — view_proj, light_dir, time, camera_pos
///   * `Instance` (`I`)      — model matrix, color, params block
///   * `IMG` / `IMG_SAMP`    — bound to the part's texture (white if none)
///   * `p(idx)`              — convenience param accessor
///
/// User shaders may define their own `@vertex fn vs_main(in: VsIn) -> VsOut`
/// (e.g. for vertex displacement) and/or `@fragment fn fs_main(in: VsOut) ->
/// @location(0) vec4<f32>`. Whichever entry point they don't define falls
/// back to the engine default.
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
    _pad: f32,
};

struct Instance {
    model: mat4x4<f32>,
    color: vec4<f32>,
    params: array<vec4<f32>, 4>,
};

@group(0) @binding(0) var<uniform> F: Frame;
@group(0) @binding(1) var<uniform> I: Instance;
@group(0) @binding(2) var IMG: texture_2d<f32>;
@group(0) @binding(3) var IMG_SAMP: sampler;

fn p(idx: u32) -> f32 {
    let v = I.params[idx >> 2u];
    let c = idx & 3u;
    if (c == 0u) { return v.x; }
    if (c == 1u) { return v.y; }
    if (c == 2u) { return v.z; }
    return v.w;
}
"#;

/// Built-in vertex shader. Always used; user shaders only override the
/// fragment stage (they prepend the prelude above and provide `fs_main`).
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
    _pad: f32,
};

struct Instance {
    model: mat4x4<f32>,
    color: vec4<f32>,
    params: array<vec4<f32>, 4>,
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

/// Default vertex shader body. Appended to user shaders that don't define
/// their own `@vertex fn vs_main`. The bind-group resources and `VsIn` /
/// `VsOut` come from the prelude.
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

/// Default fragment shader: half-Lambert lighting + texture * color.
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

/// Build a 3D render pipeline from a vertex + fragment shader module.
/// Depth test = Less, depth write = true. Cull = back-face. Standard alpha
/// blending so transparent parts behave correctly.
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
            // Cull disabled while we settle the matrix conventions — re-enable
            // once the smoke test is visibly correct.
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

// ---------------------------------------------------------------------------
// Math helpers — perspective / view matrix builders for the camera.
// ---------------------------------------------------------------------------

// All matrices in this module are stored ROW-MAJOR (`m[row][col]`). Standard
// math convention: `M * v` is `out[i] = sum_k m[i][k] * v[k]`. Just before
// upload we run `transpose()` so wgpu (which reads uniforms as column-major)
// sees the values in the layout it expects.

/// Right-handed perspective matrix matching wgpu's NDC: x ∈ [-1,1],
/// y ∈ [-1,1], z ∈ [0,1] (D3D-style depth). Camera looks down -Z.
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

/// R = Rx · Ry · Rz (XYZ Euler order, radians). Row-major.
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

/// World matrix for a part: M = T(pos) · R(rot) · S(size). Row-major. The
/// scale fits into the rotation columns (column-of-R times scalar size[j]).
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

/// View matrix: V = R^T · T(-pos). The 3×3 part is the transpose of the
/// rotation; the 4th column is the rotated negative position.
pub fn view_matrix(pos: [f32; 3], rot: [f32; 3]) -> [[f32; 4]; 4] {
    let r = euler_rotation_matrix(rot);
    // (R^T · -pos)[i] = -sum_k r[k][i] * pos[k]
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

/// Standard row-major 4×4 multiplication: `r = a · b`.
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

/// Transpose a 4×4 matrix. Used at upload time to convert our row-major
/// representation to wgpu's column-major uniform layout.
pub fn transpose4(m: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    [
        [m[0][0], m[1][0], m[2][0], m[3][0]],
        [m[0][1], m[1][1], m[2][1], m[3][1]],
        [m[0][2], m[1][2], m[2][2], m[3][2]],
        [m[0][3], m[1][3], m[2][3], m[3][3]],
    ]
}
