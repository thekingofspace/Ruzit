use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, Debug)]
pub struct Vertex3D {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

pub struct Mesh {
    pub vertices: Vec<Vertex3D>,
    pub indices: Vec<u32>,
}

pub fn deform_blob(
    vertices: &mut [Vertex3D],
    center: [f32; 3],
    envelope: f32,
    displacement: [f32; 3],
) -> usize {
    if envelope <= 0.0 {
        return 0;
    }
    let env2 = envelope * envelope;
    let mut touched = 0usize;
    for v in vertices.iter_mut() {
        let dx = v.position[0] - center[0];
        let dy = v.position[1] - center[1];
        let dz = v.position[2] - center[2];
        let d2 = dx * dx + dy * dy + dz * dz;
        if d2 > env2 {
            continue;
        }
        let t = (d2 / env2).sqrt();

        let s = 1.0 - (3.0 * t * t - 2.0 * t * t * t);
        v.position[0] += displacement[0] * s;
        v.position[1] += displacement[1] * s;
        v.position[2] += displacement[2] * s;
        touched += 1;
    }
    touched
}

pub fn recompute_normals(vertices: &mut [Vertex3D], indices: &[u32]) {
    let mut accum = vec![[0.0_f32; 3]; vertices.len()];
    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if a >= vertices.len() || b >= vertices.len() || c >= vertices.len() {
            continue;
        }
        let pa = vertices[a].position;
        let pb = vertices[b].position;
        let pc = vertices[c].position;
        let ab = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let ac = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let n = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        for &i in &[a, b, c] {
            accum[i][0] += n[0];
            accum[i][1] += n[1];
            accum[i][2] += n[2];
        }
    }
    for (i, n) in accum.iter().enumerate() {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 1e-6 {
            vertices[i].normal = [n[0] / len, n[1] / len, n[2] / len];
        } else {
            vertices[i].normal = [0.0, 1.0, 0.0];
        }
    }
}

pub fn simplify(mesh: &Mesh, target_ratio: f32) -> Mesh {
    let ratio = target_ratio.clamp(0.01, 1.0);
    if mesh.vertices.is_empty() || mesh.indices.is_empty() {
        return Mesh {
            vertices: mesh.vertices.clone(),
            indices: mesh.indices.clone(),
        };
    }
    if ratio >= 0.999 {
        return Mesh {
            vertices: mesh.vertices.clone(),
            indices: mesh.indices.clone(),
        };
    }

    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in &mesh.vertices {
        for i in 0..3 {
            if v.position[i] < min[i] {
                min[i] = v.position[i];
            }
            if v.position[i] > max[i] {
                max[i] = v.position[i];
            }
        }
    }
    let span = [
        (max[0] - min[0]).max(1e-6),
        (max[1] - min[1]).max(1e-6),
        (max[2] - min[2]).max(1e-6),
    ];
    let target_clusters = (mesh.vertices.len() as f32 * ratio).max(4.0);
    let cells_per_axis = target_clusters.cbrt().round().max(2.0);
    let cell = [
        span[0] / cells_per_axis,
        span[1] / cells_per_axis,
        span[2] / cells_per_axis,
    ];

    let mut bucket: HashMap<(i32, i32, i32), Vec<u32>> = HashMap::new();
    for (i, v) in mesh.vertices.iter().enumerate() {
        let key = (
            ((v.position[0] - min[0]) / cell[0]).floor() as i32,
            ((v.position[1] - min[1]) / cell[1]).floor() as i32,
            ((v.position[2] - min[2]) / cell[2]).floor() as i32,
        );
        bucket.entry(key).or_default().push(i as u32);
    }

    let mut cluster_of: Vec<u32> = vec![0; mesh.vertices.len()];
    let mut new_vertices: Vec<Vertex3D> = Vec::with_capacity(bucket.len());
    let mut keys: Vec<(i32, i32, i32)> = bucket.keys().copied().collect();
    keys.sort();
    for (cluster_idx, key) in keys.iter().enumerate() {
        let members = &bucket[key];
        let mut sum_pos = [0.0_f32; 3];
        let mut sum_uv = [0.0_f32; 2];
        for &m in members {
            let v = &mesh.vertices[m as usize];
            for i in 0..3 {
                sum_pos[i] += v.position[i];
            }
            sum_uv[0] += v.uv[0];
            sum_uv[1] += v.uv[1];
            cluster_of[m as usize] = cluster_idx as u32;
        }
        let n = members.len() as f32;
        new_vertices.push(Vertex3D {
            position: [sum_pos[0] / n, sum_pos[1] / n, sum_pos[2] / n],
            normal: [0.0; 3],
            uv: [sum_uv[0] / n, sum_uv[1] / n],
        });
    }

    let mut new_indices: Vec<u32> = Vec::with_capacity(mesh.indices.len());
    for tri in mesh.indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let a = cluster_of[tri[0] as usize];
        let b = cluster_of[tri[1] as usize];
        let c = cluster_of[tri[2] as usize];
        if a == b || b == c || a == c {
            continue;
        }
        new_indices.push(a);
        new_indices.push(b);
        new_indices.push(c);
    }

    if new_indices.is_empty() {
        return Mesh {
            vertices: mesh.vertices.clone(),
            indices: mesh.indices.clone(),
        };
    }

    recompute_normals(&mut new_vertices, &new_indices);
    Mesh {
        vertices: new_vertices,
        indices: new_indices,
    }
}

