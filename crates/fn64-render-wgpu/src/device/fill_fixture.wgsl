struct FillParameters {
    rgba_le: u32,
};

@group(0) @binding(0) var<uniform> parameters: FillParameters;
@group(0) @binding(1) var<storage, read_write> output_words: array<u32, 4>;

@compute @workgroup_size(4)
fn fill(@builtin(global_invocation_id) id: vec3<u32>) {
    output_words[id.x] = parameters.rgba_le;
}

