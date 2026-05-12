use std::num::NonZeroU64;
use std::sync::{Arc, Mutex, OnceLock};

use bytemuck::{Pod, Zeroable};

use super::render::{GPU_DEVICE, GPU_QUEUE};
use super::{PrimitiveState, Shape};

const RAYCAST_2D_WGSL: &str = r#"
struct PrimHeader {
    pos_size: vec4<f32>,
    shape_alive_pad: vec4<u32>,
}
struct Raycast2DParams {
    origin_max: vec4<f32>,
    direction_count: vec4<f32>,
}
struct Raycast2DHit {
    t: f32,
    px: f32, py: f32,
    valid: u32,
}

@group(0) @binding(0) var<uniform> rc_params: Raycast2DParams;
@group(0) @binding(1) var<storage, read> rc_prims: array<PrimHeader>;
@group(0) @binding(2) var<storage, read_write> rc_hits: array<Raycast2DHit>;

fn ray_aabb(o: vec2<f32>, d: vec2<f32>, lo: vec2<f32>, hi: vec2<f32>) -> f32 {
    var tmin: f32 = -1e30;
    var tmax: f32 = 1e30;
    for (var i: i32 = 0; i < 2; i = i + 1) {
        let oi = o[i]; let di = d[i]; let lov = lo[i]; let hiv = hi[i];
        if (abs(di) < 1e-8) {
            if (oi < lov || oi > hiv) { return -1.0; }
        } else {
            let inv = 1.0 / di;
            var t1 = (lov - oi) * inv;
            var t2 = (hiv - oi) * inv;
            if (t1 > t2) { let tmp = t1; t1 = t2; t2 = tmp; }
            if (t1 > tmin) { tmin = t1; }
            if (t2 < tmax) { tmax = t2; }
            if (tmin > tmax) { return -1.0; }
        }
    }
    if (tmax < 0.0) { return -1.0; }
    return select(tmax, tmin, tmin > 0.0);
}

fn ray_ellipse(o: vec2<f32>, d: vec2<f32>, center: vec2<f32>, half: vec2<f32>) -> f32 {
    let r = max(half, vec2<f32>(1e-6));
    let oo = (o - center) / r;
    let dd = d / r;
    let a = dot(dd, dd);
    let b = 2.0 * dot(oo, dd);
    let c = dot(oo, oo) - 1.0;
    let disc = b * b - 4.0 * a * c;
    if (disc < 0.0 || abs(a) < 1e-8) { return -1.0; }
    let sq = sqrt(disc);
    let t1 = (-b - sq) / (2.0 * a);
    let t2 = (-b + sq) / (2.0 * a);
    if (t2 < 0.0) { return -1.0; }
    return select(t2, t1, t1 > 0.0);
}

fn ray_segment(o: vec2<f32>, d: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let s = b - a;
    let denom = d.x * s.y - d.y * s.x;
    if (abs(denom) < 1e-8) { return -1.0; }
    let diff = a - o;
    let t = (diff.x * s.y - diff.y * s.x) / denom;
    let u = (diff.x * d.y - diff.y * d.x) / denom;
    if (t < 0.0 || u < 0.0 || u > 1.0) { return -1.0; }
    return t;
}

fn ray_triangle(o: vec2<f32>, d: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>) -> f32 {
    var best: f32 = -1.0;
    let t1 = ray_segment(o, d, a, b);
    if (t1 >= 0.0 && (best < 0.0 || t1 < best)) { best = t1; }
    let t2 = ray_segment(o, d, b, c);
    if (t2 >= 0.0 && (best < 0.0 || t2 < best)) { best = t2; }
    let t3 = ray_segment(o, d, c, a);
    if (t3 >= 0.0 && (best < 0.0 || t3 < best)) { best = t3; }

    let v0 = b - a;
    let v1 = c - a;
    let v2 = o - a;
    let den = v0.x * v1.y - v1.x * v0.y;
    if (abs(den) > 1e-8) {
        let u = (v2.x * v1.y - v1.x * v2.y) / den;
        let v = (v0.x * v2.y - v2.x * v0.y) / den;
        if (u >= 0.0 && v >= 0.0 && (u + v) <= 1.0) {
            return 0.0;
        }
    }
    return best;
}