pub fn cube() -> Mesh {
    let s = 0.5_f32;
    let v = vec![
        Vertex3D {
            position: [s, -s, -s],
            normal: [1.0, 0.0, 0.0],
            uv: [0.0, 1.0],
        },
        Vertex3D {
            position: [s, s, -s],
            normal: [1.0, 0.0, 0.0],
            uv: [0.0, 0.0],
        },
        Vertex3D {
            position: [s, s, s],
            normal: [1.0, 0.0, 0.0],
            uv: [1.0, 0.0],
        },
        Vertex3D {
            position: [s, -s, s],
            normal: [1.0, 0.0, 0.0],
            uv: [1.0, 1.0],
        },
        Vertex3D {
            position: [-s, -s, s],
            normal: [-1.0, 0.0, 0.0],
            uv: [0.0, 1.0],
        },
        Vertex3D {
            position: [-s, s, s],
            normal: [-1.0, 0.0, 0.0],
            uv: [0.0, 0.0],
        },
        Vertex3D {
            position: [-s, s, -s],
            normal: [-1.0, 0.0, 0.0],
            uv: [1.0, 0.0],
        },
        Vertex3D {
            position: [-s, -s, -s],
            normal: [-1.0, 0.0, 0.0],
            uv: [1.0, 1.0],
        },
        Vertex3D {
            position: [-s, s, -s],
            normal: [0.0, 1.0, 0.0],
            uv: [0.0, 1.0],
        },
        Vertex3D {
            position: [-s, s, s],
            normal: [0.0, 1.0, 0.0],
            uv: [0.0, 0.0],
        },
        Vertex3D {
            position: [s, s, s],
            normal: [0.0, 1.0, 0.0],
            uv: [1.0, 0.0],
        },
        Vertex3D {
            position: [s, s, -s],
            normal: [0.0, 1.0, 0.0],
            uv: [1.0, 1.0],
        },
        Vertex3D {
            position: [-s, -s, s],
            normal: [0.0, -1.0, 0.0],
            uv: [0.0, 1.0],
        },
        Vertex3D {
            position: [-s, -s, -s],
            normal: [0.0, -1.0, 0.0],
            uv: [0.0, 0.0],
        },
        Vertex3D {
            position: [s, -s, -s],
            normal: [0.0, -1.0, 0.0],
            uv: [1.0, 0.0],
        },
        Vertex3D {
            position: [s, -s, s],
            normal: [0.0, -1.0, 0.0],
            uv: [1.0, 1.0],
        },
        Vertex3D {
            position: [s, -s, s],
            normal: [0.0, 0.0, 1.0],
            uv: [0.0, 1.0],
        },
        Vertex3D {
            position: [s, s, s],
            normal: [0.0, 0.0, 1.0],
            uv: [0.0, 0.0],
        },
        Vertex3D {
            position: [-s, s, s],
            normal: [0.0, 0.0, 1.0],
            uv: [1.0, 0.0],
        },
        Vertex3D {
            position: [-s, -s, s],
            normal: [0.0, 0.0, 1.0],
            uv: [1.0, 1.0],
        },
        Vertex3D {
            position: [-s, -s, -s],
            normal: [0.0, 0.0, -1.0],
            uv: [0.0, 1.0],
        },
        Vertex3D {
            position: [-s, s, -s],
            normal: [0.0, 0.0, -1.0],
            uv: [0.0, 0.0],
        },
        Vertex3D {
            position: [s, s, -s],
            normal: [0.0, 0.0, -1.0],
            uv: [1.0, 0.0],
        },
        Vertex3D {
            position: [s, -s, -s],
            normal: [0.0, 0.0, -1.0],
            uv: [1.0, 1.0],
        },
    ];
    let mut i: Vec<u32> = Vec::with_capacity(36);
    for face in 0..6_u32 {
        let base = face * 4;
        i.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh {
        vertices: v,
        indices: i,
    }
}

pub fn sphere(lat: u32, long: u32) -> Mesh {
    let lat = lat.max(2);
    let long = long.max(3);
    let mut vertices = Vec::with_capacity(((lat + 1) * (long + 1)) as usize);
    let pi = std::f32::consts::PI;
    for i in 0..=lat {
        let theta = (i as f32) * pi / lat as f32;
        let st = theta.sin();
        let ct = theta.cos();
        for j in 0..=long {
            let phi = (j as f32) * 2.0 * pi / long as f32;
            let x = st * phi.cos();
            let y = ct;
            let z = st * phi.sin();
            vertices.push(Vertex3D {
                position: [x * 0.5, y * 0.5, z * 0.5],
                normal: [x, y, z],
                uv: [j as f32 / long as f32, i as f32 / lat as f32],
            });
        }
    }
    let row = long + 1;
    let mut indices = Vec::with_capacity((lat * long * 6) as usize);
    for i in 0..lat {
        for j in 0..long {
            let a = i * row + j;
            let b = a + row;
            indices.extend_from_slice(&[a, b, a + 1, b, b + 1, a + 1]);
        }
    }
    Mesh { vertices, indices }
}

pub fn load_obj(text: &str) -> Result<Mesh, String> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut vertices: Vec<Vertex3D> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut cache: HashMap<(u32, u32, u32), u32> = HashMap::new();
    let mut had_normals = false;

    fn norm_idx(idx: i32, len: usize) -> u32 {
        if idx > 0 {
            (idx - 1).max(0) as u32
        } else if idx < 0 {
            ((len as i32) + idx).max(0) as u32
        } else {
            0
        }
    }

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        match parts.next() {
            Some("v") => {
                let xyz: Vec<f32> = parts.take(3).filter_map(|s| s.parse().ok()).collect();
                if xyz.len() == 3 {
                    positions.push([xyz[0], xyz[1], xyz[2]]);
                }
            }
            Some("vn") => {
                let xyz: Vec<f32> = parts.take(3).filter_map(|s| s.parse().ok()).collect();
                if xyz.len() == 3 {
                    normals.push([xyz[0], xyz[1], xyz[2]]);
                    had_normals = true;
                }
            }
            Some("vt") => {
                let xy: Vec<f32> = parts.take(2).filter_map(|s| s.parse().ok()).collect();
                if xy.len() >= 2 {
                    uvs.push([xy[0], 1.0 - xy[1]]);
                }
            }
            Some("f") => {
                let face_verts: Vec<u32> = parts
                    .map(|tok| {
                        let parts_t: Vec<&str> = tok.split('/').collect();
                        let pi = parts_t
                            .first()
                            .and_then(|s| s.parse::<i32>().ok())
                            .unwrap_or(0);
                        let ti = parts_t
                            .get(1)
                            .and_then(|s| {
                                if s.is_empty() {
                                    None
                                } else {
                                    s.parse::<i32>().ok()
                                }
                            })
                            .unwrap_or(0);
                        let ni = parts_t
                            .get(2)
                            .and_then(|s| s.parse::<i32>().ok())
                            .unwrap_or(0);
                        let p = norm_idx(pi, positions.len());
                        let t = if ti != 0 {
                            norm_idx(ti, uvs.len())
                        } else {
                            u32::MAX
                        };
                        let n = if ni != 0 {
                            norm_idx(ni, normals.len())
                        } else {
                            u32::MAX
                        };
                        let key = (p, t, n);
                        if let Some(&idx) = cache.get(&key) {
                            idx
                        } else {
                            let pos = positions.get(p as usize).copied().unwrap_or([0.0; 3]);
                            let uv = if t != u32::MAX {
                                uvs.get(t as usize).copied().unwrap_or([0.0; 2])
                            } else {
                                [0.0; 2]
                            };
                            let normal = if n != u32::MAX {
                                normals.get(n as usize).copied().unwrap_or([0.0, 1.0, 0.0])
                            } else {
                                [0.0, 0.0, 0.0]
                            };
                            let idx = vertices.len() as u32;
                            vertices.push(Vertex3D {
                                position: pos,
                                normal,
                                uv,
                            });
                            cache.insert(key, idx);
                            idx
                        }
                    })
                    .collect();
                if face_verts.len() >= 3 {
                    for i in 1..face_verts.len() - 1 {
                        indices.push(face_verts[0]);
                        indices.push(face_verts[i]);
                        indices.push(face_verts[i + 1]);
                    }
                }
            }
            _ => {}
        }
    }

    if vertices.is_empty() {
        return Err("OBJ has no vertices".into());
    }

    if !had_normals {
        let mut accum = vec![[0.0_f32; 3]; vertices.len()];
        for tri in indices.chunks(3) {
            if tri.len() < 3 {
                continue;
            }
            let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let pa = vertices[a].position;
            let pb = vertices[b].position;
            let pc = vertices[c].position;
            let ab = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
            let ac = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
            let n = [
                ab[1] * ac[2] - ab[2] * ac[1],
                ab[2] * ac[0] - ab[0] * ac[2],
                ab[0] * ac[1] - ab[1] * ac[0],
            ];
            for &i in &[a, b, c] {
                accum[i][0] += n[0];
                accum[i][1] += n[1];
                accum[i][2] += n[2];
            }
        }
        for (i, n) in accum.iter().enumerate() {
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            if len > 1e-6 {
                vertices[i].normal = [n[0] / len, n[1] / len, n[2] / len];
            } else {
                vertices[i].normal = [0.0, 1.0, 0.0];
            }
        }
    }

    Ok(Mesh { vertices, indices })
}

