// Fragment-callable port of `tmem/read.rs` + `tmem/sample.rs`'s committed
// physical-TMEM addressing/filter chain (published committed-TMEM textured
// draw card §2/§3, Option B -- MANDATORY, not the rejected Option A
// CPU-resolve-then-textureSample/textureLoad design). Every function in this
// file runs per-fragment against real interpolated UVs, taking the raw
// committed TMEM byte image plus its validity bitmap as a read-only storage
// buffer (`tmem/gpu_projection.rs`'s `TmemGpuProjection`), never a
// pre-resolved image sampled through wgpu's own hardware texture units.
//
// This file is a library of ordinary `fn`s (no `@group`/`@binding`, no entry
// point), concatenated ahead of `triangle_pipeline_fragment.wgsl` at the
// Rust build seam (mirroring `color_combiner.wgsl`'s own established
// pattern) so `fs_main` can call it directly.
//
// Scope: two arms, selected by `tlut_en` BEFORE any format dispatch,
// matching the hardware order.
//
//  * **TLUT enabled** (`tmem_tile_binding.lut_mode != TMEM_TLUT_MODE_DISABLED`):
//    the tile format is IGNORED and every 4/8/16-bit texel indexes the
//    palette. n64brew `Reality_Display_Processor/Pipeline` (CC BY-SA 4.0):
//    "If tlut_en is set in othermodes the final texel will be sourced from a
//    palette and the tile format is ignored, for tiles that indicate a
//    4-bit texel size the TMEM address for the palette is indicated in the
//    tile's palette field, the tile size is otherwise ignored." RT64's
//    `sampleTMEM` (pinned `5473732a`, `src/shaders/TextureDecoder.hlsli`
//    :149-208) branches on `usesTlut` before any format dispatch and never
//    reads `fmt` inside that arm. This module mirrors
//    `tmem/read.rs`'s `read_texel` + `tmem/texel.rs`'s
//    `resolve_indexed_texel`/`decode_tlut_entry` for that arm, byte-for-byte
//    with the CPU reader as fixed at `4c412a96`. 32-bit under an enabled
//    TLUT stays REFUSED, matching that same CPU fix, which deliberately did
//    not widen there (the index byte would have to be re-derived against the
//    RGBA32 low/high bank split, and no title in this corpus reaches it).
//    `validate_address_scope`'s canonical low-half requirement is mirrored
//    here too (`TMEM_SAMPLE_STATUS_CI_SOURCE_OUTSIDE_LOW_HALF`); it was
//    missing until a CPU-vs-GPU sweep found it, and its absence made the
//    shader silently sample palette bytes as indexes where the CPU reader
//    aborted by name.
//
//    **That mirroring makes the two lanes AGREE; it does not settle whether
//    the rule itself is right.** `docs/RT64-LANE-DIVERGENCES.md` D14 scores
//    this same refusal against `fn64-render-reference`, which imposes a
//    low-half constraint only on the genuine split-bank formats (RGBA32,
//    YUV) and addresses 4/8/16-bit CI across all 4 KiB. D14's verdict is
//    "REFERENCE on the divergence; UNKNOWN on hardware" -- this crate's own
//    justification is a self-citation to M4.3.3b, not a measurement. So if
//    D14 is resolved in the reference's favour, the correct repair is to
//    drop the rule from BOTH lanes together, not to re-open the gap between
//    them. Until then the two lanes at least fail the same way instead of
//    one aborting while the other paints palette bytes as a picture.
//
//    Two known differences from the CPU reader remain on this arm, both
//    pinned by tests in `targets/triangle_pipeline/tests.rs` rather than
//    resolved here:
//
//      * At 16 bits the CPU reads BOTH bytes of the big-endian texel and
//        then discards the low one, so it refuses when the low byte is
//        invalid; this shader reads only the high byte. Public documentation
//        does not say whether a partially-loaded 16-bit texel's palette index
//        is well-defined, so neither behaviour is corrected on the other's
//        authority. See
//        `a_sixteen_bit_tlut_texel_with_an_invalid_low_byte_splits_the_two_lanes`.
//      * The CPU refuses a palette selector wider than four bits
//        (`Ci4Palette::try_new`); this shader masks with `& 0x0f`. Latent --
//        the reference `SetTile` decode narrows the field -- but
//        `TileDescriptor::from_neutral_parts` is public and does not. See
//        `an_out_of_range_palette_is_refused_by_the_cpu_and_masked_by_the_shader`.
//  * **TLUT disabled**: RGBA16 direct-format texels only (`ImageFormat::Rgba`,
//    `PixelSize::Bits16`). RGBA32, IA4/IA8/IA16, I4/I8, and YUV direct
//    decodes are still not ported here; a disabled-TLUT footprint requiring
//    any of them reports `TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT`.
//
// First-row parity is DERIVED PER TILE from the bound tile's own `low_t`,
// never frozen -- see `tmem_first_row_parity_odd` below, which carries the
// same rule `targets/texrect.rs` applies on the CPU side.
//
// GPU-side validity-sentinel encoding (card §6, matching
// `tmem/gpu_projection.rs`'s own doc verbatim): a parallel bitmap, one bit
// per TMEM byte address, packed 32 bits per `u32` word. `tmem_byte_valid`
// below is the sole reader of that bitmap.
//
// Direct texel decode formulas: MIT RT64 Rust-port pinned commit
// `5473732a822a4423b5696e7cb18fecc425a59875`, `src/shaders/Formats.hlsli`
// (`expand_five_to_eight`/RGBA16 decode, ported byte-for-byte from this
// crate's own already-Naga-validated `shaders/direct_texel_decode.wgsl`).
// Physical TMEM addressing (odd-row XOR4, tile-relative linear byte
// address): public SGI Nintendo 64 RDP Command Summary Tables 1, 3, 6-10 and
// Programming Manual §13.9, ported from `tmem/read.rs`. Point/cell
// addressing (S10.5 clamp/wrap/mirror/mask, tile-shift): Programming Manual
// "TF: Texture Filter"/"Sampling Overview" §13.7, ported from
// `tmem/sample.rs`'s `address_texture_cell`. Three-nearest bilerp: ported
// from `fn64-render-reference/src/gbi/types.rs:954-972` via
// `tmem/sample.rs`'s `filter_three_nearest_committed_cell`, itself already
// transcribed to WGSL once before in this crate's own
// `shaders/three_nearest_filter.wgsl` (reused here as the same formula, not
// imported -- WGSL has no cross-module include mechanism reachable in this
// wgpu version, matching every other twin in this crate).

