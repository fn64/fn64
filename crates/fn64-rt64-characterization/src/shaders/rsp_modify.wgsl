// RSPModifyCS arithmetic. Characterization-only; not wired into any draw
// path, dispatch, or bind group layout used elsewhere in this crate.
//
// Literal WGSL re-expression of `decode_modify_cs` (`crate::rt64_rsp_world_modify`,
// mirroring RT64's `src/shaders/RSPModifyCS.hlsl`, pinned commit
// 5473732a822a4423b5696e7cb18fecc425a59875, SHA-256
// 761633642c76b3e5a09f8e9077d646150291bd4d831fc21aa952d8eb6339fb6c). Reproduces
// exactly:
// - the `modifyZ = srcModifyPos[modifyOffset] & 0x1` selector bit and the
//   `vertexIndex = srcModifyPos[modifyOffset] >> 1` index field
// - the Z branch: `modifyValue / 65536.0f`, an UNSIGNED u32-to-f32 widening
//   conversion, not a signed s16.16 reinterpret (see
//   `crate::rt64_rsp_world_modify` module doc "Admitted domain")
// - the XY branch: `(int(ext) << 16) >> 16` sign-extension idiom on each
//   16-bit half, then `/ 4.0f`
//
// `[numthreads(64,1,1)]`, `SV_DispatchThreadID`, the `modifyIndex >=
// gConstants.modifyCount` dispatch guard, and the `srcModifyPos`/`screenPos`
// buffer binds/loads/stores are all deliberately NOT ported -- this file has
// no `@compute` entry point, only the plain arithmetic functions, per this
// ticket's scope statement (dispatch scaffolding excluded). No `lerp`/`mix`
// appears in `RSPModifyCS.hlsl`, so there is no HLSL-`lerp`-vs-WGSL-`mix`
// concern in this file.

// `RSP::modifyVertex`'s Z-select vs XY-select tag bit, and the vertex index
// packed into the same u32 -- see `RSPModifyCS.hlsl:19-21`.
fn rsp_modify_is_z(modify_pos_low: u32) -> bool {
    return (modify_pos_low & 0x1u) != 0u;
}

fn rsp_modify_vertex_index(modify_pos_low: u32) -> u32 {
    return modify_pos_low >> 1u;
}

// Z branch (`RSPModifyCS.hlsl:23`): unsigned u32 -> f32 widening conversion,
// then divide by 65536.0. Not a signed s16.16 reinterpret.
fn rsp_modify_z(modify_value: u32) -> f32 {
    return f32(modify_value) / 65536.0;
}

// XY branch (`RSPModifyCS.hlsl:26-30`): extract each 16-bit half, sign-extend
// via the `(i32(ext) << 16) >> 16` idiom (WGSL `>>` on a signed i32 is an
// arithmetic shift, matching HLSL's `int` right-shift), then divide by 4.0.
fn rsp_modify_xy(modify_value: u32) -> vec2<f32> {
    let ext_x = (modify_value >> 16u) & 0xFFFFu;
    let ext_y = modify_value & 0xFFFFu;
    let int_x = (i32(ext_x) << 16u) >> 16u;
    let int_y = (i32(ext_y) << 16u) >> 16u;
    return vec2<f32>(f32(int_x) / 4.0, f32(int_y) / 4.0);
}