#[derive(Clone, Debug, Default)]
pub struct FbxAnimClip {
    pub name: String,
    pub duration: f32,

    pub samples: Vec<(f32, Option<[f32; 3]>, Option<[f32; 3]>)>,
}

pub struct LoadedFbx {
    pub mesh: Mesh,
    pub animations: Vec<FbxAnimClip>,
}

pub fn load_fbx_full(bytes: &[u8]) -> Result<LoadedFbx, String> {
    if !bytes.starts_with(b"Kaydara FBX Binary") {
        let mesh = load_fbx_ascii(bytes)?;
        return Ok(LoadedFbx {
            mesh,
            animations: Vec::new(),
        });
    }
    load_fbx_binary_full(bytes)
}

fn load_fbx_binary_full(bytes: &[u8]) -> Result<LoadedFbx, String> {
    use fbxcel_dom::any::AnyDocument;
    use fbxcel_dom::v7400::object::TypedObjectHandle;

    let cursor = std::io::Cursor::new(bytes);
    let doc = AnyDocument::from_seekable_reader(cursor).map_err(|e| format!("FBX parse: {e}"))?;
    let doc = match doc {
        AnyDocument::V7400(_, doc) => doc,
        _ => return Err("FBX: only v7.4+ binary FBX is supported".into()),
    };

    let mut vertices: Vec<Vertex3D> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    for obj in doc.objects() {
        let geom = match obj.get_typed() {
            TypedObjectHandle::Geometry(
                fbxcel_dom::v7400::object::geometry::TypedGeometryHandle::Mesh(m),
            ) => m,
            _ => continue,
        };

        let polygon_vertices = match geom.polygon_vertices() {
            Ok(pv) => pv,
            Err(_) => continue,
        };

        let triangles = match polygon_vertices.triangulate_each(triangulator) {
            Ok(t) => t,
            Err(_) => continue,
        };

        let control_points: Vec<[f32; 3]> = polygon_vertices
            .raw_control_points()
            .map_err(|e| format!("FBX control points: {e}"))?
            .map(|p| [p.x as f32, p.y as f32, p.z as f32])
            .collect();

        let base = vertices.len() as u32;
        for cp in &control_points {
            vertices.push(Vertex3D {
                position: *cp,
                normal: [0.0, 0.0, 0.0],
                uv: [0.0, 0.0],
            });
        }

        for tri_vi in triangles.triangle_vertex_indices() {
            let cpi = match triangles.control_point_index(tri_vi) {
                Some(i) => i.to_u32(),
                None => continue,
            };
            indices.push(base + cpi);
        }
    }

    if vertices.is_empty() {
        return Err("FBX has no mesh data".into());
    }

    let mut accum = vec![[0.0_f32; 3]; vertices.len()];
    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let pa = vertices[a].position;
        let pb = vertices[b].position;
        let pc = vertices[c].position;
        let ab = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let ac = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let n = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        for &i in &[a, b, c] {
            accum[i][0] += n[0];
            accum[i][1] += n[1];
            accum[i][2] += n[2];
        }
    }
    for (i, n) in accum.iter().enumerate() {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 1e-6 {
            vertices[i].normal = [n[0] / len, n[1] / len, n[2] / len];
        } else {
            vertices[i].normal = [0.0, 1.0, 0.0];
        }
    }

    let animations = parse_fbx_animations(&doc);
    Ok(LoadedFbx {
        mesh: Mesh { vertices, indices },
        animations,
    })
}