const TMEM_BYTES: u32 = 4096u;
const TMEM_VALIDITY_WORDS: u32 = 128u;
const TMEM_ADDRESS_MASK: u32 = 0x0fffu;

// **First-row parity is derived per tile, never frozen.**
// `tmem/read.rs`'s `TmemFirstRowParity` is explicit caller input -- the
// reader never infers it -- so this shader owes the reader the same parity
// the *writer* used, exactly as `targets/texrect.rs`'s
// `execute_scheduled_texrect` does on the CPU side (fixed at `aa6f644e`).
// The writer's rule is `tmem/types.rs`'s `project_tmem_transfer_word`,
// `TmemLoadKind::Tile` arm: `odd_row_exchange = (bounds.low_t().integer()
// + row) & 1`. The reader's rule is `tmem/read.rs`'s `odd_row_exchange`:
// `first_is_odd ^ (row & 1)`. The two agree exactly when `first_is_odd ==
// low_t.integer() & 1`, and `tmem_first_row_parity_odd` below is that
// equality -- the SAME one-line expression `targets/texrect.rs:1237` uses,
// over the SAME `low_t` this tile binding already uploads.
//
// A frozen `false` ("first row even") was previously used here. That is
// correct only for a tile whose T origin is even. Measured on the real ROM,
// WM2000's sprite-strip tile has `low_t.integer() == 47`, an ODD origin, so
// the frozen constant inverted the exchange for every row and each
// rectangle row's last texel addressed a byte the load never wrote --
// surfacing as `TMEM_SAMPLE_STATUS_INVALID_BYTE` on the GPU triangle path
// while the CPU texel reader, which had already been fixed, sampled the
// same tile cleanly.
//
// The low-half TLUT guard below depends on this too: its own doc requires
// the FULLY addressed byte, "post odd-row XOR4 exchange", so a frozen
// parity would have had it testing the wrong address as well.
//
// `TileCoordinate::integer()` is `raw >> 2` (S10.2), so the parity bit is
// bit 2 of the raw field.
fn tmem_first_row_parity_odd(tile: TileBindingParams) -> bool {
    return ((tile.low_t >> 2u) & 1u) != 0u;
}

const TEXEL_FRACTION_BITS: i32 = 5;
const TEXEL_FRACTION_SCALE: i32 = 32; // 1 << TEXEL_FRACTION_BITS
const TEXEL_FRACTION_HALF_SCALE: i32 = 16;
const TILE_TO_TEXEL_FRACTION_SCALE: i32 = 8; // TEXEL_FRACTION_SCALE / 4

// `ImageFormat`/`PixelSize` wire codes, matching `shader_manifest.rs`'s
// existing `format_code`/`size_code` test-side convention exactly
// (`ImageFormat::Rgba` = 0, `PixelSize::Bits16` = 2) -- reused here as the
// host-side encoding this slice's tile-binding upload writes, so the
// fragment shader can loudly reject any tile whose bound format/size is not
// the one production format this slice ports (card §6/production-format
// rejection: "loudly reject non-RGBA/Bits16", never silently sample as if
// it were RGBA16).
const TMEM_IMAGE_FORMAT_RGBA: u32 = 0u;
const TMEM_PIXEL_SIZE_BITS4: u32 = 0u;
const TMEM_PIXEL_SIZE_BITS8: u32 = 1u;
const TMEM_PIXEL_SIZE_BITS16: u32 = 2u;
const TMEM_PIXEL_SIZE_BITS32: u32 = 3u;

