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
    pub origin: [f32; 3],
    pub pivot: [f32; 3],
    /// Local rotation (Lcl Rotation) of the model node, **radians** XYZ.
    /// `[0, 0, 0]` when vertices are baked-to-world (ASCII full loader).
    pub rotation: [f32; 3],
}

pub struct MeshFragment {
    pub name: String,
    pub mesh: Mesh,
    pub origin: [f32; 3],
    pub pivot: [f32; 3],
    /// Local rotation (Lcl Rotation) of the model node, **radians** XYZ.
    pub rotation: [f32; 3],
}

pub fn load_fbx_full(bytes: &[u8]) -> Result<LoadedFbx, String> {
    if !bytes.starts_with(b"Kaydara FBX Binary") {
        let mesh = load_fbx_ascii(bytes)?;
        return Ok(LoadedFbx {
            mesh,
            animations: Vec::new(),
            origin: [0.0, 0.0, 0.0],
            pivot: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
        });
    }
    load_fbx_binary_full(bytes)
}


pub fn load_fbx_fragments(bytes: &[u8]) -> Result<Vec<MeshFragment>, String> {
    if !bytes.starts_with(b"Kaydara FBX Binary") {
        return load_fbx_ascii_fragments(bytes);
    }
    load_fbx_binary_fragments(bytes)
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
    let (origin, pivot, rotation) = first_model_transform(&doc);
    Ok(LoadedFbx {
        mesh: Mesh { vertices, indices },
        animations,
        origin,
        pivot,
        rotation,
    })
}

fn load_fbx_binary_fragments(bytes: &[u8]) -> Result<Vec<MeshFragment>, String> {
    use fbxcel_dom::any::AnyDocument;
    use fbxcel_dom::v7400::object::TypedObjectHandle;
    use fbxcel_dom::v7400::object::geometry::TypedGeometryHandle;

    let cursor = std::io::Cursor::new(bytes);
    let doc = AnyDocument::from_seekable_reader(cursor).map_err(|e| format!("FBX parse: {e}"))?;
    let doc = match doc {
        AnyDocument::V7400(_, doc) => doc,
        _ => return Err("FBX: only v7.4+ binary FBX is supported".into()),
    };

    let mut fragments: Vec<MeshFragment> = Vec::new();
    let mut fallback_idx = 0usize;

    for obj in doc.objects() {
        let geom = match obj.get_typed() {
            TypedObjectHandle::Geometry(TypedGeometryHandle::Mesh(m)) => m,
            _ => continue,
        };

        let mesh = match build_geom_mesh(&geom) {
            Some(m) if !m.vertices.is_empty() => m,
            _ => continue,
        };

        let model = geom.models().next();

        let name = model
            .and_then(|m| m.name())
            .map(|s| s.split('\u{0001}').next().unwrap_or(s).trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                fallback_idx += 1;
                format!("Mesh{fallback_idx}")
            });

        let (origin, pivot, rotation) = model
            .map(|m| model_transform(&m))
            .unwrap_or(([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0]));

        fragments.push(MeshFragment {
            name,
            mesh,
            origin,
            pivot,
            rotation,
        });
    }

    if fragments.is_empty() {
        return Err("FBX has no mesh data".into());
    }
    Ok(fragments)
}

fn build_geom_mesh(
    geom: &fbxcel_dom::v7400::object::geometry::MeshHandle,
) -> Option<Mesh> {
    let polygon_vertices = geom.polygon_vertices().ok()?;
    let triangles = polygon_vertices.triangulate_each(triangulator).ok()?;
    let control_points: Vec<[f32; 3]> = polygon_vertices
        .raw_control_points()
        .ok()?
        .map(|p| [p.x as f32, p.y as f32, p.z as f32])
        .collect();

    let mut vertices: Vec<Vertex3D> = control_points
        .iter()
        .map(|cp| Vertex3D {
            position: *cp,
            normal: [0.0, 0.0, 0.0],
            uv: [0.0, 0.0],
        })
        .collect();

    let mut indices: Vec<u32> = Vec::new();
    for tri_vi in triangles.triangle_vertex_indices() {
        if let Some(cpi) = triangles.control_point_index(tri_vi) {
            indices.push(cpi.to_u32());
        }
    }

    recompute_normals(&mut vertices, &indices);
    Some(Mesh { vertices, indices })
}

