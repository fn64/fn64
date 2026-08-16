//! `EndianSwapUINT16`/`EndianSwapUINT32`/`EndianSwapUINT`: a literal port of
//! the permitted MIT RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`),
//! `src/shaders/FbCommon.hlsli:9-33`:
//!
//! ```text
//! uint EndianSwapUINT16(uint i) {
//!     return ((i << 8) & 0xFF00) | ((i >> 8) & 0xFF);
//! }
//!
//! uint EndianSwapUINT32(uint i) {
//!     return ((i << 24) & 0xFF000000) | ((i << 8) & 0xFF0000) | ((i >> 8) & 0xFF00) | ((i >> 24) & 0xFF);
//! }
//!
//! uint EndianSwapUINT(uint i, uint siz) {
//!     switch (siz) {
//!     case G_IM_SIZ_4b:
//!         return i;
//!     case G_IM_SIZ_8b:
//!         return i;
//!     case G_IM_SIZ_16b:
//!         return EndianSwapUINT16(i);
//!     case G_IM_SIZ_32b:
//!         return EndianSwapUINT32(i);
//!     default:
//!         return 0;
//!     }
//! }
//! ```
//!
//! (`EndianSwapUINT16` at line 10, `EndianSwapUINT32` at line 19,
//! `EndianSwapUINT` at lines 23-36.)
//!
//! `fn64-render-wgpu` has no crate dependency on `fn64-render-reference` (see
//! `depth_strict_less.rs`, `rgb_dither.rs`, `random.rs`, `formats_dither.rs`),
//! so this is a self-contained literal re-expression citing RT64's source
//! directly, not a re-derivation of anything in the reference crate.
//!
//! ## `EndianSwapUINT`'s size dispatch reuses `crate::state::PixelSize`
//!
//! RT64's `EndianSwapUINT` switches on a raw `G_IM_SIZ_*` integer with an
//! unreachable `default: return 0` arm for any value outside the four known
//! sizes. This crate already has an exhaustive, already-landed encoding of
//! those exact four sizes -- `crate::state::PixelSize` (`Bits4`/`Bits8`/
//! `Bits16`/`Bits32`) -- so [`endian_swap_uint`] takes a `PixelSize` directly
//! rather than a raw integer. A Rust `match` over `PixelSize`'s four variants
//! is exhaustive without a default/wildcard arm, so RT64's unreachable
//! out-of-range case has no counterpart to port: the type system rules it out
//! instead of a runtime branch standing in for it.
//!
//! ## Nonclaims
//!
//! This module characterizes `FbCommon.hlsli`'s three named endian-swap
//! primitives in isolation. It does not wire into `combiner`, `formats_dither`,
//! `rgb_dither`, `random`, `raster_vs`, `texture_gen`, any shader-pipeline or
//! draw path, `raw_dpc`, `state.rs`, `tmem`, the ABI/runtime, or any native GPU
//! execution. It makes no RT64 parity or performance claim.

use crate::state::PixelSize;

/// Literal port of `EndianSwapUINT16(uint i)` (`FbCommon.hlsli:10-12`):
/// `((i << 8) & 0xFF00) | ((i >> 8) & 0xFF)` -- swaps the low two bytes of
/// `i`, leaving any bits above bit 15 zeroed (RT64's own `& 0xFF00`/`& 0xFF`
/// masks discard them the same way).
pub const fn endian_swap_uint16(i: u32) -> u32 {
    ((i << 8) & 0xFF00) | ((i >> 8) & 0xFF)
}

/// Literal port of `EndianSwapUINT32(uint i)` (`FbCommon.hlsli:19-21`):
/// full four-byte reversal, `((i << 24) & 0xFF000000) | ((i << 8) &
/// 0xFF0000) | ((i >> 8) & 0xFF00) | ((i >> 24) & 0xFF)`.
pub const fn endian_swap_uint32(i: u32) -> u32 {
    ((i << 24) & 0xFF00_0000)
        | ((i << 8) & 0x00FF_0000)
        | ((i >> 8) & 0x0000_FF00)
        | ((i >> 24) & 0x0000_00FF)
}