fn parse_fbx_animations(doc: &fbxcel_dom::v7400::Document) -> Vec<FbxAnimClip> {
    use fbxcel_dom::fbxcel::low::v7400::AttributeValue;

    const FBX_TIME_PER_SECOND: f64 = 46_186_158_000.0;

    let mut clips: Vec<FbxAnimClip> = Vec::new();

    fn read_i64_array(
        node: fbxcel_dom::fbxcel::tree::v7400::NodeHandle<'_>,
        child: &str,
    ) -> Vec<i64> {
        for c in node.children_by_name(child) {
            for attr in c.attributes() {
                if let AttributeValue::ArrI64(v) = attr {
                    return v.clone();
                }
            }
        }
        Vec::new()
    }
    fn read_f32_array(
        node: fbxcel_dom::fbxcel::tree::v7400::NodeHandle<'_>,
        child: &str,
    ) -> Vec<f32> {
        for c in node.children_by_name(child) {
            for attr in c.attributes() {
                match attr {
                    AttributeValue::ArrF32(v) => return v.clone(),
                    AttributeValue::ArrF64(v) => {
                        return v.iter().map(|d| *d as f32).collect();
                    }
                    _ => {}
                }
            }
        }
        Vec::new()
    }

    for stack_obj in doc.objects() {
        if stack_obj.class() != "AnimationStack" {
            continue;
        }
        let clip_name = stack_obj
            .name()
            .map(|s| s.split('\u{0001}').next().unwrap_or(s).to_string())
            .unwrap_or_else(|| format!("Stack_{}", stack_obj.object_id().raw()));

        let mut sample_times: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
        let mut t_curves: [Vec<(i64, f32)>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut r_curves: [Vec<(i64, f32)>; 3] = [Vec::new(), Vec::new(), Vec::new()];

        for layer in stack_obj
            .destination_objects()
            .chain(stack_obj.source_objects())
        {
            let Some(layer_obj) = layer.object_handle() else {
                continue;
            };
            if layer_obj.class() != "AnimationLayer" {
                continue;
            }

            for cn_conn in layer_obj
                .destination_objects()
                .chain(layer_obj.source_objects())
            {
                let Some(cn_obj) = cn_conn.object_handle() else {
                    continue;
                };
                if cn_obj.class() != "AnimCurveNode" {
                    continue;
                }
                let cn_name = cn_obj.name().unwrap_or("");
                let target_kind: Option<usize> = match cn_name {
                    "T" => Some(0),
                    "R" => Some(1),
                    _ => None,
                };
                let Some(kind) = target_kind else { continue };

                for curve_conn in cn_obj.destination_objects().chain(cn_obj.source_objects()) {
                    let Some(curve_obj) = curve_conn.object_handle() else {
                        continue;
                    };
                    if curve_obj.class() != "AnimCurve" {
                        continue;
                    }
                    let label = curve_conn.label().unwrap_or("");
                    let axis = if label.ends_with("X") {
                        0
                    } else if label.ends_with("Y") {
                        1
                    } else if label.ends_with("Z") {
                        2
                    } else {
                        continue;
                    };
                    let node = curve_obj.node();
                    let times = read_i64_array(node, "KeyTime");
                    let values = read_f32_array(node, "KeyValueFloat");
                    let n = times.len().min(values.len());
                    let bucket = if kind == 0 {
                        &mut t_curves[axis]
                    } else {
                        &mut r_curves[axis]
                    };
                    for i in 0..n {
                        bucket.push((times[i], values[i]));
                        sample_times.insert(times[i]);
                    }
                }
            }
        }

        if sample_times.is_empty() {
            continue;
        }

        fn sample_curve(curve: &[(i64, f32)], t: i64) -> Option<f32> {
            if curve.is_empty() {
                return None;
            }

            let mut sorted = curve.to_vec();
            sorted.sort_by_key(|(time, _)| *time);
            if t <= sorted[0].0 {
                return Some(sorted[0].1);
            }
            if t >= sorted.last().unwrap().0 {
                return Some(sorted.last().unwrap().1);
            }
            for w in sorted.windows(2) {
                if t >= w[0].0 && t <= w[1].0 {
                    let dt = (w[1].0 - w[0].0) as f32;
                    if dt.abs() < 1e-6 {
                        return Some(w[0].1);
                    }
                    let blend = (t - w[0].0) as f32 / dt;
                    return Some(w[0].1 + (w[1].1 - w[0].1) * blend);
                }
            }
            None
        }

        let mut samples: Vec<(f32, Option<[f32; 3]>, Option<[f32; 3]>)> = Vec::new();
        let any_t = !t_curves[0].is_empty() || !t_curves[1].is_empty() || !t_curves[2].is_empty();
        let any_r = !r_curves[0].is_empty() || !r_curves[1].is_empty() || !r_curves[2].is_empty();
        for ticks in &sample_times {
            let secs = (*ticks as f64 / FBX_TIME_PER_SECOND) as f32;
            let t = if any_t {
                Some([
                    sample_curve(&t_curves[0], *ticks).unwrap_or(0.0),
                    sample_curve(&t_curves[1], *ticks).unwrap_or(0.0),
                    sample_curve(&t_curves[2], *ticks).unwrap_or(0.0),
                ])
            } else {
                None
            };
            let r = if any_r {
                Some([
                    sample_curve(&r_curves[0], *ticks).unwrap_or(0.0),
                    sample_curve(&r_curves[1], *ticks).unwrap_or(0.0),
                    sample_curve(&r_curves[2], *ticks).unwrap_or(0.0),
                ])
            } else {
                None
            };
            samples.push((secs, t, r));
        }
        let duration = samples.iter().map(|(t, _, _)| *t).fold(0.0_f32, f32::max);

        clips.push(FbxAnimClip {
            name: clip_name,
            duration,
            samples,
        });
    }

    clips
}

fn triangulator(
    pvs: &fbxcel_dom::v7400::data::mesh::PolygonVertices<'_>,
    poly: &[fbxcel_dom::v7400::data::mesh::PolygonVertexIndex],
    out: &mut Vec<[fbxcel_dom::v7400::data::mesh::PolygonVertexIndex; 3]>,
) -> anyhow::Result<()> {
    let _ = pvs;
    if poly.len() < 3 {
        return Ok(());
    }
    for i in 1..poly.len() - 1 {
        out.push([poly[0], poly[i], poly[i + 1]]);
    }
    Ok(())
}

fn load_fbx_ascii(bytes: &[u8]) -> Result<Mesh, String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("ASCII FBX: not UTF-8: {e}"))?;

    let mut vertices: Vec<Vertex3D> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let mut cursor = 0usize;
    while let Some(geom_start) = text[cursor..].find("Geometry:") {
        let abs = cursor + geom_start;
        let block_start = match text[abs..].find('{') {
            Some(b) => abs + b + 1,
            None => break,
        };
        let block_end = find_matching_brace(text, block_start);
        let block = &text[block_start..block_end];

        let positions = parse_named_float_array(block, "Vertices");
        let raw_indices = parse_named_int_array(block, "PolygonVertexIndex");

        if positions.is_empty() || raw_indices.is_empty() {
            cursor = block_end;
            continue;
        }
        if positions.len() % 3 != 0 {
            cursor = block_end;
            continue;
        }

        let base = vertices.len() as u32;
        for chunk in positions.chunks_exact(3) {
            vertices.push(Vertex3D {
                position: [chunk[0] as f32, chunk[1] as f32, chunk[2] as f32],
                normal: [0.0; 3],
                uv: [0.0; 2],
            });
        }

        let mut current: Vec<u32> = Vec::new();
        for raw in raw_indices {
            if raw < 0 {
                current.push((!raw) as u32);
                if current.len() >= 3 {
                    for i in 1..current.len() - 1 {
                        indices.push(base + current[0]);
                        indices.push(base + current[i]);
                        indices.push(base + current[i + 1]);
                    }
                }
                current.clear();
            } else {
                current.push(raw as u32);
            }
        }

        cursor = block_end;
    }

    if vertices.is_empty() {
        return Err("ASCII FBX: no Geometry/Vertices found".into());
    }

    let mut accum = vec![[0.0_f32; 3]; vertices.len()];
    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let (a, b, c) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if a >= vertices.len() || b >= vertices.len() || c >= vertices.len() {
            continue;
        }
        let pa = vertices[a].position;
        let pb = vertices[b].position;
        let pc = vertices[c].position;
        let ab = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
        let ac = [pc[0] - pa[0], pc[1] - pa[1], pc[2] - pa[2]];
        let n = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        for &i in &[a, b, c] {
            accum[i][0] += n[0];
            accum[i][1] += n[1];
            accum[i][2] += n[2];
        }
    }
    for (i, n) in accum.iter().enumerate() {
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 1e-6 {
            vertices[i].normal = [n[0] / len, n[1] / len, n[2] / len];
        } else {
            vertices[i].normal = [0.0, 1.0, 0.0];
        }
    }

    Ok(Mesh { vertices, indices })
}

