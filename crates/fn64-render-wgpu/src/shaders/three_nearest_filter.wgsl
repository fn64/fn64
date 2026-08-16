// Repository-owned M2.5.3b shared semantic component. Inputs are four
// caller-supplied RGBA8888 corners and 5-bit S/T fractions; this module
// performs no TMEM address, tile, or coordinate work.

struct ThreeNearestFilterInput {
    // Corner naming matches fn64-render-reference's own
    // filter_three_nearest_s10_5 parameter order, NOT CommittedTextureCell's
    // [UL, LL, UR, LR] order: c00 = upper-left, c10 = upper-right,
    // c01 = lower-left, c11 = lower-right. A future caller passing a
    // CommittedTextureCell must remap corners by name before filling this
    // struct; this component owns only the formula, not the remap.
    c00: u32,
    c10: u32,
    c01: u32,
    c11: u32,
    // 5-bit S/T fractions, required in 0..32. Packed as u32 to match this
    // crate's existing wire convention of u32-per-field storage inputs.
    sf: u32,
    tf: u32,
    reserved_zero: u32,
    reserved_zero_2: u32,
}

struct ThreeNearestFilterOutput {
    status: u32,
    rgba8888_be: u32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<ThreeNearestFilterInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<ThreeNearestFilterOutput>;

const STATUS_OK: u32 = 0u;
const STATUS_INVALID_FRACTION: u32 = 1u;
const SCALE: i32 = 32;

fn channel(corner: u32, shift: u32) -> i32 {
    return i32((corner >> shift) & 0xffu);
}

// Bound: each channel byte is in [0, 255], SCALE = 32, sf/tf in [0, 32). The
// c*32 term is at most 255*32 = 8160; each sf/tf cross term is at most
// 31*255 = 7905. Total magnitude is at most 8160 + 7905 + 7905 = 23970,
// symmetric for the upper branch -- i32 (i32::MAX = 2147483647) is exact and
// sufficient. No i64/vec2<u32> emulation is needed here.
//
// value = c00*(32-sf-tf) + sf*c10 + tf*c01 (lower branch, sf+tf<=32) is a sum
// of non-negative terms since c00,c10,c01 >= 0, so value is always
// non-negative for valid byte-range corners and in-range fractions; the
// symmetric substitution holds for the upper branch. WGSL's truncating `/`
// on i32 therefore never diverges from the reference's truncating i64 `/` at
// this magnitude -- both truncate identically on non-negative operands.
fn filter_channel(c00: i32, c10: i32, c01: i32, c11: i32, sf: i32, tf: i32) -> i32 {
    var value: i32;
    if sf + tf <= SCALE {
        value = c00 * SCALE + sf * (c10 - c00) + tf * (c01 - c00);
    } else {
        value = c11 * SCALE + (SCALE - sf) * (c01 - c11) + (SCALE - tf) * (c10 - c11);
    }
    return clamp((value + SCALE / 2) / SCALE, 0, 255);
}

fn filter_corners(input: ThreeNearestFilterInput) -> ThreeNearestFilterOutput {
    if input.sf >= 32u || input.tf >= 32u {
        return ThreeNearestFilterOutput(STATUS_INVALID_FRACTION, 0u);
    }

    let sf = i32(input.sf);
    let tf = i32(input.tf);

    let red = filter_channel(
        channel(input.c00, 24u), channel(input.c10, 24u),
        channel(input.c01, 24u), channel(input.c11, 24u), sf, tf,
    );
    let green = filter_channel(
        channel(input.c00, 16u), channel(input.c10, 16u),
        channel(input.c01, 16u), channel(input.c11, 16u), sf, tf,
    );
    let blue = filter_channel(
        channel(input.c00, 8u), channel(input.c10, 8u),
        channel(input.c01, 8u), channel(input.c11, 8u), sf, tf,
    );
    let alpha = filter_channel(
        channel(input.c00, 0u), channel(input.c10, 0u),
        channel(input.c01, 0u), channel(input.c11, 0u), sf, tf,
    );

    let rgba8888_be = (u32(red) << 24u) | (u32(green) << 16u) | (u32(blue) << 8u) | u32(alpha);
    return ThreeNearestFilterOutput(STATUS_OK, rgba8888_be);
}

@compute @workgroup_size(64, 1, 1)
fn filter_three_nearest(@builtin(global_invocation_id) invocation: vec3<u32>) {
    if invocation.x >= arrayLength(&inputs) {
        return;
    }
    outputs[invocation.x] = filter_corners(inputs[invocation.x]);
}
