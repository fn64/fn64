struct I64 {
    lo: u32,
    hi: i32,
}

struct AttributePlane {
    base: i32,
    dx: i32,
    de: i32,
    dy: i32,
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
    planes: array<AttributePlane, 7>,
    env_rgba8: u32,
    prim_rgba8: u32,
}

struct CoverageParams {
    width: u32,
    height: u32,
    pixels_per_triangle: u32,
    triangle_count: u32,
}

struct SampleLine {
    left: I64,
    right: I64,
    major: I64,
}

struct CoverageSample {
    coverage: u32,
    delta_y_eighth: i32,
    delta_x_q16: I64,
    plane_values: array<I64, 7>,
}

// Group 1 is reserved for compute-raster state. Group 0 remains compatible
// with `tmem_sample.wgsl` so the color-producing entry point can reuse its
// already-proven committed-TMEM bindings without renumbering either module.
@group(1) @binding(0)
var<storage, read> triangles: array<TriangleEdges>;

@group(1) @binding(1)
var<uniform> params: CoverageParams;

@group(1) @binding(2)
var<storage, read_write> samples: array<CoverageSample>;

@group(1) @binding(3)
var<storage, read_write> color_target_words: array<u32>;

@group(1) @binding(4)
var<storage, read_write> color_status: array<u32>;

// Diagnostic-only stage trace used by the explicitly enabled game-derived
// CPU/compute differential. One pixel records the exact inputs crossing the
// texture, shade, and combiner boundaries so a byte mismatch names its first
// divergent stage instead of inviting shader-wide guesses.
@group(1) @binding(5)
var<storage, read_write> color_trace: array<vec4<u32>>;

fn i64_from_i32(value: i32) -> I64 {
    return I64(u32(value), select(0, -1, value < 0));
}

fn i64_add(a: I64, b: I64) -> I64 {
    let lo = a.lo + b.lo;
    let carry = select(0u, 1u, lo < a.lo);
    return I64(lo, bitcast<i32>(u32(a.hi) + u32(b.hi) + carry));
}

