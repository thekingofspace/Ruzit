

use mlua::{AnyUserData, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};


fn lerp_f(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn clamp_u8_f(v: f32) -> u8 {
    if v < 0.0 {
        0
    } else if v > 255.0 {
        255
    } else {
        v as u8
    }
}

fn clamp_u8_i(v: i64) -> u8 {
    v.clamp(0, 255) as u8
}

fn as_scalar(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => Some(*n as f32),
        Value::Integer(n) => Some(*n as f32),
        _ => None,
    }
}

fn as_userdata<T: 'static + Copy + Clone>(v: &Value) -> Option<T> {
    if let Value::UserData(ud) = v {
        return ud.borrow::<T>().ok().map(|r| *r);
    }
    None
}


#[derive(Clone, Copy, Debug)]
pub struct Dim {
    pub x: f32,
    pub y: f32,
}

impl Dim {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl UserData for Dim {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("X", |_, this| Ok(this.x));
        f.add_field_method_get("Y", |_, this| Ok(this.y));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "Lerp",
            |_, this, (other, t): (AnyUserData, f32)| -> mlua::Result<Dim> {
                let o = *other.borrow::<Dim>()?;
                Ok(Dim::new(lerp_f(this.x, o.x, t), lerp_f(this.y, o.y, t)))
            },
        );

        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!("Dim({}, {})", this.x, this.y))
        });

        m.add_meta_function(
            "__add",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<Dim> {
                let a = *a.borrow::<Dim>()?;
                let b = *b.borrow::<Dim>()?;
                Ok(Dim::new(a.x + b.x, a.y + b.y))
            },
        );
        m.add_meta_function(
            "__sub",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<Dim> {
                let a = *a.borrow::<Dim>()?;
                let b = *b.borrow::<Dim>()?;
                Ok(Dim::new(a.x - b.x, a.y - b.y))
            },
        );
        
        m.add_meta_function("__mul", |_, (a, b): (Value, Value)| -> mlua::Result<Dim> {
            let (d, s) = pair_with_scalar::<Dim>(&a, &b, "Dim * expects (Dim, number)")?;
            Ok(Dim::new(d.x * s, d.y * s))
        });
        m.add_meta_function("__div", |_, (a, b): (Value, Value)| -> mlua::Result<Dim> {
            let (d, s) = pair_with_scalar::<Dim>(&a, &b, "Dim / expects (Dim, number)")?;
            Ok(Dim::new(d.x / s, d.y / s))
        });
        m.add_meta_function("__unm", |_, ud: AnyUserData| -> mlua::Result<Dim> {
            let d = *ud.borrow::<Dim>()?;
            Ok(Dim::new(-d.x, -d.y))
        });
        m.add_meta_function(
            "__eq",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<Dim>()?;
                let b = b.borrow::<Dim>()?;
                Ok(a.x == b.x && a.y == b.y)
            },
        );
        m.add_meta_function(
            "__lt",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<Dim>()?;
                let b = b.borrow::<Dim>()?;
                Ok(a.x < b.x && a.y < b.y)
            },
        );
        m.add_meta_function(
            "__le",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<Dim>()?;
                let b = b.borrow::<Dim>()?;
                Ok(a.x <= b.x && a.y <= b.y)
            },
        );
    }
}


#[derive(Clone, Copy, Debug)]
pub struct Color3 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color3 {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

impl UserData for Color3 {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("R", |_, this| Ok(this.r as i64));
        f.add_field_method_get("G", |_, this| Ok(this.g as i64));
        f.add_field_method_get("B", |_, this| Ok(this.b as i64));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "Lerp",
            |_, this, (other, t): (AnyUserData, f32)| -> mlua::Result<Color3> {
                let o = *other.borrow::<Color3>()?;
                Ok(Color3::new(
                    clamp_u8_f(lerp_f(this.r as f32, o.r as f32, t)),
                    clamp_u8_f(lerp_f(this.g as f32, o.g as f32, t)),
                    clamp_u8_f(lerp_f(this.b as f32, o.b as f32, t)),
                ))
            },
        );

        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!("Color3({}, {}, {})", this.r, this.g, this.b))
        });

        
        m.add_meta_function(
            "__add",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<Color3> {
                let a = *a.borrow::<Color3>()?;
                let b = *b.borrow::<Color3>()?;
                Ok(Color3::new(
                    clamp_u8_f(a.r as f32 + b.r as f32),
                    clamp_u8_f(a.g as f32 + b.g as f32),
                    clamp_u8_f(a.b as f32 + b.b as f32),
                ))
            },
        );
        m.add_meta_function(
            "__sub",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<Color3> {
                let a = *a.borrow::<Color3>()?;
                let b = *b.borrow::<Color3>()?;
                Ok(Color3::new(
                    clamp_u8_f(a.r as f32 - b.r as f32),
                    clamp_u8_f(a.g as f32 - b.g as f32),
                    clamp_u8_f(a.b as f32 - b.b as f32),
                ))
            },
        );
        m.add_meta_function(
            "__eq",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<Color3>()?;
                let b = b.borrow::<Color3>()?;
                Ok(a.r == b.r && a.g == b.g && a.b == b.b)
            },
        );
        m.add_meta_function(
            "__lt",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<Color3>()?;
                let b = b.borrow::<Color3>()?;
                Ok(a.r < b.r && a.g < b.g && a.b < b.b)
            },
        );
        m.add_meta_function(
            "__le",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<Color3>()?;
                let b = b.borrow::<Color3>()?;
                Ok(a.r <= b.r && a.g <= b.g && a.b <= b.b)
            },
        );
    }
}


