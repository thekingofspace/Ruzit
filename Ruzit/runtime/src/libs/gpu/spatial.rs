use std::sync::{Arc, Mutex, OnceLock};

use bytemuck::{Pod, Zeroable};

use crate::libs::gui::render::{GPU_DEVICE, GPU_QUEUE};
use crate::libs::primitives::{CFrame, Vector};
use crate::libs::renderable::{self, PartShape, PartState};

const RAYCAST_SHADER: &str = r#"
struct PartHeader {
    pos_shape: vec4<f32>,
    rot_ignore: vec4<f32>,
    size_tri_count: vec4<f32>,
    tri_start_unused: vec4<u32>,
}
struct RaycastParams {
    origin_max: vec4<f32>,
    direction_count: vec4<f32>,
}
struct RaycastHit {
    t: f32,
    px: f32, py: f32, pz: f32,
    nx: f32, ny: f32, nz: f32,
    valid: u32,
}

@group(0) @binding(0) var<uniform> rc_params: RaycastParams;
@group(0) @binding(1) var<storage, read> rc_parts: array<PartHeader>;
@group(0) @binding(2) var<storage, read> rc_tris: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> rc_hits: array<RaycastHit>;

fn euler_mat(r: vec3<f32>) -> mat3x3<f32> {
    let sx = sin(r.x); let cx = cos(r.x);
    let sy = sin(r.y); let cy = cos(r.y);
    let sz = sin(r.z); let cz = cos(r.z);
    return mat3x3<f32>(
        vec3<f32>(cy*cz, sx*sy*cz + cx*sz, -cx*sy*cz + sx*sz),
        vec3<f32>(-cy*sz, -sx*sy*sz + cx*cz, cx*sy*sz + sx*cz),
        vec3<f32>(sy, -sx*cy, cx*cy),
    );
}

fn ray_tri(o: vec3<f32>, d: vec3<f32>, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>) -> f32 {
    let e1 = b - a;
    let e2 = c - a;
    let p = cross(d, e2);
    let det = dot(e1, p);
    if (abs(det) < 1e-7) { return -1.0; }
    let inv = 1.0 / det;
    let tv = o - a;
    let u = dot(tv, p) * inv;
    if (u < 0.0 || u > 1.0) { return -1.0; }
    let q = cross(tv, e1);
    let v = dot(d, q) * inv;
    if (v < 0.0 || u + v > 1.0) { return -1.0; }
    let t = dot(e2, q) * inv;
    if (t <= 0.0) { return -1.0; }
    return t;
}

fn ray_obb_local(o: vec3<f32>, d: vec3<f32>, half: vec3<f32>) -> vec4<f32> {
    var tmin = -1e30;
    var tmax = 1e30;
    var ax: i32 = 0;
    var sg: f32 = -1.0;
    for (var i: i32 = 0; i < 3; i = i + 1) {
        let oi = o[i]; let di = d[i]; let hi = half[i];
        if (abs(di) < 1e-8) {
            if (oi < -hi || oi > hi) { return vec4<f32>(-1.0, 0.0, 0.0, 0.0); }
        } else {
            let inv = 1.0 / di;
            var t1 = (-hi - oi) * inv;
            var t2 = (hi - oi) * inv;
            var s = -1.0;
            if (t1 > t2) { let tmp = t1; t1 = t2; t2 = tmp; s = 1.0; }
            if (t1 > tmin) { tmin = t1; ax = i; sg = s; }
            if (t2 < tmax) { tmax = t2; }
            if (tmin > tmax) { return vec4<f32>(-1.0, 0.0, 0.0, 0.0); }
        }
    }
    let t = select(tmax, tmin, tmin > 0.0);
    if (t < 0.0) { return vec4<f32>(-1.0, 0.0, 0.0, 0.0); }
    var n = vec3<f32>(0.0, 0.0, 0.0);
    if (ax == 0) { n.x = sg; } else if (ax == 1) { n.y = sg; } else { n.z = sg; }
    return vec4<f32>(t, n.x, n.y, n.z);
}

fn ray_ellipsoid_local(o: vec3<f32>, d: vec3<f32>, half: vec3<f32>) -> vec4<f32> {
    let r = max(half, vec3<f32>(1e-6));
    let oo = o / r;
    let dd = d / r;
    let a = dot(dd, dd);
    let b = 2.0 * dot(oo, dd);
    let c = dot(oo, oo) - 1.0;
    let disc = b*b - 4.0*a*c;
    if (disc < 0.0 || abs(a) < 1e-8) { return vec4<f32>(-1.0, 0.0, 0.0, 0.0); }
    let sq = sqrt(disc);
    let t1 = (-b - sq) / (2.0 * a);
    let t2 = (-b + sq) / (2.0 * a);
    let t = select(t2, t1, t1 > 0.0);
    if (t < 0.0) { return vec4<f32>(-1.0, 0.0, 0.0, 0.0); }
    let local = o + d * t;
    let n = normalize(local / (r * r));
    return vec4<f32>(t, n.x, n.y, n.z);
}

