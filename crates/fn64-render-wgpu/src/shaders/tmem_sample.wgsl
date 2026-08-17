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
// Scope: RGBA16 direct-format texels only (`ImageFormat::Rgba`,
// `PixelSize::Bits16`) -- the only format this slice's frozen fixture (card
// §4) exercises. CI4/CI8/TLUT, RGBA32, IA4/IA8/IA16, I4/I8, and YUV are not
// ported here; a footprint that would require any of them is out of scope
// for this slice and is not requested by its own fixed fragment-shader call
// site. `TmemFirstRowParity::Even` is frozen for every sample this slice
// issues (card §6) -- see `TMEM_FIRST_ROW_PARITY_EVEN` below.
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

// Card §6: `TmemFirstRowParity::Even` is frozen for every sample this slice
// issues. `false` here means "first row is even" (no XOR4 exchange bias),
// matching `tmem/read.rs`'s `TmemFirstRowParity::Even` variant exactly.
const TMEM_FIRST_ROW_PARITY_ODD: bool = false;

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
const TMEM_PIXEL_SIZE_BITS16: u32 = 2u;

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
    // `TMEM_IMAGE_FORMAT_*`/`TMEM_PIXEL_SIZE_*` constants above). Checked by
    // `sample_committed_rgba16_three_nearest` before any addressing math
    // runs -- this slice ports RGBA16 only (module doc), so any other
    // format/size pair is a named rejection, not a silent fallback to
    // treating the bytes as RGBA16.
    format: u32,
    pixel_size: u32,
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
fn tmem_rgba16_byte_address(linear: u32, row: u32) -> u32 {
    let address = linear & TMEM_ADDRESS_MASK;
    let first_is_odd = TMEM_FIRST_ROW_PARITY_ODD;
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
    let hi_address = tmem_rgba16_byte_address(linear, row);
    let lo_address = tmem_rgba16_byte_address(linear + 1u, row);
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

    if tile.format != TMEM_IMAGE_FORMAT_RGBA || tile.pixel_size != TMEM_PIXEL_SIZE_BITS16 {
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
    let c_ul = tmem_sample_rgba16_texel(tile, cell.s0, cell.t0, &ok_ul);
    let c_ll = tmem_sample_rgba16_texel(tile, cell.s0, cell.t1, &ok_ll);
    let c_ur = tmem_sample_rgba16_texel(tile, cell.s1, cell.t0, &ok_ur);
    let c_lr = tmem_sample_rgba16_texel(tile, cell.s1, cell.t1, &ok_lr);
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
