use mlua::{AnyUserData, FromLua, Lua, Table, UserData, UserDataFields, UserDataMethods, Value};

fn lerp_f(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
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

        m.add_method(
            "Scale",
            |_,
             this,
             (reference, target, maintain): (AnyUserData, AnyUserData, Option<bool>)|
             -> mlua::Result<Dim> {
                let r = *reference.borrow::<Dim>()?;
                let t = *target.borrow::<Dim>()?;
                let rx = if r.x.abs() > 1e-6 { t.x / r.x } else { 1.0 };
                let ry = if r.y.abs() > 1e-6 { t.y / r.y } else { 1.0 };
                let (sx, sy) = if maintain.unwrap_or(false) {
                    let s = rx.min(ry);
                    (s, s)
                } else {
                    (rx, ry)
                };
                Ok(Dim::new(this.x * sx, this.y * sy))
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
    pub r: f32,
    pub g: f32,
    pub b: f32,
}

impl Color3 {
    pub fn new(r: f32, g: f32, b: f32) -> Self {
        Self {
            r: r.clamp(0.0, 1.0),
            g: g.clamp(0.0, 1.0),
            b: b.clamp(0.0, 1.0),
        }
    }
}

impl UserData for Color3 {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("R", |_, this| Ok(this.r as f64));
        f.add_field_method_get("G", |_, this| Ok(this.g as f64));
        f.add_field_method_get("B", |_, this| Ok(this.b as f64));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "Lerp",
            |_, this, (other, t): (AnyUserData, f32)| -> mlua::Result<Color3> {
                let o = *other.borrow::<Color3>()?;
                Ok(Color3::new(
                    lerp_f(this.r, o.r, t),
                    lerp_f(this.g, o.g, t),
                    lerp_f(this.b, o.b, t),
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
                Ok(Color3::new(a.r + b.r, a.g + b.g, a.b + b.b))
            },
        );
        m.add_meta_function(
            "__sub",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<Color3> {
                let a = *a.borrow::<Color3>()?;
                let b = *b.borrow::<Color3>()?;
                Ok(Color3::new(a.r - b.r, a.g - b.g, a.b - b.b))
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

    pub fn dot(&self, o: &Vector) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(&self, o: &Vector) -> Vector {
        Vector::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn normalized(&self) -> Vector {
        let m = self.magnitude();
        if m <= 1e-6 {
            *self
        } else {
            Vector::new(self.x / m, self.y / m, self.z / m)
        }
    }

    pub fn distance(&self, o: &Vector) -> f32 {
        let dx = self.x - o.x;
        let dy = self.y - o.y;
        let dz = self.z - o.z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }

    pub fn to_native(&self) -> mlua::Vector {
        mlua::Vector::new(self.x, self.y, self.z)
    }

    pub fn from_native(v: mlua::Vector) -> Self {
        Vector::new(v.x(), v.y(), v.z())
    }
}

impl FromLua for Vector {
    fn from_lua(value: Value, _: &Lua) -> mlua::Result<Self> {
        match value {
            Value::UserData(ud) => Ok(*ud.borrow::<Vector>()?),
            Value::Vector(v) => Ok(Vector::from_native(v)),
            other => Err(mlua::Error::FromLuaConversionError {
                from: other.type_name(),
                to: "Vector".to_string(),
                message: Some(
                    "expected a Ruzit Vector userdata or a Luau native vector".into(),
                ),
            }),
        }
    }
}

pub fn value_to_vector_opt(v: &Value) -> Option<Vector> {
    match v {
        Value::UserData(ud) => ud.borrow::<Vector>().ok().map(|r| *r),
        Value::Vector(nv) => Some(Vector::from_native(*nv)),
        _ => None,
    }
}

fn value_to_f32(v: Value) -> mlua::Result<f32> {
    match v {
        Value::Number(n) => Ok(n as f32),
        Value::Integer(n) => Ok(n as f32),
        _ => Err(mlua::Error::RuntimeError(
            "expected a number for x/y/z component".into(),
        )),
    }
}

pub fn read_xyz_from_args(
    args: mlua::MultiValue,
) -> mlua::Result<((f32, f32, f32), Option<Value>)> {
    let mut iter = args.into_iter();
    let first = iter.next().unwrap_or(Value::Nil);
    if let Some(v) = value_to_vector_opt(&first) {
        return Ok(((v.x, v.y, v.z), iter.next()));
    }
    let x = value_to_f32(first)?;
    let y = value_to_f32(iter.next().unwrap_or(Value::Nil))?;
    let z = value_to_f32(iter.next().unwrap_or(Value::Nil))?;
    Ok(((x, y, z), iter.next()))
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
            |_, this, (o, t): (Vector, f32)| -> mlua::Result<Vector> {
                Ok(Vector::new(
                    lerp_f(this.x, o.x, t),
                    lerp_f(this.y, o.y, t),
                    lerp_f(this.z, o.z, t),
                ))
            },
        );
        m.add_method("Dot", |_, this, o: Vector| -> mlua::Result<f32> {
            Ok(this.dot(&o))
        });
        m.add_method("Cross", |_, this, o: Vector| -> mlua::Result<Vector> {
            Ok(this.cross(&o))
        });
        m.add_method("Normalized", |_, this, _: ()| -> mlua::Result<Vector> {
            Ok(this.normalized())
        });
        m.add_method("Distance", |_, this, o: Vector| -> mlua::Result<f32> {
            Ok(this.distance(&o))
        });
        m.add_method("ToNative", |_, this, _: ()| -> mlua::Result<mlua::Vector> {
            Ok(this.to_native())
        });

        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!("Vector({}, {}, {})", this.x, this.y, this.z))
        });

        m.add_meta_function(
            "__add",
            |_, (a, b): (Vector, Vector)| -> mlua::Result<Vector> {
                Ok(Vector::new(a.x + b.x, a.y + b.y, a.z + b.z))
            },
        );
        m.add_meta_function(
            "__sub",
            |_, (a, b): (Vector, Vector)| -> mlua::Result<Vector> {
                Ok(Vector::new(a.x - b.x, a.y - b.y, a.z - b.z))
            },
        );
        m.add_meta_function(
            "__mul",
            |_, (a, b): (Value, Value)| -> mlua::Result<Vector> {
                let (v, s) = pair_vec_scalar(&a, &b, "Vector * expects (Vector, number)")?;
                Ok(Vector::new(v.x * s, v.y * s, v.z * s))
            },
        );
        m.add_meta_function(
            "__div",
            |_, (a, b): (Value, Value)| -> mlua::Result<Vector> {
                let (v, s) = pair_vec_scalar(&a, &b, "Vector / expects (Vector, number)")?;
                Ok(Vector::new(v.x / s, v.y / s, v.z / s))
            },
        );
        m.add_meta_function("__unm", |_, v: Vector| -> mlua::Result<Vector> {
            Ok(Vector::new(-v.x, -v.y, -v.z))
        });
        m.add_meta_function(
            "__eq",
            |_, (a, b): (Vector, Vector)| -> mlua::Result<bool> {
                Ok(a.x == b.x && a.y == b.y && a.z == b.z)
            },
        );
        m.add_meta_function(
            "__lt",
            |_, (a, b): (Vector, Vector)| -> mlua::Result<bool> {
                Ok(a.x < b.x && a.y < b.y && a.z < b.z)
            },
        );
        m.add_meta_function(
            "__le",
            |_, (a, b): (Vector, Vector)| -> mlua::Result<bool> {
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

        m.add_method(
            "FuzzyEq",
            |_,
             this,
             (other, epsilon): (AnyUserData, Option<f32>)|
             -> mlua::Result<bool> {
                let o = *other.borrow::<CFrame>()?;
                let eps = epsilon.unwrap_or(1e-5).max(0.0);
                let close = |a: f32, b: f32| (a - b).abs() <= eps;
                Ok(close(this.position.x, o.position.x)
                    && close(this.position.y, o.position.y)
                    && close(this.position.z, o.position.z)
                    && close(this.rotation.x, o.rotation.x)
                    && close(this.rotation.y, o.rotation.y)
                    && close(this.rotation.z, o.rotation.z))
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
                    mlua::Error::RuntimeError("CFrame * expects CFrame on the left".into())
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
                if let Some(v) = value_to_vector_opt(&b) {
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

fn pair_vec_scalar(a: &Value, b: &Value, err: &str) -> mlua::Result<(Vector, f32)> {
    if let (Some(v), Some(s)) = (value_to_vector_opt(a), as_scalar(b)) {
        return Ok((v, s));
    }
    if let (Some(s), Some(v)) = (as_scalar(a), value_to_vector_opt(b)) {
        return Ok((v, s));
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
        lua.create_function(|_, (r, g, b): (f32, f32, f32)| Ok(Color3::new(r, g, b)))?,
    )?;
    let from_rgb = lua.create_function(|_, (r, g, b): (i64, i64, i64)| {
        Ok(Color3::new(
            r.clamp(0, 255) as f32 / 255.0,
            g.clamp(0, 255) as f32 / 255.0,
            b.clamp(0, 255) as f32 / 255.0,
        ))
    })?;
    color_class.set("FromRGB", from_rgb.clone())?;
    color_class.set("fromRGB", from_rgb)?;
    let from_hex = lua.create_function(|_, hex: String| -> mlua::Result<Color3> {
        let s = hex.trim_start_matches('#');
        let bytes = match s.len() {
            6 => {
                let r = u8::from_str_radix(&s[0..2], 16)
                    .map_err(|e| mlua::Error::RuntimeError(format!("Color3.FromHex: {e}")))?;
                let g = u8::from_str_radix(&s[2..4], 16)
                    .map_err(|e| mlua::Error::RuntimeError(format!("Color3.FromHex: {e}")))?;
                let b = u8::from_str_radix(&s[4..6], 16)
                    .map_err(|e| mlua::Error::RuntimeError(format!("Color3.FromHex: {e}")))?;
                (r, g, b)
            }
            3 => {
                let r = u8::from_str_radix(&s[0..1], 16)
                    .map_err(|e| mlua::Error::RuntimeError(format!("Color3.FromHex: {e}")))?;
                let g = u8::from_str_radix(&s[1..2], 16)
                    .map_err(|e| mlua::Error::RuntimeError(format!("Color3.FromHex: {e}")))?;
                let b = u8::from_str_radix(&s[2..3], 16)
                    .map_err(|e| mlua::Error::RuntimeError(format!("Color3.FromHex: {e}")))?;
                (r * 17, g * 17, b * 17)
            }
            _ => {
                return Err(mlua::Error::RuntimeError(format!(
                    "Color3.FromHex: expected 3 or 6 hex chars, got '{hex}'"
                )));
            }
        };
        Ok(Color3::new(
            bytes.0 as f32 / 255.0,
            bytes.1 as f32 / 255.0,
            bytes.2 as f32 / 255.0,
        ))
    })?;
    color_class.set("FromHex", from_hex.clone())?;
    color_class.set("fromHex", from_hex)?;
    t.set("Color3", color_class)?;

    let vec_class = lua.create_table()?;
    vec_class.set(
        "new",
        lua.create_function(|_, args: mlua::MultiValue| -> mlua::Result<Vector> {
            let mut iter = args.into_iter();
            let x = iter.next().and_then(|v| as_scalar(&v)).unwrap_or(0.0);
            let y = iter.next().and_then(|v| as_scalar(&v)).unwrap_or(0.0);
            let z = iter.next().and_then(|v| as_scalar(&v)).unwrap_or(0.0);
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
    vec_class.set(
        "FromNative",
        lua.create_function(|_, native: mlua::Vector| -> mlua::Result<Vector> {
            Ok(Vector::new(native.x(), native.y(), native.z()))
        })?,
    )?;
    t.set("Vector", vec_class)?;

    let cframe_class = lua.create_table()?;
    cframe_class.set(
        "new",
        lua.create_function(
            |_, (pos, rot): (Option<Vector>, Option<Vector>)| -> mlua::Result<CFrame> {
                Ok(CFrame::new(
                    pos.unwrap_or(Vector::new(0.0, 0.0, 0.0)),
                    rot.unwrap_or(Vector::new(0.0, 0.0, 0.0)),
                ))
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
    cframe_class.set(
        "LookAt",
        lua.create_function(
            |_, (eye, target, up): (Vector, Vector, Option<Vector>)| -> mlua::Result<CFrame> {
                let up = up.unwrap_or(Vector::new(0.0, 1.0, 0.0));
                let forward = Vector::new(
                    target.x - eye.x,
                    target.y - eye.y,
                    target.z - eye.z,
                )
                .normalized();
                let right = forward.cross(&up).normalized();
                let new_up = right.cross(&forward).normalized();
                let mat: Mat3 = [
                    [right.x, new_up.x, forward.x],
                    [right.y, new_up.y, forward.y],
                    [right.z, new_up.z, forward.z],
                ];
                Ok(CFrame::new(eye, matrix_to_euler(mat)))
            },
        )?,
    )?;
    t.set("CFrame", cframe_class)?;

    let udim_class = lua.create_table()?;
    udim_class.set(
        "new",
        lua.create_function(|_, (x, y): (Option<f32>, Option<f32>)| -> mlua::Result<UDim> {
            Ok(UDim::new(x.unwrap_or(0.0), y.unwrap_or(0.0)))
        })?,
    )?;
    udim_class.set(
        "zero",
        lua.create_function(|_, _: ()| Ok(UDim::new(0.0, 0.0)))?,
    )?;
    udim_class.set(
        "one",
        lua.create_function(|_, _: ()| Ok(UDim::new(1.0, 1.0)))?,
    )?;
    udim_class.set(
        "FromDim",
        lua.create_function(
            |_, (sx, sy, parent_ud): (f32, f32, AnyUserData)| -> mlua::Result<Dim> {
                let parent = *parent_ud.borrow::<Dim>()?;
                Ok(Dim::new(parent.x * sx, parent.y * sy))
            },
        )?,
    )?;
    t.set("UDim", udim_class)?;

    Ok(t)
}

#[derive(Clone, Copy, Debug)]
pub struct UDim {
    pub x: f32,
    pub y: f32,
}

impl UDim {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl UserData for UDim {
    fn add_fields<F: UserDataFields<Self>>(f: &mut F) {
        f.add_field_method_get("X", |_, this| Ok(this.x));
        f.add_field_method_get("Y", |_, this| Ok(this.y));
    }

    fn add_methods<M: UserDataMethods<Self>>(m: &mut M) {
        m.add_method(
            "ToDim",
            |_, this, parent: AnyUserData| -> mlua::Result<Dim> {
                let p = *parent.borrow::<Dim>()?;
                Ok(Dim::new(this.x * p.x, this.y * p.y))
            },
        );
        m.add_method(
            "Lerp",
            |_, this, (other, t): (AnyUserData, f32)| -> mlua::Result<UDim> {
                let o = *other.borrow::<UDim>()?;
                Ok(UDim::new(lerp_f(this.x, o.x, t), lerp_f(this.y, o.y, t)))
            },
        );

        m.add_meta_method("__tostring", |_, this, _: ()| {
            Ok(format!("UDim({}, {})", this.x, this.y))
        });

        m.add_meta_function(
            "__add",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<UDim> {
                let a = *a.borrow::<UDim>()?;
                let b = *b.borrow::<UDim>()?;
                Ok(UDim::new(a.x + b.x, a.y + b.y))
            },
        );
        m.add_meta_function(
            "__sub",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<UDim> {
                let a = *a.borrow::<UDim>()?;
                let b = *b.borrow::<UDim>()?;
                Ok(UDim::new(a.x - b.x, a.y - b.y))
            },
        );
        m.add_meta_function("__mul", |_, (a, b): (Value, Value)| -> mlua::Result<UDim> {
            let (u, s) = pair_with_scalar::<UDim>(&a, &b, "UDim * expects (UDim, number)")?;
            Ok(UDim::new(u.x * s, u.y * s))
        });
        m.add_meta_function("__div", |_, (a, b): (Value, Value)| -> mlua::Result<UDim> {
            let (u, s) = pair_with_scalar::<UDim>(&a, &b, "UDim / expects (UDim, number)")?;
            Ok(UDim::new(u.x / s, u.y / s))
        });
        m.add_meta_function("__unm", |_, ud: AnyUserData| -> mlua::Result<UDim> {
            let u = *ud.borrow::<UDim>()?;
            Ok(UDim::new(-u.x, -u.y))
        });
        m.add_meta_function(
            "__eq",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<UDim>()?;
                let b = b.borrow::<UDim>()?;
                Ok(a.x == b.x && a.y == b.y)
            },
        );
        m.add_meta_function(
            "__lt",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<UDim>()?;
                let b = b.borrow::<UDim>()?;
                Ok(a.x < b.x && a.y < b.y)
            },
        );
        m.add_meta_function(
            "__le",
            |_, (a, b): (AnyUserData, AnyUserData)| -> mlua::Result<bool> {
                let a = a.borrow::<UDim>()?;
                let b = b.borrow::<UDim>()?;
                Ok(a.x <= b.x && a.y <= b.y)
            },
        );
    }
}