@compute @workgroup_size(64)
fn raycast_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    let count = u32(rc_params.direction_count.w);
    if (i >= count) { return; }
    rc_hits[i].valid = 0u;
    rc_hits[i].t = 1e30;

    let h = rc_parts[i];
    if (h.rot_ignore.w > 0.5) { return; }

    let shape = u32(h.pos_shape.w + 0.5);
    let tri_start = h.tri_start_unused.x;
    let tri_count = u32(h.size_tri_count.w + 0.5);
    let max_dist = rc_params.origin_max.w;

    let rot = euler_mat(h.rot_ignore.xyz);
    let inv_rot = transpose(rot);
    let local_o = inv_rot * (rc_params.origin_max.xyz - h.pos_shape.xyz);
    let local_d = inv_rot * rc_params.direction_count.xyz;
    let half = h.size_tri_count.xyz * 0.5;

    var t = -1.0;
    var n_local = vec3<f32>(0.0, 0.0, 0.0);
    var p_local = vec3<f32>(0.0, 0.0, 0.0);

    if (shape == 1u) {
        let r = ray_ellipsoid_local(local_o, local_d, half);
        if (r.x > 0.0 && r.x <= max_dist) {
            t = r.x;
            p_local = local_o + local_d * t;
            n_local = r.yzw;
        }
    } else if (shape == 2u && tri_count > 0u) {
        var best_t = 1e30;
        var best_a = vec3<f32>(0.0, 0.0, 0.0);
        var best_b = vec3<f32>(0.0, 0.0, 0.0);
        var best_c = vec3<f32>(0.0, 0.0, 0.0);
        var hit_any = false;
        let scl = h.size_tri_count.xyz;
        for (var ti: u32 = 0u; ti < tri_count; ti = ti + 1u) {
            let base = tri_start + ti * 3u;
            let a = rc_tris[base].xyz * scl;
            let b = rc_tris[base + 1u].xyz * scl;
            let c = rc_tris[base + 2u].xyz * scl;
            let tt = ray_tri(local_o, local_d, a, b, c);
            if (tt > 0.0 && tt < best_t && tt <= max_dist) {
                best_t = tt;
                best_a = a; best_b = b; best_c = c;
                hit_any = true;
            }
        }
        if (hit_any) {
            t = best_t;
            p_local = local_o + local_d * t;
            n_local = normalize(cross(best_b - best_a, best_c - best_a));
            if (dot(n_local, local_d) > 0.0) { n_local = -n_local; }
        }
    } else {
        let r = ray_obb_local(local_o, local_d, half);
        if (r.x > 0.0 && r.x <= max_dist) {
            t = r.x;
            p_local = local_o + local_d * t;
            n_local = r.yzw;
        }
    }

    if (t > 0.0) {
        let p_world = rot * p_local + h.pos_shape.xyz;
        let n_world = normalize(rot * n_local);
        rc_hits[i].t = t;
        rc_hits[i].px = p_world.x; rc_hits[i].py = p_world.y; rc_hits[i].pz = p_world.z;
        rc_hits[i].nx = n_world.x; rc_hits[i].ny = n_world.y; rc_hits[i].nz = n_world.z;
        rc_hits[i].valid = 1u;
    }
}
"#;

const ZONE_SHADER: &str = r#"
struct PartHeader {
    pos_shape: vec4<f32>,
    rot_ignore: vec4<f32>,
    size_tri_count: vec4<f32>,
    tri_start_unused: vec4<u32>,
}
struct ZoneParams {
    zone_pos: vec4<f32>,
    zone_half_count: vec4<f32>,
    inv_rot_0: vec4<f32>,
    inv_rot_1: vec4<f32>,
    inv_rot_2: vec4<f32>,
}

@group(0) @binding(0) var<uniform> zp: ZoneParams;
@group(0) @binding(1) var<storage, read> z_parts: array<PartHeader>;
@group(0) @binding(2) var<storage, read> z_tris: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read_write> z_count: atomic<u32>;
@group(0) @binding(4) var<storage, read_write> z_indices: array<u32>;

fn z_euler_mat(r: vec3<f32>) -> mat3x3<f32> {
    let sx = sin(r.x); let cx = cos(r.x);
    let sy = sin(r.y); let cy = cos(r.y);
    let sz = sin(r.z); let cz = cos(r.z);
    return mat3x3<f32>(
        vec3<f32>(cy*cz, sx*sy*cz + cx*sz, -cx*sy*cz + sx*sz),
        vec3<f32>(-cy*sz, -sx*sy*sz + cx*cz, cx*sy*sz + sx*cz),
        vec3<f32>(sy, -sx*cy, cx*cy),
    );
}

fn point_in_zone(p: vec3<f32>) -> bool {
    let rel = p - zp.zone_pos.xyz;
    let local = vec3<f32>(
        dot(rel, zp.inv_rot_0.xyz),
        dot(rel, zp.inv_rot_1.xyz),
        dot(rel, zp.inv_rot_2.xyz),
    );
    let half = zp.zone_half_count.xyz;
    return abs(local.x) <= half.x && abs(local.y) <= half.y && abs(local.z) <= half.z;
}

fn point_in_part(p_world: vec3<f32>, part_pos: vec3<f32>, inv_rot: mat3x3<f32>, half: vec3<f32>) -> bool {
    let local = inv_rot * (p_world - part_pos);
    return abs(local.x) <= half.x && abs(local.y) <= half.y && abs(local.z) <= half.z;
}

@compute @workgroup_size(64)
fn zone_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    let count = u32(zp.zone_half_count.w);
    if (i >= count) { return; }

    let h = z_parts[i];
    let shape = u32(h.pos_shape.w + 0.5);
    let tri_count = u32(h.size_tri_count.w + 0.5);
    let tri_start = h.tri_start_unused.x;
    let part_pos = h.pos_shape.xyz;
    let part_size = h.size_tri_count.xyz;
    let part_half = part_size * 0.5;
    let part_rot = z_euler_mat(h.rot_ignore.xyz);
    let part_inv_rot = transpose(part_rot);

    var hit = false;

    for (var ci: u32 = 0u; ci < 8u; ci = ci + 1u) {
        let sx = select(-1.0, 1.0, (ci & 1u) != 0u);
        let sy = select(-1.0, 1.0, (ci & 2u) != 0u);
        let sz = select(-1.0, 1.0, (ci & 4u) != 0u);
        let local = vec3<f32>(part_half.x * sx, part_half.y * sy, part_half.z * sz);
        let world = part_rot * local + part_pos;
        if (point_in_zone(world)) { hit = true; }
    }

    if (!hit && point_in_zone(part_pos)) { hit = true; }

    if (!hit && point_in_part(zp.zone_pos.xyz, part_pos, part_inv_rot, part_half)) {
        hit = true;
    }

    if (!hit && shape == 2u && tri_count > 0u) {
        let vert_count = tri_count * 3u;
        for (var vi: u32 = 0u; vi < vert_count; vi = vi + 1u) {
            let local_v = z_tris[tri_start + vi].xyz * part_size;
            let world_v = part_rot * local_v + part_pos;
            if (point_in_zone(world_v)) {
                hit = true;
                break;
            }
        }
    }

    if (hit) {
        let slot = atomicAdd(&z_count, 1u);
        if (slot < arrayLength(&z_indices)) {
            z_indices[slot] = i;
        }
    }
}
"#;

