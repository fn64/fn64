// RT64 texture-coordinate generation seam. Characterization-only; not wired
// into any draw path or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `normalize_safe`/`compute_texture_gen`
// (`fn64-render-wgpu/src/texture_gen.rs`), itself a literal port of RT64's
// `normalizeSafe`/`computeTextureGen` (`TextureGen.hlsli:9-34`, pinned
// commit 5473732a822a4423b5696e7cb18fecc425a59875). See texture_gen.rs's
// module doc for the `mul(vector, matrix)` row-vector operand-order note
// this shader also preserves.

struct TextureGenInput {
    input_uv: vec2<f32>,
    input_normal: vec3<f32>,
    look_at_x: vec3<f32>,
    look_at_y: vec3<f32>,
    world_matrix_row0: vec4<f32>,
    world_matrix_row1: vec4<f32>,
    world_matrix_row2: vec4<f32>,
    world_matrix_row3: vec4<f32>,
    texture_gen_linear: u32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<TextureGenInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<vec2<f32>>;

fn normalize_safe(v: vec3<f32>) -> vec3<f32> {
    let l = length(v);
    if (l > 0.0) {
        return v / l;
    } else {
        return v;
    }
}

fn mul_row_vector_matrix(x: vec4<f32>, row0: vec4<f32>, row1: vec4<f32>, row2: vec4<f32>, row3: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(
        x.x * row0.x + x.y * row1.x + x.z * row2.x + x.w * row3.x,
        x.x * row0.y + x.y * row1.y + x.z * row2.y + x.w * row3.y,
        x.x * row0.z + x.y * row1.z + x.z * row2.z + x.w * row3.z,
        x.x * row0.w + x.y * row1.w + x.z * row2.w + x.w * row3.w,
    );
}

fn compute_texture_gen(input: TextureGenInput) -> vec2<f32> {
    let transformed_x = mul_row_vector_matrix(
        vec4<f32>(input.look_at_x, 0.0),
        input.world_matrix_row0, input.world_matrix_row1, input.world_matrix_row2, input.world_matrix_row3,
    );
    let transformed_y = mul_row_vector_matrix(
        vec4<f32>(input.look_at_y, 0.0),
        input.world_matrix_row0, input.world_matrix_row1, input.world_matrix_row2, input.world_matrix_row3,
    );
    let axis_x = normalize_safe(transformed_x.xyz);
    let axis_y = normalize_safe(transformed_y.xyz);

    var texgen_uv = vec2<f32>(dot(input.input_normal, axis_x), dot(input.input_normal, axis_y));
    texgen_uv = clamp(texgen_uv, vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0));

    if (input.texture_gen_linear != 0u) {
        texgen_uv = acos(-texgen_uv) * 325.94932;
    } else {
        texgen_uv += vec2<f32>(1.0, 1.0);
        texgen_uv *= 512.0;
    }

    return (input.input_uv / 65536.0) * texgen_uv;
}

@compute @workgroup_size(64)
fn compute_texture_gen_entry(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&inputs)) {
        return;
    }
    outputs[index] = compute_texture_gen(inputs[index]);
}