fn i64_sub(a: I64, b: I64) -> I64 {
    return i64_add(a, i64_neg(b));
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

fn i64_floor_div_65536(value: I64) -> I64 {
    return I64((value.lo >> 16u) | (u32(value.hi) << 16u), value.hi >> 16u);
}

fn i64_shift_left_16(value: I64) -> I64 {
    return I64(value.lo << 16u, bitcast<i32>((u32(value.hi) << 16u) | (value.lo >> 16u)));
}

fn i64_mul_i32_u32(a: i32, b: u32) -> I64 {
    let magnitude_a = select(u32(a), ~u32(a) + 1u, a < 0);
    let magnitude = unsigned_mul_32(magnitude_a, b);
    if a < 0 {
        return i64_neg(magnitude);
    }
    return magnitude;
}

// `b = b.hi * 2^32 + b.lo`. Splitting on that identity keeps every
// intermediate in two words while preserving floor division for negatives.
fn i64_mul_i32_i64_floor_q16(a: i32, b: I64) -> I64 {
    let high_term = i64_shift_left_16(i64_mul_i32(a, b.hi));
    let low_term = i64_floor_div_65536(i64_mul_i32_u32(a, b.lo));
    return i64_add(high_term, low_term);
}

fn evaluate_plane(plane: AttributePlane, delta_y_eighth: i32, delta_x_q16: I64) -> I64 {
    let edge_term = i64_floor_div_8(i64_mul_i32(plane.de, delta_y_eighth));
    let x_term = i64_mul_i32_i64_floor_q16(plane.dx, delta_x_q16);
    return i64_add(i64_add(i64_from_i32(plane.base), edge_term), x_term);
}

fn i64_to_f32(value: I64) -> f32 {
    if value.hi < 0 {
        let magnitude = i64_neg(value);
        return -(f32(u32(magnitude.hi)) * 4294967296.0 + f32(magnitude.lo));
    }
    return f32(u32(value.hi)) * 4294967296.0 + f32(value.lo);
}

fn shade_channel(value: I64) -> f32 {
    // `execute_raw_triangle` converts its Q16.16 shade plane with
    // `div_euclid(65536)` before clamping to an RGBA8 channel. Preserve
    // that integer boundary instead of leaking the fractional plane bits
    // into the float combiner.
    let whole = bitcast<i32>(i64_floor_div_65536(value).lo);
    return f32(clamp(whole, 0, 255)) / 255.0;
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

fn sample_line(triangle: TriangleEdges, sample_y_eighth: i32) -> SampleLine {
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
    return SampleLine(left, right, major);
}

fn sample_is_covered(line: SampleLine, sample_x: I64) -> bool {
    return i64_greater_equal(sample_x, line.left) && i64_less(sample_x, line.right);
}

fn evaluate_coverage_sample(triangle: TriangleEdges, x: u32, y: u32) -> CoverageSample {
    let sample_y_offsets = array<i32, 4>(1, 3, 5, 7);
    let high_origin_eighth = (triangle.yh & ~3) * 2;
    let yh_eighth = triangle.yh * 2;
    let yl_eighth = triangle.yl * 2;
    var count = 0u;
    var found_sample = false;
    var first_delta_y_eighth = 0;
    var first_delta_x_q16 = I64(0u, 0);
    var plane_values: array<I64, 7>;
    for (var row = 0u; row < 4u; row += 1u) {
        let offset_y = sample_y_offsets[row];
        let sample_y_eighth = i32(y) * 8 + offset_y;
        if sample_y_eighth < yh_eighth || sample_y_eighth >= yl_eighth {
            continue;
        }
        let columns = sample_columns(offset_y);
        let line = sample_line(triangle, sample_y_eighth);
        let sample_x0 = pixel_sample_x(x, columns.x);
        if sample_is_covered(line, sample_x0) {
            if !found_sample {
                found_sample = true;
                first_delta_y_eighth = sample_y_eighth - high_origin_eighth;
                first_delta_x_q16 = i64_sub(sample_x0, line.major);
            }
            count += 1u;
        }
        let sample_x1 = pixel_sample_x(x, columns.y);
        if sample_is_covered(line, sample_x1) {
            if !found_sample {
                found_sample = true;
                first_delta_y_eighth = sample_y_eighth - high_origin_eighth;
                first_delta_x_q16 = i64_sub(sample_x1, line.major);
            }
            count += 1u;
        }
    }
    if found_sample {
        for (var plane = 0u; plane < 7u; plane += 1u) {
            plane_values[plane] = evaluate_plane(
                triangle.planes[plane], first_delta_y_eighth, first_delta_x_q16
            );
        }
    }
    return CoverageSample(count, first_delta_y_eighth, first_delta_x_q16, plane_values);
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
    samples[output_index] = evaluate_coverage_sample(triangles[triangle_index], x, y);
}

fn packed_rgba8(value: u32) -> vec4<f32> {
    return vec4<f32>(
        f32((value >> 24u) & 0xffu),
        f32((value >> 16u) & 0xffu),
        f32((value >> 8u) & 0xffu),
        f32(value & 0xffu),
    ) / 255.0;
}

fn rgba16_color(value: u32) -> vec4<f32> {
    let r = (value >> 11u) & 0x1fu;
    let g = (value >> 6u) & 0x1fu;
    let b = (value >> 1u) & 0x1fu;
    let a = select(0u, 255u, (value & 1u) != 0u);
    return vec4<f32>(
        f32((r << 3u) | (r >> 2u)),
        f32((g << 3u) | (g >> 2u)),
        f32((b << 3u) | (b >> 2u)),
        f32(a),
    ) / 255.0;
}

fn pack_rgba16(color: vec4<f32>, coverage_bit: u32) -> u32 {
    let bytes = vec3<u32>(round(clamp(color.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0));
    return ((bytes.r >> 3u) << 11u)
        | ((bytes.g >> 3u) << 6u)
        | ((bytes.b >> 3u) << 1u)
        | (coverage_bit & 1u);
}

fn quantize_rgba8(color: vec4<f32>) -> vec4<f32> {
    return round(clamp(color, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0) / 255.0;
}

fn perspective_coordinate(numerator: I64, denominator: I64) -> i32 {
    let raw = i64_to_f32(numerator) / i64_to_f32(denominator) * 32768.0;
    if raw != raw {
        return 0;
    }
    return i32(clamp(raw, -32768.0, 32767.0));
}

struct ColorPixelResult {
    rgba16: u32,
    status: u32,
    trace: vec4<u32>,
}

fn execute_hot_color_pixel(pixel_index: u32, initial_rgba16: u32) -> ColorPixelResult {
    let x = pixel_index % params.width;
    let y = pixel_index / params.width;
    var current = initial_rgba16;
    var status = 0u;
    var trace = vec4<u32>(0u);
    for (var triangle_index = 0u; triangle_index < params.triangle_count; triangle_index += 1u) {
        let triangle = triangles[triangle_index];
        let raster = evaluate_coverage_sample(triangle, x, y);
        if raster.coverage == 0u {
            continue;
        }
        let raw_s = perspective_coordinate(raster.plane_values[4], raster.plane_values[6]);
        let raw_t = perspective_coordinate(raster.plane_values[5], raster.plane_values[6]);
        let texel = sample_committed_rgba16_point_bound(raw_s, raw_t);
        if texel.status != TMEM_SAMPLE_STATUS_OK {
            status = texel.status;
            continue;
        }
        var inputs: CombinerInputs;
        inputs.tex_val0 = texel.color;
        inputs.tex_val1 = texel.color;
        inputs.prim_color = packed_rgba8(triangle.prim_rgba8);
        inputs.shade_color = vec4<f32>(
            shade_channel(raster.plane_values[0]),
            shade_channel(raster.plane_values[1]),
            shade_channel(raster.plane_values[2]),
            shade_channel(raster.plane_values[3]),
        );
        inputs.env_color = packed_rgba8(triangle.env_rgba8);
        inputs.key_center = vec3<f32>(0.0);
        inputs.key_scale = vec3<f32>(0.0);
        inputs.lod_fraction = 0.0;
        inputs.prim_lod_frac = 0.0;
        inputs.noise = 0.0;
        inputs.k4 = 0.0;
        inputs.k5 = 0.0;
        // The CPU executor closes the combiner stage to RGBA8 before the
        // blender reads it (`combine_one_texel` -> `blend_fragment`). Keep
        // that representation boundary here: carrying the combiner's f32
        // result directly into the blender changes values at RGBA16's 5-bit
        // packing thresholds for real WM2000 operands.
        let combined = quantize_rgba8(
            run_one_cycle(CombineParams(0xfc5196a3u, 0x112cfe7fu), inputs).combiner_color
        );
        let texel_bytes = vec4<u32>(round(clamp(texel.color, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0));
        let shade_bytes = vec4<u32>(round(clamp(inputs.shade_color, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0));
        let combined_bytes = vec4<u32>(round(clamp(combined, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0));
        trace = vec4<u32>(
            (u32(raw_s) & 0xffffu) | ((u32(raw_t) & 0xffffu) << 16u),
            texel_bytes.r | (texel_bytes.g << 8u) | (texel_bytes.b << 16u) | (texel_bytes.a << 24u),
            shade_bytes.r | (shade_bytes.g << 8u) | (shade_bytes.b << 16u) | (shade_bytes.a << 24u),
            combined_bytes.r | (combined_bytes.g << 8u) | (combined_bytes.b << 16u) | (combined_bytes.a << 24u),
        );
        let shade_alpha = round(clamp(inputs.shade_color.a, 0.0, 1.0) * 255.0) / 255.0;
        let memory_count = select(1u, 8u, (current & 1u) != 0u);
        let coverage = coverage_fragment_fn(8u, memory_count, 1u, 1u, 1u, 1u, 0u, 0u, 0u);
        if coverage.wraps == 0u {
            continue;
        }
        let destination = rgba16_color(current);
        let blended = blend_fragment_memory_composite_fn(
            1u,
            0u, 0u, 1u, 0u,
            0u, 0u, 0u, 0u,
            combined,
            shade_alpha,
            vec4<f32>(0.0),
            vec4<f32>(0.0),
            coverage.blend_enabled,
            destination,
        );
        current = pack_rgba16(blended, (coverage.destination_count - 1u) >> 2u);
    }
    return ColorPixelResult(current, status, trace);
}

// One invocation owns both halfwords of a storage word. This closes the
// read-modify-write race two independent pixel invocations would have on a
// packed RGBA16 target.
@compute @workgroup_size(64)
fn compute_triangle_hot_color(@builtin(global_invocation_id) id: vec3<u32>) {
    let word_index = id.x;
    if word_index >= arrayLength(&color_target_words) {
        return;
    }
    let device_word = color_target_words[word_index];
    let first_pixel = word_index * 2u;
    let first_device = device_word & 0xffffu;
    let first_logical = ((first_device & 0xffu) << 8u) | (first_device >> 8u);
    let first = execute_hot_color_pixel(first_pixel, first_logical);
    var output_word = ((first.rgba16 & 0xffu) << 8u) | (first.rgba16 >> 8u);
    color_status[first_pixel] = first.status;
    color_trace[first_pixel] = first.trace;
    let second_pixel = first_pixel + 1u;
    if second_pixel < params.pixels_per_triangle {
        let second_device = device_word >> 16u;
        let second_logical = ((second_device & 0xffu) << 8u) | (second_device >> 8u);
        let second = execute_hot_color_pixel(second_pixel, second_logical);
        let packed_device = ((second.rgba16 & 0xffu) << 8u) | (second.rgba16 >> 8u);
        output_word |= packed_device << 16u;
        color_status[second_pixel] = second.status;
        color_trace[second_pixel] = second.trace;
    } else {
        output_word |= device_word & 0xffff0000u;
    }
    color_target_words[word_index] = output_word;
}