@compute @workgroup_size(64)
fn raycast2d_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    let count = u32(rc_params.direction_count.w);
    if (i >= count) { return; }
    rc_hits[i].valid = 0u;
    rc_hits[i].t = 1e30;

    let h = rc_prims[i];
    let alive = h.shape_alive_pad.y;
    if (alive == 0u) { return; }
    let shape = h.shape_alive_pad.x;
    let lo = h.pos_size.xy;
    let hi = lo + h.pos_size.zw;
    let center = (lo + hi) * 0.5;
    let half = (hi - lo) * 0.5;
    let max_t = rc_params.origin_max.z;
    let o = rc_params.origin_max.xy;
    let d = rc_params.direction_count.xy;

    var t: f32 = -1.0;
    if (shape == 1u) {
        t = ray_ellipse(o, d, center, half);
    } else if (shape == 2u) {
        let a = vec2<f32>(center.x, lo.y);
        let bp = vec2<f32>(lo.x, hi.y);
        let cp = vec2<f32>(hi.x, hi.y);
        t = ray_triangle(o, d, a, bp, cp);
    } else {
        t = ray_aabb(o, d, lo, hi);
    }
    if (t < 0.0 || t > max_t) { return; }
    let hit = o + d * t;
    rc_hits[i].t = t;
    rc_hits[i].px = hit.x;
    rc_hits[i].py = hit.y;
    rc_hits[i].valid = 1u;
}
"#;

const OVERLAP_2D_WGSL: &str = r#"
struct Aabb2D {
    lo: vec2<f32>,
    hi: vec2<f32>,
    alive: u32,
    _pad: u32,
}

struct Overlap2DParams {
    a: vec4<f32>,
    b: vec4<f32>,
    kind_count: vec4<u32>,
}

@group(0) @binding(0) var<uniform> ov_params: Overlap2DParams;
@group(0) @binding(1) var<storage, read> ov_prims: array<Aabb2D>;
@group(0) @binding(2) var<storage, read_write> ov_indices: array<u32>;
@group(0) @binding(3) var<storage, read_write> ov_counter: atomic<u32>;

fn aabb_overlap(lo1: vec2<f32>, hi1: vec2<f32>, lo2: vec2<f32>, hi2: vec2<f32>) -> bool {
    return !(hi1.x < lo2.x || lo1.x > hi2.x || hi1.y < lo2.y || lo1.y > hi2.y);
}

fn circle_aabb_overlap(c: vec2<f32>, r: f32, lo: vec2<f32>, hi: vec2<f32>) -> bool {
    let cx = clamp(c.x, lo.x, hi.x);
    let cy = clamp(c.y, lo.y, hi.y);
    let dx = c.x - cx;
    let dy = c.y - cy;
    return (dx * dx + dy * dy) <= r * r;
}

fn obb_aabb_overlap(
    obb_center: vec2<f32>, obb_half: vec2<f32>, obb_angle: f32,
    lo: vec2<f32>, hi: vec2<f32>
) -> bool {
    let cs = cos(obb_angle);
    let sn = sin(obb_angle);
    let ex = vec2<f32>(cs, sn);
    let ey = vec2<f32>(-sn, cs);
    let abs_ex = abs(ex);
    let abs_ey = abs(ey);
    let obb_world_extent = abs_ex * obb_half.x + abs_ey * obb_half.y;
    let obb_lo = obb_center - obb_world_extent;
    let obb_hi = obb_center + obb_world_extent;
    if (!aabb_overlap(obb_lo, obb_hi, lo, hi)) { return false; }
    let aabb_center = (lo + hi) * 0.5;
    let aabb_half = (hi - lo) * 0.5;
    let to_local = aabb_center - obb_center;
    let local_x = dot(to_local, ex);
    let local_y = dot(to_local, ey);
    let aabb_local_extent_x = abs_ex.x * aabb_half.x + abs_ex.y * aabb_half.y;
    let aabb_local_extent_y = abs_ey.x * aabb_half.x + abs_ey.y * aabb_half.y;
    if (abs(local_x) > obb_half.x + aabb_local_extent_x) { return false; }
    if (abs(local_y) > obb_half.y + aabb_local_extent_y) { return false; }
    return true;
}