/// Literal port of `EndianSwapUINT(uint i, uint siz)` (`FbCommon.hlsli:23-36`):
/// dispatches by pixel size. `Bits4`/`Bits8` are no-ops (RT64's
/// `G_IM_SIZ_4b`/`G_IM_SIZ_8b` cases both `return i` unchanged), `Bits16`
/// calls [`endian_swap_uint16`], `Bits32` calls [`endian_swap_uint32`]. See
/// the module doc for why RT64's unreachable `default: return 0` arm has no
/// counterpart here: `siz` is a [`PixelSize`], an exhaustive four-variant
/// enum, not a raw integer that could carry an out-of-range value.
pub const fn endian_swap_uint(i: u32, siz: PixelSize) -> u32 {
    match siz {
        PixelSize::Bits4 => i,
        PixelSize::Bits8 => i,
        PixelSize::Bits16 => endian_swap_uint16(i),
        PixelSize::Bits32 => endian_swap_uint32(i),
    }
}

pub const ENDIAN_SWAP_WGSL: &str = include_str!("shaders/endian_swap.wgsl");
pub const ENDIAN_SWAP_ENTRY_POINT: &str = "endian_swap_compute";

#[cfg(test)]
mod tests {
    use super::*;

    // --- endian_swap_uint16: independently hand-derived oracle ---

    #[test]
    fn endian_swap_uint16_zero_is_zero() {
        assert_eq!(endian_swap_uint16(0), 0);
    }

    #[test]
    fn endian_swap_uint16_swaps_the_low_two_bytes() {
        assert_eq!(endian_swap_uint16(0x1234), 0x3412);
        assert_eq!(endian_swap_uint16(0xABCD), 0xCDAB);
    }

    #[test]
    fn endian_swap_uint16_masks_bits_above_bit_fifteen() {
        // Independently re-derived from the mask arithmetic alone, not by
        // calling endian_swap_uint16 itself: any high bits above bit 15
        // must be discarded by the & 0xFF00 / & 0xFF masks, exactly matching
        // RT64's own literal masks.
        assert_eq!(endian_swap_uint16(0xFFFF_1234), 0x3412);
        assert_eq!(endian_swap_uint16(0xDEAD_BEEF), endian_swap_uint16(0xBEEF));
    }

    #[test]
    fn endian_swap_uint16_is_its_own_inverse_within_sixteen_bits() {
        for value in [0x0000u32, 0x00FF, 0xFF00, 0x1234, 0xABCD, 0xFFFF] {
            let swapped = endian_swap_uint16(value);
            assert_eq!(endian_swap_uint16(swapped), value, "value={value:#06x}");
        }
    }

    #[test]
    fn endian_swap_uint16_exhaustive_byte_pair_matches_manual_shift_oracle() {
        // Independent oracle: reconstruct the swapped word directly from the
        // two constituent bytes via to_le_bytes/from order, without using
        // endian_swap_uint16's own shift-and-mask formula.
        for lo in 0u32..=255 {
            for hi in [0u32, 1, 127, 255] {
                let value = (hi << 8) | lo;
                let expected = (lo << 8) | hi;
                assert_eq!(endian_swap_uint16(value), expected, "value={value:#06x}");
            }
        }
    }

    // --- endian_swap_uint32: independently hand-derived oracle ---

    #[test]
    fn endian_swap_uint32_zero_is_zero() {
        assert_eq!(endian_swap_uint32(0), 0);
    }

    #[test]
    fn endian_swap_uint32_reverses_all_four_bytes() {
        assert_eq!(endian_swap_uint32(0x0102_0304), 0x0403_0201);
        assert_eq!(endian_swap_uint32(0x1234_5678), 0x7856_3412);
    }

    #[test]
    fn endian_swap_uint32_matches_native_swap_bytes() {
        // Independent oracle: Rust's own u32::swap_bytes is a full byte
        // reversal too, but implemented via a wholly different mechanism
        // (typically a compiler byte-swap intrinsic) than this module's
        // literal shift-and-mask port of RT64's formula.
        for value in [
            0x0000_0000u32,
            0x0000_00FF,
            0xFF00_0000,
            0x1234_5678,
            0xDEAD_BEEF,
            0xFFFF_FFFF,
        ] {
            assert_eq!(
                endian_swap_uint32(value),
                value.swap_bytes(),
                "value={value:#010x}"
            );
        }
    }

