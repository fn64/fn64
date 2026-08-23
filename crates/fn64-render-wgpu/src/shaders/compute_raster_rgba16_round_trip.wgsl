@group(0) @binding(0)
var<storage, read> source_words: array<u32>;

@group(0) @binding(1)
var<storage, read_write> target_words: array<u32>;

@compute @workgroup_size(64)
fn round_trip_rgba16(@builtin(global_invocation_id) id: vec3<u32>) {
    let word = id.x;
    if word < arrayLength(&source_words) {
        target_words[word] = source_words[word];
    }
}