// `TextureLutMode` wire codes, mirroring `tmem/gpu_projection.rs`'s
// `TLUT_MODE_*`. The reserved encoding never reaches this shader --
// `OtherMode::texture_lut_mode()` refuses it by name host-side.
const TMEM_TLUT_MODE_DISABLED: u32 = 0u;
const TMEM_TLUT_MODE_RGBA16: u32 = 2u;
const TMEM_TLUT_MODE_IA16: u32 = 3u;

// Physical byte address of TLUT entry 0. `tmem/texel.rs`'s
// `resolve_indexed_texel` computes `0x0800 + index * 8` -- each entry is
// quadricated into four identical big-endian 16-bit lanes across eight
// bytes, and this module requires all four (see
// `tmem_read_canonical_tlut_entry`), exactly as
// `tmem/read.rs`'s `read_canonical_tlut_entry` does.
const TMEM_TLUT_BASE: u32 = 0x0800u;
const TMEM_TLUT_ENTRY_STRIDE: u32 = 8u;

struct TileBindingParams {
    // `SetTile` fields (`TileDescriptor`, `tmem/types.rs`), each already
    // narrowed to the exact bit width its own public RDP field carries.
    tmem_word_address: u32,
    line_words: u32,
    mask_s: u32,
    shift_s: u32,
    mode_s_mirror: u32,
    mode_s_clamp: u32,
    mask_t: u32,
    shift_t: u32,
    mode_t_mirror: u32,
    mode_t_clamp: u32,
    // `SetTileSize` fields (`TileSize`), raw S10.2 fixed-point.
    low_s: u32,
    low_t: u32,
    high_s: u32,
    high_t: u32,
    // 0 = no binding was snapshotted for this triangle's tile (card §6:
    // "missing `TileDescriptor`" must surface as a named error, never a
    // silent default); 1 = a real binding is present.
    bound: u32,
    // `TileDescriptor::format()`/`::size()` wire codes (see the
    // `TMEM_IMAGE_FORMAT_*`/`TMEM_PIXEL_SIZE_*` constants above). `format`
    // is read ONLY on the TLUT-disabled arm, where the direct decode this
    // module ports is RGBA16 alone and any other pair is a named rejection
    // rather than a silent fallback to treating the bytes as RGBA16. Under
    // an enabled TLUT `format` is not read at all -- see the module header.
    format: u32,
    pixel_size: u32,
    // `SetTile`'s four-bit palette selector. Read only on the 4-bit
    // enabled-TLUT path, where it supplies the palette's own TMEM address
    // (n64brew, quoted in this file's header).
    palette: u32,
    // `OtherMode::texture_lut_mode()`'s wire code (`TMEM_TLUT_MODE_*`).
    // Consulted BEFORE `format`, because `tlut_en` is a pipeline mode and
    // the tile format is ignored while it is set.
    lut_mode: u32,
    reserved_zero: u32,
}

// Matches `PhysicalTexelReadError::InvalidTexelByte` / a missing tile
// binding / an out-of-bounds address / an unsupported production format --
// card §6's failure-semantics list. `STATUS_OK` mirrors
// `direct_texel_decode.wgsl`/`three_nearest_filter.wgsl`'s own `STATUS_*`
// convention. `NO_TILE_BINDING` and `REVERSED_EXTENT` are deliberately
// distinct (card audit repair: "distinct reversed-extent and no-tile
// statuses") -- a missing `TileDescriptor` snapshot and a clamped axis whose
// `high < low` (`PointAddressError::ReversedClampExtent` on the CPU oracle)
// are different failure causes and must not collapse into one status code.
const TMEM_SAMPLE_STATUS_OK: u32 = 0u;
const TMEM_SAMPLE_STATUS_NO_TILE_BINDING: u32 = 1u;
const TMEM_SAMPLE_STATUS_INVALID_BYTE: u32 = 2u;
const TMEM_SAMPLE_STATUS_REVERSED_EXTENT: u32 = 3u;
const TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT: u32 = 4u;
// The enabled-TLUT canonical-source refusal, mirroring `tmem/read.rs`'s
// `validate_address_scope` -> `PhysicalTexelReadError::
// EnabledCiSourceOutsideLowHalf`. Under `tlut_en` the palette occupies the
// high 2 KiB of TMEM from `TMEM_TLUT_BASE`, so a tile whose index source
// lands at or above that address would be reading the palette as image
// data. Deliberately NOT folded into `INVALID_BYTE`: the bytes at `0x0800`
// are usually perfectly valid TLUT bytes, so a validity failure would never
// fire and the wrong color would be painted silently -- which is exactly
// what this shader did before this status existed.
const TMEM_SAMPLE_STATUS_CI_SOURCE_OUTSIDE_LOW_HALF: u32 = 5u;

struct TmemSampleResult {
    status: u32,
    color: vec4<f32>,
}