#[derive(Clone, Copy, Debug)]
pub struct Vector {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vector {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn magnitude(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }
}

impl UserData for Vector {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("X", |_, this| Ok(this.x));
        f.add_field_method_get("Y", |_, this| Ok(this.y));
        f.add_field_method_get("Z", |_, this| Ok(this.z));
        f.add_field_method_get("Magnitude", |_, this| Ok(this.magnitude()));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "Lerp",
            |_, this, (other, t): (AnyUserData, f32)| -> mlua::Result<Vector> {
                let o = *other.borrow::<Vector>()?;
                Ok(Vector::new(
                    lerp_f(this.x, o.x, t),
                    lerp_f(this.y, o.y, t),
                    lerp_f(this.z, o.z, t),
                ))
            },
        );

        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!("Vector({}, {}, {})", this.x, this.y, this.z))
        });

        m.add_meta_function(
            "__add",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<Vector> {
                let a = *a.borrow::<Vector>()?;
                let b = *b.borrow::<Vector>()?;
                Ok(Vector::new(a.x + b.x, a.y + b.y, a.z + b.z))
            },
        );
        m.add_meta_function(
            "__sub",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<Vector> {
                let a = *a.borrow::<Vector>()?;
                let b = *b.borrow::<Vector>()?;
                Ok(Vector::new(a.x - b.x, a.y - b.y, a.z - b.z))
            },
        );
        m.add_meta_function(
            "__mul",
            |_, (a, b): (Value, Value)| -> mlua::Result<Vector> {
                let (v, s) =
                    pair_with_scalar::<Vector>(&a, &b, "Vector * expects (Vector, number)")?;
                Ok(Vector::new(v.x * s, v.y * s, v.z * s))
            },
        );
        m.add_meta_function(
            "__div",
            |_, (a, b): (Value, Value)| -> mlua::Result<Vector> {
                let (v, s) =
                    pair_with_scalar::<Vector>(&a, &b, "Vector / expects (Vector, number)")?;
                Ok(Vector::new(v.x / s, v.y / s, v.z / s))
            },
        );
        m.add_meta_function("__unm", |_, ud: AnyUserData| -> mlua::Result<Vector> {
            let v = *ud.borrow::<Vector>()?;
            Ok(Vector::new(-v.x, -v.y, -v.z))
        });
        m.add_meta_function(
            "__eq",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<Vector>()?;
                let b = b.borrow::<Vector>()?;
                Ok(a.x == b.x && a.y == b.y && a.z == b.z)
            },
        );
        m.add_meta_function(
            "__lt",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<Vector>()?;
                let b = b.borrow::<Vector>()?;
                Ok(a.x < b.x && a.y < b.y && a.z < b.z)
            },
        );
        m.add_meta_function(
            "__le",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<Vector>()?;
                let b = b.borrow::<Vector>()?;
                Ok(a.x <= b.x && a.y <= b.y && a.z <= b.z)
            },
        );
    }
}


#[derive(Clone, Copy, Debug)]
pub struct CFrame {
    pub position: Vector,
    pub rotation: Vector,
}