fn find_matching_brace(text: &str, start: usize) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 1i32;
    let mut i = start;
    while i < bytes.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    i.saturating_sub(1)
}

fn parse_named_float_array(block: &str, name: &str) -> Vec<f64> {
    let needle = format!("{name}:");
    let mut idx = match block.find(&needle) {
        Some(i) => i,
        None => return Vec::new(),
    };
    idx += needle.len();
    let after = &block[idx..];
    let inner_open = match after.find('{') {
        Some(i) => i,
        None => return Vec::new(),
    };
    let inner_close = find_matching_brace(after, inner_open + 1);
    let inner = &after[inner_open + 1..inner_close];
    let a_idx = match inner.find("a:") {
        Some(i) => i + 2,
        None => return Vec::new(),
    };
    let payload = &inner[a_idx..];
    payload
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<f64>().ok())
        .collect()
}

fn parse_named_int_array(block: &str, name: &str) -> Vec<i32> {
    let needle = format!("{name}:");
    let mut idx = match block.find(&needle) {
        Some(i) => i,
        None => return Vec::new(),
    };
    idx += needle.len();
    let after = &block[idx..];
    let inner_open = match after.find('{') {
        Some(i) => i,
        None => return Vec::new(),
    };
    let inner_close = find_matching_brace(after, inner_open + 1);
    let inner = &after[inner_open + 1..inner_close];
    let a_idx = match inner.find("a:") {
        Some(i) => i + 2,
        None => return Vec::new(),
    };
    let payload = &inner[a_idx..];
    payload
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse::<i32>().ok())
        .collect()
}