// Module-scope storage bindings for the committed TMEM byte image and its
// validity bitmap (`tmem/gpu_projection.rs`'s `TmemGpuProjection`, packed
// four bytes per `u32` word for the byte buffer -- see `tmem_read_byte`'s
// own doc -- and one bit per address for the validity bitmap, matching
// `gpu_projection.rs`'s doc verbatim). WGSL function parameters cannot carry
// a `ptr<storage, ...>` in this crate's pinned wgpu/Naga version (only
// module-scope `var<storage>` bindings and pointers to `function`/`private`
// address spaces are accepted as parameters), so every sampling function
// below reads these two module-scope bindings directly rather than
// threading them through as arguments -- the same shape this crate's own
// `direct_texel_decode.wgsl`/`three_nearest_filter.wgsl` twins already use
// for their own `@group`/`@binding` storage arrays.
@group(0) @binding(2)
var<storage, read> tmem_bytes: array<u32, 1024>;
@group(0) @binding(3)
var<storage, read> tmem_validity_words: array<u32, 128>;
@group(0) @binding(4)
var<uniform> tmem_tile_binding: TileBindingParams;

fn tmem_byte_valid(address: u32) -> bool {
    if address >= TMEM_BYTES {
        return false;
    }
    let word = tmem_validity_words[address / 32u];
    return (word & (1u << (address % 32u))) != 0u;
}

fn tmem_read_byte(address: u32, out_ok: ptr<function, bool>) -> u32 {
    if !tmem_byte_valid(address) {
        *out_ok = false;
        return 0u;
    }
    // Four bytes packed per `u32` word, byte 0 in the low 8 bits (matches
    // the CPU-side projection's `bytes[address]` -> `validity_words[address
    // / 32]` indexing convention; `tmem_gpu_projection.rs` packs the byte
    // buffer separately from the validity bitmap, so this function's own
    // packing choice for the byte buffer -- little-endian 4-per-word -- is
    // this WGSL module's own upload-layout decision, matching this crate's
    // existing `array<u32>` storage-buffer convention for packed byte data.
    let word = tmem_bytes[address / 4u];
    let shift = (address % 4u) * 8u;
    *out_ok = true;
    return (word >> shift) & 0xffu;
}

fn expand_five_to_eight(value: u32) -> u32 {
    return (value << 3u) | (value >> 2u);
}

// `decode_rgba16`, `tmem/texel.rs` -- RGBA5551: 5 bits each R/G/B expanded to
// 8 bits, low bit is a 1-bit alpha flag expanded to 0x00/0xff.
fn decode_rgba16(raw_be: u32) -> vec4<f32> {
    let r5 = (raw_be >> 11u) & 0x1fu;
    let g5 = (raw_be >> 6u) & 0x1fu;
    let b5 = (raw_be >> 1u) & 0x1fu;
    let a = raw_be & 1u;
    let r = expand_five_to_eight(r5);
    let g = expand_five_to_eight(g5);
    let b = expand_five_to_eight(b5);
    let alpha = select(0u, 0xffu, a != 0u);
    return vec4<f32>(f32(r), f32(g), f32(b), f32(alpha)) / 255.0;
}

// `linear_byte_address`/`odd_row_exchange` (`tmem/read.rs`), specialized to
// `PixelSize::Bits16` (`bytes_per_texel = 2`) -- the only size this module
// ports. `column`/`row` are already-addressed integer texel coordinates
// (`AddressedTmemTexel`, post clamp/wrap/mirror/mask). Returns the
// UNMASKED, UNEXCHANGED linear base address for this texel's first byte --
// `tmem_rgba16_byte_address` below applies the `TMEM_ADDRESS_MASK`
// wrap and the odd-row XOR4 exchange independently to each of the two
// bytes this texel spans, exactly matching `tmem/read.rs`'s
// `read_linear_bytes`, which computes `(linear + offset) & MASK` then
// conditionally XORs *per byte offset* -- not once on a shared base and
// then blindly added, which would disagree with the CPU oracle exactly at
// the 0xfff wrap boundary (`base` and `base + 1` can straddle it).
fn tmem_rgba16_linear_base(
    tile: TileBindingParams,
    column: u32,
    row: u32,
) -> u32 {
    return tile.tmem_word_address * 8u
        + row * tile.line_words * 8u
        + column * 2u;
}

// Masks and, if this row's parity triggers the odd-row exchange, XORs ONE
// already-offset linear address -- called independently per byte (see
// `tmem_rgba16_linear_base`'s doc above).
fn tmem_rgba16_byte_address(tile: TileBindingParams, linear: u32, row: u32) -> u32 {
    let address = linear & TMEM_ADDRESS_MASK;
    let first_is_odd = tmem_first_row_parity_odd(tile);
    let row_is_odd = (row & 1u) != 0u;
    let exchange = first_is_odd != row_is_odd;
    if exchange {
        return address ^ 4u;
    }
    return address;
}