const OVERLAP_SHADER: &str = r#"
struct OverlapParams {
    query_type_count: vec4<u32>,
    sphere: vec4<f32>,
    box_center: vec4<f32>,
    box_axis_x: vec4<f32>,
    box_axis_y: vec4<f32>,
    box_axis_z: vec4<f32>,
    plane0: vec4<f32>, plane1: vec4<f32>, plane2: vec4<f32>,
    plane3: vec4<f32>, plane4: vec4<f32>, plane5: vec4<f32>,
}
struct Aabb {
    aabb_min: vec4<f32>,
    aabb_max: vec4<f32>,
}

@group(0) @binding(0) var<uniform> ov_params: OverlapParams;
@group(0) @binding(1) var<storage, read> ov_aabbs: array<Aabb>;
@group(0) @binding(2) var<storage, read_write> ov_count: atomic<u32>;
@group(0) @binding(3) var<storage, read_write> ov_indices: array<u32>;

fn test_sphere(amin: vec3<f32>, amax: vec3<f32>) -> bool {
    let cl = clamp(ov_params.sphere.xyz, amin, amax);
    let d = ov_params.sphere.xyz - cl;
    return dot(d, d) <= ov_params.sphere.w * ov_params.sphere.w;
}

fn test_box(amin: vec3<f32>, amax: vec3<f32>) -> bool {
    let extents = abs(ov_params.box_axis_x.xyz) + abs(ov_params.box_axis_y.xyz) + abs(ov_params.box_axis_z.xyz);
    let bmin = ov_params.box_center.xyz - extents;
    let bmax = ov_params.box_center.xyz + extents;
    return all(amax >= bmin) && all(bmax >= amin);
}

fn test_frustum(amin: vec3<f32>, amax: vec3<f32>) -> bool {
    var planes: array<vec4<f32>, 6>;
    planes[0] = ov_params.plane0;
    planes[1] = ov_params.plane1;
    planes[2] = ov_params.plane2;
    planes[3] = ov_params.plane3;
    planes[4] = ov_params.plane4;
    planes[5] = ov_params.plane5;
    for (var i: u32 = 0u; i < 6u; i = i + 1u) {
        let p = planes[i];
        let pv = vec3<f32>(
            select(amin.x, amax.x, p.x > 0.0),
            select(amin.y, amax.y, p.y > 0.0),
            select(amin.z, amax.z, p.z > 0.0),
        );
        if (dot(p.xyz, pv) + p.w < 0.0) { return false; }
    }
    return true;
}

