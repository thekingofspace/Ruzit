

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

pub fn cube() -> Mesh {
    let s = 0.5_f32;
    let v = vec![
        
        Vertex3D { position: [s, -s, -s], normal: [1.0, 0.0, 0.0], uv: [0.0, 1.0] },
        Vertex3D { position: [s,  s, -s], normal: [1.0, 0.0, 0.0], uv: [0.0, 0.0] },
        Vertex3D { position: [s,  s,  s], normal: [1.0, 0.0, 0.0], uv: [1.0, 0.0] },
        Vertex3D { position: [s, -s,  s], normal: [1.0, 0.0, 0.0], uv: [1.0, 1.0] },
        
        Vertex3D { position: [-s, -s,  s], normal: [-1.0, 0.0, 0.0], uv: [0.0, 1.0] },
        Vertex3D { position: [-s,  s,  s], normal: [-1.0, 0.0, 0.0], uv: [0.0, 0.0] },
        Vertex3D { position: [-s,  s, -s], normal: [-1.0, 0.0, 0.0], uv: [1.0, 0.0] },
        Vertex3D { position: [-s, -s, -s], normal: [-1.0, 0.0, 0.0], uv: [1.0, 1.0] },
        
        Vertex3D { position: [-s, s, -s], normal: [0.0, 1.0, 0.0], uv: [0.0, 1.0] },
        Vertex3D { position: [-s, s,  s], normal: [0.0, 1.0, 0.0], uv: [0.0, 0.0] },
        Vertex3D { position: [ s, s,  s], normal: [0.0, 1.0, 0.0], uv: [1.0, 0.0] },
        Vertex3D { position: [ s, s, -s], normal: [0.0, 1.0, 0.0], uv: [1.0, 1.0] },
        
        Vertex3D { position: [-s, -s,  s], normal: [0.0, -1.0, 0.0], uv: [0.0, 1.0] },
        Vertex3D { position: [-s, -s, -s], normal: [0.0, -1.0, 0.0], uv: [0.0, 0.0] },
        Vertex3D { position: [ s, -s, -s], normal: [0.0, -1.0, 0.0], uv: [1.0, 0.0] },
        Vertex3D { position: [ s, -s,  s], normal: [0.0, -1.0, 0.0], uv: [1.0, 1.0] },
        
        Vertex3D { position: [ s, -s, s], normal: [0.0, 0.0, 1.0], uv: [0.0, 1.0] },
        Vertex3D { position: [ s,  s, s], normal: [0.0, 0.0, 1.0], uv: [0.0, 0.0] },
        Vertex3D { position: [-s,  s, s], normal: [0.0, 0.0, 1.0], uv: [1.0, 0.0] },
        Vertex3D { position: [-s, -s, s], normal: [0.0, 0.0, 1.0], uv: [1.0, 1.0] },
        
        Vertex3D { position: [-s, -s, -s], normal: [0.0, 0.0, -1.0], uv: [0.0, 1.0] },
        Vertex3D { position: [-s,  s, -s], normal: [0.0, 0.0, -1.0], uv: [0.0, 0.0] },
        Vertex3D { position: [ s,  s, -s], normal: [0.0, 0.0, -1.0], uv: [1.0, 0.0] },
        Vertex3D { position: [ s, -s, -s], normal: [0.0, 0.0, -1.0], uv: [1.0, 1.0] },
    ];
    let mut i: Vec<u32> = Vec::with_capacity(36);
    for face in 0..6_u32 {
        let base = face * 4;
        i.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    Mesh { vertices: v, indices: i }
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
                let xyz: Vec<f32> = parts
                    .take(3)
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if xyz.len() == 3 {
                    positions.push([xyz[0], xyz[1], xyz[2]]);
                }
            }
            Some("vn") => {
                let xyz: Vec<f32> = parts
                    .take(3)
                    .filter_map(|s| s.parse().ok())
                    .collect();
                if xyz.len() == 3 {
                    normals.push([xyz[0], xyz[1], xyz[2]]);
                    had_normals = true;
                }
            }
            Some("vt") => {
                let xy: Vec<f32> = parts
                    .take(2)
                    .filter_map(|s| s.parse().ok())
                    .collect();
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
                            .and_then(|s| if s.is_empty() { None } else { s.parse::<i32>().ok() })
                            .unwrap_or(0);
                        let ni = parts_t
                            .get(2)
                            .and_then(|s| s.parse::<i32>().ok())
                            .unwrap_or(0);
                        let p = norm_idx(pi, positions.len());
                        let t = if ti != 0 { norm_idx(ti, uvs.len()) } else { u32::MAX };
                        let n = if ni != 0 { norm_idx(ni, normals.len()) } else { u32::MAX };
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