@compute @workgroup_size(64)
fn overlap2d_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    let count = ov_params.kind_count.y;
    if (i >= count) { return; }
    let prim = ov_prims[i];
    if (prim.alive == 0u) { return; }

    let kind = ov_params.kind_count.x;
    var hit = false;
    if (kind == 0u) {
        let c = ov_params.a.xy;
        let r = ov_params.a.z;
        hit = circle_aabb_overlap(c, r, prim.lo, prim.hi);
    } else if (kind == 1u) {
        let center = ov_params.a.xy;
        let half = ov_params.a.zw;
        let angle = ov_params.b.x;
        if (abs(angle) < 1e-5) {
            let qlo = center - half;
            let qhi = center + half;
            hit = aabb_overlap(prim.lo, prim.hi, qlo, qhi);
        } else {
            hit = obb_aabb_overlap(center, half, angle, prim.lo, prim.hi);
        }
    } else if (kind == 2u) {
        let qlo = ov_params.a.xy;
        let qhi = ov_params.b.xy;
        hit = aabb_overlap(prim.lo, prim.hi, qlo, qhi);
    }
    if (hit) {
        let slot = atomicAdd(&ov_counter, 1u);
        ov_indices[slot] = i;
    }
}
"#;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct PrimHeaderGpu {
    pos_size: [f32; 4],
    shape_alive_pad: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct Raycast2DParamsGpu {
    origin_max: [f32; 4],
    direction_count: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct Raycast2DHitGpu {
    t: f32,
    px: f32,
    py: f32,
    valid: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct Aabb2DGpu {
    lo: [f32; 2],
    hi: [f32; 2],
    alive: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
struct Overlap2DParamsGpu {
    a: [f32; 4],
    b: [f32; 4],
    kind_count: [u32; 4],
}

struct Resources2D {
    raycast_pipeline: wgpu::ComputePipeline,
    raycast_layout: wgpu::BindGroupLayout,
    overlap_pipeline: wgpu::ComputePipeline,
    overlap_layout: wgpu::BindGroupLayout,

    raycast_params: wgpu::Buffer,
    overlap_params: wgpu::Buffer,

    prims_raycast: wgpu::Buffer,
    prims_raycast_capacity: u32,
    hits_buffer: wgpu::Buffer,
    hits_capacity: u32,
    hits_readback: wgpu::Buffer,
    hits_readback_capacity: u32,

    prims_overlap: wgpu::Buffer,
    prims_overlap_capacity: u32,
    indices_buffer: wgpu::Buffer,
    indices_capacity: u32,
    indices_readback: wgpu::Buffer,
    indices_readback_capacity: u32,
    counter_buffer: wgpu::Buffer,
    counter_readback: wgpu::Buffer,
}

static RESOURCES: OnceLock<Mutex<Resources2D>> = OnceLock::new();

fn make_storage(device: &wgpu::Device, label: &str, count: u32, stride: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (count.max(1) as u64) * stride,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn make_storage_rw(device: &wgpu::Device, label: &str, count: u32, stride: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (count.max(1) as u64) * stride,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

fn make_readback(device: &wgpu::Device, label: &str, count: u32, stride: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (count.max(1) as u64) * stride,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    })
}

fn grow(current: u32, needed: u32) -> u32 {
    let mut cap = current.max(64);
    while cap < needed {
        cap = cap.saturating_mul(2);
    }
    cap
}

fn ensure_resources() -> Option<&'static Mutex<Resources2D>> {
    if let Some(r) = RESOURCES.get() {
        return Some(r);
    }
    let device = GPU_DEVICE.get()?;

    let raycast_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ruzit gui raycast2d"),
        source: wgpu::ShaderSource::Wgsl(RAYCAST_2D_WGSL.into()),
    });
    let overlap_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ruzit gui overlap2d"),
        source: wgpu::ShaderSource::Wgsl(OVERLAP_2D_WGSL.into()),
    });

    let raycast_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ruzit gui raycast2d layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Raycast2DParamsGpu>() as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<PrimHeaderGpu>() as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Raycast2DHitGpu>() as u64),
                },
                count: None,
            },
        ],
    });
    let raycast_pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ruzit gui raycast2d pl"),
        bind_group_layouts: &[&raycast_layout],
        push_constant_ranges: &[],
    });
    let raycast_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("ruzit gui raycast2d pipe"),
        layout: Some(&raycast_pipe_layout),
        module: &raycast_module,
        entry_point: "raycast2d_main",
        compilation_options: Default::default(),
        cache: None,
    });

    let overlap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ruzit gui overlap2d layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Overlap2DParamsGpu>() as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(std::mem::size_of::<Aabb2DGpu>() as u64),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(4),
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    min_binding_size: NonZeroU64::new(4),
                },
                count: None,
            },
        ],
    });
    let overlap_pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ruzit gui overlap2d pl"),
        bind_group_layouts: &[&overlap_layout],
        push_constant_ranges: &[],
    });
    let overlap_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("ruzit gui overlap2d pipe"),
        layout: Some(&overlap_pipe_layout),
        module: &overlap_module,
        entry_point: "overlap2d_main",
        compilation_options: Default::default(),
        cache: None,
    });

    use wgpu::util::DeviceExt;
    let raycast_params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ruzit gui raycast2d params"),
        contents: bytemuck::bytes_of(&Raycast2DParamsGpu::zeroed()),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let overlap_params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ruzit gui overlap2d params"),
        contents: bytemuck::bytes_of(&Overlap2DParamsGpu::zeroed()),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });

    let initial: u32 = 64;
    let prims_raycast = make_storage(device, "ruzit gui prims rc", initial, std::mem::size_of::<PrimHeaderGpu>() as u64);
    let hits_buffer = make_storage_rw(device, "ruzit gui hits", initial, std::mem::size_of::<Raycast2DHitGpu>() as u64);
    let hits_readback = make_readback(device, "ruzit gui hits rb", initial, std::mem::size_of::<Raycast2DHitGpu>() as u64);

    let prims_overlap = make_storage(device, "ruzit gui prims ov", initial, std::mem::size_of::<Aabb2DGpu>() as u64);
    let indices_buffer = make_storage_rw(device, "ruzit gui idx", initial, 4);
    let indices_readback = make_readback(device, "ruzit gui idx rb", initial, 4);
    let counter_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ruzit gui counter"),
        size: 4,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let counter_readback = make_readback(device, "ruzit gui counter rb", 1, 4);

    let _ = RESOURCES.set(Mutex::new(Resources2D {
        raycast_pipeline,
        raycast_layout,
        overlap_pipeline,
        overlap_layout,
        raycast_params,
        overlap_params,
        prims_raycast,
        prims_raycast_capacity: initial,
        hits_buffer,
        hits_capacity: initial,
        hits_readback,
        hits_readback_capacity: initial,
        prims_overlap,
        prims_overlap_capacity: initial,
        indices_buffer,
        indices_capacity: initial,
        indices_readback,
        indices_readback_capacity: initial,
        counter_buffer,
        counter_readback,
    }));
    RESOURCES.get()
}

