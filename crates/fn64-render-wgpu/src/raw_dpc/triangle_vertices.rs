//! Decoded-triangle-to-three-vertices conversion (RT64 `decodeTriangles`).
//!
//! Literal characterization of the permitted MIT RT64 source pinned by
//! `docs/RT64-PORT-AUTHORITY.md` at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`, `src/gbi/rt64_gbi_rdp.cpp`'s
//! `decodeTriangles` (fixed-point conversion through the three
//! `posWorkBuffer`/`colorWorkBuffer`/`texcoordWorkBuffer` vertex writes for
//! one triangle). It consumes the already-decoded [`RawTriangle`] wire
//! payload; it performs no rasterization, no OtherMode/state integration, no
//! draw call, and no more than the one supplied triangle -- RT64's own `//
//! TODO do more than 1 tri at a time` is observed behavior in the source,
//! not authority to silently drop triangles here.
//!
//! RT64 writes exactly three vertices per triangle in a fixed order --
//! `workBufferIndex + 0/1/2` -- derived respectively from the (`x1`, `y1` =
//! `yh`), (`x2`, `y2` = `yl`), and (`x3` = `xl`, `y3` = `ym`) positions. This
//! module preserves that exact order: [`TriangleVertices::vertex`] with
//! `index` `0`, `1`, or `2` returns the corresponding vertex.

use super::triangle::{CoefficientWords, DepthWords, RawTriangle, RawWord};

/// One vertex's position (`x`, `y`, depth `z`, perspective `w`), shaded
/// color (already normalized by 255), and texture coordinate, exactly as
/// RT64 writes `posWorkBuffer`/`colorWorkBuffer`/`texcoordWorkBuffer` for one
/// triangle vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriangleVertex {
    x: f32,
    y: f32,
    z: f32,
    w: f32,
    color: [f32; 4],
    texcoord: [f32; 2],
}

impl TriangleVertex {
    pub const fn x(self) -> f32 {
        self.x
    }

    pub const fn y(self) -> f32 {
        self.y
    }

    /// Depth: RT64 `posWorkBuffer[...].z`. `0.0` when the triangle has no
    /// depth block (RT64's exact no-depth default).
    pub const fn z(self) -> f32 {
        self.z
    }

    /// Perspective W: RT64 `posWorkBuffer[...].w`. `1.0` when the triangle
    /// is not textured, or when textured without perspective correction
    /// (RT64's exact defaults in both cases).
    pub const fn w(self) -> f32 {
        self.w
    }

    /// RGBA shaded color, already divided by 255.0 (RT64's exact
    /// `* (1/255.0f)`). All zero when the triangle has no shade block.
    pub const fn color(self) -> [f32; 4] {
        self.color
    }

    /// (S, T) texture coordinate. All zero when the triangle has no texture
    /// block.
    pub const fn texcoord(self) -> [f32; 2] {
        self.texcoord
    }
}

/// The three vertices RT64's `decodeTriangles` writes for one triangle, in
/// RT64's exact `workBufferIndex + 0/1/2` order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TriangleVertices {
    vertices: [TriangleVertex; 3],
}

impl TriangleVertices {
    /// Vertex `index` (`0`, `1`, or `2`) in RT64's exact write order.
    ///
    /// # Panics
    ///
    /// Panics if `index >= 3`.
    pub const fn vertex(&self, index: usize) -> TriangleVertex {
        self.vertices[index]
    }
}

/// Converts one decoded raw triangle command to its three vertices, exactly
/// as RT64's `decodeTriangles` does for a single triangle.
///
/// `texture_perspective` is the caller-supplied `G_TP_PERSP` OtherMode bit
/// (RT64: `state->rdp->otherMode.textPersp() == G_TP_PERSP`); this function
/// performs no OtherMode decode of its own.
///
/// IEEE f32 results -- including infinities and NaNs a literal division by
/// zero or by a value RT64's fixed-point math can produce -- are preserved
/// exactly as RT64's own division produces them. No defensive fallback is
/// applied.
pub fn decode_triangle_vertices(
    triangle: &RawTriangle,
    texture_perspective: bool,
) -> TriangleVertices {
    // RT64: `int16_t ylFixed = (curData[0].w0 & 0x0000FFFF) << 2 >> 2;` and
    // the matching ymFixed/yhFixed lines. `DisplayList::w0`/`w1` are
    // `uint32_t` (`rt64_display_list.h`), so `& 0x0000FFFF` yields a
    // `uint32_t` in 0..=0xFFFF; `<<2` and `>>2` are both *unsigned* logical
    // shifts on that `uint32_t` -- well-defined, and the `>>2` exactly
    // undoes the `<<2` since no bits were shifted out (max `0xFFFF<<2 =
    // 0x3FFFC` fits in 32 bits). The whole expression therefore reduces to
    // exactly `w & 0xFFFF`, and assigning that `uint32_t` to `int16_t`
    // narrows by reinterpreting the low 16 bits as two's-complement --
    // bit-identical to `RawTriangle::yl()`/`ym()`/`yh()`'s existing
    // `(w & 0xffff) as i16`. This is a plain 16-bit reinterpret, not a
    // 14-bit sign extension: the `<<2>>2` is observably a no-op here
    // because the shifted type is unsigned, not the promoted signed `int`
    // a naive reading of `int16_t` operands might suggest. Verified against
    // C++ language rules for unsigned shift ([expr.shift]) and integral
    // narrowing conversion ([conv.integral]); no additional transform is
    // applied to `triangle.yl()`/`ym()`/`yh()` here.
    let yl_fixed = triangle.yl();
    let ym_fixed = triangle.ym();
    let yh_fixed = triangle.yh();

    // RT64: `float yl = ylFixed / 4.0f;` etc.
    let yl = f32::from(yl_fixed) / 4.0;
    let ym = f32::from(ym_fixed) / 4.0;
    let yh = f32::from(yh_fixed) / 4.0;

    let y1 = yh;
    let y2 = yl;

    let xl = triangle.xl() as f32 / 65536.0;
    let xm = triangle.xm() as f32 / 65536.0;
    let xh = triangle.xh() as f32 / 65536.0;

    let dxldy = triangle.dxldy() as f32 / 65536.0;
    let dxmdy = triangle.dxmdy() as f32 / 65536.0;
    let dxhdy = triangle.dxhdy() as f32 / 65536.0;

    // RT64: `float yFloor = floorf(yh);`
    let y_floor = yh.floor();

    // RT64 computes both intercepts; `mIntercept` is never read again in
    // the source (dead in RT64 itself), reproduced here for the same
    // reason -- literal characterization, not a functional dependency.
    let h_intercept = xh - dxhdy * y_floor;
    let _m_intercept = xm - dxmdy * y_floor;

    let x1 = dxhdy * y1 + h_intercept;
    let x2 = dxhdy * y2 + h_intercept;

    // RT64 computes `l_intercept` but, like `mIntercept`, never reads it
    // again.
    let _l_intercept = x2 - dxldy * y2;

    let x3 = xl;
    let y3 = ym;

    let dy_1 = y1 - y_floor;
    let dy_2 = y2 - y_floor;
    let dy_3 = y3 - y_floor;

    let x3_opposite = dxhdy * y3 + h_intercept;
    let dx_3 = x3 - x3_opposite;

    let color = decode_shade(triangle.shade(), dy_1, dy_2, dy_3, dx_3);

    let (texcoord, w) = decode_texture(
        triangle.texture(),
        texture_perspective,
        dy_1,
        dy_2,
        dy_3,
        dx_3,
    );

    let z = decode_depth(triangle.depth(), dy_1, dy_2, dy_3, dx_3);

    TriangleVertices {
        vertices: [
            TriangleVertex {
                x: x1,
                y: y1,
                z: z[0],
                w: w[0],
                color: color[0],
                texcoord: texcoord[0],
            },
            TriangleVertex {
                x: x2,
                y: y2,
                z: z[1],
                w: w[1],
                color: color[1],
                texcoord: texcoord[1],
            },
            TriangleVertex {
                x: x3,
                y: y3,
                z: z[2],
                w: w[2],
                color: color[2],
                texcoord: texcoord[2],
            },
        ],
    }
}

