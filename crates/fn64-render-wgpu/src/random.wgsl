// initRand / nextRandUint / nextRand seam. Characterization-only; not wired
// into any draw path or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `RandomState::{init_with_backoff, next_uint,
// next_unit_float}` (`fn64-render-wgpu/src/random.rs`), itself a literal port
// of RT64's `initRand`/`nextRandUint`/`nextRand` (`Random.hlsli:10-33`,
// pinned commit 5473732a822a4423b5696e7cb18fecc425a59875). WGSL's `u32`
// arithmetic already wraps on overflow like HLSL's `uint`, so no explicit
// wrapping helper is needed here (unlike `rgb_dither.wgsl`'s `euclid_rem4`,
// which exists because WGSL's `%` truncates toward zero and Rust's does not
// -- there is no such divergence for `+`/`*`/`<<`/`>>` on unsigned types).

struct RandomInput {
    val0: u32,
    val1: u32,
    backoff: u32,
    steps: u32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<RandomInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<u32>;

fn init_rand(val0: u32, val1: u32, backoff: u32) -> u32 {
    var v0 = val0;
    var v1 = val1;
    var s0: u32 = 0u;
    for (var n: u32 = 0u; n < backoff; n = n + 1u) {
        s0 = s0 + 0x9e3779b9u;
        v0 = v0 + (((v1 << 4u) + 0xa341316cu) ^ (v1 + s0) ^ ((v1 >> 5u) + 0xc8013ea4u));
        v1 = v1 + (((v0 << 4u) + 0xad90777du) ^ (v0 + s0) ^ ((v0 >> 5u) + 0x7e95761eu));
    }
    return v0;
}

fn next_rand_uint(s: u32) -> u32 {
    return 1664525u * s + 1013904223u;
}

// Returns the advanced state's low 24 bits, matching `nextRand`'s numerator
// before the caller divides by 0x01000000 as f32. Kept as a u32 (rather than
// returning the f32 quotient directly) so the exact masked numerator is
// independently inspectable by this module's differential test, matching
// this crate's convention of exposing intermediate integer seams (see
// `rgb_dither.wgsl`'s `quantize_channel`).
fn next_rand_masked_numerator(s: u32) -> u32 {
    return next_rand_uint(s) & 0x00FFFFFFu;
}

@compute @workgroup_size(64)
fn random_advance(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&inputs)) {
        return;
    }
    let input = inputs[index];
    var state = init_rand(input.val0, input.val1, input.backoff);
    var last_masked_numerator: u32 = 0u;
    var step: u32 = 0u;
    loop {
        if (step >= input.steps) {
            break;
        }
        last_masked_numerator = next_rand_masked_numerator(state);
        state = next_rand_uint(state);
        step = step + 1u;
    }
    outputs[index] = last_masked_numerator;
}