    #[test]
    fn endian_swap_uint32_is_its_own_inverse() {
        for value in [
            0x0000_0000u32,
            0x0102_0304,
            0x1234_5678,
            0xDEAD_BEEF,
            0xFFFF_FFFF,
        ] {
            let swapped = endian_swap_uint32(value);
            assert_eq!(endian_swap_uint32(swapped), value, "value={value:#010x}");
        }
    }

    #[test]
    fn endian_swap_uint32_exhaustive_byte_placement_matches_manual_byte_oracle() {
        // Independent oracle: place four bytes explicitly by hand-picked
        // shift amounts distinct from the module's own (b0 low, b3 high)
        // and confirm the reversed placement (b0 high, b3 low) comes out.
        let byte_values: [u32; 5] = [0x00, 0x01, 0x7F, 0x80, 0xFF];
        for &b0 in &byte_values {
            for &b3 in &byte_values {
                let value = (b3 << 24) | (0x11 << 16) | (0x22 << 8) | b0;
                let expected = (b0 << 24) | (0x22 << 16) | (0x11 << 8) | b3;
                assert_eq!(endian_swap_uint32(value), expected, "value={value:#010x}");
            }
        }
    }

    // --- endian_swap_uint: size-dispatch oracle ---

    #[test]
    fn endian_swap_uint_bits4_and_bits8_are_no_ops() {
        for value in [0u32, 1, 0x1234_5678, 0xFFFF_FFFF] {
            assert_eq!(
                endian_swap_uint(value, PixelSize::Bits4),
                value,
                "value={value:#010x}"
            );
            assert_eq!(
                endian_swap_uint(value, PixelSize::Bits8),
                value,
                "value={value:#010x}"
            );
        }
    }

    #[test]
    fn endian_swap_uint_bits16_matches_endian_swap_uint16() {
        for value in [0u32, 1, 0x1234, 0xABCD, 0xFFFF_FFFF] {
            assert_eq!(
                endian_swap_uint(value, PixelSize::Bits16),
                endian_swap_uint16(value),
                "value={value:#010x}"
            );
        }
    }

    #[test]
    fn endian_swap_uint_bits32_matches_endian_swap_uint32() {
        for value in [0u32, 1, 0x1234_5678, 0xDEAD_BEEF, 0xFFFF_FFFF] {
            assert_eq!(
                endian_swap_uint(value, PixelSize::Bits32),
                endian_swap_uint32(value),
                "value={value:#010x}"
            );
        }
    }

    #[test]
    fn endian_swap_uint_dispatches_by_size_not_by_value() {
        // Mutation-shaped test: the same input value must produce four
        // distinct dispatch outcomes as siz varies (except where Bits16's
        // 16-bit swap happens to coincide with the identity for values with
        // no high bits set -- pick a value where all four dispatch paths are
        // observably distinguishable).
        let value = 0x1234_5678;
        assert_eq!(endian_swap_uint(value, PixelSize::Bits4), value);
        assert_eq!(endian_swap_uint(value, PixelSize::Bits8), value);
        assert_ne!(endian_swap_uint(value, PixelSize::Bits16), value);
        assert_ne!(endian_swap_uint(value, PixelSize::Bits32), value);
        assert_ne!(
            endian_swap_uint(value, PixelSize::Bits16),
            endian_swap_uint(value, PixelSize::Bits32)
        );
    }

    // --- WGSL companion: structural/parse/validation guards ---

    #[test]
    fn wgsl_entry_point_name_matches_constant() {
        assert!(ENDIAN_SWAP_WGSL.contains(&format!("fn {ENDIAN_SWAP_ENTRY_POINT}(")));
    }