fn tmem_sample_rgba16_texel(
    tile: TileBindingParams,
    column: u32,
    row: u32,
    out_ok: ptr<function, bool>,
) -> vec4<f32> {
    let linear = tmem_rgba16_linear_base(tile, column, row);
    let hi_address = tmem_rgba16_byte_address(tile, linear, row);
    let lo_address = tmem_rgba16_byte_address(tile, linear + 1u, row);
    var ok_hi = false;
    var ok_lo = false;
    let hi = tmem_read_byte(hi_address, &ok_hi);
    let lo = tmem_read_byte(lo_address, &ok_lo);
    if !ok_hi || !ok_lo {
        *out_ok = false;
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    *out_ok = true;
    let raw_be = (hi << 8u) | lo;
    return decode_rgba16(raw_be);
}

// `decode_ia16`, `tmem/texel.rs` (`IA16ToFloat4`, Formats.hlsli:108-112).
// High byte is intensity, low byte is alpha; both already 8 bits,
// big-endian packed. The second of the two TLUT entry formats.
fn decode_ia16(raw_be: u32) -> vec4<f32> {
    let i = (raw_be >> 8u) & 0xffu;
    let a = raw_be & 0xffu;
    return vec4<f32>(f32(i), f32(i), f32(i), f32(a)) / 255.0;
}

// `linear_byte_address` (`tmem/read.rs`) generalized over pixel size, in
// the same three cases the CPU reader spells out: a 4-bit texel advances
// half a byte per column (`column / 2`), an 8-bit texel one byte, and a
// 16-bit texel two. Returns the UNMASKED, UNEXCHANGED linear address of the
// texel's first byte, for the same reason `tmem_rgba16_linear_base` does --
// the mask and the XOR4 exchange are applied per byte by
// `tmem_rgba16_byte_address`, never once on a shared base.
//
// 32-bit is absent deliberately: it needs the RGBA32 low/high bank split
// (`rgba32_low_address`), and both arms of this module refuse 32-bit before
// any addressing runs.
fn tmem_indexed_linear_base(
    tile: TileBindingParams,
    column: u32,
    row: u32,
) -> u32 {
    var column_offset: u32;
    if tile.pixel_size == TMEM_PIXEL_SIZE_BITS4 {
        column_offset = column / 2u;
    } else if tile.pixel_size == TMEM_PIXEL_SIZE_BITS8 {
        column_offset = column;
    } else {
        column_offset = column * 2u;
    }
    return tile.tmem_word_address * 8u
        + row * tile.line_words * 8u
        + column_offset;
}

// `read_canonical_tlut_entry` (`tmem/read.rs`): the eight bytes at
// `0x800 + index * 8` are four identical big-endian 16-bit lanes. All eight
// must be valid, and all four lanes must agree -- both are the CPU
// reader's own requirements (`IncompleteTlutEntry`/`NonCanonicalTlutEntry`),
// carried here as `out_ok = false` rather than dropped, since a
// disagreeing entry means the TLUT was never fully loaded and inventing a
// lane would silently paint the wrong palette color.
fn tmem_read_canonical_tlut_entry(index: u32, out_ok: ptr<function, bool>) -> u32 {
    let base = TMEM_TLUT_BASE + index * TMEM_TLUT_ENTRY_STRIDE;
    var lane0 = 0u;
    for (var lane = 0u; lane < 4u; lane = lane + 1u) {
        var ok_hi = false;
        var ok_lo = false;
        let hi = tmem_read_byte(base + lane * 2u, &ok_hi);
        let lo = tmem_read_byte(base + lane * 2u + 1u, &ok_lo);
        if !ok_hi || !ok_lo {
            *out_ok = false;
            return 0u;
        }
        let value = (hi << 8u) | lo;
        if lane == 0u {
            lane0 = value;
        } else if value != lane0 {
            *out_ok = false;
            return 0u;
        }
    }
    *out_ok = true;
    return lane0;
}

// The enabled-TLUT texel path, mirroring `tmem/read.rs`'s `read_texel`
// `ReadKind::Indexed` arm plus `tmem/texel.rs`'s `resolve_indexed_texel`
// (as fixed at `4c412a96`) and `decode_tlut_entry`, in that order:
//
//  1. read the raw index from TMEM at this texel's own physical address,
//     narrowing by size -- 4-bit unpacks the nibble selected by the
//     column's parity (`unpack_ci4_texel`: even column is bits 7:4, odd is
//     3:0) and prefixes the tile's palette field (`(palette << 4) | nibble`),
//     8-bit takes the byte, 16-bit takes the HIGH (big-endian first) byte;
//  2. look up the quadricated entry at `0x800 + index * 8`;
//  3. decode that entry as RGBA16 or IA16 per the TLUT mode -- NOT per the
//     tile format, which is ignored on this arm.
//
// The tile's declared `format` is never read here. That is the whole point
// of the arm.
fn tmem_sample_tlut_texel(
    tile: TileBindingParams,
    column: u32,
    row: u32,
    out_ok: ptr<function, bool>,
    out_low_half_violation: ptr<function, bool>,
) -> vec4<f32> {
    let linear = tmem_indexed_linear_base(tile, column, row);

    // `validate_address_scope` (`tmem/read.rs`): under an enabled TLUT the
    // CI source byte must lie in the canonical low half. The CPU reader
    // checks the FULLY addressed byte -- post twelve-bit wrap, post odd-row
    // XOR4 exchange -- via `first_physical_byte`, so this must too; checking
    // the unwrapped `linear` instead would disagree with the oracle exactly
    // at the wrap boundary and across the exchange.
    if tmem_rgba16_byte_address(tile, linear, row) >= TMEM_TLUT_BASE {
        *out_low_half_violation = true;
        *out_ok = false;
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }

    var index: u32;
    if tile.pixel_size == TMEM_PIXEL_SIZE_BITS16 {
        // The high byte of the big-endian 16-bit texel: the first byte at
        // this texel's address, not the second.
        var ok_hi = false;
        let hi = tmem_read_byte(tmem_rgba16_byte_address(tile, linear, row), &ok_hi);
        if !ok_hi {
            *out_ok = false;
            return vec4<f32>(0.0, 0.0, 0.0, 0.0);
        }
        index = hi;
    } else {
        var ok_byte = false;
        let byte = tmem_read_byte(tmem_rgba16_byte_address(tile, linear, row), &ok_byte);
        if !ok_byte {
            *out_ok = false;
            return vec4<f32>(0.0, 0.0, 0.0, 0.0);
        }
        if tile.pixel_size == TMEM_PIXEL_SIZE_BITS4 {
            var nibble: u32;
            if (column & 1u) == 0u {
                nibble = (byte >> 4u) & 0x0fu;
            } else {
                nibble = byte & 0x0fu;
            }
            index = ((tile.palette & 0x0fu) << 4u) | nibble;
        } else {
            index = byte;
        }
    }

    var entry_ok = false;
    let entry = tmem_read_canonical_tlut_entry(index, &entry_ok);
    if !entry_ok {
        *out_ok = false;
        return vec4<f32>(0.0, 0.0, 0.0, 0.0);
    }
    *out_ok = true;
    if tile.lut_mode == TMEM_TLUT_MODE_IA16 {
        return decode_ia16(entry);
    }
    return decode_rgba16(entry);
}

// Dispatches one already-addressed texel to whichever of the two arms this
// tile's `lut_mode` selects. Both arms' unsupported cases are refused
// before this function is ever called (see
// `sample_committed_rgba16_three_nearest`), so `out_ok = false` here means
// an invalid TMEM byte or a non-canonical TLUT entry, never a format
// refusal.
fn tmem_sample_texel(
    tile: TileBindingParams,
    column: u32,
    row: u32,
    out_ok: ptr<function, bool>,
    out_low_half_violation: ptr<function, bool>,
) -> vec4<f32> {
    if tile.lut_mode != TMEM_TLUT_MODE_DISABLED {
        return tmem_sample_tlut_texel(tile, column, row, out_ok, out_low_half_violation);
    }
    return tmem_sample_rgba16_texel(tile, column, row, out_ok);
}

// `relative_axis_coordinate` (`tmem/sample.rs`): shifts the S10.5 raw
// coordinate by the tile's `G_SETTILE` shift field, then subtracts the
// tile's own S10.2 origin, in texel-fraction (5-bit) units. Returns the
// integer base texel and its 5-bit fraction, matching the CPU oracle's
// `div_euclid`/`rem_euclid` floor-toward-negative-infinity convention
// (`i32` truncating division does NOT match `div_euclid` for negative
// operands, so this function implements the euclidean form explicitly
// rather than relying on WGSL's `/`/`%`).
fn relative_axis_coordinate(
    raw_s10_5: i32,
    shift: u32,
    low_raw: u32,
    base_texel: ptr<function, i32>,
    fraction_five_bit: ptr<function, u32>,
) {
    var shifted: i32;
    if shift == 0u {
        shifted = raw_s10_5;
    } else if shift <= 10u {
        shifted = raw_s10_5 >> shift;
    } else {
        shifted = raw_s10_5 * (1i << (16u - shift));
    }
    let origin = i32(low_raw) * TILE_TO_TEXEL_FRACTION_SCALE;
    let relative = shifted - origin;
    // Euclidean floor division/remainder by the positive constant
    // TEXEL_FRACTION_SCALE (32): WGSL's `/`/`%` truncate toward zero, so a
    // negative `relative` needs an explicit floor correction to match Rust's
    // `div_euclid`/`rem_euclid`.
    var quotient = relative / TEXEL_FRACTION_SCALE;
    var remainder = relative % TEXEL_FRACTION_SCALE;
    if remainder < 0i {
        remainder = remainder + TEXEL_FRACTION_SCALE;
        quotient = quotient - 1i;
    }
    *base_texel = quotient;
    *fraction_five_bit = u32(remainder);
}

// `address_axis_texel` (`tmem/sample.rs`): clamp (implicit when mask==0, or
// explicit via the tile's clamp bit), then mirror/mask. `dimension` is
// `high.integer() - low.integer() + 1` in whole-texel units (S10.2's
// `integer()` divides the raw field by 4).
fn address_axis_texel(
    coordinate_in: i32,
    low_raw: u32,
    high_raw: u32,
    mirror: bool,
    clamp_bit: bool,
    mask: u32,
) -> u32 {
    let clamps = mask == 0u || clamp_bit;
    var coordinate = coordinate_in;
    if clamps {
        let low_integer = i32(low_raw >> 2u);
        let high_integer = i32(high_raw >> 2u);
        let dimension = high_integer - low_integer + 1i;
        coordinate = clamp(coordinate, 0i, dimension - 1i);
    }
    if mask == 0u {
        return u32(coordinate);
    }
    let low_mask = (1i << mask) - 1i;
    if mirror && (coordinate & (1i << mask)) != 0i {
        return u32((~coordinate) & low_mask);
    }
    return u32(coordinate & low_mask);
}

// `address_texture_cell` (`tmem/sample.rs`): addresses all four corners of
// the integer cell containing one S10.5 point, plus the cell's own 5-bit
// S/T fractions. `out_ok` is false only when a clamped axis has a reversed
// extent (`PointAddressError::ReversedClampExtent`) -- this slice's frozen
// fixture never exercises that branch, but it is still checked and
// propagated as a named failure (card §6), never silently ignored.
struct AddressedCellWgsl {
    s0: u32,
    s1: u32,
    t0: u32,
    t1: u32,
    sf: u32,
    tf: u32,
}

fn address_texture_cell_wgsl(
    tile: TileBindingParams,
    raw_s: i32,
    raw_t: i32,
    out_ok: ptr<function, bool>,
) -> AddressedCellWgsl {
    var result: AddressedCellWgsl;
    *out_ok = true;

    if tile.mode_s_clamp == 0u && tile.mask_s != 0u {
        // Non-clamped axis: reversed-extent check does not apply (matches
        // `address_axis_texel`'s own `clamps` gate).
    } else if tile.high_s >> 2u < tile.low_s >> 2u {
        *out_ok = false;
    }
    if tile.mode_t_clamp == 0u && tile.mask_t != 0u {
    } else if tile.high_t >> 2u < tile.low_t >> 2u {
        *out_ok = false;
    }
    if !(*out_ok) {
        return result;
    }

    var base_s: i32;
    var frac_s: u32;
    relative_axis_coordinate(raw_s, tile.shift_s, tile.low_s, &base_s, &frac_s);
    var base_t: i32;
    var frac_t: u32;
    relative_axis_coordinate(raw_t, tile.shift_t, tile.low_t, &base_t, &frac_t);

    let mirror_s = tile.mode_s_mirror != 0u;
    let clamp_s = tile.mode_s_clamp != 0u;
    let mirror_t = tile.mode_t_mirror != 0u;
    let clamp_t = tile.mode_t_clamp != 0u;

    result.s0 = address_axis_texel(base_s, tile.low_s, tile.high_s, mirror_s, clamp_s, tile.mask_s);
    result.s1 = address_axis_texel(base_s + 1i, tile.low_s, tile.high_s, mirror_s, clamp_s, tile.mask_s);
    result.t0 = address_axis_texel(base_t, tile.low_t, tile.high_t, mirror_t, clamp_t, tile.mask_t);
    result.t1 = address_axis_texel(base_t + 1i, tile.low_t, tile.high_t, mirror_t, clamp_t, tile.mask_t);
    result.sf = frac_s;
    result.tf = frac_t;
    return result;
}

// `filter_three_nearest`/`filter_three_nearest_committed_cell`
// (`tmem/sample.rs`), byte-for-byte the same fixed-point formula this
// crate's own `shaders/three_nearest_filter.wgsl` already carries (see that
// file's own bound proof for why `i32` is exact and sufficient at this
// magnitude) -- reused here as the identical arithmetic, operating on
// already-normalized `[0,1]` `vec4<f32>` corners instead of packed
// `u32` RGBA8888, to match this fragment shader's own `CombinerInputs`
// convention (`tex_val0`/`tex_val1` are `vec4<f32>`).
fn filter_three_nearest_wgsl(
    c00: vec4<f32>,
    c10: vec4<f32>,
    c01: vec4<f32>,
    c11: vec4<f32>,
    sf: u32,
    tf: u32,
) -> vec4<f32> {
    let sf_i = i32(sf);
    let tf_i = i32(tf);
    let c00_255 = c00 * 255.0;
    let c10_255 = c10 * 255.0;
    let c01_255 = c01 * 255.0;
    let c11_255 = c11 * 255.0;

    var out = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    for (var channel = 0u; channel < 4u; channel = channel + 1u) {
        let a = i32(c00_255[channel]);
        let b = i32(c10_255[channel]);
        let c = i32(c01_255[channel]);
        let d = i32(c11_255[channel]);
        var value: i32;
        if sf_i + tf_i <= TEXEL_FRACTION_SCALE {
            value = a * TEXEL_FRACTION_SCALE + sf_i * (b - a) + tf_i * (c - a);
        } else {
            value = d * TEXEL_FRACTION_SCALE
                + (TEXEL_FRACTION_SCALE - sf_i) * (c - d)
                + (TEXEL_FRACTION_SCALE - tf_i) * (b - d);
        }
        let rounded = clamp((value + TEXEL_FRACTION_HALF_SCALE) / TEXEL_FRACTION_SCALE, 0i, 255i);
        out[channel] = f32(rounded) / 255.0;
    }
    return out;
}

// Top-level entry: point coordinates in raw S10.5 -> three-nearest-filtered
// RGBA8888 color in `[0,1]`, or a named failure status (card §6). This is
// the only function `triangle_pipeline_fragment.wgsl` calls; everything
// above is this function's own decomposition, mirroring
// `gather_committed_texture_cell`/`filter_three_nearest_committed_cell`'s
// call shape in `tmem/sample.rs`.
fn sample_committed_rgba16_three_nearest(
    tile: TileBindingParams,
    raw_s: i32,
    raw_t: i32,
) -> TmemSampleResult {
    var result: TmemSampleResult;
    result.color = vec4<f32>(0.0, 0.0, 0.0, 0.0);

    if tile.bound == 0u {
        result.status = TMEM_SAMPLE_STATUS_NO_TILE_BINDING;
        return result;
    }

    // **`lut_mode` first, `format` only as the disabled arm's tiebreak.**
    // Under `tlut_en` the RDP sources the final texel from a palette and
    // the tile format is ignored (module header: n64brew + RT64
    // `sampleTMEM`), so consulting `format` here would refuse WM2000's
    // measured IA4-under-`G_TT_RGBA16` tile for a property the hardware
    // does not read. 32-bit stays refused on BOTH arms, mirroring the CPU
    // fix at `4c412a96`, which deliberately did not widen there.
    if tile.lut_mode == TMEM_TLUT_MODE_DISABLED {
        if tile.format != TMEM_IMAGE_FORMAT_RGBA || tile.pixel_size != TMEM_PIXEL_SIZE_BITS16 {
            result.status = TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT;
            return result;
        }
    } else if tile.pixel_size == TMEM_PIXEL_SIZE_BITS32 {
        result.status = TMEM_SAMPLE_STATUS_UNSUPPORTED_FORMAT;
        return result;
    }

    var addressed_ok = false;
    let cell = address_texture_cell_wgsl(tile, raw_s, raw_t, &addressed_ok);
    if !addressed_ok {
        result.status = TMEM_SAMPLE_STATUS_REVERSED_EXTENT;
        return result;
    }

    var ok_ul = false;
    var ok_ll = false;
    var ok_ur = false;
    var ok_lr = false;
    // One shared violation flag across the four corners: the low-half rule
    // is a property of the tile's placement, so any corner tripping it means
    // the whole draw is sourcing indexes from the palette's own half.
    var low_half_violation = false;
    let c_ul = tmem_sample_texel(tile, cell.s0, cell.t0, &ok_ul, &low_half_violation);
    let c_ll = tmem_sample_texel(tile, cell.s0, cell.t1, &ok_ll, &low_half_violation);
    let c_ur = tmem_sample_texel(tile, cell.s1, cell.t0, &ok_ur, &low_half_violation);
    let c_lr = tmem_sample_texel(tile, cell.s1, cell.t1, &ok_lr, &low_half_violation);
    // Checked BEFORE the validity verdict, mirroring the CPU reader's own
    // order: `validate_address_scope` runs before the first byte is read, so
    // a high-half tile is refused by placement even when its bytes happen to
    // be invalid too.
    if low_half_violation {
        result.status = TMEM_SAMPLE_STATUS_CI_SOURCE_OUTSIDE_LOW_HALF;
        return result;
    }
    if !ok_ul || !ok_ll || !ok_ur || !ok_lr {
        result.status = TMEM_SAMPLE_STATUS_INVALID_BYTE;
        return result;
    }

    result.status = TMEM_SAMPLE_STATUS_OK;
    result.color = filter_three_nearest_wgsl(c_ul, c_ur, c_ll, c_lr, cell.sf, cell.tf);
    return result;
}

// Fragment-stage entry point: reads this triangle's own tile binding from
// the module-scope `tmem_tile_binding` uniform (`@group(0) @binding(4)`)
// rather than taking it as a parameter -- `triangle_pipeline_fragment.wgsl`'s
// `fs_main` calls only this function, never
// `sample_committed_rgba16_three_nearest` directly.
fn sample_committed_rgba16_three_nearest_bound(raw_s: i32, raw_t: i32) -> TmemSampleResult {
    return sample_committed_rgba16_three_nearest(tmem_tile_binding, raw_s, raw_t);
}
