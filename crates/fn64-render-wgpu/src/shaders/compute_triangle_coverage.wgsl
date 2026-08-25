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
    tmem_state_index: u32,
    program_id: u32,
}

struct CoverageParams {
    width: u32,
    height: u32,
    pixels_per_triangle: u32,
    triangle_count: u32,
    first_word: u32,
    word_count: u32,
    dispatch_words_per_row: u32,
    target_words_per_row: u32,
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

struct ColorWorkItem {
    target_word: u32,
    first_triangle_index: u32,
    triangle_count: u32,
}

// Group 1 is reserved for compute-raster state. Group 0 retains the proven
// `tmem_sample.wgsl` binding numbers while widening its resources into
// immutable per-dispatch state tables.
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

@group(1) @binding(5)
var<storage, read> color_work_items: array<ColorWorkItem>;

@group(1) @binding(6)
var<storage, read> color_work_triangle_indices: array<u32>;

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

fn i64_mul_i32_wide(value: I64, factor: i32) -> I64 {
    let value_negative = value.hi < 0;
    let factor_negative = factor < 0;
    var magnitude_value = value;
    if value_negative {
        magnitude_value = i64_neg(value);
    }
    let magnitude_factor = select(u32(factor), ~u32(factor) + 1u, factor_negative);
    let low_product = unsigned_mul_32(magnitude_value.lo, magnitude_factor);
    let high = u32(magnitude_value.hi) * magnitude_factor + u32(low_product.hi);
    let magnitude = I64(low_product.lo, bitcast<i32>(high));
    if value_negative != factor_negative {
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

const FOG_ALPHA_DITHER_MAGIC_SQUARE: array<u32, 16> = array<u32, 16>(
    0u, 6u, 1u, 7u,
    4u, 2u, 5u, 3u,
    3u, 5u, 2u, 4u,
    7u, 1u, 6u, 0u,
);

fn fog_alpha_dither(alpha: f32, x: u32, y: u32) -> f32 {
    let alpha_byte = u32(round(clamp(alpha, 0.0, 1.0) * 255.0));
    let threshold = FOG_ALPHA_DITHER_MAGIC_SQUARE[((y & 3u) << 2u) | (x & 3u)];
    let rounded = min(31u, (alpha_byte >> 3u) + select(0u, 1u, (alpha_byte & 7u) > threshold));
    let expanded = (rounded << 3u) | (rounded >> 2u);
    return f32(expanded) / 255.0;
}

fn noise_alpha_dither(alpha: f32) -> f32 {
    let alpha_byte = u32(round(clamp(alpha, 0.0, 1.0) * 255.0));
    let truncated = alpha_byte >> 3u;
    return f32((truncated << 3u) | (truncated >> 2u)) / 255.0;
}

fn materialize_f32(value: f32) -> f32 {
    return bitcast<f32>(bitcast<u32>(value));
}

// Exact specialization of `(Primitive - Environment) * Texel0 +
// Environment`, with `Texel0.a * Primitive.a` for alpha. The bitcast
// materializes the multiply as an f32 before the add, matching the CPU
// oracle's two operations and preventing a contracted GPU FMA from crossing
// the later RGBA16 quantization threshold.
fn combine_full_coverage_program(inputs: CombinerInputs) -> vec4<f32> {
    var output: vec4<f32>;
    for (var channel = 0u; channel < 3u; channel += 1u) {
        output[channel] = clamp(
            materialize_f32(
                (inputs.prim_color[channel] - inputs.env_color[channel])
                    * inputs.tex_val0[channel]
            ) + inputs.env_color[channel],
            0.0,
            1.0,
        );
    }
    output.a = clamp(materialize_f32(inputs.tex_val0.a * inputs.prim_color.a), 0.0, 1.0);
    return quantize_rgba8(output);
}

// Exact specialization of the admitted fog program's two-cycle combiner.
// Cycle 0 produces Texel0 * ShadeAlpha for RGB and Texel0Alpha *
// PrimitiveAlpha for alpha. Cycle 1 computes
// `(Environment - Combined) * Primitive + Combined` for RGB and carries the
// prior alpha. Every multiply is materialized before its following add so
// GPU contraction cannot cross an RGBA8/RGBA16 quantization threshold that
// the CPU oracle's separate f32 operations do not cross.
fn combine_fog_program(inputs: CombinerInputs) -> vec4<f32> {
    var first: vec4<f32>;
    for (var channel = 0u; channel < 3u; channel += 1u) {
        first[channel] = materialize_f32(inputs.tex_val0[channel] * inputs.shade_color.a);
    }
    first.a = materialize_f32(inputs.tex_val0.a * inputs.prim_color.a);

    var output = first;
    for (var channel = 0u; channel < 3u; channel += 1u) {
        output[channel] = clamp(
            materialize_f32(
                (inputs.env_color[channel] - first[channel]) * inputs.prim_color[channel]
            ) + first[channel],
            0.0,
            1.0,
        );
    }
    output.a = clamp(first.a, 0.0, 1.0);
    return quantize_rgba8(output);
}

fn blend_framebuffer_alpha_program(src_bytes: vec4<u32>, dst: vec4<f32>) -> vec4<f32> {
    // This exact program selects alpha-dither Noise. The admitted CPU path
    // has already closed the combiner and alpha-dither stages to RGBA8 before
    // the blender reads CombinedAlpha.
    let dst_bytes = round(clamp(dst, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0);
    let alpha = f32(src_bytes.a) / 255.0;
    var output: vec4<f32>;
    for (var channel = 0u; channel < 3u; channel += 1u) {
        output[channel] = round(clamp(
            materialize_f32(f32(src_bytes[channel]) * alpha)
                + materialize_f32(dst_bytes[channel] * (1.0 - alpha)),
            0.0,
            255.0,
        ));
    }
    output.a = round(clamp(
        materialize_f32(255.0 * alpha)
            + materialize_f32(dst_bytes.a * (1.0 - alpha)),
        0.0,
        255.0,
    ));
    return output / 255.0;
}

fn blend_full_coverage_program(src: vec4<f32>, dst: vec4<f32>) -> vec4<f32> {
    var src_bytes = vec4<u32>(round(clamp(src, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0));
    // This exact program selects alpha-dither Noise. The admitted CPU path
    // uses its proved endpoint threshold 7, so no low-three-bit value can
    // round upward: truncate to five bits, then expand back to RGBA8 before
    // the blender reads CombinedAlpha.
    let alpha_five = src_bytes.a >> 3u;
    src_bytes.a = (alpha_five << 3u) | (alpha_five >> 2u);
    return blend_framebuffer_alpha_program(src_bytes, dst);
}

fn blend_fog_program(src: vec4<f32>, dst: vec4<f32>) -> vec4<f32> {
    let src_bytes = vec4<u32>(round(clamp(src, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0));
    return blend_framebuffer_alpha_program(src_bytes, dst);
}

fn perspective_coordinate(numerator: I64, denominator: I64) -> i32 {
    let numerator_f32 = i64_to_f32(numerator);
    let denominator_f32 = i64_to_f32(denominator);
    let ratio = materialize_f32(numerator_f32 / denominator_f32);
    let raw = materialize_f32(ratio * 32768.0);
    if raw != raw {
        return 0;
    }
    var coordinate = i32(clamp(raw, -32768.0, 32767.0));
    // Metal may round the division/scale pair exactly onto an integer texel
    // boundary where Rust's separately rounded f32 operations remain one
    // ULP below it. Only arbitrate that exact-boundary case, using the same
    // already-rounded f32 operands as the CPU before cross multiplication.
    if raw == f32(coordinate)
        && abs(numerator_f32) <= 2147483520.0
        && abs(denominator_f32) <= 2147483520.0
        && denominator_f32 != 0.0
    {
        let rounded_numerator = i32(numerator_f32);
        let rounded_denominator = i32(denominator_f32);
        let scaled = i64_mul_i32(rounded_numerator, 32768);
        let boundary = i64_mul_i32_wide(i64_from_i32(rounded_denominator), coordinate);
        let rational_is_below = select(
            i64_less(boundary, scaled),
            i64_less(scaled, boundary),
            rounded_denominator > 0,
        );
        if coordinate > 0 && rational_is_below {
            coordinate -= 1;
        } else if coordinate < 0
            && !rational_is_below
            && (i64_less(boundary, scaled) || i64_less(scaled, boundary))
        {
            coordinate += 1;
        }
    }
    return coordinate;
}

struct ColorPixelResult {
    rgba16: u32,
    status: u32,
}

fn execute_hot_color_pixel(
    pixel_index: u32,
    initial_rgba16: u32,
    first_triangle_index: u32,
    work_triangle_count: u32,
) -> ColorPixelResult {
    let x = pixel_index % params.width;
    let y = pixel_index / params.width;
    var current = initial_rgba16;
    var status = 0u;
    for (var work_index = 0u; work_index < work_triangle_count; work_index += 1u) {
        var triangle_index = first_triangle_index + work_index;
        if params.dispatch_words_per_row != 0xffffffffu {
            triangle_index = color_work_triangle_indices[triangle_index];
        }
        let triangle = triangles[triangle_index];
        let raster = evaluate_coverage_sample(triangle, x, y);
        if raster.coverage == 0u {
            continue;
        }
        let raw_s = perspective_coordinate(raster.plane_values[4], raster.plane_values[6]);
        let raw_t = perspective_coordinate(raster.plane_values[5], raster.plane_values[6]);
        tmem_state_index = triangle.tmem_state_index;
        let texel = sample_committed_rgba16_point_bound(raw_s, raw_t);
        if texel.status != TMEM_SAMPLE_STATUS_OK {
            if status == TMEM_SAMPLE_STATUS_OK {
                status = (triangle.tmem_state_index << 8u) | texel.status;
            }
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
        var combined: vec4<f32>;
        if triangle.program_id == 1u {
            combined = combine_full_coverage_program(inputs);
        } else if triangle.program_id == 2u {
            combined = combine_fog_program(inputs);
            combined.a = fog_alpha_dither(combined.a, x, y);
        } else if triangle.program_id == 3u {
            combined = combine_fog_program(inputs);
            // ac8f selects alpha Pattern, but the CPU orders that stage after
            // coverage-times-alpha/alpha-coverage-select. This program's
            // blender ignores alpha, so dithering here would incorrectly
            // perturb the coverage input; the selected dither is downstream-
            // dead for this exact key.
        } else {
            combined = quantize_rgba8(
                run_one_cycle(CombineParams(0xfc5196a3u, 0x112cfe7fu), inputs).combiner_color
            );
            combined.a = noise_alpha_dither(combined.a);
        }
        let shade_alpha = round(clamp(inputs.shade_color.a, 0.0, 1.0) * 255.0) / 255.0;
        let memory_count = select(1u, 8u, (current & 1u) != 0u);
        // Program 1's exact other-mode uses CVG_DST_FULL with AA disabled;
        // program 0 uses CVG_DST_WRAP with AA enabled. The destination count
        // reaches the packed RGBA16 coverage bit, so this is part of program
        // identity rather than a dead render-state distinction.
        let full_coverage_program = triangle.program_id == 1u;
        let fog_program = triangle.program_id == 2u;
        let coverage_fog_program = triangle.program_id == 3u;
        var coverage = coverage_fragment_fn(
            8u, memory_count, 1u, 1u,
            select(1u, 0u, full_coverage_program),
            select(1u, 2u, full_coverage_program),
            0u, 0u, 0u,
        );
        if fog_program {
            coverage = coverage_fragment_fn(8u, memory_count, 1u, 1u, 0u, 2u, 0u, 0u, 0u);
        }
        if coverage_fog_program {
            let combined_alpha = u32(round(clamp(combined.a, 0.0, 1.0) * 255.0));
            coverage = coverage_fragment_fn(
                8u, memory_count, 0u, 1u, 1u, 0u, 1u, 1u, combined_alpha,
            );
            combined.a = f32(coverage.adjusted_alpha) / 255.0;
        }
        if select(
            coverage.wraps == 0u,
            coverage.adjusted_coverage_count == 0u,
            coverage_fog_program,
        ) {
            continue;
        }
        let destination = rgba16_color(current);
        var blended: vec4<f32>;
        if full_coverage_program {
            blended = blend_full_coverage_program(combined, destination);
        } else if fog_program {
            blended = blend_fog_program(combined, destination);
        } else if coverage_fog_program {
            blended = vec4<f32>(combined.rgb, 1.0);
        } else {
            blended = blend_fragment_memory_composite_fn(
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
        }
        current = pack_rgba16(blended, (coverage.destination_count - 1u) >> 2u);
    }
    return ColorPixelResult(current, status);
}

// One invocation owns both halfwords of a storage word. This closes the
// read-modify-write race two independent pixel invocations would have on a
// packed RGBA16 target.
@compute @workgroup_size(64)
fn compute_triangle_hot_color(@builtin(global_invocation_id) id: vec3<u32>) {
    let local_word_index = id.x;
    if local_word_index >= params.word_count {
        return;
    }
    var work_item = ColorWorkItem(local_word_index, 0u, params.triangle_count);
    if params.dispatch_words_per_row != 0xffffffffu {
        work_item = color_work_items[local_word_index];
    }
    let word_index = work_item.target_word;
    if word_index >= arrayLength(&color_target_words) {
        return;
    }
    let device_word = color_target_words[word_index];
    let first_pixel = word_index * 2u;
    let first_device = device_word & 0xffffu;
    let first_logical = ((first_device & 0xffu) << 8u) | (first_device >> 8u);
    var first_current = first_logical;
    var first_result_status = 0u;
    let first_status = local_word_index * 2u;
    let second_pixel = first_pixel + 1u;
    var second_current = 0u;
    var second_result_status = 0u;
    if second_pixel < params.pixels_per_triangle {
        let second_device = device_word >> 16u;
        second_current = ((second_device & 0xffu) << 8u) | (second_device >> 8u);
    }
    for (var event_offset = 0u; event_offset < work_item.triangle_count; event_offset += 1u) {
        let event = color_work_triangle_indices[work_item.first_triangle_index + event_offset];
        if (event & 0x80000000u) != 0u {
            var checkpoint_word = ((first_current & 0xffu) << 8u) | (first_current >> 8u);
            if second_pixel < params.pixels_per_triangle {
                let second_device = ((second_current & 0xffu) << 8u) | (second_current >> 8u);
                checkpoint_word |= second_device << 16u;
            } else {
                checkpoint_word |= device_word & 0xffff0000u;
            }
            color_status[params.first_word + (event & 0x7fffffffu)] = checkpoint_word;
            continue;
        }
        let event_index = work_item.first_triangle_index + event_offset;
        let first = execute_hot_color_pixel(first_pixel, first_current, event_index, 1u);
        first_current = first.rgba16;
        if first_result_status == 0u {
            first_result_status = first.status;
        }
        if second_pixel < params.pixels_per_triangle {
            let second = execute_hot_color_pixel(second_pixel, second_current, event_index, 1u);
            second_current = second.rgba16;
            if second_result_status == 0u {
                second_result_status = second.status;
            }
        }
    }
    color_status[first_status] = first_result_status;
    var output_word = ((first_current & 0xffu) << 8u) | (first_current >> 8u);
    if second_pixel < params.pixels_per_triangle {
        let packed_device = ((second_current & 0xffu) << 8u) | (second_current >> 8u);
        output_word |= packed_device << 16u;
        color_status[first_status + 1u] = second_result_status;
    } else {
        output_word |= device_word & 0xffff0000u;
    }
    color_target_words[word_index] = output_word;
}