fn model_transform(
    model: &fbxcel_dom::v7400::object::model::MeshHandle,
) -> ([f32; 3], [f32; 3], [f32; 3]) {
    let props = model.properties_by_native_typename("FbxNode");
    let origin = read_prop_vec3(&props, "Lcl Translation").unwrap_or([0.0, 0.0, 0.0]);
    let pivot = read_prop_vec3(&props, "RotationPivot").unwrap_or([0.0, 0.0, 0.0]);
    let rot_deg = read_prop_vec3(&props, "Lcl Rotation").unwrap_or([0.0, 0.0, 0.0]);
    let rotation = [
        rot_deg[0].to_radians(),
        rot_deg[1].to_radians(),
        rot_deg[2].to_radians(),
    ];
    (origin, pivot, rotation)
}

/// Transform (origin, pivot, rotation) of the first mesh model node.
fn first_model_transform(
    doc: &fbxcel_dom::v7400::Document,
) -> ([f32; 3], [f32; 3], [f32; 3]) {
    use fbxcel_dom::v7400::object::TypedObjectHandle;
    use fbxcel_dom::v7400::object::model::TypedModelHandle;
    for obj in doc.objects() {
        if let TypedObjectHandle::Model(TypedModelHandle::Mesh(m)) = obj.get_typed() {
            return model_transform(&m);
        }
    }
    ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.0])
}

fn read_prop_vec3(
    props: &fbxcel_dom::v7400::object::property::ObjectProperties,
    name: &str,
) -> Option<[f32; 3]> {
    use fbxcel_dom::fbxcel::low::v7400::AttributeValue;
    let handle = props.get_property(name)?;
    let nums: Vec<f32> = handle
        .value_part()
        .iter()
        .filter_map(|a| match a {
            AttributeValue::F64(v) => Some(*v as f32),
            AttributeValue::F32(v) => Some(*v),
            AttributeValue::I32(v) => Some(*v as f32),
            AttributeValue::I64(v) => Some(*v as f32),
            _ => None,
        })
        .collect();
    let n = nums.len();
    if n >= 3 {
        Some([nums[n - 3], nums[n - 2], nums[n - 1]])
    } else {
        None
    }
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

// ---- FBX node transforms (so multi-mesh models -- e.g. multi-tile rooms whose
// pieces are positioned by per-node Lcl Translation -- assemble correctly) -------
type Mat3 = [[f32; 3]; 3];

fn mat3_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut r = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    r
}

