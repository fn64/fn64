// Repository-owned M2.5.3a shared semantic component. Inputs are already
// isolated raw values; this module performs no TMEM address or sampling work.

struct DirectTexelInput {
    format: u32,
    size: u32,
    value: u32,
    reserved_zero: u32,
}

struct DirectTexelOutput {
    status: u32,
    rgba8888_be: u32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<DirectTexelInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<DirectTexelOutput>;

const STATUS_DIRECT: u32 = 0u;
const STATUS_INDEXED_SEPARATE: u32 = 1u;
const STATUS_YUV_DEFERRED: u32 = 2u;
const STATUS_UNSUPPORTED_PAIR: u32 = 3u;
const STATUS_INVALID_INPUT: u32 = 4u;

fn pack_rgba(red: u32, green: u32, blue: u32, alpha: u32) -> u32 {
    return (red << 24u) | (green << 16u) | (blue << 8u) | alpha;
}

fn expand_five_to_eight(value: u32) -> u32 {
    return (value << 3u) | (value >> 2u);
}

fn direct(rgba8888_be: u32) -> DirectTexelOutput {
    return DirectTexelOutput(STATUS_DIRECT, rgba8888_be);
}

fn classify(input: DirectTexelInput) -> DirectTexelOutput {
    if input.reserved_zero != 0u || input.format > 4u || input.size > 3u {
        return DirectTexelOutput(STATUS_INVALID_INPUT, 0u);
    }

    // Wire selector order follows the public RDP image-format and size fields:
    // RGBA/YUV/CI/IA/I = 0..4 and 4/8/16/32-bit = 0..3.
    if input.format == 0u && input.size == 2u {
        let value = input.value;
        let red = expand_five_to_eight((value >> 11u) & 0x1fu);
        let green = expand_five_to_eight((value >> 6u) & 0x1fu);
        let blue = expand_five_to_eight((value >> 1u) & 0x1fu);
        let alpha = select(0u, 0xffu, (value & 1u) != 0u);
        return direct(pack_rgba(red, green, blue, alpha));
    }
    if input.format == 0u && input.size == 3u {
        return direct(input.value);
    }
    if input.format == 3u && input.size == 0u {
        let intensity_bits = input.value & 0xeu;
        let intensity = (intensity_bits << 4u) | (intensity_bits << 1u) | (intensity_bits >> 2u);
        let alpha = select(0u, 0xffu, (input.value & 1u) != 0u);
        return direct(pack_rgba(intensity, intensity, intensity, alpha));
    }
    if input.format == 3u && input.size == 1u {
        let intensity_nibble = (input.value >> 4u) & 0xfu;
        let alpha_nibble = input.value & 0xfu;
        let intensity = intensity_nibble | (intensity_nibble << 4u);
        let alpha = alpha_nibble | (alpha_nibble << 4u);
        return direct(pack_rgba(intensity, intensity, intensity, alpha));
    }
    if input.format == 3u && input.size == 2u {
        let intensity = (input.value >> 8u) & 0xffu;
        let alpha = input.value & 0xffu;
        return direct(pack_rgba(intensity, intensity, intensity, alpha));
    }
    if input.format == 4u && input.size == 0u {
        let intensity = (input.value & 0xfu) * 0x11u;
        return direct(pack_rgba(intensity, intensity, intensity, intensity));
    }
    if input.format == 4u && input.size == 1u {
        let intensity = input.value & 0xffu;
        return direct(pack_rgba(intensity, intensity, intensity, intensity));
    }
    if input.format == 2u {
        return DirectTexelOutput(STATUS_INDEXED_SEPARATE, 0u);
    }
    if input.format == 1u {
        return DirectTexelOutput(STATUS_YUV_DEFERRED, 0u);
    }
    return DirectTexelOutput(STATUS_UNSUPPORTED_PAIR, 0u);
}

@compute @workgroup_size(64, 1, 1)
fn decode_direct_texels(@builtin(global_invocation_id) invocation: vec3<u32>) {
    if invocation.x >= arrayLength(&inputs) {
        return;
    }
    outputs[invocation.x] = classify(inputs[invocation.x]);
}