fn shape_to_id(s: Shape) -> u32 {
    match s {
        Shape::Square => 0,
        Shape::Circle => 1,
        Shape::Triangle => 2,
        Shape::Image => 0,
        Shape::Text => 0,
        Shape::Clippable => 0,
    }
}

fn build_prim_headers() -> (Vec<PrimHeaderGpu>, Vec<Arc<Mutex<PrimitiveState>>>) {
    let states = super::list_primitive_states();
    let mut headers = Vec::with_capacity(states.len());
    let mut kept = Vec::with_capacity(states.len());
    for state_arc in states.iter() {
        let s = state_arc.lock().unwrap();
        if !s.alive || !s.visible {
            continue;
        }
        let size = s.size;
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        headers.push(PrimHeaderGpu {
            pos_size: [s.position.x, s.position.y, size.x, size.y],
            shape_alive_pad: [shape_to_id(s.shape), 1, 0, 0],
        });
        drop(s);
        kept.push(state_arc.clone());
    }
    (headers, kept)
}

fn build_prim_aabbs() -> (Vec<Aabb2DGpu>, Vec<Arc<Mutex<PrimitiveState>>>) {
    let states = super::list_primitive_states();
    let mut aabbs = Vec::with_capacity(states.len());
    let mut kept = Vec::with_capacity(states.len());
    for state_arc in states.iter() {
        let s = state_arc.lock().unwrap();
        if !s.alive || !s.visible {
            continue;
        }
        let size = s.size;
        if size.x <= 0.0 || size.y <= 0.0 {
            continue;
        }
        let lo = [s.position.x, s.position.y];
        let hi = [s.position.x + size.x, s.position.y + size.y];
        aabbs.push(Aabb2DGpu {
            lo,
            hi,
            alive: 1,
            _pad: 0,
        });
        drop(s);
        kept.push(state_arc.clone());
    }
    (aabbs, kept)
}