@compute @workgroup_size(64)
fn overlap_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    let count = ov_params.query_type_count.y;
    if (i >= count) { return; }
    let a = ov_aabbs[i];
    let qt = ov_params.query_type_count.x;
    var hit = false;
    if (qt == 0u) { hit = test_sphere(a.aabb_min.xyz, a.aabb_max.xyz); }
    else if (qt == 1u) { hit = test_box(a.aabb_min.xyz, a.aabb_max.xyz); }
    else if (qt == 2u) { hit = test_frustum(a.aabb_min.xyz, a.aabb_max.xyz); }
    if (hit) {
        let slot = atomicAdd(&ov_count, 1u);
        if (slot < arrayLength(&ov_indices)) {
            ov_indices[slot] = i;
        }
    }
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct PartHeaderGpu {
    pos_shape: [f32; 4],
    rot_ignore: [f32; 4],
    size_tri_count: [f32; 4],
    tri_start_unused: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct RaycastParamsGpu {
    origin_max: [f32; 4],
    direction_count: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct RaycastHitGpu {
    t: f32,
    pos: [f32; 3],
    normal: [f32; 3],
    valid: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct OverlapParamsGpu {
    query_type_count: [u32; 4],
    sphere: [f32; 4],
    box_center: [f32; 4],
    box_axis_x: [f32; 4],
    box_axis_y: [f32; 4],
    box_axis_z: [f32; 4],
    plane0: [f32; 4],
    plane1: [f32; 4],
    plane2: [f32; 4],
    plane3: [f32; 4],
    plane4: [f32; 4],
    plane5: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct AabbGpu {
    min: [f32; 4],
    max: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Default)]
struct ZoneParamsGpu {
    zone_pos: [f32; 4],
    zone_half_count: [f32; 4],
    inv_rot_0: [f32; 4],
    inv_rot_1: [f32; 4],
    inv_rot_2: [f32; 4],
}

struct Resources {
    raycast_pipeline: wgpu::ComputePipeline,
    raycast_layout: wgpu::BindGroupLayout,
    overlap_pipeline: wgpu::ComputePipeline,
    overlap_layout: wgpu::BindGroupLayout,
    zone_pipeline: wgpu::ComputePipeline,
    zone_layout: wgpu::BindGroupLayout,
    zone_params: wgpu::Buffer,

    raycast_params: wgpu::Buffer,
    parts_buffer: wgpu::Buffer,
    parts_capacity: u32,
    tris_buffer: wgpu::Buffer,
    tris_capacity: u32,
    hits_buffer: wgpu::Buffer,
    hits_capacity: u32,
    hits_readback: wgpu::Buffer,
    hits_readback_capacity: u32,

    overlap_params: wgpu::Buffer,
    aabbs_buffer: wgpu::Buffer,
    aabbs_capacity: u32,
    indices_buffer: wgpu::Buffer,
    indices_capacity: u32,
    indices_readback: wgpu::Buffer,
    indices_readback_capacity: u32,
    count_buffer: wgpu::Buffer,
    count_readback: wgpu::Buffer,
}

static RESOURCES: OnceLock<Mutex<Resources>> = OnceLock::new();

fn min_capacity(needed: u32) -> u32 {
    needed.max(64)
}

fn grow(current: u32, needed: u32) -> u32 {
    let mut cap = current.max(64);
    while cap < needed {
        cap = cap.saturating_mul(2).max(needed);
    }
    cap
}

fn init_resources(device: &wgpu::Device) -> Resources {
    let raycast_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ruzit spatial raycast shader"),
        source: wgpu::ShaderSource::Wgsl(RAYCAST_SHADER.into()),
    });
    let overlap_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ruzit spatial overlap shader"),
        source: wgpu::ShaderSource::Wgsl(OVERLAP_SHADER.into()),
    });
    let zone_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ruzit spatial zone shader"),
        source: wgpu::ShaderSource::Wgsl(ZONE_SHADER.into()),
    });

    let raycast_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ruzit raycast bind layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    let overlap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ruzit overlap bind layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });

    let raycast_pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ruzit raycast pipeline layout"),
        bind_group_layouts: &[&raycast_layout],
        push_constant_ranges: &[],
    });
    let raycast_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("ruzit raycast pipeline"),
        layout: Some(&raycast_pipe_layout),
        module: &raycast_module,
        entry_point: "raycast_main",
        compilation_options: Default::default(),
        cache: None,
    });

    let overlap_pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ruzit overlap pipeline layout"),
        bind_group_layouts: &[&overlap_layout],
        push_constant_ranges: &[],
    });
    let overlap_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("ruzit overlap pipeline"),
        layout: Some(&overlap_pipe_layout),
        module: &overlap_module,
        entry_point: "overlap_main",
        compilation_options: Default::default(),
        cache: None,
    });

    let zone_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ruzit zone bind layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let zone_pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ruzit zone pipeline layout"),
        bind_group_layouts: &[&zone_layout],
        push_constant_ranges: &[],
    });
    let zone_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("ruzit zone pipeline"),
        layout: Some(&zone_pipe_layout),
        module: &zone_module,
        entry_point: "zone_main",
        compilation_options: Default::default(),
        cache: None,
    });
    let zone_params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ruzit zone params"),
        size: std::mem::size_of::<ZoneParamsGpu>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let raycast_params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ruzit raycast params"),
        size: std::mem::size_of::<RaycastParamsGpu>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let overlap_params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ruzit overlap params"),
        size: std::mem::size_of::<OverlapParamsGpu>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let parts_capacity = min_capacity(64);
    let parts_buffer = make_storage(device, "ruzit parts", parts_capacity, std::mem::size_of::<PartHeaderGpu>() as u64);
    let tris_capacity = min_capacity(256);
    let tris_buffer = make_storage(device, "ruzit tris", tris_capacity, 16);
    let hits_capacity = min_capacity(64);
    let hits_buffer = make_storage(device, "ruzit hits", hits_capacity, std::mem::size_of::<RaycastHitGpu>() as u64);
    let hits_readback_capacity = hits_capacity;
    let hits_readback = make_readback(device, "ruzit hits readback", hits_readback_capacity, std::mem::size_of::<RaycastHitGpu>() as u64);

    let aabbs_capacity = min_capacity(64);
    let aabbs_buffer = make_storage(device, "ruzit aabbs", aabbs_capacity, std::mem::size_of::<AabbGpu>() as u64);
    let indices_capacity = min_capacity(64);
    let indices_buffer = make_storage_rw(device, "ruzit indices", indices_capacity, 4);
    let indices_readback_capacity = indices_capacity;
    let indices_readback = make_readback(device, "ruzit indices readback", indices_readback_capacity, 4);
    let count_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ruzit count"),
        size: 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let count_readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ruzit count readback"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    Resources {
        raycast_pipeline,
        raycast_layout,
        overlap_pipeline,
        overlap_layout,
        zone_pipeline,
        zone_layout,
        zone_params,
        raycast_params,
        parts_buffer,
        parts_capacity,
        tris_buffer,
        tris_capacity,
        hits_buffer,
        hits_capacity,
        hits_readback,
        hits_readback_capacity,
        overlap_params,
        aabbs_buffer,
        aabbs_capacity,
        indices_buffer,
        indices_capacity,
        indices_readback,
        indices_readback_capacity,
        count_buffer,
        count_readback,
    }
}