impl CFrame {
    pub fn new(position: Vector, rotation: Vector) -> Self {
        Self { position, rotation }
    }
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


fn matrix_to_euler(m: Mat3) -> Vector {
    let sy = m[0][2].clamp(-1.0, 1.0);
    let cy_sq = 1.0 - sy * sy;
    if cy_sq > 1e-6 {
        Vector::new(
            (-m[1][2]).atan2(m[2][2]),
            sy.asin(),
            (-m[0][1]).atan2(m[0][0]),
        )
    } else {
        Vector::new(m[2][1].atan2(m[1][1]), sy.asin(), 0.0)
    }
}

fn mat3_mul(a: Mat3, b: Mat3) -> Mat3 {
    let mut out = [[0.0_f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

fn mat3_apply(m: Mat3, v: Vector) -> Vector {
    Vector::new(
        m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
        m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
        m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
    )
}

impl UserData for CFrame {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("Position", |_, this| Ok(this.position));
        f.add_field_method_get("Rotation", |_, this| Ok(this.rotation));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "Lerp",
            |_, this, (other, t): (AnyUserData, f32)| -> mlua::Result<CFrame> {
                let o = *other.borrow::<CFrame>()?;
                Ok(CFrame::new(
                    Vector::new(
                        lerp_f(this.position.x, o.position.x, t),
                        lerp_f(this.position.y, o.position.y, t),
                        lerp_f(this.position.z, o.position.z, t),
                    ),
                    Vector::new(
                        lerp_f(this.rotation.x, o.rotation.x, t),
                        lerp_f(this.rotation.y, o.rotation.y, t),
                        lerp_f(this.rotation.z, o.rotation.z, t),
                    ),
                ))
            },
        );

        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!(
                "CFrame(Position=Vector({}, {}, {}), Rotation=Vector({}, {}, {}))",
                this.position.x,
                this.position.y,
                this.position.z,
                this.rotation.x,
                this.rotation.y,
                this.rotation.z,
            ))
        });

        m.add_meta_function(
            "__eq",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<CFrame>()?;
                let b = b.borrow::<CFrame>()?;
                Ok(a.position.x == b.position.x
                    && a.position.y == b.position.y
                    && a.position.z == b.position.z
                    && a.rotation.x == b.rotation.x
                    && a.rotation.y == b.rotation.y
                    && a.rotation.z == b.rotation.z)
            },
        );
        
        
        m.add_meta_function(
            "__lt",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<CFrame>()?;
                let b = b.borrow::<CFrame>()?;
                Ok(a.position.x < b.position.x
                    && a.position.y < b.position.y
                    && a.position.z < b.position.z
                    && a.rotation.x < b.rotation.x
                    && a.rotation.y < b.rotation.y
                    && a.rotation.z < b.rotation.z)
            },
        );
        m.add_meta_function(
            "__le",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<CFrame>()?;
                let b = b.borrow::<CFrame>()?;
                Ok(a.position.x <= b.position.x
                    && a.position.y <= b.position.y
                    && a.position.z <= b.position.z
                    && a.rotation.x <= b.rotation.x
                    && a.rotation.y <= b.rotation.y
                    && a.rotation.z <= b.rotation.z)
            },
        );
        
        
        m.add_meta_function(
            "__add",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<CFrame> {
                let a = *a.borrow::<CFrame>()?;
                let b = *b.borrow::<CFrame>()?;
                Ok(CFrame::new(
                    Vector::new(
                        a.position.x + b.position.x,
                        a.position.y + b.position.y,
                        a.position.z + b.position.z,
                    ),
                    Vector::new(
                        a.rotation.x + b.rotation.x,
                        a.rotation.y + b.rotation.y,
                        a.rotation.z + b.rotation.z,
                    ),
                ))
            },
        );
        m.add_meta_function(
            "__sub",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<CFrame> {
                let a = *a.borrow::<CFrame>()?;
                let b = *b.borrow::<CFrame>()?;
                Ok(CFrame::new(
                    Vector::new(
                        a.position.x - b.position.x,
                        a.position.y - b.position.y,
                        a.position.z - b.position.z,
                    ),
                    Vector::new(
                        a.rotation.x - b.rotation.x,
                        a.rotation.y - b.rotation.y,
                        a.rotation.z - b.rotation.z,
                    ),
                ))
            },
        );

        
        m.add_meta_function(
            "__mul",
            |lua, (a, b): (Value, Value)| -> mlua::Result<Value> {
                let lhs = as_userdata::<CFrame>(&a).ok_or_else(|| {
                    mlua::Error::RuntimeError(
                        "CFrame * expects CFrame on the left".into(),
                    )
                })?;
                let lhs_mat = euler_to_matrix(lhs.rotation);

                if let Some(rhs) = as_userdata::<CFrame>(&b) {
                    let rhs_mat = euler_to_matrix(rhs.rotation);
                    let combined = mat3_mul(lhs_mat, rhs_mat);
                    let rotated_offset = mat3_apply(lhs_mat, rhs.position);
                    let position = Vector::new(
                        lhs.position.x + rotated_offset.x,
                        lhs.position.y + rotated_offset.y,
                        lhs.position.z + rotated_offset.z,
                    );
                    let rotation = matrix_to_euler(combined);
                    let ud = lua.create_userdata(CFrame::new(position, rotation))?;
                    return Ok(Value::UserData(ud));
                }
                if let Some(v) = as_userdata::<Vector>(&b) {
                    let rotated = mat3_apply(lhs_mat, v);
                    let result = Vector::new(
                        lhs.position.x + rotated.x,
                        lhs.position.y + rotated.y,
                        lhs.position.z + rotated.z,
                    );
                    let ud = lua.create_userdata(result)?;
                    return Ok(Value::UserData(ud));
                }
                Err(mlua::Error::RuntimeError(
                    "CFrame * expects a CFrame or Vector on the right".into(),
                ))
            },
        );
    }
}


