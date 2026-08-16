// EndianSwapUINT16 / EndianSwapUINT32 / EndianSwapUINT. Characterization-only;
// not wired into any draw path or bind group layout used elsewhere in this
// crate.
//
// Literal WGSL re-expression of `endian_swap_uint16`/`endian_swap_uint32`/
// `endian_swap_uint` (`fn64-render-wgpu/src/endian_swap.rs`), itself a
// literal port of RT64's `EndianSwapUINT16`/`EndianSwapUINT32`/
// `EndianSwapUINT` (`FbCommon.hlsli:9-33`, pinned commit
// 5473732a822a4423b5696e7cb18fecc425a59875). `siz` selector: 0=Bits4,
// 1=Bits8, 2=Bits16, 3=Bits32, matching `crate::state::PixelSize`'s
// declaration order; Bits4/Bits8 are no-ops.
//
// Unlike the Rust side, `siz` here is a raw `u32`, not `PixelSize` --
// WGSL has no enum type to make an out-of-range value unrepresentable, so
// `endian_swap_uint` below replicates RT64's `switch(siz)`'s explicit
// `default: return 0;` (`FbCommon.hlsli:33-34`) literally for any `siz`
// outside `0u..=3u`, rather than falling through to the Bits4/Bits8 no-op.

struct EndianSwapInput {
    value: u32,
    siz: u32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<EndianSwapInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<u32>;

fn endian_swap_uint16(i: u32) -> u32 {
    return ((i << 8u) & 0xFF00u) | ((i >> 8u) & 0xFFu);
}

fn endian_swap_uint32(i: u32) -> u32 {
    return ((i << 24u) & 0xFF000000u) | ((i << 8u) & 0xFF0000u) | ((i >> 8u) & 0xFF00u) | ((i >> 24u) & 0xFFu);
}

fn endian_swap_uint(i: u32, siz: u32) -> u32 {
    if (siz == 0u) {
        return i;
    }
    if (siz == 1u) {
        return i;
    }
    if (siz == 2u) {
        return endian_swap_uint16(i);
    }
    if (siz == 3u) {
        return endian_swap_uint32(i);
    }
    return 0u;
}

@compute @workgroup_size(64)
fn endian_swap_compute(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&inputs)) {
        return;
    }
    let input = inputs[index];
    outputs[index] = endian_swap_uint(input.value, input.siz);
}