fn make_storage(device: &wgpu::Device, label: &str, count: u32, stride: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (count as u64) * stride,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn make_storage_rw(device: &wgpu::Device, label: &str, count: u32, stride: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (count as u64) * stride,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn make_readback(device: &wgpu::Device, label: &str, count: u32, stride: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (count as u64) * stride,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

fn ensure_resources() -> Option<&'static Mutex<Resources>> {
    if let Some(r) = RESOURCES.get() {
        return Some(r);
    }
    let device = GPU_DEVICE.get()?;
    let r = init_resources(device);
    let _ = RESOURCES.set(Mutex::new(r));
    RESOURCES.get()
}

pub struct GpuRayHit {
    pub state: Arc<Mutex<PartState>>,
    pub distance: f32,
    pub position: Vector,
    pub normal: Vector,
}

pub fn gpu_raycast(
    origin: Vector,
    direction: Vector,
    max_dist: f32,
) -> Option<Vec<GpuRayHit>> {
    let res_lock = ensure_resources()?;
    let device = GPU_DEVICE.get()?;
    let queue = GPU_QUEUE.get()?;

    let states = renderable::list_part_states();
    if states.is_empty() {
        return Some(Vec::new());
    }

    let mut headers: Vec<PartHeaderGpu> = Vec::with_capacity(states.len());
    let mut tris: Vec<[f32; 4]> = Vec::new();
    let mut kept_states: Vec<Arc<Mutex<PartState>>> = Vec::with_capacity(states.len());

    for state_arc in states.iter() {
        let s = state_arc.lock().unwrap_or_else(|e| e.into_inner());
        if !s.alive || !s.render {
            continue;
        }
        let shape_id: u32 = match s.shape {
            PartShape::Cube => 0,
            PartShape::Sphere => 1,
            PartShape::Model => 2,
        };
        let mut tri_start: u32 = tris.len() as u32 / 3 * 3;
        let mut tri_count: u32 = 0;
        if matches!(s.shape, PartShape::Model) {
            let model = s.deformed.as_ref().or(s.model.as_ref());
            if let Some(m) = model {
                if !m.indices.is_empty() && m.indices.len() % 3 == 0 {
                    tri_start = tris.len() as u32;
                    let verts = &m.vertices;
                    for chunk in m.indices.chunks_exact(3) {
                        let ia = chunk[0] as usize;
                        let ib = chunk[1] as usize;
                        let ic = chunk[2] as usize;
                        if ia >= verts.len() || ib >= verts.len() || ic >= verts.len() {
                            continue;
                        }
                        tris.push([verts[ia].position[0], verts[ia].position[1], verts[ia].position[2], 0.0]);
                        tris.push([verts[ib].position[0], verts[ib].position[1], verts[ib].position[2], 0.0]);
                        tris.push([verts[ic].position[0], verts[ic].position[1], verts[ic].position[2], 0.0]);
                        tri_count += 1;
                    }
                }
            }
        }
        let cf = s.current_cframe();
        let ignore = if s.ignore_raycast { 1.0 } else { 0.0 };
        headers.push(PartHeaderGpu {
            pos_shape: [cf.position.x, cf.position.y, cf.position.z, shape_id as f32],
            rot_ignore: [cf.rotation.x, cf.rotation.y, cf.rotation.z, ignore],
            size_tri_count: [s.size.x, s.size.y, s.size.z, tri_count as f32],
            tri_start_unused: [tri_start, 0, 0, 0],
        });
        drop(s);
        kept_states.push(state_arc.clone());
    }

    if headers.is_empty() {
        return Some(Vec::new());
    }
    if tris.is_empty() {
        tris.push([0.0; 4]);
        tris.push([0.0; 4]);
        tris.push([0.0; 4]);
    }

    let part_count = headers.len() as u32;
    let mut res = res_lock.lock().unwrap_or_else(|e| e.into_inner());

    if part_count > res.parts_capacity {
        let cap = grow(res.parts_capacity, part_count);
        res.parts_buffer = make_storage(device, "ruzit parts", cap, std::mem::size_of::<PartHeaderGpu>() as u64);
        res.parts_capacity = cap;
    }
    if (tris.len() as u32) > res.tris_capacity {
        let cap = grow(res.tris_capacity, tris.len() as u32);
        res.tris_buffer = make_storage(device, "ruzit tris", cap, 16);
        res.tris_capacity = cap;
    }
    if part_count > res.hits_capacity {
        let cap = grow(res.hits_capacity, part_count);
        res.hits_buffer = make_storage_rw(device, "ruzit hits", cap, std::mem::size_of::<RaycastHitGpu>() as u64);
        res.hits_capacity = cap;
    }
    if part_count > res.hits_readback_capacity {
        let cap = grow(res.hits_readback_capacity, part_count);
        res.hits_readback = make_readback(device, "ruzit hits readback", cap, std::mem::size_of::<RaycastHitGpu>() as u64);
        res.hits_readback_capacity = cap;
    }

    queue.write_buffer(&res.parts_buffer, 0, bytemuck::cast_slice(&headers));
    queue.write_buffer(&res.tris_buffer, 0, bytemuck::cast_slice(&tris));

    let dir_len = direction.magnitude().max(1e-6);
    let dir = Vector::new(direction.x / dir_len, direction.y / dir_len, direction.z / dir_len);
    let params = RaycastParamsGpu {
        origin_max: [origin.x, origin.y, origin.z, max_dist],
        direction_count: [dir.x, dir.y, dir.z, part_count as f32],
    };
    queue.write_buffer(&res.raycast_params, 0, bytemuck::bytes_of(&params));

    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ruzit raycast bind"),
        layout: &res.raycast_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: res.raycast_params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: res.parts_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: res.tris_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: res.hits_buffer.as_entire_binding(),
            },
        ],
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ruzit raycast encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ruzit raycast pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&res.raycast_pipeline);
        pass.set_bind_group(0, &bind, &[]);
        let groups = part_count.div_ceil(64);
        pass.dispatch_workgroups(groups, 1, 1);
    }
    let hit_bytes = (part_count as u64) * std::mem::size_of::<RaycastHitGpu>() as u64;
    encoder.copy_buffer_to_buffer(&res.hits_buffer, 0, &res.hits_readback, 0, hit_bytes);
    queue.submit(Some(encoder.finish()));

    let hits: Vec<RaycastHitGpu> = {
        let slice = res.hits_readback.slice(0..hit_bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        let _ = rx.recv();
        let data = slice.get_mapped_range();
        let v = bytemuck::cast_slice::<u8, RaycastHitGpu>(&data).to_vec();
        drop(data);
        res.hits_readback.unmap();
        v
    };

    drop(res);

    let mut out: Vec<GpuRayHit> = Vec::new();
    for (i, h) in hits.iter().enumerate().take(part_count as usize) {
        if h.valid == 0 {
            continue;
        }
        if let Some(state) = kept_states.get(i) {
            out.push(GpuRayHit {
                state: state.clone(),
                distance: h.t,
                position: Vector::new(h.pos[0], h.pos[1], h.pos[2]),
                normal: Vector::new(h.normal[0], h.normal[1], h.normal[2]),
            });
        }
    }
    out.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
    Some(out)
}