pub struct GuiRayHit {
    pub state: Arc<Mutex<PrimitiveState>>,
    pub distance: f32,
    pub position: [f32; 2],
}

pub fn gpu_raycast_2d(
    origin: [f32; 2],
    direction: [f32; 2],
    max_dist: f32,
) -> Option<Vec<GuiRayHit>> {
    let res_lock = ensure_resources()?;
    let device = GPU_DEVICE.get()?;
    let queue = GPU_QUEUE.get()?;

    let (headers, kept) = build_prim_headers();
    if headers.is_empty() {
        return Some(Vec::new());
    }

    let count = headers.len() as u32;
    let mut res = res_lock.lock().unwrap();
    if count > res.prims_raycast_capacity {
        let cap = grow(res.prims_raycast_capacity, count);
        res.prims_raycast = make_storage(device, "ruzit gui prims rc", cap, std::mem::size_of::<PrimHeaderGpu>() as u64);
        res.prims_raycast_capacity = cap;
    }
    if count > res.hits_capacity {
        let cap = grow(res.hits_capacity, count);
        res.hits_buffer = make_storage_rw(device, "ruzit gui hits", cap, std::mem::size_of::<Raycast2DHitGpu>() as u64);
        res.hits_capacity = cap;
    }
    if count > res.hits_readback_capacity {
        let cap = grow(res.hits_readback_capacity, count);
        res.hits_readback = make_readback(device, "ruzit gui hits rb", cap, std::mem::size_of::<Raycast2DHitGpu>() as u64);
        res.hits_readback_capacity = cap;
    }

    queue.write_buffer(&res.prims_raycast, 0, bytemuck::cast_slice(&headers));

    let dir_len = (direction[0] * direction[0] + direction[1] * direction[1]).sqrt().max(1e-6);
    let dx = direction[0] / dir_len;
    let dy = direction[1] / dir_len;
    let params = Raycast2DParamsGpu {
        origin_max: [origin[0], origin[1], max_dist, 0.0],
        direction_count: [dx, dy, 0.0, count as f32],
    };
    queue.write_buffer(&res.raycast_params, 0, bytemuck::bytes_of(&params));

    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ruzit gui rc bind"),
        layout: &res.raycast_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: res.raycast_params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: res.prims_raycast.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: res.hits_buffer.as_entire_binding(),
            },
        ],
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ruzit gui rc encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ruzit gui rc pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&res.raycast_pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
    }
    let hit_bytes = (count as u64) * std::mem::size_of::<Raycast2DHitGpu>() as u64;
    encoder.copy_buffer_to_buffer(&res.hits_buffer, 0, &res.hits_readback, 0, hit_bytes);
    queue.submit(Some(encoder.finish()));

    let hits: Vec<Raycast2DHitGpu> = {
        let slice = res.hits_readback.slice(0..hit_bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        let _ = rx.recv();
        let data = slice.get_mapped_range();
        let v = bytemuck::cast_slice::<u8, Raycast2DHitGpu>(&data).to_vec();
        drop(data);
        res.hits_readback.unmap();
        v
    };
    drop(res);

    let mut out: Vec<GuiRayHit> = Vec::new();
    for (i, h) in hits.iter().enumerate().take(count as usize) {
        if h.valid == 0 {
            continue;
        }
        if let Some(state) = kept.get(i) {
            out.push(GuiRayHit {
                state: state.clone(),
                distance: h.t,
                position: [h.px, h.py],
            });
        }
    }
    out.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap_or(std::cmp::Ordering::Equal));
    Some(out)
}

#[derive(Clone, Copy)]
pub enum OverlapShape2D {
    Circle { center: [f32; 2], radius: f32 },
    Box { center: [f32; 2], size: [f32; 2], rotation: f32 },
    Aabb { lo: [f32; 2], hi: [f32; 2] },
}

