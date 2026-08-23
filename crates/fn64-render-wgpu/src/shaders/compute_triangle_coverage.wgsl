struct I64 {
    lo: u32,
    hi: i32,
}

struct TriangleEdges {
    left_major: u32,
    yl: i32,
    ym: i32,
    yh: i32,
    xl: i32,
    xh: i32,
    xm: i32,
    dxldy: i32,
    dxhdy: i32,
    dxmdy: i32,
}

struct CoverageParams {
    width: u32,
    height: u32,
    pixels_per_triangle: u32,
    triangle_count: u32,
}

@group(0) @binding(0)
var<storage, read> triangles: array<TriangleEdges>;

@group(0) @binding(1)
var<uniform> params: CoverageParams;

@group(0) @binding(2)
var<storage, read_write> coverage: array<u32>;

fn i64_from_i32(value: i32) -> I64 {
    return I64(u32(value), select(0, -1, value < 0));
}

fn i64_add(a: I64, b: I64) -> I64 {
    let lo = a.lo + b.lo;
    let carry = select(0u, 1u, lo < a.lo);
    return I64(lo, bitcast<i32>(u32(a.hi) + u32(b.hi) + carry));
}

fn i64_neg(value: I64) -> I64 {
    let lo = ~value.lo + 1u;
    let carry = select(0u, 1u, lo == 0u);
    return I64(lo, bitcast<i32>(~u32(value.hi) + carry));
}

fn unsigned_mul_32(a: u32, b: u32) -> I64 {
    let a0 = a & 0xffffu;
    let a1 = a >> 16u;
    let b0 = b & 0xffffu;
    let b1 = b >> 16u;
    let p0 = a0 * b0;
    let p1 = a0 * b1;
    let p2 = a1 * b0;
    let p3 = a1 * b1;
    let middle = (p0 >> 16u) + (p1 & 0xffffu) + (p2 & 0xffffu);
    let lo = (p0 & 0xffffu) | (middle << 16u);
    let hi = p3 + (p1 >> 16u) + (p2 >> 16u) + (middle >> 16u);
    return I64(lo, bitcast<i32>(hi));
}

fn i64_mul_i32(a: i32, b: i32) -> I64 {
    let magnitude_a = select(u32(a), ~u32(a) + 1u, a < 0);
    let magnitude_b = select(u32(b), ~u32(b) + 1u, b < 0);
    let magnitude = unsigned_mul_32(magnitude_a, magnitude_b);
    if (a < 0) != (b < 0) {
        return i64_neg(magnitude);
    }
    return magnitude;
}

// Arithmetic shift is floor division by eight, matching Rust's
// `div_euclid(8)` for a positive divisor, including negative products.
fn i64_floor_div_8(value: I64) -> I64 {
    return I64((value.lo >> 3u) | (u32(value.hi) << 29u), value.hi >> 3u);
}

fn i64_less(a: I64, b: I64) -> bool {
    return a.hi < b.hi || (a.hi == b.hi && a.lo < b.lo);
}

fn i64_greater_equal(a: I64, b: I64) -> bool {
    return !i64_less(a, b);
}

fn edge_x(base: i32, slope: i32, delta_y_eighth: i32) -> I64 {
    return i64_add(i64_from_i32(base), i64_floor_div_8(i64_mul_i32(slope, delta_y_eighth)));
}

fn pixel_sample_x(x: u32, offset_eighth: i32) -> I64 {
    return i64_from_i32((i32(x) * 8 + offset_eighth) * 8192);
}

fn sample_columns(offset_y: i32) -> vec2<i32> {
    return select(vec2<i32>(3, 7), vec2<i32>(1, 5), offset_y == 1 || offset_y == 5);
}

fn sample_is_covered(triangle: TriangleEdges, x: u32, sample_y_eighth: i32, offset_x: i32) -> bool {
    let high_origin_eighth = (triangle.yh & ~3) * 2;
    let middle_eighth = triangle.ym * 2;
    let major = edge_x(triangle.xh, triangle.dxhdy, sample_y_eighth - high_origin_eighth);
    var minor: I64;
    if sample_y_eighth < middle_eighth {
        minor = edge_x(triangle.xm, triangle.dxmdy, sample_y_eighth - high_origin_eighth);
    } else {
        minor = edge_x(triangle.xl, triangle.dxldy, sample_y_eighth - middle_eighth);
    }
    var left = minor;
    var right = major;
    if triangle.left_major != 0u {
        left = major;
        right = minor;
    }
    let sample_x = pixel_sample_x(x, offset_x);
    return i64_greater_equal(sample_x, left) && i64_less(sample_x, right);
}

@compute @workgroup_size(64)
fn compute_triangle_coverage(@builtin(global_invocation_id) id: vec3<u32>) {
    let output_index = id.x;
    let output_count = params.pixels_per_triangle * params.triangle_count;
    if output_index >= output_count {
        return;
    }
    let triangle_index = output_index / params.pixels_per_triangle;
    let pixel_index = output_index % params.pixels_per_triangle;
    let x = pixel_index % params.width;
    let y = pixel_index / params.width;
    let triangle = triangles[triangle_index];
    let sample_y_offsets = array<i32, 4>(1, 3, 5, 7);
    let yh_eighth = triangle.yh * 2;
    let yl_eighth = triangle.yl * 2;
    var count = 0u;
    for (var row = 0u; row < 4u; row += 1u) {
        let offset_y = sample_y_offsets[row];
        let sample_y_eighth = i32(y) * 8 + offset_y;
        if sample_y_eighth < yh_eighth || sample_y_eighth >= yl_eighth {
            continue;
        }
        let columns = sample_columns(offset_y);
        if sample_is_covered(triangle, x, sample_y_eighth, columns.x) {
            count += 1u;
        }
        if sample_is_covered(triangle, x, sample_y_eighth, columns.y) {
            count += 1u;
        }
    }
    coverage[output_index] = count;
}