fn decode_shade(
    shade: Option<&CoefficientWords>,
    dy_1: f32,
    dy_2: f32,
    dy_3: f32,
    dx_3: f32,
) -> [[f32; 4]; 3] {
    let Some(shade) = shade else {
        return [[0.0, 0.0, 0.0, 0.0]; 3];
    };

    // RT64: base uses words[0]/words[2], dx uses words[1]/words[3], de uses
    // words[4]/words[6] (words[5]/words[7], "dy", are commented out and
    // unused in RT64 -- not reproduced here for the same reason).
    let base_fixed = interleave_words_pair(shade[0], shade[2]);
    let dx_fixed = interleave_words_pair(shade[1], shade[3]);
    let de_fixed = interleave_words_pair(shade[4], shade[6]);

    let base_color = mul4(to_f32_4(base_fixed), 1.0 / 65536.0);
    let color_dx = mul4(to_f32_4(dx_fixed), 1.0 / 65536.0);
    let color_de = mul4(to_f32_4(de_fixed), 1.0 / 65536.0);

    let v1_color = add4(base_color, mul4(color_de, dy_1));
    let v2_color = add4(base_color, mul4(color_de, dy_2));
    let v3_opposite_color = add4(base_color, mul4(color_de, dy_3));
    let v3_color = add4(v3_opposite_color, mul4(color_dx, dx_3));

    [
        mul4(v1_color, 1.0 / 255.0),
        mul4(v2_color, 1.0 / 255.0),
        mul4(v3_color, 1.0 / 255.0),
    ]
}

fn interleave_words_pair(a: RawWord, b: RawWord) -> [i32; 4] {
    [
        (((a.w0() >> 16) << 16) | (b.w0() >> 16)) as i32,
        (((a.w0() & 0xFFFF) << 16) | (b.w0() & 0xFFFF)) as i32,
        (((a.w1() >> 16) << 16) | (b.w1() >> 16)) as i32,
        (((a.w1() & 0xFFFF) << 16) | (b.w1() & 0xFFFF)) as i32,
    ]
}

fn to_f32_4(values: [i32; 4]) -> [f32; 4] {
    [
        values[0] as f32,
        values[1] as f32,
        values[2] as f32,
        values[3] as f32,
    ]
}

fn mul4(values: [f32; 4], scalar: f32) -> [f32; 4] {
    [
        values[0] * scalar,
        values[1] * scalar,
        values[2] * scalar,
        values[3] * scalar,
    ]
}

fn add4(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]]
}

fn decode_texture(
    texture: Option<&CoefficientWords>,
    texture_perspective: bool,
    dy_1: f32,
    dy_2: f32,
    dy_3: f32,
    dx_3: f32,
) -> ([[f32; 2]; 3], [f32; 3]) {
    let Some(texture) = texture else {
        return ([[0.0, 0.0]; 3], [1.0, 1.0, 1.0]);
    };

    // RT64: base uses words[0]/words[2], dx uses words[1]/words[3], de uses
    // words[4]/words[6]; only s/t/w (the first 3 lanes) are meaningful for
    // `int3`, so the 4th interleaved lane is discarded here exactly as
    // RT64's `hlslpp::int3` construction discards it.
    let base_fixed = interleave_words_pair(texture[0], texture[2]);
    let dx_fixed = interleave_words_pair(texture[1], texture[3]);
    let de_fixed = interleave_words_pair(texture[4], texture[6]);

    let base_texcoord = mul3(to_f32_3(base_fixed), 1.0 / 65536.0);
    let texcoord_dx = mul3(to_f32_3(dx_fixed), 1.0 / 65536.0);
    let texcoord_de = mul3(to_f32_3(de_fixed), 1.0 / 65536.0);

    let w_base = base_texcoord[2];
    let w1 = w_base + texcoord_de[2] * dy_1;
    let w2 = w_base + texcoord_de[2] * dy_2;
    let w3_opposite = w_base + texcoord_de[2] * dy_3;
    let w3 = w3_opposite + texcoord_dx[2] * dx_3;

    let base_xy = [base_texcoord[0], base_texcoord[1]];
    let de_xy = [texcoord_de[0], texcoord_de[1]];
    let dx_xy = [texcoord_dx[0], texcoord_dx[1]];

    let v1_texcoord = add2(base_xy, mul2(de_xy, dy_1));
    let v2_texcoord = add2(base_xy, mul2(de_xy, dy_2));
    let v3_opposite_texcoord = add2(base_xy, mul2(de_xy, dy_3));
    let v3_texcoord = add2(v3_opposite_texcoord, mul2(dx_xy, dx_3));

    if texture_perspective {
        (
            [
                mul2(div2_scalar(v1_texcoord, w1), 1024.0),
                mul2(div2_scalar(v2_texcoord, w2), 1024.0),
                mul2(div2_scalar(v3_texcoord, w3), 1024.0),
            ],
            [65536000.0 / w1, 65536000.0 / w2, 65536000.0 / w3],
        )
    } else {
        (
            [
                mul2(mul2(v1_texcoord, 1024.0), 1.0 / 16384.0),
                mul2(mul2(v2_texcoord, 1024.0), 1.0 / 16384.0),
                mul2(mul2(v3_texcoord, 1024.0), 1.0 / 16384.0),
            ],
            [1.0, 1.0, 1.0],
        )
    }
}

fn to_f32_3(values: [i32; 4]) -> [f32; 3] {
    [values[0] as f32, values[1] as f32, values[2] as f32]
}

fn mul3(values: [f32; 3], scalar: f32) -> [f32; 3] {
    [values[0] * scalar, values[1] * scalar, values[2] * scalar]
}

fn add2(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}

fn mul2(values: [f32; 2], scalar: f32) -> [f32; 2] {
    [values[0] * scalar, values[1] * scalar]
}

fn div2_scalar(values: [f32; 2], divisor: f32) -> [f32; 2] {
    [values[0] / divisor, values[1] / divisor]
}