#[derive(Clone, Copy)]
pub enum OverlapShape {
    Sphere {
        center: Vector,
        radius: f32,
    },
    Box {
        center: Vector,
        size: Vector,
        rotation: Vector,
    },
    Frustum {
        planes: [[f32; 4]; 6],
    },
}

pub fn gpu_overlap(query: OverlapShape) -> Option<Vec<Arc<Mutex<PartState>>>> {
    let res_lock = ensure_resources()?;
    let device = GPU_DEVICE.get()?;
    let queue = GPU_QUEUE.get()?;

    let states = renderable::list_part_states();
    if states.is_empty() {
        return Some(Vec::new());
    }

    let mut aabbs: Vec<AabbGpu> = Vec::with_capacity(states.len());
    let mut kept: Vec<Arc<Mutex<PartState>>> = Vec::with_capacity(states.len());
    for state_arc in states.iter() {
        let s = state_arc.lock().unwrap_or_else(|e| e.into_inner());
        if !s.alive || !s.render {
            continue;
        }
        let cf = s.current_cframe();
        let (mn, mx) = world_aabb(cf, s.size);
        drop(s);
        aabbs.push(AabbGpu {
            min: [mn.x, mn.y, mn.z, 0.0],
            max: [mx.x, mx.y, mx.z, 0.0],
        });
        kept.push(state_arc.clone());
    }

    let count = aabbs.len() as u32;
    if count == 0 {
        return Some(Vec::new());
    }

    let mut res = res_lock.lock().unwrap_or_else(|e| e.into_inner());

    if count > res.aabbs_capacity {
        let cap = grow(res.aabbs_capacity, count);
        res.aabbs_buffer = make_storage(device, "ruzit aabbs", cap, std::mem::size_of::<AabbGpu>() as u64);
        res.aabbs_capacity = cap;
    }
    if count > res.indices_capacity {
        let cap = grow(res.indices_capacity, count);
        res.indices_buffer = make_storage_rw(device, "ruzit indices", cap, 4);
        res.indices_capacity = cap;
    }
    if count > res.indices_readback_capacity {
        let cap = grow(res.indices_readback_capacity, count);
        res.indices_readback = make_readback(device, "ruzit indices readback", cap, 4);
        res.indices_readback_capacity = cap;
    }

    queue.write_buffer(&res.aabbs_buffer, 0, bytemuck::cast_slice(&aabbs));
    queue.write_buffer(&res.count_buffer, 0, bytemuck::bytes_of(&0u32));

    let mut params = OverlapParamsGpu::default();
    params.query_type_count = [0, count, 0, 0];
    match query {
        OverlapShape::Sphere { center, radius } => {
            params.query_type_count[0] = 0;
            params.sphere = [center.x, center.y, center.z, radius];
        }
        OverlapShape::Box { center, size, rotation } => {
            params.query_type_count[0] = 1;
            let rot = euler_to_matrix(rotation);
            let half = Vector::new(size.x * 0.5, size.y * 0.5, size.z * 0.5);
            params.box_center = [center.x, center.y, center.z, 0.0];
            params.box_axis_x = [rot[0][0] * half.x, rot[1][0] * half.x, rot[2][0] * half.x, 0.0];
            params.box_axis_y = [rot[0][1] * half.y, rot[1][1] * half.y, rot[2][1] * half.y, 0.0];
            params.box_axis_z = [rot[0][2] * half.z, rot[1][2] * half.z, rot[2][2] * half.z, 0.0];
        }
        OverlapShape::Frustum { planes } => {
            params.query_type_count[0] = 2;
            params.plane0 = planes[0];
            params.plane1 = planes[1];
            params.plane2 = planes[2];
            params.plane3 = planes[3];
            params.plane4 = planes[4];
            params.plane5 = planes[5];
        }
    }
    queue.write_buffer(&res.overlap_params, 0, bytemuck::bytes_of(&params));

    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ruzit overlap bind"),
        layout: &res.overlap_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: res.overlap_params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: res.aabbs_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: res.count_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: res.indices_buffer.as_entire_binding(),
            },
        ],
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ruzit overlap encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ruzit overlap pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&res.overlap_pipeline);
        pass.set_bind_group(0, &bind, &[]);
        let groups = count.div_ceil(64);
        pass.dispatch_workgroups(groups, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&res.count_buffer, 0, &res.count_readback, 0, 4);
    let idx_bytes = (count as u64) * 4;
    encoder.copy_buffer_to_buffer(&res.indices_buffer, 0, &res.indices_readback, 0, idx_bytes);
    queue.submit(Some(encoder.finish()));

    let hit_count: u32 = {
        let slice = res.count_readback.slice(0..4);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        let _ = rx.recv();
        let data = slice.get_mapped_range();
        let v = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        drop(data);
        res.count_readback.unmap();
        v
    };
    let bounded = hit_count.min(count);

    if bounded == 0 {
        return Some(Vec::new());
    }

    let indices: Vec<u32> = {
        let slice = res.indices_readback.slice(0..(bounded as u64) * 4);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        let _ = rx.recv();
        let data = slice.get_mapped_range();
        let v = bytemuck::cast_slice::<u8, u32>(&data).to_vec();
        drop(data);
        res.indices_readback.unmap();
        v
    };

    drop(res);

    let mut out: Vec<Arc<Mutex<PartState>>> = Vec::with_capacity(bounded as usize);
    for idx in indices {
        if let Some(s) = kept.get(idx as usize) {
            out.push(s.clone());
        }
    }
    Some(out)
}

