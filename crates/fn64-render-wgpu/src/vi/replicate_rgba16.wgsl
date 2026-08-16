// Repository-owned M3.3d mechanism shader. The CPU validator fixes this to
// one 4x2, 1:1 progressive/replicate dispatch before a GPU owner may submit it.

struct Parameters {
    width: u32,
    height: u32,
    source_stride_pixels: u32,
    _padding: u32,
}

@group(0) @binding(0)
var<uniform> parameters: Parameters;

// The binding consumes the exact device-order RGBA16 bytes as packed u32
// words. Byte swapping happens here rather than creating a second upload.
@group(0) @binding(1)
var<storage, read> source_rgba16_words: array<u32>;

// Each u32 is packed so a little-endian buffer readback yields BGRA8 bytes.
@group(0) @binding(2)
var<storage, read_write> output_bgra8: array<u32>;

fn expand_five_to_eight(value: u32) -> u32 {
    return (value << 3u) | (value >> 2u);
}

@compute @workgroup_size(4, 2, 1)
fn replicate_rgba16(@builtin(global_invocation_id) invocation: vec3<u32>) {
    if invocation.x >= parameters.width || invocation.y >= parameters.height {
        return;
    }

    let source_index = invocation.y * parameters.source_stride_pixels + invocation.x;
    let packed = source_rgba16_words[source_index >> 1u];
    let packed_shift = (source_index & 1u) * 16u;
    let native_half = (packed >> packed_shift) & 0xffffu;
    let rgba16 = ((native_half & 0xffu) << 8u) | ((native_half >> 8u) & 0xffu);
    let red = expand_five_to_eight((rgba16 >> 11u) & 0x1fu);
    let green = expand_five_to_eight((rgba16 >> 6u) & 0x1fu);
    let blue = expand_five_to_eight((rgba16 >> 1u) & 0x1fu);
    let alpha = select(0u, 0xffu, (rgba16 & 1u) != 0u);

    let output_index = invocation.y * parameters.width + invocation.x;
    output_bgra8[output_index] = blue | (green << 8u) | (red << 16u) | (alpha << 24u);
}
