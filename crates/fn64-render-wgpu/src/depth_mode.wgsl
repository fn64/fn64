// Full RDP depth-mode compare/update seam. Characterization-only; not wired
// into any draw path or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `depth_mode.rs`'s `relations`/`mode_passes`/
// `depth_mode_decision` (themselves literal ports of
// `fn64-render-reference`'s `depth::{relations, mode_passes}` and
// `raster::coverage::depth_coverage_decision`). All four ZMODE_* variants
// are named explicitly; there is no default/fallthrough branch, matching
// the Rust side's exhaustive match. Interpenetrating x coverage_wraps is a
// named unsupported sentinel, not a silent pass or reject -- Programming
// Manual "Blender Modes and Assumptions" requires a coverage-adjustment
// path here it does not publish the arithmetic for.

const MODE_OPAQUE: u32 = 0u;
const MODE_INTERPENETRATING: u32 = 1u;
const MODE_TRANSLUCENT: u32 = 2u;
const MODE_DECAL: u32 = 3u;

const DECISION_REJECT: u32 = 0u;
const DECISION_PASS: u32 = 1u;
const DECISION_UNSUPPORTED_INTERPENETRATING: u32 = 2u;

struct DepthRelations {
    memory_is_max: u32,
    farther: u32,
    nearer: u32,
    in_front: u32,
}

struct DepthModeInput {
    pixel_z: u32,
    pixel_delta_z: u32,
    memory_z: u32,
    // Stored four-bit DeltaZ exponent (matches `EncodedDepth`'s packed
    // form), not an already-decoded delta -- decoded via decode_delta_z()
    // below, mirroring the Rust side's asymmetric relations() signature.
    memory_encoded_delta_z: u32,
    mode: u32,
    coverage_wraps: u32,
}

struct DepthModeOutput {
    decision: u32,
    reserved: u32,
}

@group(0) @binding(0)
var<storage, read> inputs: array<DepthModeInput>;

@group(0) @binding(1)
var<storage, read_write> outputs: array<DepthModeOutput>;

// decode_delta_z(): expand a stored four-bit DeltaZ exponent to its
// power-of-two floor, clamped to the four-bit maximum (exponent 15).
fn decode_delta_z(encoded_delta: u32) -> u32 {
    return 1u << min(encoded_delta, 15u);
}

fn compute_relations(pixel_z: u32, pixel_delta_z: u32, memory_z: u32, memory_encoded_delta_z: u32) -> DepthRelations {
    let memory_delta_z: u32 = decode_delta_z(memory_encoded_delta_z);
    var delta_z_max: u32 = pixel_delta_z;
    if (memory_delta_z > delta_z_max) {
        delta_z_max = memory_delta_z;
    }
    var relations: DepthRelations;
    relations.memory_is_max = select(0u, 1u, memory_z >= 0x3ffffu);
    // saturating_add: matches Rust's `u32::saturating_add` unconditionally,
    // including inputs near u32::MAX where a plain WGSL `+` would wrap --
    // this is not restricted to the RDP's documented 18-bit Z range, which
    // is a caller convention, not an enforced input bound.
    var raised: u32 = 0xffffffffu;
    if (pixel_z <= 0xffffffffu - delta_z_max) {
        raised = pixel_z + delta_z_max;
    }
    relations.farther = select(0u, 1u, raised >= memory_z);
    // saturating_sub: WGSL u32 subtraction wraps on underflow, so this must
    // clamp explicitly to match Rust's `saturating_sub` unconditionally.
    var lowered: u32 = 0u;
    if (pixel_z > delta_z_max) {
        lowered = pixel_z - delta_z_max;
    }
    relations.nearer = select(0u, 1u, lowered <= memory_z);
    relations.in_front = select(0u, 1u, pixel_z < memory_z);
    return relations;
}

fn mode_passes(mode: u32, relations: DepthRelations) -> bool {
    if (mode == MODE_OPAQUE || mode == MODE_INTERPENETRATING) {
        return relations.nearer == 1u;
    }
    if (mode == MODE_TRANSLUCENT) {
        return relations.in_front == 1u;
    }
    // MODE_DECAL
    return relations.farther == 1u && relations.nearer == 1u && relations.memory_is_max == 0u;
}

fn depth_mode_decision(mode: u32, relations: DepthRelations, coverage_wraps: u32) -> u32 {
    if (mode == MODE_INTERPENETRATING && coverage_wraps == 1u) {
        return DECISION_UNSUPPORTED_INTERPENETRATING;
    }
    var passes: bool;
    if (mode == MODE_OPAQUE && coverage_wraps == 1u) {
        passes = relations.in_front == 1u;
    } else {
        passes = mode_passes(mode, relations);
    }
    return select(DECISION_REJECT, DECISION_PASS, passes);
}

fn evaluate(input: DepthModeInput) -> DepthModeOutput {
    let relations = compute_relations(
        input.pixel_z,
        input.pixel_delta_z,
        input.memory_z,
        input.memory_encoded_delta_z,
    );
    var out: DepthModeOutput;
    out.decision = depth_mode_decision(input.mode, relations, input.coverage_wraps);
    out.reserved = 0u;
    return out;
}

@compute @workgroup_size(64)
fn depth_mode_decision_batch(@builtin(global_invocation_id) global_id: vec3<u32>) {
    let index = global_id.x;
    if (index >= arrayLength(&inputs)) {
        return;
    }
    outputs[index] = evaluate(inputs[index]);
}
