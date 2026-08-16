// Repository-owned M3.3c mechanism shader. Admission fixes the dispatch to
// one complete 4x2 RGBA16 fill before this pipeline can be submitted.

struct Parameters {
    rgba16: u32,
    width: u32,
    height: u32,
    _padding: u32,
}

@group(0) @binding(0)
var<uniform> parameters: Parameters;

// Two adjacent RGBA16 pixels share one storage word. Each half is byte-swapped
// so little-endian buffer bytes retain the logical/device big-endian domain.
@group(0) @binding(1)
var<storage, read_write> target_rgba16_words: array<u32>;

@compute @workgroup_size(2, 2, 1)
fn fill_rgba16(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let word_width = parameters.width >> 1u;
    if invocation.x >= word_width || invocation.y >= parameters.height {
        return;
    }

    let rgba16 = parameters.rgba16 & 0xffffu;
    let device_half = ((rgba16 & 0xffu) << 8u) | ((rgba16 >> 8u) & 0xffu);
    let output_index = invocation.y * word_width + invocation.x;
    target_rgba16_words[output_index] = device_half | (device_half << 16u);
}