fn world_aabb(cf: CFrame, size: Vector) -> (Vector, Vector) {
    let rot = euler_to_matrix(cf.rotation);
    let half = Vector::new(size.x * 0.5, size.y * 0.5, size.z * 0.5);
    let signs = [-1.0_f32, 1.0_f32];
    let mut mn = Vector::new(f32::INFINITY, f32::INFINITY, f32::INFINITY);
    let mut mx = Vector::new(f32::NEG_INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
    for sx in signs {
        for sy in signs {
            for sz in signs {
                let local = Vector::new(half.x * sx, half.y * sy, half.z * sz);
                let r = mat3_mul(rot, local);
                let w = Vector::new(cf.position.x + r.x, cf.position.y + r.y, cf.position.z + r.z);
                mn.x = mn.x.min(w.x); mn.y = mn.y.min(w.y); mn.z = mn.z.min(w.z);
                mx.x = mx.x.max(w.x); mx.y = mx.y.max(w.y); mx.z = mx.z.max(w.z);
            }
        }
    }
    (mn, mx)
}

type Mat3 = [[f32; 3]; 3];

fn euler_to_matrix(rot: Vector) -> Mat3 {
    let (sx, cx) = rot.x.sin_cos();
    let (sy, cy) = rot.y.sin_cos();
    let (sz, cz) = rot.z.sin_cos();
    [
        [cy * cz, -cy * sz, sy],
        [sx * sy * cz + cx * sz, -sx * sy * sz + cx * cz, -sx * cy],
        [-cx * sy * cz + sx * sz, cx * sy * sz + sx * cz, cx * cy],
    ]
}

fn mat3_mul(m: Mat3, v: Vector) -> Vector {
    Vector::new(
        m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
        m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
        m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
    )
}

pub fn frustum_planes_from_camera(
    cam_pos: Vector,
    cam_rot: Vector,
    fov_deg: f32,
    aspect: f32,
    near: f32,
    far: f32,
) -> [[f32; 4]; 6] {
    let rot = euler_to_matrix(cam_rot);
    let forward = mat3_mul(rot, Vector::new(0.0, 0.0, -1.0));
    let right = mat3_mul(rot, Vector::new(1.0, 0.0, 0.0));
    let up = mat3_mul(rot, Vector::new(0.0, 1.0, 0.0));

    let half_v = (fov_deg.to_radians() * 0.5).tan() * far;
    let half_h = half_v * aspect;
    let far_center = Vector::new(
        cam_pos.x + forward.x * far,
        cam_pos.y + forward.y * far,
        cam_pos.z + forward.z * far,
    );

    let near_center = Vector::new(
        cam_pos.x + forward.x * near,
        cam_pos.y + forward.y * near,
        cam_pos.z + forward.z * near,
    );

    let make_plane = |n: Vector, p: Vector| -> [f32; 4] {
        let nl = (n.x * n.x + n.y * n.y + n.z * n.z).sqrt().max(1e-6);
        let nn = Vector::new(n.x / nl, n.y / nl, n.z / nl);
        [nn.x, nn.y, nn.z, -(nn.x * p.x + nn.y * p.y + nn.z * p.z)]
    };

    let near_plane = make_plane(forward, near_center);
    let far_plane = make_plane(Vector::new(-forward.x, -forward.y, -forward.z), far_center);

    let aux_r = Vector::new(
        forward.x * far + right.x * half_h,
        forward.y * far + right.y * half_h,
        forward.z * far + right.z * half_h,
    );
    let aux_l = Vector::new(
        forward.x * far - right.x * half_h,
        forward.y * far - right.y * half_h,
        forward.z * far - right.z * half_h,
    );
    let aux_u = Vector::new(
        forward.x * far + up.x * half_v,
        forward.y * far + up.y * half_v,
        forward.z * far + up.z * half_v,
    );
    let aux_d = Vector::new(
        forward.x * far - up.x * half_v,
        forward.y * far - up.y * half_v,
        forward.z * far - up.z * half_v,
    );

    let normal_right = cross(up, aux_r);
    let normal_left = cross(aux_l, up);
    let normal_top = cross(aux_u, right);
    let normal_bottom = cross(right, aux_d);

    [
        near_plane,
        far_plane,
        make_plane(normal_left, cam_pos),
        make_plane(normal_right, cam_pos),
        make_plane(normal_top, cam_pos),
        make_plane(normal_bottom, cam_pos),
    ]
}

fn cross(a: Vector, b: Vector) -> Vector {
    Vector::new(
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x,
    )
}

pub fn gpu_zone_query(
    zone_cf: CFrame,
    zone_size: Vector,
) -> Option<Vec<Arc<Mutex<PartState>>>> {
    let res_lock = ensure_resources()?;
    let device = GPU_DEVICE.get()?;
    let queue = GPU_QUEUE.get()?;

    let states = renderable::list_part_states();
    if states.is_empty() {
        return Some(Vec::new());
    }

    let mut headers: Vec<PartHeaderGpu> = Vec::with_capacity(states.len());
    let mut tris: Vec<[f32; 4]> = Vec::new();
    let mut kept_states: Vec<Arc<Mutex<PartState>>> = Vec::with_capacity(states.len());

    for state_arc in states.iter() {
        let s = state_arc.lock().unwrap_or_else(|e| e.into_inner());
        if !s.alive || !s.render {
            continue;
        }
        let shape_id: u32 = match s.shape {
            PartShape::Cube => 0,
            PartShape::Sphere => 1,
            PartShape::Model => 2,
        };
        let mut tri_start: u32 = tris.len() as u32;
        let mut tri_count: u32 = 0;
        if matches!(s.shape, PartShape::Model) {
            let model = s.deformed.as_ref().or(s.model.as_ref());
            if let Some(m) = model {
                if !m.indices.is_empty() && m.indices.len() % 3 == 0 {
                    tri_start = tris.len() as u32;
                    let verts = &m.vertices;
                    for chunk in m.indices.chunks_exact(3) {
                        let ia = chunk[0] as usize;
                        let ib = chunk[1] as usize;
                        let ic = chunk[2] as usize;
                        if ia >= verts.len() || ib >= verts.len() || ic >= verts.len() {
                            continue;
                        }
                        tris.push([verts[ia].position[0], verts[ia].position[1], verts[ia].position[2], 0.0]);
                        tris.push([verts[ib].position[0], verts[ib].position[1], verts[ib].position[2], 0.0]);
                        tris.push([verts[ic].position[0], verts[ic].position[1], verts[ic].position[2], 0.0]);
                        tri_count += 1;
                    }
                }
            }
        }
        let cf = s.current_cframe();
        headers.push(PartHeaderGpu {
            pos_shape: [cf.position.x, cf.position.y, cf.position.z, shape_id as f32],
            rot_ignore: [cf.rotation.x, cf.rotation.y, cf.rotation.z, 0.0],
            size_tri_count: [s.size.x, s.size.y, s.size.z, tri_count as f32],
            tri_start_unused: [tri_start, 0, 0, 0],
        });
        drop(s);
        kept_states.push(state_arc.clone());
    }

    if headers.is_empty() {
        return Some(Vec::new());
    }
    if tris.is_empty() {
        tris.push([0.0; 4]);
        tris.push([0.0; 4]);
        tris.push([0.0; 4]);
    }

    let part_count = headers.len() as u32;
    let mut res = res_lock.lock().unwrap_or_else(|e| e.into_inner());

    if part_count > res.parts_capacity {
        let cap = grow(res.parts_capacity, part_count);
        res.parts_buffer = make_storage(device, "ruzit parts", cap, std::mem::size_of::<PartHeaderGpu>() as u64);
        res.parts_capacity = cap;
    }
    if (tris.len() as u32) > res.tris_capacity {
        let cap = grow(res.tris_capacity, tris.len() as u32);
        res.tris_buffer = make_storage(device, "ruzit tris", cap, 16);
        res.tris_capacity = cap;
    }
    if part_count > res.indices_capacity {
        let cap = grow(res.indices_capacity, part_count);
        res.indices_buffer = make_storage_rw(device, "ruzit indices", cap, 4);
        res.indices_capacity = cap;
    }
    if part_count > res.indices_readback_capacity {
        let cap = grow(res.indices_readback_capacity, part_count);
        res.indices_readback = make_readback(device, "ruzit indices readback", cap, 4);
        res.indices_readback_capacity = cap;
    }

    queue.write_buffer(&res.parts_buffer, 0, bytemuck::cast_slice(&headers));
    queue.write_buffer(&res.tris_buffer, 0, bytemuck::cast_slice(&tris));
    queue.write_buffer(&res.count_buffer, 0, bytemuck::bytes_of(&0u32));

    let half = Vector::new(zone_size.x.abs() * 0.5, zone_size.y.abs() * 0.5, zone_size.z.abs() * 0.5);
    let rot = euler_to_matrix(zone_cf.rotation);
    let inv = transpose_mat3(rot);
    let params = ZoneParamsGpu {
        zone_pos: [zone_cf.position.x, zone_cf.position.y, zone_cf.position.z, 0.0],
        zone_half_count: [half.x, half.y, half.z, part_count as f32],
        inv_rot_0: [inv[0][0], inv[0][1], inv[0][2], 0.0],
        inv_rot_1: [inv[1][0], inv[1][1], inv[1][2], 0.0],
        inv_rot_2: [inv[2][0], inv[2][1], inv[2][2], 0.0],
    };
    queue.write_buffer(&res.zone_params, 0, bytemuck::bytes_of(&params));

    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ruzit zone bind"),
        layout: &res.zone_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: res.zone_params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: res.parts_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: res.tris_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: res.count_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: res.indices_buffer.as_entire_binding(),
            },
        ],
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ruzit zone encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ruzit zone pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&res.zone_pipeline);
        pass.set_bind_group(0, &bind, &[]);
        let groups = part_count.div_ceil(64);
        pass.dispatch_workgroups(groups, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&res.count_buffer, 0, &res.count_readback, 0, 4);
    let idx_bytes = (part_count as u64) * 4;
    encoder.copy_buffer_to_buffer(&res.indices_buffer, 0, &res.indices_readback, 0, idx_bytes);
    queue.submit(Some(encoder.finish()));

    let hit_count: u32 = {
        let slice = res.count_readback.slice(0..4);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        let _ = rx.recv();
        let data = slice.get_mapped_range();
        let v = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        drop(data);
        res.count_readback.unmap();
        v
    };
    let bounded = hit_count.min(part_count);
    if bounded == 0 {
        return Some(Vec::new());
    }

    let indices: Vec<u32> = {
        let slice = res.indices_readback.slice(0..(bounded as u64) * 4);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        let _ = rx.recv();
        let data = slice.get_mapped_range();
        let v = bytemuck::cast_slice::<u8, u32>(&data).to_vec();
        drop(data);
        res.indices_readback.unmap();
        v
    };

    drop(res);

    let mut out: Vec<Arc<Mutex<PartState>>> = Vec::with_capacity(bounded as usize);
    for idx in indices {
        if let Some(s) = kept_states.get(idx as usize) {
            out.push(s.clone());
        }
    }
    Some(out)
}

fn transpose_mat3(m: Mat3) -> Mat3 {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}