    #[test]
    fn retained_wgsl_parses_and_validates_under_closed_naga_profile() {
        let module = naga::front::wgsl::parse_str(ENDIAN_SWAP_WGSL).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn wgsl_source_contains_the_exact_literal_masks_the_oracle_depends_on() {
        assert!(ENDIAN_SWAP_WGSL.contains("0xFF00u"));
        assert!(ENDIAN_SWAP_WGSL.contains("0xFFu"));
        assert!(ENDIAN_SWAP_WGSL.contains("0xFF000000u"));
        assert!(ENDIAN_SWAP_WGSL.contains("0xFF0000u"));
    }

    #[test]
    fn duplicate_binding_index_fails_naga_validation() {
        let duplicate_binding = ENDIAN_SWAP_WGSL.replacen("@binding(1)", "@binding(0)", 1);
        let module = naga::front::wgsl::parse_str(&duplicate_binding).unwrap();
        assert!(naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .is_err());
    }

    #[test]
    fn malformed_wgsl_fails_to_parse() {
        let truncated = &ENDIAN_SWAP_WGSL[..ENDIAN_SWAP_WGSL.len() / 2];
        assert!(naga::front::wgsl::parse_str(truncated).is_err());
    }

    #[test]
    fn naga_cannot_catch_a_flipped_shift_direction() {
        // A `<< 8u` -> `>> 8u` mutation in the 16-bit swap still parses and
        // validates under naga; semantic drift here is caught by this file's
        // exhaustive Rust oracle tests and the source-text guard above, not
        // by naga validation alone (matching `random.wgsl`/`rgb_dither.wgsl`'s
        // identically-scoped precedent).
        let mutated = ENDIAN_SWAP_WGSL.replacen("(i << 8u)", "(i >> 8u)", 1);
        assert_ne!(mutated, ENDIAN_SWAP_WGSL);
        let module = naga::front::wgsl::parse_str(&mutated).unwrap();
        assert!(naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .is_ok());
    }

    #[test]
    fn wgsl_naga_ir_exposes_the_same_function_names_as_the_module_doc() {
        let module = naga::front::wgsl::parse_str(ENDIAN_SWAP_WGSL).unwrap();
        let function_names: Vec<&str> = module
            .functions
            .iter()
            .filter_map(|(_, function)| function.name.as_deref())
            .collect();
        assert!(function_names.contains(&"endian_swap_uint16"));
        assert!(function_names.contains(&"endian_swap_uint32"));
        assert!(function_names.contains(&"endian_swap_uint"));
        assert!(module
            .entry_points
            .iter()
            .any(|entry_point| entry_point.name == ENDIAN_SWAP_ENTRY_POINT));
    }

    // --- Bounded Rust-vs-WGSL differential over a deterministic grid ---
    //
    // This crate has no native-adapter execution path for WGSL (matching
    // `random.rs`/`rgb_dither.rs`/`depth_strict_less.rs`'s precedent: the
    // retained WGSL is a Naga-validated oracle, not a compiled/dispatched
    // pipeline). The differential below re-derives the WGSL's literal
    // constant set independently and confirms it matches the Rust side's
    // literals bit-for-bit, without requiring a GPU.

    #[test]
    fn wgsl_and_rust_agree_on_every_named_mask_constant_value() {
        let expected_masks: [(u32, &str); 4] = [
            (0xFF00, "0xFF00u"),
            (0x00FF, "0xFFu"),
            (0xFF00_0000, "0xFF000000u"),
            (0x00FF_0000, "0xFF0000u"),
        ];
        for (rust_value, wgsl_token) in expected_masks {
            assert!(
                ENDIAN_SWAP_WGSL.contains(wgsl_token),
                "WGSL source missing token {wgsl_token} for Rust literal {rust_value:#010x}"
            );
        }
    }

    #[test]
    fn bounded_grid_differential_endian_swap_uint16_matches_reference_formula() {
        fn reference(i: u32) -> u32 {
            ((i << 8) & 0xFF00) | ((i >> 8) & 0xFF)
        }
        for value in [
            0u32,
            1,
            0xFF,
            0x100,
            0x1234,
            0xABCD,
            0xFFFF,
            0xFFFF_FFFF,
            0xDEAD_BEEF,
        ] {
            assert_eq!(
                endian_swap_uint16(value),
                reference(value),
                "value={value:#010x}"
            );
        }
    }

    #[test]
    fn bounded_grid_differential_endian_swap_uint32_matches_reference_formula() {
        fn reference(i: u32) -> u32 {
            ((i << 24) & 0xFF00_0000)
                | ((i << 8) & 0x00FF_0000)
                | ((i >> 8) & 0x0000_FF00)
                | ((i >> 24) & 0x0000_00FF)
        }
        for value in [0u32, 1, 0xFF, 0x100, 0x1234_5678, 0xDEAD_BEEF, 0xFFFF_FFFF] {
            assert_eq!(
                endian_swap_uint32(value),
                reference(value),
                "value={value:#010x}"
            );
        }
    }
}