pub fn gpu_overlap_2d(query: OverlapShape2D) -> Option<Vec<Arc<Mutex<PrimitiveState>>>> {
    let res_lock = ensure_resources()?;
    let device = GPU_DEVICE.get()?;
    let queue = GPU_QUEUE.get()?;

    let (aabbs, kept) = build_prim_aabbs();
    if aabbs.is_empty() {
        return Some(Vec::new());
    }

    let count = aabbs.len() as u32;
    let mut res = res_lock.lock().unwrap();
    if count > res.prims_overlap_capacity {
        let cap = grow(res.prims_overlap_capacity, count);
        res.prims_overlap = make_storage(device, "ruzit gui prims ov", cap, std::mem::size_of::<Aabb2DGpu>() as u64);
        res.prims_overlap_capacity = cap;
    }
    if count > res.indices_capacity {
        let cap = grow(res.indices_capacity, count);
        res.indices_buffer = make_storage_rw(device, "ruzit gui idx", cap, 4);
        res.indices_capacity = cap;
    }
    if count > res.indices_readback_capacity {
        let cap = grow(res.indices_readback_capacity, count);
        res.indices_readback = make_readback(device, "ruzit gui idx rb", cap, 4);
        res.indices_readback_capacity = cap;
    }

    queue.write_buffer(&res.prims_overlap, 0, bytemuck::cast_slice(&aabbs));
    queue.write_buffer(&res.counter_buffer, 0, bytemuck::bytes_of(&0u32));

    let params = match query {
        OverlapShape2D::Circle { center, radius } => Overlap2DParamsGpu {
            a: [center[0], center[1], radius, 0.0],
            b: [0.0; 4],
            kind_count: [0, count, 0, 0],
        },
        OverlapShape2D::Box { center, size, rotation } => Overlap2DParamsGpu {
            a: [center[0], center[1], size[0] * 0.5, size[1] * 0.5],
            b: [rotation, 0.0, 0.0, 0.0],
            kind_count: [1, count, 0, 0],
        },
        OverlapShape2D::Aabb { lo, hi } => Overlap2DParamsGpu {
            a: [lo[0], lo[1], 0.0, 0.0],
            b: [hi[0], hi[1], 0.0, 0.0],
            kind_count: [2, count, 0, 0],
        },
    };
    queue.write_buffer(&res.overlap_params, 0, bytemuck::bytes_of(&params));

    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ruzit gui ov bind"),
        layout: &res.overlap_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: res.overlap_params.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: res.prims_overlap.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: res.indices_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: res.counter_buffer.as_entire_binding(),
            },
        ],
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("ruzit gui ov encoder"),
    });
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("ruzit gui ov pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&res.overlap_pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(count.div_ceil(64), 1, 1);
    }
    encoder.copy_buffer_to_buffer(&res.counter_buffer, 0, &res.counter_readback, 0, 4);
    let idx_bytes = (count as u64) * 4;
    encoder.copy_buffer_to_buffer(&res.indices_buffer, 0, &res.indices_readback, 0, idx_bytes);
    queue.submit(Some(encoder.finish()));

    let counter: u32 = {
        let slice = res.counter_readback.slice(0..4);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::Maintain::Wait);
        let _ = rx.recv();
        let data = slice.get_mapped_range();
        let v = bytemuck::cast_slice::<u8, u32>(&data)[0];
        drop(data);
        res.counter_readback.unmap();
        v
    };
    let counter = counter.min(count) as usize;
    let indices: Vec<u32> = if counter == 0 {
        Vec::new()
    } else {
        let bytes = counter as u64 * 4;
        let slice = res.indices_readback.slice(0..bytes);
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

    let mut out: Vec<Arc<Mutex<PrimitiveState>>> = Vec::with_capacity(indices.len());
    for idx in indices {
        if let Some(s) = kept.get(idx as usize) {
            out.push(s.clone());
        }
    }
    Some(out)
}

pub fn viewport_bounds() -> Option<[f32; 4]> {
    let device = GPU_DEVICE.get()?;
    let _ = device;
    let (w, h) = super::render::current_viewport_size()?;
    Some([0.0, 0.0, w as f32, h as f32])
}