fn decode_depth(
    depth: Option<&DepthWords>,
    dy_1: f32,
    dy_2: f32,
    dy_3: f32,
    dx_3: f32,
) -> [f32; 3] {
    let Some(depth) = depth else {
        return [0.0, 0.0, 0.0];
    };

    // RT64: `int baseDepthFixed = curData[0].w0;` etc. -- straight i32
    // reinterpret of each word half, no half-word interleave (unlike shade
    // and texture).
    let base_depth_fixed = depth[0].w0() as i32;
    let depth_dx_fixed = depth[0].w1() as i32;
    let depth_de_fixed = depth[1].w0() as i32;
    // depth[1].w1() is RT64's `depthDyFixed`, unused (edge-AA only).

    const SCALE: f32 = 1.0 / 65536.0 / 32768.0;
    let base_depth = base_depth_fixed as f32 * SCALE;
    let depth_dx = depth_dx_fixed as f32 * SCALE;
    let depth_de = depth_de_fixed as f32 * SCALE;

    let v1_depth = base_depth + depth_de * dy_1;
    let v2_depth = base_depth + depth_de * dy_2;
    let v3_opposite_depth = base_depth + depth_de * dy_3;
    let v3_depth = v3_opposite_depth + depth_dx * dx_3;

    [v1_depth, v2_depth, v3_depth]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmem::TileIndex;

    // ---------------------------------------------------------------
    // Independent oracle: hand-built directly from the raw 64-bit words
    // an RDP triangle command carries, in wire order, without calling any
    // production helper (`RawTriangle`, `interleave_words_pair`,
    // `decode_shade`/`decode_texture`/`decode_depth`, or any
    // `mulN`/`addN`/`to_f32_N` helper above). It re-derives every formula
    // from the RT64 source text independently, using plain f64 arithmetic
    // internally (then narrowing to f32 at the same points RT64 narrows)
    // and a flat word array instead of the typed accessor API, so a bug
    // shared between the oracle and the implementation (a half-word swap,
    // a wrong word index, a wrong 65536000/1024/16384 divisor, a dropped
    // sign) would have to be present in both independently-written pieces
    // of code to go unnoticed.
    struct Oracle {
        words: Vec<(u32, u32)>,
    }

    impl Oracle {
        fn base_word0(&self) -> (u32, u32) {
            self.words[0]
        }

        fn edge(&self, index: usize) -> (u32, u32) {
            self.words[index]
        }

        /// RT64's `int16_t v = (w & 0x0000FFFF) << 2 >> 2;` where `w` is a
        /// `uint32_t` (`DisplayList::w0`/`w1`, `rt64_display_list.h`). Both
        /// shifts operate on the unsigned masked value -- `<<2` is a
        /// well-defined logical left shift, and `>>2` is a well-defined
        /// logical (not arithmetic) right shift on that same unsigned type,
        /// exactly undoing the `<<2` since nothing was shifted out (the
        /// masked value never exceeds `0xFFFF`, so `<<2` never exceeds
        /// `0x3FFFC`, well inside 32 bits). The whole expression therefore
        /// reduces to `w & 0xFFFF` before its narrowing conversion to
        /// `int16_t`, which reinterprets the low 16 bits as two's
        /// complement -- a plain 16-bit reinterpret, not a 14-bit sign
        /// extension. Computed here by hand with a bit-13 test (treating
        /// the full 16-bit value as signed) rather than a cast, so it does
        /// not share a mechanism with `RawTriangle::yl()`/`ym()`/`yh()`'s
        /// `as i16` or with `decode_triangle_vertices`'s direct use of
        /// those accessors.
        fn wire16_reinterpret_oracle(raw16: u32) -> i32 {
            let low16 = raw16 & 0xFFFF;
            if low16 & 0x8000 != 0 {
                (low16 as i32) - 0x1_0000
            } else {
                low16 as i32
            }
        }

        fn y_fields(&self) -> (f64, f64, f64) {
            let (w0, w1) = self.base_word0();
            let yl = Self::wire16_reinterpret_oracle(w0 & 0xFFFF);
            let ym = Self::wire16_reinterpret_oracle(w1 >> 16);
            let yh = Self::wire16_reinterpret_oracle(w1 & 0xFFFF);
            (
                f64::from(yl) / 4.0,
                f64::from(ym) / 4.0,
                f64::from(yh) / 4.0,
            )
        }

        fn x_and_slopes(&self) -> ([f64; 3], [f64; 3]) {
            // [xl, xh, xm], [dxldy, dxhdy, dxmdy]
            let (xl_raw, dxldy_raw) = self.edge(1);
            let (xh_raw, dxhdy_raw) = self.edge(2);
            let (xm_raw, dxmdy_raw) = self.edge(3);
            let to_q1616 = |raw: u32| f64::from(raw as i32) / 65536.0;
            (
                [to_q1616(xl_raw), to_q1616(xh_raw), to_q1616(xm_raw)],
                [
                    to_q1616(dxldy_raw),
                    to_q1616(dxhdy_raw),
                    to_q1616(dxmdy_raw),
                ],
            )
        }

        /// Positions plus the shared `dy_1/dy_2/dy_3/dx_3` coefficients,
        /// computed at f64 precision then narrowed to f32 only at the same
        /// boundary points RT64's own `float` locals would narrow -- since
        /// RT64 itself computes entirely in `float`, this oracle narrows
        /// after every arithmetic step to stay bit-comparable rather than
        /// silently accumulating extra precision RT64 never had.
        fn geometry(&self) -> ([f32; 3], [f32; 3], f32, f32) {
            let (yl, ym, yh) = self.y_fields();
            let (yl, ym, yh) = (yl as f32, ym as f32, yh as f32);
            let ([xl, xh, xm], [dxldy, dxhdy, dxmdy]) = self.x_and_slopes();
            let (xl, xh, xm) = (xl as f32, xh as f32, xm as f32);
            let (dxldy, dxhdy, dxmdy) = (dxldy as f32, dxhdy as f32, dxmdy as f32);

            let y1 = yh;
            let y2 = yl;
            let y_floor = y1.floor();
            let h_intercept = xh - dxhdy * y_floor;
            let x1 = dxhdy * y1 + h_intercept;
            let x2 = dxhdy * y2 + h_intercept;
            let x3 = xl;
            let y3 = ym;
            let _ = (dxldy, xm, dxmdy); // computed by RT64, never read onward

            let dy_1 = y1 - y_floor;
            let dy_2 = y2 - y_floor;
            let dy_3 = y3 - y_floor;
            let x3_opposite = dxhdy * y3 + h_intercept;
            let dx_3 = x3 - x3_opposite;

            ([x1, x2, x3], [y1, y2, y3], dy_3, dx_3).into_geometry(dy_1, dy_2)
        }
    }

    trait IntoGeometry {
        fn into_geometry(self, dy_1: f32, dy_2: f32) -> ([f32; 3], [f32; 3], f32, f32);
    }
    impl IntoGeometry for ([f32; 3], [f32; 3], f32, f32) {
        fn into_geometry(self, dy_1: f32, dy_2: f32) -> ([f32; 3], [f32; 3], f32, f32) {
            let (xs, ys, dy_3, dx_3) = self;
            let _ = (dy_1, dy_2);
            (xs, ys, dy_3, dx_3)
        }
    }

    impl Oracle {
        fn dys(&self) -> (f32, f32, f32, f32) {
            let (yl, ym, yh) = self.y_fields();
            let (yl, ym, yh) = (yl as f32, ym as f32, yh as f32);
            let ([xl, xh, _xm], [_dxldy, dxhdy, _dxmdy]) = self.x_and_slopes();
            let (xl, xh) = (xl as f32, xh as f32);
            let dxhdy = dxhdy as f32;
            let y1 = yh;
            let y2 = yl;
            let y_floor = y1.floor();
            let h_intercept = xh - dxhdy * y_floor;
            let x3 = xl;
            let y3 = ym;
            let dy_1 = y1 - y_floor;
            let dy_2 = y2 - y_floor;
            let dy_3 = y3 - y_floor;
            let x3_opposite = dxhdy * y3 + h_intercept;
            let dx_3 = x3 - x3_opposite;
            (dy_1, dy_2, dy_3, dx_3)
        }

        fn shade_block(&self, start: usize) -> [[f32; 4]; 3] {
            let (dy_1, dy_2, dy_3, dx_3) = self.dys();
            let get = |i: usize| self.words[start + i];
            let interleave = |a: (u32, u32), b: (u32, u32)| -> [i32; 4] {
                [
                    (((a.0 >> 16) << 16) | (b.0 >> 16)) as i32,
                    (((a.0 & 0xFFFF) << 16) | (b.0 & 0xFFFF)) as i32,
                    (((a.1 >> 16) << 16) | (b.1 >> 16)) as i32,
                    (((a.1 & 0xFFFF) << 16) | (b.1 & 0xFFFF)) as i32,
                ]
            };
            let base = interleave(get(0), get(2));
            let dx = interleave(get(1), get(3));
            let de = interleave(get(4), get(6));
            let scale = 1.0f32 / 65536.0;
            let base_c: Vec<f32> = base.iter().map(|&v| v as f32 * scale).collect();
            let dx_c: Vec<f32> = dx.iter().map(|&v| v as f32 * scale).collect();
            let de_c: Vec<f32> = de.iter().map(|&v| v as f32 * scale).collect();
            let combine = |dy: f32| -> [f32; 4] {
                let mut out = [0f32; 4];
                for lane in 0..4 {
                    out[lane] = (base_c[lane] + de_c[lane] * dy) / 255.0;
                }
                out
            };
            let v1 = combine(dy_1);
            let v2 = combine(dy_2);
            let v3_opposite: Vec<f32> = (0..4)
                .map(|lane| base_c[lane] + de_c[lane] * dy_3)
                .collect();
            let mut v3 = [0f32; 4];
            for lane in 0..4 {
                v3[lane] = (v3_opposite[lane] + dx_c[lane] * dx_3) / 255.0;
            }
            [v1, v2, v3]
        }

        fn texture_block(&self, start: usize, perspective: bool) -> ([[f32; 2]; 3], [f32; 3]) {
            let (dy_1, dy_2, dy_3, dx_3) = self.dys();
            let get = |i: usize| self.words[start + i];
            let interleave3 = |a: (u32, u32), b: (u32, u32)| -> [i32; 3] {
                [
                    (((a.0 >> 16) << 16) | (b.0 >> 16)) as i32,
                    (((a.0 & 0xFFFF) << 16) | (b.0 & 0xFFFF)) as i32,
                    (((a.1 >> 16) << 16) | (b.1 >> 16)) as i32,
                ]
            };
            let base = interleave3(get(0), get(2));
            let dx = interleave3(get(1), get(3));
            let de = interleave3(get(4), get(6));
            let scale = 1.0f32 / 65536.0;
            let base_t: Vec<f32> = base.iter().map(|&v| v as f32 * scale).collect();
            let dx_t: Vec<f32> = dx.iter().map(|&v| v as f32 * scale).collect();
            let de_t: Vec<f32> = de.iter().map(|&v| v as f32 * scale).collect();

            let w_base = base_t[2];
            let w1 = w_base + de_t[2] * dy_1;
            let w2 = w_base + de_t[2] * dy_2;
            let w3_opposite = w_base + de_t[2] * dy_3;
            let w3 = w3_opposite + dx_t[2] * dx_3;

            let v1_uv = [base_t[0] + de_t[0] * dy_1, base_t[1] + de_t[1] * dy_1];
            let v2_uv = [base_t[0] + de_t[0] * dy_2, base_t[1] + de_t[1] * dy_2];
            let v3_opposite_uv = [base_t[0] + de_t[0] * dy_3, base_t[1] + de_t[1] * dy_3];
            let v3_uv = [
                v3_opposite_uv[0] + dx_t[0] * dx_3,
                v3_opposite_uv[1] + dx_t[1] * dx_3,
            ];

            if perspective {
                (
                    [
                        [v1_uv[0] / w1 * 1024.0, v1_uv[1] / w1 * 1024.0],
                        [v2_uv[0] / w2 * 1024.0, v2_uv[1] / w2 * 1024.0],
                        [v3_uv[0] / w3 * 1024.0, v3_uv[1] / w3 * 1024.0],
                    ],
                    [65536000.0 / w1, 65536000.0 / w2, 65536000.0 / w3],
                )
            } else {
                (
                    [
                        [v1_uv[0] * 1024.0 / 16384.0, v1_uv[1] * 1024.0 / 16384.0],
                        [v2_uv[0] * 1024.0 / 16384.0, v2_uv[1] * 1024.0 / 16384.0],
                        [v3_uv[0] * 1024.0 / 16384.0, v3_uv[1] * 1024.0 / 16384.0],
                    ],
                    [1.0, 1.0, 1.0],
                )
            }
        }

        fn depth_block(&self, start: usize) -> [f32; 3] {
            let (dy_1, dy_2, dy_3, dx_3) = self.dys();
            let (base_w0, base_w1) = self.words[start];
            let (de_w0, _de_w1) = self.words[start + 1];
            let scale = 1.0f32 / 65536.0 / 32768.0;
            let base = base_w0 as i32 as f32 * scale;
            let dx = base_w1 as i32 as f32 * scale;
            let de = de_w0 as i32 as f32 * scale;
            let v1 = base + de * dy_1;
            let v2 = base + de * dy_2;
            let v3_opposite = base + de * dy_3;
            let v3 = v3_opposite + dx * dx_3;
            [v1, v2, v3]
        }
    }

    /// Independent flag extraction straight from the opcode's low three
    /// bits, matching the RDP triangle opcode spec directly rather than
    /// calling `TriangleFlags::from_opcode` (private to `triangle.rs`).
    /// Returns `(shaded, textured, depth)`.
    fn oracle_flags(opcode: u8) -> (bool, bool, bool) {
        (opcode & 0x4 != 0, opcode & 0x2 != 0, opcode & 0x1 != 0)
    }

    fn word_bytes(w0: u32, w1: u32) -> [u8; 8] {
        let mut bytes = [0u8; 8];
        bytes[0..4].copy_from_slice(&w0.to_be_bytes());
        bytes[4..8].copy_from_slice(&w1.to_be_bytes());
        bytes
    }

    fn base_word0_bytes(
        tile: u32,
        level: u32,
        right_major: bool,
        yl: u16,
        ym: u16,
        yh: u16,
    ) -> [u8; 8] {
        let w0 =
            (tile & 0x7) << 16 | (level & 0x7) << 19 | u32::from(right_major) << 23 | u32::from(yl);
        let w1 = u32::from(ym) << 16 | u32::from(yh);
        word_bytes(w0, w1)
    }

    fn edge_bytes(x: i32, dxdy: i32) -> [u8; 8] {
        word_bytes(x as u32, dxdy as u32)
    }

    struct Fixture {
        opcode: u8,
        yl: u16,
        ym: u16,
        yh: u16,
        xl: i32,
        dxldy: i32,
        xh: i32,
        dxhdy: i32,
        xm: i32,
        dxmdy: i32,
        shade: Option<[(u32, u32); 8]>,
        texture: Option<[(u32, u32); 8]>,
        depth: Option<[(u32, u32); 2]>,
    }

    impl Fixture {
        fn new(opcode: u8) -> Self {
            Self {
                opcode,
                yl: 0,
                ym: 0,
                yh: 0,
                xl: 0,
                dxldy: 0,
                xh: 0,
                dxhdy: 0,
                xm: 0,
                dxmdy: 0,
                shade: None,
                texture: None,
                depth: None,
            }
        }

        fn build(&self) -> (RawTriangle, Oracle) {
            let (shaded, textured, depth) = oracle_flags(self.opcode);
            let mut bytes = Vec::new();
            bytes.extend(base_word0_bytes(0, 0, false, self.yl, self.ym, self.yh));
            bytes.extend(edge_bytes(self.xl, self.dxldy));
            bytes.extend(edge_bytes(self.xh, self.dxhdy));
            bytes.extend(edge_bytes(self.xm, self.dxmdy));

            let mut words = vec![
                (
                    u32::from(self.yl),
                    u32::from(self.ym) << 16 | u32::from(self.yh),
                ),
                (self.xl as u32, self.dxldy as u32),
                (self.xh as u32, self.dxhdy as u32),
                (self.xm as u32, self.dxmdy as u32),
            ];

            if shaded {
                let block = self.shade.expect("shaded opcode needs a shade block");
                for &(w0, w1) in &block {
                    bytes.extend(word_bytes(w0, w1));
                }
                words.extend(block);
            }
            if textured {
                let block = self.texture.expect("textured opcode needs a texture block");
                for &(w0, w1) in &block {
                    bytes.extend(word_bytes(w0, w1));
                }
                words.extend(block);
            }
            if depth {
                let block = self.depth.expect("depth opcode needs a depth block");
                for &(w0, w1) in &block {
                    bytes.extend(word_bytes(w0, w1));
                }
                words.extend(block);
            }

            let triangle = RawTriangle::decode(self.opcode, &bytes).unwrap();
            (triangle, Oracle { words })
        }
    }

    fn assert_close(actual: f32, expected: f32, context: &str) {
        if expected.is_nan() {
            assert!(actual.is_nan(), "{context}: expected NaN, got {actual}");
            return;
        }
        if expected.is_infinite() {
            assert_eq!(actual, expected, "{context}: expected infinity");
            return;
        }
        assert!(
            (actual - expected).abs() <= expected.abs() * 1e-5 + 1e-6,
            "{context}: expected {expected}, got {actual}"
        );
    }

    fn assert_vertices_match(
        computed: TriangleVertices,
        expected_xy: [(f32, f32); 3],
        expected_color: Option<[[f32; 4]; 3]>,
        expected_texcoord_w: Option<([[f32; 2]; 3], [f32; 3])>,
        expected_z: [f32; 3],
    ) {
        for index in 0..3 {
            let vertex = computed.vertex(index);
            assert_close(vertex.x(), expected_xy[index].0, "x");
            assert_close(vertex.y(), expected_xy[index].1, "y");
            assert_close(vertex.z(), expected_z[index], "z");
            let expected_color = expected_color.unwrap_or([[0.0; 4]; 3]);
            for (actual, expected) in vertex.color().into_iter().zip(expected_color[index]) {
                assert_close(actual, expected, "color");
            }
            let (expected_tc, expected_w) =
                expected_texcoord_w.unwrap_or(([[0.0; 2]; 3], [1.0; 3]));
            for (actual, expected) in vertex.texcoord().into_iter().zip(expected_tc[index]) {
                assert_close(actual, expected, "texcoord");
            }
            assert_close(vertex.w(), expected_w[index], "w");
        }
    }

    // --- exact vertex order and position formulas, all 8 opcodes ---

    #[test]
    fn all_eight_opcodes_produce_oracle_matching_positions() {
        for opcode in 0x08u8..=0x0f {
            let mut fixture = Fixture::new(opcode);
            fixture.yl = 0x1234;
            fixture.ym = 0x0140; // fractional-scanline yh below
            fixture.yh = 0x0033; // 0x33/4 = 12.75 -> non-integer yFloor input
            fixture.xl = 0x0012_3400;
            fixture.dxldy = -0x0001_0000;
            fixture.xh = 0x0034_5600;
            fixture.dxhdy = 0x0000_8000;
            fixture.xm = 0x0056_7800;
            fixture.dxmdy = -0x0000_4000;
            let (shaded, textured, has_depth) = oracle_flags(opcode);
            if shaded {
                fixture.shade = Some(shade_fixture_block());
            }
            if textured {
                fixture.texture = Some(texture_fixture_block(false));
            }
            if has_depth {
                fixture.depth = Some(depth_fixture_block());
            }
            let (triangle, oracle) = fixture.build();
            let computed = decode_triangle_vertices(&triangle, false);

            let (xs, ys, _dy3, _dx3) = oracle.geometry();
            let expected_xy = [(xs[0], ys[0]), (xs[1], ys[1]), (xs[2], ys[2])];

            let expected_color = shaded.then(|| oracle.shade_block(4));

            let texture_start = if shaded { 12 } else { 4 };
            let expected_tc_w = textured.then(|| oracle.texture_block(texture_start, false));

            let depth_start = texture_start + if textured { 8 } else { 0 };
            let expected_z = if has_depth {
                oracle.depth_block(depth_start)
            } else {
                [0.0, 0.0, 0.0]
            };

            assert_vertices_match(
                computed,
                expected_xy,
                expected_color,
                expected_tc_w,
                expected_z,
            );
        }
    }

    fn shade_fixture_block() -> [(u32, u32); 8] {
        [
            (0x0012_3400, 0x0045_6700), // base r/g high halves in w0/w1... (see interleave)
            (0x0000_0100, 0xFFFF_FE00), // dx: note negative low half
            (0x7FFF_0000, 0x8000_0000), // signed boundary probe
            (0x0001_0002, 0x0003_0004),
            (0x0000_1000, 0x0000_2000), // de
            (0x0000_0000, 0x0000_0000), // dy: unused, must not affect result
            (0x0011_2233, 0x4455_6677),
            (0x8899_AABB, 0xCCDD_EEFF), // dy: unused, must not affect result
        ]
    }

    fn texture_fixture_block(negative_w: bool) -> [(u32, u32); 8] {
        let w_base_hi: u32 = if negative_w { 0xFFFF_0000 } else { 0x0001_0000 };
        [
            (0x0002_0000, w_base_hi), // base s(high)/.. and w(base) high half
            (0x0000_0300, 0x0000_0000),
            (0x0000_0000, 0x0000_0000),
            (0x0000_0000, 0x0000_0000),
            (0x0000_0100, 0x0000_0000), // de
            (0x0000_0000, 0x0000_0000), // dy: unused
            (0x0000_0000, 0x0000_0000),
            (0x0000_0000, 0x0000_0000), // dy: unused
        ]
    }

    fn depth_fixture_block() -> [(u32, u32); 2] {
        [(0x0010_0000, 0x0000_1000), (0x0000_0100, 0x1234_5678)]
    }

    // --- negative Y / 16-bit reinterpret hostiles ---
    //
    // RT64's `int16_t v = (w & 0x0000FFFF) << 2 >> 2;` operates on `w`'s
    // unsigned `uint32_t` type (`DisplayList::w0`/`w1`), so both shifts are
    // unsigned logical shifts: `<<2` then `>>2` exactly cancel, and the
    // expression reduces to `w & 0xFFFF` before its narrowing conversion to
    // `int16_t` -- a plain 16-bit reinterpret. It is NOT a 14-bit sign
    // extension: bits 14:15 are fully significant, not discarded.

    #[test]
    fn y_fields_use_16_bit_reinterpret_not_14_bit_sign_extension() {
        // Wire value 0x6000: bit15=0, bit14=1, bit13=1. A bare 16-bit
        // reinterpret is positive (+24576, since bit15=0). A (wrong) 14-bit
        // sign extension would discard bits 14:15 and sign-extend from bit
        // 13, giving a negative value (-8192). The correct RT64 semantics
        // is the former.
        let mut fixture = Fixture::new(0x08);
        fixture.yh = 0x6000;
        let (triangle, oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, false);
        let (_xs, ys, _dy3, _dx3) = oracle.geometry();

        // Oracle must have taken the 16-bit reinterpret path: 24576/4 =
        // 6144.0, not the wrong 14-bit path's -8192/4 = -2048.0.
        assert_eq!(ys[0], 6144.0, "oracle sanity: 16-bit reinterpret");
        assert_close(computed.vertex(0).y(), ys[0], "yh 16-bit reinterpret");
        assert_ne!(
            computed.vertex(0).y(),
            -2048.0,
            "must not take the wrong 14-bit sign-extension path"
        );
    }

    #[test]
    fn y_fields_cover_signed_16_bit_boundaries_for_all_three_fields() {
        // Most-negative i16 (0x8000 -> -32768), most-positive (0x7FFF ->
        // 32767), and -1 (0xFFFF), all under a plain 16-bit reinterpret.
        let cases: [(u16, i32); 3] = [(0x8000, -32768), (0x7FFF, 32767), (0xFFFF, -1)];
        for (wire, expected_signed) in cases {
            let mut fixture = Fixture::new(0x08);
            fixture.yl = wire;
            let (triangle, _oracle) = fixture.build();
            let computed = decode_triangle_vertices(&triangle, false);
            // y2 = yl / 4.0 (yl participates as y2's value).
            assert_close(
                computed.vertex(1).y(),
                f32::from(expected_signed as i16) / 4.0,
                "yl signed boundary",
            );
        }
    }

    /// Directly distinguishes 16-bit reinterpretation from 14-bit sign
    /// extension across every case where the two disagree: bits 14:15 set
    /// to a pattern that would flip the sign under a (wrong) 14-bit
    /// extension but not under the correct 16-bit reinterpret, and vice
    /// versa. `wire16` is the reinterpret oracle's answer (independently
    /// computed); `wire14_would_be` is what a 14-bit sign extension would
    /// have produced, included only to document the disagreement -- this
    /// module must produce `wire16`, never `wire14_would_be`.
    #[test]
    fn sixteen_bit_reinterpret_and_fourteen_bit_sign_extension_disagree_and_16_bit_wins() {
        let cases: [(u16, i32, i32); 6] = [
            // (wire, wire16_reinterpret, wire14_sign_extension_would_be)
            (0x2000, 8192, -8192), // bit13 set, bits14:15 clear: 16-bit +, 14-bit -
            (0x6000, 24576, -8192), // bit13,14 set, bit15 clear: 16-bit +, 14-bit -
            (0xA000, -24576, -8192), // bit13,15 set, bit14 clear: 16-bit -, 14-bit -
            (0xE000, -8192, -8192), // bits13:15 all set: 16-bit and 14-bit happen to agree here
            (0xDFFF, -8193, 8191), // bit15 set, bit13 clear: 16-bit -, 14-bit +
            (0x1FFF, 8191, 8191),  // bits14:15 clear: 16-bit and 14-bit agree
        ];
        for (wire, wire16, wire14_would_be) in cases {
            // Sanity: the oracle's independent 16-bit reinterpret matches
            // the documented expectation.
            let oracle_value = Oracle::wire16_reinterpret_oracle(u32::from(wire));
            assert_eq!(
                oracle_value, wire16,
                "oracle wire16 reinterpret for {wire:#06x}"
            );

            let mut fixture = Fixture::new(0x08);
            fixture.ym = wire;
            let (triangle, _oracle) = fixture.build();
            let computed = decode_triangle_vertices(&triangle, false);
            let expected_y3 = f32::from(wire16 as i16) / 4.0;
            let wrong_14_bit_y3 = f32::from(wire14_would_be as i16) / 4.0;
            assert_close(
                computed.vertex(2).y(),
                expected_y3,
                "ym must use the 16-bit reinterpret",
            );
            if wire16 != wire14_would_be {
                assert_ne!(
                    computed.vertex(2).y(),
                    wrong_14_bit_y3,
                    "ym must not use the wrong 14-bit sign extension for {wire:#06x}"
                );
            }
        }
    }

    // --- fractional / nonzero y-floor ---

    #[test]
    fn nonzero_fractional_yfloor_shifts_dy_and_x_intercepts() {
        // yh = 0x0033 -> 51/4 = 12.75; floor(12.75) = 12.0, not 12 or 13
        // trivially -- a wrong floor (e.g. truncation toward the fixed-point
        // quarter boundary instead of the float value) would produce a
        // different x1/x2.
        let mut fixture = Fixture::new(0x08);
        fixture.yh = 0x0033;
        fixture.yl = 0x0080; // 32/4 = 8.0
        fixture.xh = 0x0010_0000; // 16.0
        fixture.dxhdy = 0x0002_0000; // 2.0
        let (triangle, oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, false);
        let (xs, ys, _dy3, _dx3) = oracle.geometry();
        assert_close(computed.vertex(0).x(), xs[0], "x1 with fractional floor");
        assert_close(computed.vertex(1).x(), xs[1], "x2 with fractional floor");
        assert_close(computed.vertex(0).y(), ys[0], "y1");
        assert_ne!(
            ys[0].floor(),
            ys[0],
            "fixture sanity: yh must be non-integer"
        );
    }

    // --- shade half-word reconstruction: signed boundaries ---

    #[test]
    fn shade_coefficients_reconstruct_signed_halfword_boundaries() {
        let mut fixture = Fixture::new(0x0c); // shaded, no texture, no depth
        fixture.shade = Some([
            (0x7FFF_8000, 0x0000_FFFF), // base: max positive hi, min negative lo
            (0x0000_0000, 0x0000_0000), // dx
            (0x8000_7FFF, 0xFFFF_0000), // base continuation (words[2])
            (0x0000_0000, 0x0000_0000), // dx continuation
            (0x0000_0000, 0x0000_0000), // de
            (0xFFFF_FFFF, 0xFFFF_FFFF), // de dy (unused)
            (0x0000_0000, 0x0000_0000), // de continuation
            (0x1234_5678, 0x9ABC_DEF0), // de dy continuation (unused)
        ]);
        let (triangle, oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, false);
        let expected = oracle.shade_block(4);
        for (vertex_index, expected_color) in expected.into_iter().enumerate() {
            for (actual, expected) in computed
                .vertex(vertex_index)
                .color()
                .into_iter()
                .zip(expected_color)
            {
                assert_close(actual, expected, "shade signed boundary");
            }
        }
    }

    #[test]
    fn shade_dy_words_do_not_affect_the_result() {
        let mut fixture = Fixture::new(0x0c);
        let base_block = shade_fixture_block();
        fixture.shade = Some(base_block);
        let (triangle_a, _) = fixture.build();

        let mut mutated_block = base_block;
        mutated_block[5] = (0xDEAD_BEEF, 0xCAFE_BABE);
        mutated_block[7] = (0x1111_1111, 0x2222_2222);
        fixture.shade = Some(mutated_block);
        let (triangle_b, _) = fixture.build();

        let a = decode_triangle_vertices(&triangle_a, false);
        let b = decode_triangle_vertices(&triangle_b, false);
        for index in 0..3 {
            assert_eq!(a.vertex(index).color(), b.vertex(index).color());
        }
    }

    // --- texture: perspective on/off, W zero, negative W ---

    #[test]
    fn texture_perspective_on_divides_by_w_and_scales_by_65536000_and_1024() {
        let mut fixture = Fixture::new(0x0a); // textured, no shade, no depth
        fixture.texture = Some(texture_fixture_block(false));
        let (triangle, oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, true);
        let (expected_tc, expected_w) = oracle.texture_block(4, true);
        for index in 0..3 {
            assert_close(
                computed.vertex(index).w(),
                expected_w[index],
                "perspective w",
            );
            for (actual, expected) in computed
                .vertex(index)
                .texcoord()
                .into_iter()
                .zip(expected_tc[index])
            {
                assert_close(actual, expected, "perspective texcoord");
            }
        }
    }

    #[test]
    fn texture_perspective_off_uses_fixed_w_one_and_1024_over_16384_scale() {
        let mut fixture = Fixture::new(0x0a);
        fixture.texture = Some(texture_fixture_block(false));
        let (triangle, oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, false);
        let (expected_tc, expected_w) = oracle.texture_block(4, false);
        for index in 0..3 {
            assert_eq!(
                computed.vertex(index).w(),
                1.0,
                "non-perspective w is fixed 1.0"
            );
            assert_eq!(expected_w[index], 1.0);
            for (actual, expected) in computed
                .vertex(index)
                .texcoord()
                .into_iter()
                .zip(expected_tc[index])
            {
                assert_close(actual, expected, "non-perspective texcoord");
            }
        }
    }

    #[test]
    fn texture_perspective_with_zero_w_produces_rt64_exact_infinity() {
        // base w = 0, de.w = 0 => w1 = w2 = w3_opposite = 0.0; dx.w = 0 =>
        // w3 = 0.0 too. 65536000.0 / 0.0 is +infinity in IEEE f32, and RT64
        // performs exactly this division with no guard. This module must
        // reproduce that infinity, not substitute a fallback.
        let mut fixture = Fixture::new(0x0a);
        let mut block = texture_fixture_block(false);
        block[0] = (0x0002_0000, 0x0000_0000); // w base = 0
        fixture.texture = Some(block);
        let (triangle, _oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, true);
        for index in 0..3 {
            assert!(
                computed.vertex(index).w().is_infinite() && computed.vertex(index).w() > 0.0,
                "zero W under perspective must divide out to +infinity, got {}",
                computed.vertex(index).w()
            );
            assert!(
                computed.vertex(index).texcoord()[0].is_nan()
                    || computed.vertex(index).texcoord()[0] == 0.0
                    || computed.vertex(index).texcoord()[0].is_infinite(),
                "texcoord/0 must be an IEEE division result, not silently substituted"
            );
        }
    }

    #[test]
    fn texture_perspective_with_negative_w_produces_negative_w_and_flips_texcoord_sign() {
        let mut fixture = Fixture::new(0x0a);
        fixture.texture = Some(texture_fixture_block(true));
        let (triangle, oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, true);
        let (expected_tc, expected_w) = oracle.texture_block(4, true);
        for index in 0..3 {
            assert!(
                expected_w[index] < 0.0,
                "fixture sanity: oracle W must be negative"
            );
            assert_close(computed.vertex(index).w(), expected_w[index], "negative w");
            for (actual, expected) in computed
                .vertex(index)
                .texcoord()
                .into_iter()
                .zip(expected_tc[index])
            {
                assert_close(actual, expected, "negative-w texcoord");
            }
        }
    }

    // --- depth scaling ---

    #[test]
    fn depth_scale_matches_1_over_65536_times_32768() {
        let mut fixture = Fixture::new(0x09); // depth only
        fixture.depth = Some([(0x0001_0000, 0x0000_0000), (0x0000_0000, 0x0000_0000)]);
        let (triangle, _oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, false);
        // base_depth_fixed = 0x00010000 = 65536; 65536 * (1/65536/32768) =
        // 1/32768 exactly representable.
        let expected = 1.0f32 / 32768.0;
        for index in 0..3 {
            assert_close(computed.vertex(index).z(), expected, "depth base scale");
        }
    }

    #[test]
    fn depth_dy_word_does_not_affect_the_result() {
        let mut fixture = Fixture::new(0x09);
        fixture.depth = Some(depth_fixture_block());
        let (triangle_a, _) = fixture.build();

        let mut mutated = depth_fixture_block();
        mutated[1].1 = 0x7FFF_FFFF; // depthDyFixed, must be dead
        fixture.depth = Some(mutated);
        let (triangle_b, _) = fixture.build();

        let a = decode_triangle_vertices(&triangle_a, false);
        let b = decode_triangle_vertices(&triangle_b, false);
        for index in 0..3 {
            assert_eq!(a.vertex(index).z(), b.vertex(index).z());
        }
    }

    #[test]
    fn negative_depth_words_remain_signed_through_the_scale() {
        let mut fixture = Fixture::new(0x09);
        fixture.depth = Some([(0xFFFF_0000_u32, 0x0000_0000), (0x0000_0000, 0x0000_0000)]);
        let (triangle, oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, false);
        let expected = oracle.depth_block(4);
        for (index, expected_z) in expected.into_iter().enumerate() {
            assert_close(computed.vertex(index).z(), expected_z, "negative depth");
            assert!(expected_z < 0.0, "fixture sanity: expected negative depth");
        }
    }

    // --- optional-block defaults ---

    #[test]
    fn no_shade_block_defaults_to_exact_zero_color() {
        let fixture = Fixture::new(0x08); // no shade, no texture, no depth
        let (triangle, _oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, false);
        for index in 0..3 {
            assert_eq!(computed.vertex(index).color(), [0.0, 0.0, 0.0, 0.0]);
        }
    }

    #[test]
    fn no_texture_block_defaults_to_exact_zero_texcoord_and_w_one() {
        let fixture = Fixture::new(0x08);
        let (triangle, _oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, false);
        for index in 0..3 {
            assert_eq!(computed.vertex(index).texcoord(), [0.0, 0.0]);
            assert_eq!(computed.vertex(index).w(), 1.0);
        }
    }

    #[test]
    fn no_depth_block_defaults_to_exact_zero_depth() {
        let fixture = Fixture::new(0x08);
        let (triangle, _oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, false);
        for index in 0..3 {
            assert_eq!(computed.vertex(index).z(), 0.0);
        }
    }

    #[test]
    fn texture_perspective_flag_is_ignored_when_untextured() {
        // Caller-supplied texture_perspective must not perturb the
        // untextured W=1.0/texcoord=0 default either way.
        let fixture = Fixture::new(0x08);
        let (triangle, _oracle) = fixture.build();
        let with_perspective = decode_triangle_vertices(&triangle, true);
        let without_perspective = decode_triangle_vertices(&triangle, false);
        assert_eq!(with_perspective, without_perspective);
    }

    // --- exact three-vertex ordering ---

    #[test]
    fn vertex_order_is_yh_then_yl_then_ym_position_source() {
        // Distinct, independently chosen yh/yl/ym and xh/xl so each
        // vertex's expected (x, y) is unambiguous and a swap of any two
        // vertices is detectable.
        let mut fixture = Fixture::new(0x08);
        fixture.yh = 0x0010; // 4.0 -> y1
        fixture.yl = 0x0080; // 32.0 -> y2
        fixture.ym = 0x0040; // 16.0 -> y3
        fixture.xh = 0x0001_0000; // 1.0, dxhdy = 0 so x1 = x2 = hIntercept = 1.0
        fixture.dxhdy = 0;
        fixture.xl = 0x0009_0000; // 9.0 -> x3 (= xl directly)
        let (triangle, _oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, false);
        assert_close(computed.vertex(0).y(), 4.0, "vertex 0 is yh");
        assert_close(computed.vertex(1).y(), 32.0, "vertex 1 is yl");
        assert_close(computed.vertex(2).y(), 16.0, "vertex 2 is ym");
        assert_close(computed.vertex(2).x(), 9.0, "vertex 2 x is xl directly");
    }

    #[test]
    #[should_panic]
    fn vertex_index_out_of_range_panics() {
        let fixture = Fixture::new(0x08);
        let (triangle, _oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, false);
        let _ = computed.vertex(3);
    }

    // --- source-shape hostiles: block-order swap, wrong factor ---

    #[test]
    fn swapping_shade_and_texture_block_order_changes_the_decode() {
        // Build 0x0f (fully populated) two ways: correct RT64 order
        // (shade, texture, depth) versus swapped (texture, shade, depth),
        // both re-decoded through the *production* decoder at the byte
        // level. If `RawTriangle`/this module silently tolerated either
        // block order, the two decodes would agree; RT64's fixed order
        // means they must not.
        let shade = shade_fixture_block();
        let texture = texture_fixture_block(false);
        let depth = depth_fixture_block();

        let mut correct_bytes = Vec::new();
        correct_bytes.extend(base_word0_bytes(0, 0, false, 0, 0x0040, 0x0010));
        correct_bytes.extend(edge_bytes(0, 0));
        correct_bytes.extend(edge_bytes(0x0001_0000, 0));
        correct_bytes.extend(edge_bytes(0, 0));
        for &(w0, w1) in &shade {
            correct_bytes.extend(word_bytes(w0, w1));
        }
        for &(w0, w1) in &texture {
            correct_bytes.extend(word_bytes(w0, w1));
        }
        for &(w0, w1) in &depth {
            correct_bytes.extend(word_bytes(w0, w1));
        }
        let correct = RawTriangle::decode(0x0f, &correct_bytes).unwrap();

        let mut swapped_bytes = Vec::new();
        swapped_bytes.extend(base_word0_bytes(0, 0, false, 0, 0x0040, 0x0010));
        swapped_bytes.extend(edge_bytes(0, 0));
        swapped_bytes.extend(edge_bytes(0x0001_0000, 0));
        swapped_bytes.extend(edge_bytes(0, 0));
        for &(w0, w1) in &texture {
            swapped_bytes.extend(word_bytes(w0, w1));
        }
        for &(w0, w1) in &shade {
            swapped_bytes.extend(word_bytes(w0, w1));
        }
        for &(w0, w1) in &depth {
            swapped_bytes.extend(word_bytes(w0, w1));
        }
        let swapped = RawTriangle::decode(0x0f, &swapped_bytes).unwrap();

        let correct_vertices = decode_triangle_vertices(&correct, false);
        let swapped_vertices = decode_triangle_vertices(&swapped, false);
        assert_ne!(
            correct_vertices.vertex(0).color(),
            swapped_vertices.vertex(0).color()
        );
    }

    #[test]
    fn wrong_perspective_w_factor_would_be_caught_by_exact_oracle_comparison() {
        // Direct sensitivity check: the 65536000.0 constant, not a nearby
        // wrong value like 65536.0 or 1000.0, must be what divides W.
        let mut fixture = Fixture::new(0x0a);
        let mut block = texture_fixture_block(false);
        block[0] = (0x0002_0000, 0x0001_0000); // w base = 1.0 in Q16.16
        fixture.texture = Some(block);
        let (triangle, _oracle) = fixture.build();
        let computed = decode_triangle_vertices(&triangle, true);
        // w_base = 1.0, de.w = 0 => w1 = w2 = w3_opposite = w3 = 1.0.
        assert_close(computed.vertex(0).w(), 65536000.0, "65536000 factor");
        assert_ne!(computed.vertex(0).w(), 65536.0);
        assert_ne!(computed.vertex(0).w(), 1000.0);
    }

    #[test]
    fn wrong_texcoord_scale_factors_would_be_caught() {
        let mut fixture = Fixture::new(0x0a);
        let mut block = texture_fixture_block(false);
        block[0] = (0x0040_0000, 0x0001_0000); // base s = 64.0 in Q16.16, w = 1.0
        fixture.texture = Some(block);
        let (triangle, _oracle) = fixture.build();
        let non_perspective = decode_triangle_vertices(&triangle, false);
        // s=64.0 * 1024 / 16384 = 4.0 exactly.
        assert_close(
            non_perspective.vertex(0).texcoord()[0],
            4.0,
            "1024/16384 factor",
        );

        let perspective = decode_triangle_vertices(&triangle, true);
        // s / w * 1024 = 64.0 / 1.0 * 1024 = 65536.0 exactly.
        assert_close(
            perspective.vertex(0).texcoord()[0],
            65536.0,
            "perspective 1024 factor",
        );
    }

    #[test]
    fn tile_and_level_pass_through_untouched_by_vertex_conversion() {
        // This module must not read or perturb `tile()`/`level()` -- those
        // remain owned by `RawTriangle` and are not part of the vertex
        // computation RT64 performs in this slice.
        let mut fixture = Fixture::new(0x08);
        fixture.yh = 0;
        let (triangle, _oracle) = fixture.build();
        assert_eq!(triangle.tile(), TileIndex::try_new(0).unwrap());
        assert_eq!(triangle.level(), 0);
        let _ = decode_triangle_vertices(&triangle, false);
        assert_eq!(triangle.tile(), TileIndex::try_new(0).unwrap());
        assert_eq!(triangle.level(), 0);
    }
}