fn pair_with_scalar<T: 'static + Copy>(a: &Value, b: &Value, err: &str) -> mlua::Result<(T, f32)> {
    if let (Some(t), Some(s)) = (as_userdata::<T>(a), as_scalar(b)) {
        return Ok((t, s));
    }
    if let (Some(s), Some(t)) = (as_scalar(a), as_userdata::<T>(b)) {
        return Ok((t, s));
    }
    Err(mlua::Error::RuntimeError(err.to_string()))
}


pub fn create(lua: &Lua) -> mlua::Result<Table> {
    let t = lua.create_table()?;

    let dim_class = lua.create_table()?;
    dim_class.set(
        "new",
        lua.create_function(|_, (x, y): (f32, f32)| Ok(Dim::new(x, y)))?,
    )?;
    t.set("Dim", dim_class)?;

    let color_class = lua.create_table()?;
    color_class.set(
        "new",
        lua.create_function(|_, (r, g, b): (i64, i64, i64)| {
            Ok(Color3::new(clamp_u8_i(r), clamp_u8_i(g), clamp_u8_i(b)))
        })?,
    )?;
    color_class.set(
        "fromHex",
        lua.create_function(|_, hex: String| -> mlua::Result<Color3> {
            let s = hex.trim_start_matches('#');
            if s.len() != 6 {
                return Err(mlua::Error::RuntimeError(format!(
                    "Color3.fromHex: expected 6 hex chars, got '{hex}'"
                )));
            }
            let r = u8::from_str_radix(&s[0..2], 16)
                .map_err(|e| mlua::Error::RuntimeError(format!("fromHex: {e}")))?;
            let g = u8::from_str_radix(&s[2..4], 16)
                .map_err(|e| mlua::Error::RuntimeError(format!("fromHex: {e}")))?;
            let b = u8::from_str_radix(&s[4..6], 16)
                .map_err(|e| mlua::Error::RuntimeError(format!("fromHex: {e}")))?;
            Ok(Color3::new(r, g, b))
        })?,
    )?;
    t.set("Color3", color_class)?;

    let vec_class = lua.create_table()?;
    vec_class.set(
        "new",
        lua.create_function(|_, args: mlua::MultiValue| -> mlua::Result<Vector> {
            
            let mut iter = args.into_iter();
            let x = iter
                .next()
                .and_then(|v| as_scalar(&v))
                .unwrap_or(0.0);
            let y = iter
                .next()
                .and_then(|v| as_scalar(&v))
                .unwrap_or(0.0);
            let z = iter
                .next()
                .and_then(|v| as_scalar(&v))
                .unwrap_or(0.0);
            Ok(Vector::new(x, y, z))
        })?,
    )?;
    vec_class.set(
        "zero",
        lua.create_function(|_, _: ()| Ok(Vector::new(0.0, 0.0, 0.0)))?,
    )?;
    vec_class.set(
        "one",
        lua.create_function(|_, _: ()| Ok(Vector::new(1.0, 1.0, 1.0)))?,
    )?;
    t.set("Vector", vec_class)?;

    let cframe_class = lua.create_table()?;
    cframe_class.set(
        "new",
        lua.create_function(
            |_, (pos, rot): (Option<AnyUserData>, Option<AnyUserData>)| -> mlua::Result<CFrame> {
                let p = match pos {
                    Some(ud) => *ud.borrow::<Vector>().map_err(|_| {
                        mlua::Error::RuntimeError(
                            "CFrame.new: first argument must be a Vector".into(),
                        )
                    })?,
                    None => Vector::new(0.0, 0.0, 0.0),
                };
                let r = match rot {
                    Some(ud) => *ud.borrow::<Vector>().map_err(|_| {
                        mlua::Error::RuntimeError(
                            "CFrame.new: second argument must be a Vector".into(),
                        )
                    })?,
                    None => Vector::new(0.0, 0.0, 0.0),
                };
                Ok(CFrame::new(p, r))
            },
        )?,
    )?;
    
    
    cframe_class.set(
        "Angles",
        lua.create_function(|_, (rx, ry, rz): (f32, f32, f32)| {
            Ok(CFrame::new(
                Vector::new(0.0, 0.0, 0.0),
                Vector::new(rx, ry, rz),
            ))
        })?,
    )?;
    t.set("CFrame", cframe_class)?;

    Ok(t)
}