fn mat3_vec(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

// Euler (degrees, XYZ order: R = Rz*Ry*Rx) to a rotation matrix.
fn euler_deg_mat(r: [f32; 3]) -> Mat3 {
    let (x, y, z) = (r[0].to_radians(), r[1].to_radians(), r[2].to_radians());
    let (cx, sx) = (x.cos(), x.sin());
    let (cy, sy) = (y.cos(), y.sin());
    let (cz, sz) = (z.cos(), z.sin());
    let rx: Mat3 = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry: Mat3 = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz: Mat3 = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    mat3_mul(&mat3_mul(&rz, &ry), &rx)
}

// Last 3 numeric values on the line holding `P: "<key>", ...,x,y,z`.
fn parse_prop_vec3(block: &str, key: &str) -> Option<[f32; 3]> {
    let needle = format!("\"{key}\"");
    let i = block.find(&needle)?;
    let end = block[i..].find('\n').map(|e| i + e).unwrap_or(block.len());
    let nums: Vec<f32> = block[i..end]
        .split(',')
        .filter_map(|s| s.trim().parse::<f32>().ok())
        .collect();
    let n = nums.len();
    if n >= 3 { Some([nums[n - 3], nums[n - 2], nums[n - 1]]) } else { None }
}

fn load_fbx_ascii(bytes: &[u8]) -> Result<Mesh, String> {
    use std::collections::HashMap;
    let text = std::str::from_utf8(bytes).map_err(|e| format!("ASCII FBX: not UTF-8: {e}"))?;

    // Object connections: child id -> parent id (OO links). A geometry's parent is
    // its Model node; a Model's parent is another Model or the root (0).
    let mut parents: HashMap<i64, i64> = HashMap::new();
    if let Some(ci) = text.find("Connections:") {
        for line in text[ci..].lines() {
            let l = line.trim();
            if l.starts_with("C:") && l.contains("\"OO\"") {
                let nums: Vec<i64> = l
                    .split(',')
                    .filter_map(|s| s.trim().trim_matches('"').parse::<i64>().ok())
                    .collect();
                if nums.len() >= 2 {
                    parents.insert(nums[0], nums[1]); // child -> parent
                }
            }
        }
    }

    // Per-Model-node local transform (linear = R*S, plus translation).
    let mut models: HashMap<i64, (Mat3, [f32; 3])> = HashMap::new();
    let objects = if let Some(oi) = text.find("Objects:") {
        match text[oi..].find('{') {
            Some(b) => { let bs = oi + b + 1; &text[bs..find_matching_brace(text, bs)] }
            None => text,
        }
    } else {
        text
    };
    {
        let mut c = 0usize;
        while let Some(p) = objects[c..].find("Model:") {
            let abs = c + p;
            let id: Option<i64> = objects[abs + 6..]
                .trim_start()
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
                .collect::<String>()
                .parse()
                .ok();
            let bstart = match objects[abs..].find('{') { Some(b) => abs + b + 1, None => break };
            let bend = find_matching_brace(objects, bstart);
            if let Some(id) = id {
                let block = &objects[bstart..bend];
                let t = parse_prop_vec3(block, "Lcl Translation").unwrap_or([0.0, 0.0, 0.0]);
                let rot = parse_prop_vec3(block, "Lcl Rotation").unwrap_or([0.0, 0.0, 0.0]);
                let s = parse_prop_vec3(block, "Lcl Scaling").unwrap_or([1.0, 1.0, 1.0]);
                let scale: Mat3 = [[s[0], 0.0, 0.0], [0.0, s[1], 0.0], [0.0, 0.0, s[2]]];
                models.insert(id, (mat3_mul(&euler_deg_mat(rot), &scale), t));
            }
            c = bend;
        }
    }

    // Walk a geometry's model chain to world space: p -> T + (R*S) * p, up to root.
    let to_world = |model_id: i64, p: [f32; 3]| -> [f32; 3] {
        let mut pt = p;
        let mut cur = model_id;
        let mut guard = 0;
        while cur != 0 && guard < 64 {
            if let Some((lin, t)) = models.get(&cur) {
                let r = mat3_vec(lin, pt);
                pt = [r[0] + t[0], r[1] + t[1], r[2] + t[2]];
            }
            cur = parents.get(&cur).copied().unwrap_or(0);
            guard += 1;
        }
        pt
    };

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

        // Control points (shared vertex positions), transformed by this geometry's
        // Model node (so multi-mesh / multi-tile FBX assemble in the right place).
        let geom_id: i64 = text[abs + "Geometry:".len()..]
            .trim_start()
            .chars()
            .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        let model_id = parents.get(&geom_id).copied().unwrap_or(0);
        let cps: Vec<[f32; 3]> = positions
            .chunks_exact(3)
            .map(|c| to_world(model_id, [c[0] as f32, c[1] as f32, c[2] as f32]))
            .collect();

        // Optional UV layer. FBX maps UVs either per control-point or per
        // polygon-vertex, with Direct or IndexToDirect referencing.
        let uv_layer = sub_block(block, "LayerElementUV");
        let uv_array = uv_layer
            .map(|b| parse_named_float_array(b, "UV"))
            .unwrap_or_default();
        let uv_index = uv_layer
            .map(|b| parse_named_int_array(b, "UVIndex"))
            .unwrap_or_default();
        let uv_by_control = uv_layer
            .and_then(|b| parse_quoted_value(b, "MappingInformationType"))
            .map(|s| s.contains("ControlPoint"))
            .unwrap_or(false);
        let uv_indexed = uv_layer
            .and_then(|b| parse_quoted_value(b, "ReferenceInformationType"))
            .map(|s| s.contains("Index"))
            .unwrap_or(false);

        // UV for the polygon-vertex at global index `pvi` whose control point is `cp`.
        let uv_at = |pvi: usize, cp: usize| -> [f32; 2] {
            if uv_array.len() < 2 {
                return [0.0, 0.0];
            }
            let key = if uv_by_control { cp } else { pvi };
            let di = if uv_indexed {
                match uv_index.get(key) {
                    Some(&v) if v >= 0 => v as usize,
                    _ => return [0.0, 0.0],
                }
            } else {
                key
            };
            let o = di * 2;
            if o + 1 < uv_array.len() {
                // FBX UV origin is bottom-left; flip V for top-left texture sampling.
                [uv_array[o] as f32, 1.0 - uv_array[o + 1] as f32]
            } else {
                [0.0, 0.0]
            }
        };

        // Expand to per-polygon-vertex vertices (so each carries its own UV) and
        // triangulate every polygon as a fan. PolygonVertexIndex entries are control-
        // point indices; a negative entry (bitwise-NOT) marks the end of a polygon.
        let mut poly: Vec<(usize, usize)> = Vec::new(); // (control-point index, polygon-vertex index)
        let mut pvi = 0usize;
        for raw in &raw_indices {
            let r = *raw;
            let cp = if r < 0 { (!r) as usize } else { r as usize };
            poly.push((cp, pvi));
            pvi += 1;
            if r < 0 {
                if poly.len() >= 3 {
                    for i in 1..poly.len() - 1 {
                        for &(cpi, pv) in &[poly[0], poly[i], poly[i + 1]] {
                            let pos = cps.get(cpi).copied().unwrap_or([0.0; 3]);
                            let idx = vertices.len() as u32;
                            vertices.push(Vertex3D {
                                position: pos,
                                normal: [0.0; 3],
                                uv: uv_at(pv, cpi),
                            });
                            indices.push(idx);
                        }
                    }
                }
                poly.clear();
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

/// Per-geometry split of an ASCII FBX. Each `Geometry:` block becomes one
/// fragment, named after its owning `Model:` node and tagged with that node's
/// `Lcl Translation` (origin) and `RotationPivot` (pivot). Vertices stay in
/// geometry-local space -- the transform is reported, not baked in.
fn load_fbx_ascii_fragments(bytes: &[u8]) -> Result<Vec<MeshFragment>, String> {
    use std::collections::HashMap;
    let text = std::str::from_utf8(bytes).map_err(|e| format!("ASCII FBX: not UTF-8: {e}"))?;

    let mut parents: HashMap<i64, i64> = HashMap::new();
    if let Some(ci) = text.find("Connections:") {
        for line in text[ci..].lines() {
            let l = line.trim();
            if l.starts_with("C:") && l.contains("\"OO\"") {
                let nums: Vec<i64> = l
                    .split(',')
                    .filter_map(|s| s.trim().trim_matches('"').parse::<i64>().ok())
                    .collect();
                if nums.len() >= 2 {
                    parents.insert(nums[0], nums[1]);
                }
            }
        }
    }

    let mut models: HashMap<i64, (String, [f32; 3], [f32; 3], [f32; 3])> = HashMap::new();
    let (objects, objects_offset) = if let Some(oi) = text.find("Objects:") {
        match text[oi..].find('{') {
            Some(b) => {
                let bs = oi + b + 1;
                (&text[bs..find_matching_brace(text, bs)], bs)
            }
            None => (text, 0),
        }
    } else {
        (text, 0)
    };
    {
        let mut c = 0usize;
        while let Some(p) = objects[c..].find("Model:") {
            let abs = c + p;
            let after_kw = &objects[abs + "Model:".len()..];
            let id: Option<i64> = after_kw
                .trim_start()
                .chars()
                .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
                .collect::<String>()
                .parse()
                .ok();
            let name = parse_quoted_name_after(after_kw)
                .map(|s| s.strip_prefix("Model::").unwrap_or(&s).trim().to_string())
                .filter(|s| !s.is_empty());
            let bstart = match objects[abs..].find('{') {
                Some(b) => abs + b + 1,
                None => break,
            };
            let bend = find_matching_brace(objects, bstart);
            if let Some(id) = id {
                let block = &objects[bstart..bend];
                let origin = parse_prop_vec3(block, "Lcl Translation").unwrap_or([0.0, 0.0, 0.0]);
                let pivot = parse_prop_vec3(block, "RotationPivot").unwrap_or([0.0, 0.0, 0.0]);
                let rot_deg = parse_prop_vec3(block, "Lcl Rotation").unwrap_or([0.0, 0.0, 0.0]);
                let rotation = [
                    rot_deg[0].to_radians(),
                    rot_deg[1].to_radians(),
                    rot_deg[2].to_radians(),
                ];
                models.insert(id, (name.unwrap_or_default(), origin, pivot, rotation));
            }
            c = bend;
        }
    }
    let _ = objects_offset;

    let mut fragments: Vec<MeshFragment> = Vec::new();
    let mut fallback_idx = 0usize;
    let mut counts: HashMap<String, usize> = HashMap::new();

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

        if positions.is_empty() || raw_indices.is_empty() || positions.len() % 3 != 0 {
            cursor = block_end;
            continue;
        }

        let geom_id: i64 = text[abs + "Geometry:".len()..]
            .trim_start()
            .chars()
            .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        let model_id = parents.get(&geom_id).copied().unwrap_or(0);

        let cps: Vec<[f32; 3]> = positions
            .chunks_exact(3)
            .map(|c| [c[0] as f32, c[1] as f32, c[2] as f32])
            .collect();

        let uv_layer = sub_block(block, "LayerElementUV");
        let uv_array = uv_layer
            .map(|b| parse_named_float_array(b, "UV"))
            .unwrap_or_default();
        let uv_index = uv_layer
            .map(|b| parse_named_int_array(b, "UVIndex"))
            .unwrap_or_default();
        let uv_by_control = uv_layer
            .and_then(|b| parse_quoted_value(b, "MappingInformationType"))
            .map(|s| s.contains("ControlPoint"))
            .unwrap_or(false);
        let uv_indexed = uv_layer
            .and_then(|b| parse_quoted_value(b, "ReferenceInformationType"))
            .map(|s| s.contains("Index"))
            .unwrap_or(false);
        let uv_at = |pvi: usize, cp: usize| -> [f32; 2] {
            if uv_array.len() < 2 {
                return [0.0, 0.0];
            }
            let key = if uv_by_control { cp } else { pvi };
            let di = if uv_indexed {
                match uv_index.get(key) {
                    Some(&v) if v >= 0 => v as usize,
                    _ => return [0.0, 0.0],
                }
            } else {
                key
            };
            let o = di * 2;
            if o + 1 < uv_array.len() {
                [uv_array[o] as f32, 1.0 - uv_array[o + 1] as f32]
            } else {
                [0.0, 0.0]
            }
        };

        let mut vertices: Vec<Vertex3D> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut poly: Vec<(usize, usize)> = Vec::new();
        let mut pvi = 0usize;
        for raw in &raw_indices {
            let r = *raw;
            let cp = if r < 0 { (!r) as usize } else { r as usize };
            poly.push((cp, pvi));
            pvi += 1;
            if r < 0 {
                if poly.len() >= 3 {
                    for i in 1..poly.len() - 1 {
                        for &(cpi, pv) in &[poly[0], poly[i], poly[i + 1]] {
                            let pos = cps.get(cpi).copied().unwrap_or([0.0; 3]);
                            let idx = vertices.len() as u32;
                            vertices.push(Vertex3D {
                                position: pos,
                                normal: [0.0; 3],
                                uv: uv_at(pv, cpi),
                            });
                            indices.push(idx);
                        }
                    }
                }
                poly.clear();
            }
        }

        cursor = block_end;

        if vertices.is_empty() {
            continue;
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
            vertices[i].normal = if len > 1e-6 {
                [n[0] / len, n[1] / len, n[2] / len]
            } else {
                [0.0, 1.0, 0.0]
            };
        }

        let (base_name, origin, pivot, rotation) = models
            .get(&model_id)
            .cloned()
            .unwrap_or_else(|| (String::new(), [0.0; 3], [0.0; 3], [0.0; 3]));
        let base = if base_name.is_empty() {
            fallback_idx += 1;
            format!("Mesh{fallback_idx}")
        } else {
            base_name
        };
        let n = counts.entry(base.clone()).or_insert(0);
        let name = if *n == 0 { base.clone() } else { format!("{base} ({n})") };
        *n += 1;

        fragments.push(MeshFragment {
            name,
            mesh: Mesh { vertices, indices },
            origin,
            pivot,
            rotation,
        });
    }

    if fragments.is_empty() {
        return Err("ASCII FBX: no Geometry/Vertices found".into());
    }
    Ok(fragments)
}

/// Read the first `"..."` literal that follows the cursor in `s`.
fn parse_quoted_name_after(s: &str) -> Option<String> {
    let q1 = s.find('"')?;
    let after = &s[q1 + 1..];
    let q2 = after.find('"')?;
    Some(after[..q2].to_string())
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

// Return the contents between the braces that follow `key:` (e.g. the body of a
// `LayerElementUV: 0 { ... }` node), or None if the key isn't present.
fn sub_block<'a>(block: &'a str, key: &str) -> Option<&'a str> {
    let needle = format!("{key}:");
    let i = block.find(&needle)?;
    let after = &block[i + needle.len()..];
    let open = after.find('{')?;
    let close = find_matching_brace(after, open + 1);
    Some(&after[open + 1..close])
}

// First double-quoted string value following `key:` (e.g. MappingInformationType).
fn parse_quoted_value(block: &str, key: &str) -> Option<String> {
    let needle = format!("{key}:");
    let i = block.find(&needle)?;
    let after = &block[i + needle.len()..];
    let q1 = after.find('"')?;
    let rest = &after[q1 + 1..];
    let q2 = rest.find('"')?;
    Some(rest[..q2].to_string())
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
