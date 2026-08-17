//! The five `RDP::` register-application and addressing behaviors from RT64's
//! RDP command state machine that this crate did **not** already own, plus the
//! command-length table its interpreter drives its display-list cursor from.
//!
//! ## Authority and digests
//!
//! Both cited files come from the permitted MIT RT64 source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875` (`docs/RT64-PORT-AUTHORITY.md`):
//!
//! - `src/hle/rt64_rdp.cpp` -- SHA-256 of the whole file,
//!   `288e186aa741a0f2ce8ff89a17d22b525fd017da81404ab709f5681c0666194c`,
//!   1,356 lines.
//! - `src/hle/rt64_rdp.h` -- SHA-256 of the whole file,
//!   `876c45dec7672ef37e5a9955df713576b7f8d3806aa644b2461a3df2eeb6eda5`,
//!   215 lines.
//!
//! Both digests were computed independently here with `shasum -a 256` against
//! the pinned checkout and cross-checked verbatim against
//! `docs/rt64-port-inventory.json`'s
//! `files[path=...].sources.port.sha256` for each path; both match, with no
//! mismatch. For both paths the inventory's `sources.oracle.sha256` records
//! the identical digest, so the oracle and port trees agree on these two files
//! byte for byte.
//!
//! ## Per-file drift disclosure -- this is a PARTIAL port
//!
//! **`src/hle/rt64_rdp.cpp`: partial.** The file defines 63 `RDP::` methods
//! plus three free functions across 1,356 lines. This module ports **five**
//! behaviors from it: `getCommandLength` (lines 30-60), `movedFromOrigin`
//! (lines 230-237), `maskAddress` (lines 241-248, with its `ExtendedMask`
//! constant at line 239), `setKeyR` (lines 1013-1018) and `setKeyGB` (lines
//! 1020-1027), and it additionally re-derives `setPrimDepth`'s two normalizing
//! constants (lines 961-968) to *pin a disagreement* against this crate's
//! existing owner rather than to replace it. By line count that is roughly
//! **35 of 1,356 lines, about 2.6%**. Everything else in the file is either
//! already owned elsewhere in this crate (see the ownership map below) or is
//! deliberately refused (see Nonclaims).
//!
//! **`src/hle/rt64_rdp.h`: cited but not ported.** The header is cited for two
//! constant families this module's ported behavior depends on
//! (`RDP_ADDRESS_MASK` at line 29, and the `RDPTriangle` enum plus the four
//! `triangle*Words` constants at lines 40-65 that `getCommandLength` reads).
//! Its `struct RDP` -- the 40-odd member fields, the `RDP_EXTENDED_STACK_SIZE`
//! parallel register stacks, `TMEM[512]`, and the 63 method declarations -- is
//! **not** ported. This module declares no `RdpState`-shaped aggregate; see
//! "Reuse, not new type".
//!
//! The inventory's whole-file digest credits a source as `ported` at **file**
//! granularity, so recording either path's `ported_as` as pointing at this
//! module would over-credit a 2.6% port as a whole-file one. This card's
//! writable surface does not include `docs/rt64-port-inventory.json` and both
//! entries currently read `"port_state": "not-started"`, `"ported_as": []`;
//! that is left untouched deliberately, and this paragraph is the drift
//! disclosure standing in its place.
//!
//! ### This module is not the only implementer of these two files
//!
//! **Important for burndown attribution.** This module is, at the time of
//! writing, the first to cite `src/hle/rt64_rdp.cpp`'s digest -- but it is
//! *not* the first to implement parts of it. At least six other modules in
//! this crate already implement behavior from this file while citing it only
//! by path and line number, without its digest:
//!
//! - `state.rs:496` implements `setEnvColor`/`setPrimColor`/`setBlendColor`/
//!   `setFogColor` (cited as `rt64_rdp.cpp:837-932`).
//! - `state.rs:551,593` implement `setPrimColor`'s LOD normalization
//!   (`:860-871`).
//! - `state.rs:619` implements `setPrimDepth` (`:961-968`) -- and see the
//!   disagreement documented on [`prim_depth_normalized_rt64`].
//! - `combiner.rs:167` implements `setCombine`'s wire split (`:295-302`).
//! - `combiner.rs:1038,1040` consume `setPrimColor`/`setEnvColor`'s
//!   normalized outputs (`:838-842, 865-871`, `:862`).
//! - `raw_dpc/texture_rectangle.rs:24` implements `drawTexRect` and
//!   `drawRect`'s conversion.
//!
//! Similarly for `src/hle/rt64_rdp.h`: `rt64_tmem_regions.rs:423` uses
//! `RDP_TMEM_WORDS` (`rt64_rdp.h:21`) and `raw_dpc/triangle.rs:4` takes the
//! `RDPTriangle` enum and the four `triangle*Words` constants from it, both
//! without the digest.
//!
//! So the ~2.6% figure above is **this module's** share, not the file's total
//! ported fraction, which is materially higher once those six are counted.
//! Anything that reads a digest citation as a whole-file port credit would
//! attribute all of their work to this module; it should not. Adding digests
//! to those other modules is deliberately *not* done here -- they are outside
//! this card's exclusive paths and are a separate decision.
//!
//! ## Reuse, not new type
//!
//! This crate already owns RT64's RDP register state, from public SGI
//! documentation, in three places, and **this module adds no parallel copy of
//! any of it**:
//!
//! - [`crate::state`] owns `RdpState`, `OtherMode` (21 accessors),
//!   `ColorImage`, `FillColor`, `Color4`, `PrimColor`/`PrimLod`, `PrimDepth`
//!   and `CombineParams`.
//! - [`crate::tmem`] owns `TileDescriptor`, `TileSize`, `TmemState` and the
//!   LoadTile/LoadBlock/LoadTLUT transfer planner and executor.
//! - [`crate::raw_dpc`] owns the live command decoder, including
//!   `texture_rectangle.rs` (`drawTexRect`/`drawRect`'s conversion) and
//!   `triangle.rs` (the `RDPTriangle` payload decode).
//!
//! A prior card on this same source correctly produced no module at all,
//! because `state.rs` already *was* the port of the register half. This module
//! exists only for the residue those three owners genuinely do not cover, and
//! it deliberately takes and returns plain scalars so that it composes with
//! the existing owners instead of shadowing them.
//!
//! ## Admitted domain
//!
//! Every function here is a pure scalar transform over values a caller has
//! already decoded. Admitted:
//!
//! - `rdp_command_word_length`: `commandId` in `0..=255`, exactly the index
//!   domain of RT64's `std::array<uint8_t, 256> commandWordLengths`.
//! - `moved_from_origin`: any `i32 x`, any `u16 ori`. RT64's own `int32_t`
//!   addition can overflow; see Nonclaims for the one deviation.
//! - `mask_address`: any `u32 address`, either `extend_rdram` polarity.
//! - `key_center_scale_r` / `key_center_scale_gb`: any `u32` operands, since
//!   RT64's parameters are `uint32_t` and it applies no mask of its own.
//! - `prim_depth_normalized_rt64`: any `u16 z`, `u16 dz`.
//!
//! Not admitted: no framebuffer, workload, draw-call, texture-cache, GPU,
//! rasterizer, parity or performance claim is made by anything here.
//!
//! ## Ownership map -- where the other 58 methods live
//!
//! Recorded so a later reader can tell "unported" from "ported elsewhere":
//!
//! - Register setters/normalizers (`setEnvColor`, `setPrimColor`,
//!   `setBlendColor`, `setFogColor`, `setFillColor`, `setOtherMode`,
//!   `setCombine`, `setColorImage`, `setPrimDepth`): [`crate::state`].
//! - Tile/TMEM (`setTile`, `setTileSize`, `setTextureImage`, `loadTile`,
//!   `loadBlock`, `loadTLUT` and their three `*Operation` planners,
//!   `loadWord`, `loadToTMEMCommon`): [`crate::tmem`].
//! - Rectangles/triangles (`fillRect`, `drawRect`, `drawTexRect`, `drawTris`):
//!   [`crate::raw_dpc`].
//! - Refused (see Nonclaims): everything framebuffer-, workload-,
//!   stack-, or crash-shaped.
//!
//! ## Nonclaims
//!
//! - **No `RdpState` aggregate, and no register stacks.** The 16-deep
//!   `colorCombinerStack`/`envColorStack`/... arrays and their eight
//!   `push*`/`pop*` pairs are RT64's *extended-GBI* feature (they exist to
//!   serve `G_EX_PUSH*`/`G_EX_POP*`), not N64 RDP hardware behavior. fn64's
//!   `RdpState` models one register set with no stack. Not ported.
//! - **No framebuffer, workload, or draw-call plumbing.**
//!   `checkFramebufferPair`, `checkFramebufferOverlap`, `checkImageOverlap`,
//!   `loadTileCopyCheck`, `loadTileReplacementCheck` (which additionally
//!   hashes with XXH3 and consults a texture-replacement cache),
//!   `updateCallTexcoords`, `setGBI`, `reset`, `crash`, and the whole
//!   `extended` block (`setRectAlign`, `setScissorAlign`, `setRectAspect`,
//!   `forceUpscale2D`, `forceTrueBilerp`, `forceScaleLOD`, `clearExtended`)
//!   all reach into `State`, `Workload`, `FramebufferManager` or
//!   `DrawCall` -- uncited types this crate does not model. Not ported.
//! - **`setScissor` is not ported.** Its arithmetic is a `std::clamp` of
//!   `movedFromOrigin` results into `ExtendedAlignment` bounds; the clamp
//!   itself is trivial, but the value it writes lands in
//!   `scissorRectStack[scissorStackSize - 1]`, a stack this module explicitly
//!   does not model, and `crate::raw_dpc`'s `SetScissor` arm already documents
//!   that fn64 admits scissor as tracked state only. `moved_from_origin` --
//!   the one genuinely arithmetic piece -- **is** ported here, so a future
//!   scissor owner can reuse it.
//! - **`setConvert` is not ported.** RT64's `setConvert` is six plain
//!   assignments into `convertK[6]` with no arithmetic;
//!   [`crate::rt64_gbi_rdp_decode::decode_set_convert`] already owns the only
//!   part with content (the bitfield extraction, including `k2`'s split across
//!   both words). A six-element copy adds nothing.
//! - **`prim_depth_normalized_rt64` is a pinning function, not a replacement.**
//!   It exists to make a measured disagreement with
//!   [`crate::state::PrimDepth`] executable and regression-guarded. It does
//!   **not** claim to be more correct than that owner; see the disagreement
//!   note on the function itself. No caller in this crate is rewired to it.
//! - **DEVIATION -- signed overflow.** RT64's `movedFromOrigin` computes
//!   `x + offset` on `int32_t`. Signed overflow is undefined behavior in C++,
//!   and this port does not reproduce UB. [`moved_from_origin`] uses
//!   `i32::wrapping_add`, which is the behavior every mainstream C++
//!   implementation actually emits for that expression on two's-complement
//!   hardware, and which is bit-identical to RT64 for every input where RT64's
//!   own behavior is defined. The deviation is only observable for inputs
//!   where RT64 has no defined answer at all. Tests that exercise it are
//!   labelled as pinning a DEVIATION.
//! - No `repr(C)`, size, alignment, or ABI claim is made about any type here;
//!   this module declares no aggregate at all beyond two small owned result
//!   structs that exist purely to name their fields.
//!
//! ## Verbatim quote
//!
//! The two functions whose exact arithmetic order this module preserves, from
//! `src/hle/rt64_rdp.cpp` at the pinned commit:
//!
//! ```text
//! int32_t RDP::movedFromOrigin(int32_t x, uint16_t ori) {
//!     if (ori < G_EX_ORIGIN_NONE) {
//!         return x + ((ori * colorImage.width * 4) / G_EX_ORIGIN_RIGHT);
//!     }
//!     else {
//!         return x;
//!     }
//! };
//!
//! constexpr uint32_t ExtendedMask = 0x80000000U;
//!
//! uint32_t RDP::maskAddress(uint32_t address) {
//!     if (state->extended.extendRDRAM && ((address & ExtendedMask) == ExtendedMask)) {
//!         return address - ExtendedMask;
//!     }
//!     else {
//!         return address & RDP_ADDRESS_MASK;
//!     }
//! }
//! ```

use crate::rt64_extended_gbi::{G_EX_ORIGIN_NONE, G_EX_ORIGIN_RIGHT};

/// `RDP_ADDRESS_MASK` (`src/hle/rt64_rdp.h:29`): `0xFFFFFF`, the 24-bit
/// physical address window `maskAddress` folds an unextended address into.
///
/// Derived two independent ways and reconciled: read literally from the
/// header as the hexadecimal `0xFFFFFF`, and derived as `(1 << 24) - 1`
/// (RDRAM's 8 MiB expanded domain needs 23 bits, so a 24-bit mask is the
/// next power-of-two window that contains it). Both readings give
/// `16_777_215`; the module test `rdp_address_mask_agrees_with_both_derivations`
/// asserts that reconciliation executably.
pub const RDP_ADDRESS_MASK: u32 = 0x00ff_ffff;

/// `ExtendedMask` (`src/hle/rt64_rdp.cpp:239`): `0x80000000U`.
///
/// Note this constant is declared **between** two methods rather than with
/// the file's other constants -- it sits after `movedFromOrigin` and
/// immediately before `maskAddress`, its only user. That placement is
/// preserved as a fact about the source rather than tidied: the constant is
/// deliberately local to `maskAddress`, and this module keeps it adjacent to
/// [`mask_address`] for the same reason.
///
/// Derived two independent ways and reconciled: read literally as
/// `0x80000000`, and derived as `1u32 << 31` (the sign bit of a 32-bit word,
/// which is what an "extended RDRAM" tag bit necessarily is when the
/// unextended window is 24 bits). Both give `2_147_483_648`.
pub const EXTENDED_MASK: u32 = 0x8000_0000;

/// `getCommandLength` (`src/hle/rt64_rdp.cpp:30-60`), the function RT64's
/// `RDP` constructor evaluates for every `commandId` in `0..256` to fill
/// `commandWordLengths` (`src/hle/rt64_rdp.h:108`,
/// `std::array<uint8_t, 256>`; filled at `rt64_rdp.cpp:68-70`).
///
/// Returns the command's length in **64-bit display-list words**, not bytes.
/// RT64's interpreter reads it as `cmdLength` and advances its
/// `DisplayList *` cursor by that many entries
/// (`src/hle/rt64_interpreter.cpp:124`).
///
/// Three branches, in the source's own order:
///
/// 1. `commandId == (G_TEXRECT & 0x3F) || commandId == (G_TEXRECTFLIP & 0x3F)`
///    -> `2`. With `G_TEXRECT = 0xe4` and `G_TEXRECTFLIP = 0xe5`
///    (`src/shared/rt64_f3d_defines.h:162-163`), those masked ids are `0x24`
///    and `0x25`.
/// 2. `commandId >= G_RDPTRI_BASE && commandId <= RDPTriangle::MaxValue`
///    -> `triangleBaseWords` plus each present coefficient block. With
///    `G_RDPTRI_BASE = 0x08` (`rt64_f3d_defines.h:169`) and
///    `RDPTriangle::MaxValue = Base | Depth | Textured | Shaded`
///    (`rt64_rdp.h:40-46`), that range is `0x08..=0x0F`.
/// 3. otherwise -> `1`.
///
/// **Note the branch order matters and is preserved.** `0x24`/`0x25` are
/// tested *first*, but they fall outside `0x08..=0x0F` anyway, so the two
/// branches are in fact disjoint and the order is not load-bearing here --
/// recorded because the ordering looks significant and is not.
///
/// The coefficient word counts come from `src/hle/rt64_rdp.h:62-65`:
/// `triangleBaseWords = 4`, `triangleShadeWords = 8`, `triangleTexWords = 8`,
/// `triangleDepthWords = 2`, added in RT64's own shaded/textured/z-buffered
/// order. The flag bits are `RDPTriangle::Depth = 1 << 0`,
/// `Textured = 1 << 1`, `Shaded = 1 << 2`.
///
/// RT64 narrows the sum into `uint8_t`. The maximum reachable value is
/// `4 + 8 + 8 + 2 = 22`, so that narrowing never truncates; the `u8` return
/// type here is therefore lossless, and the module test
/// `every_command_length_fits_the_u8_return_without_truncation` pins it.
///
/// This function's *triangle* branch is the same fact
/// [`crate::raw_dpc::triangle_word_count`] already owns from the same header;
/// this one covers the whole 256-entry domain, including the texrect and
/// default branches that helper does not have.
pub const fn rdp_command_word_length(command_id: u8) -> u8 {
    // `G_TEXRECT & 0x3F` and `G_TEXRECTFLIP & 0x3F`.
    if command_id == 0x24 || command_id == 0x25 {
        return 2;
    }
    // `G_RDPTRI_BASE ..= RDPTriangle::MaxValue`.
    if command_id >= 0x08 && command_id <= 0x0f {
        let shaded = (command_id & 0x4) != 0;
        let textured = (command_id & 0x2) != 0;
        let z_buffered = (command_id & 0x1) != 0;

        // `triangleBaseWords`, then RT64's own add order: shade, tex, depth.
        let mut command_length: u8 = 4;
        if shaded {
            command_length += 8;
        }
        if textured {
            command_length += 8;
        }
        if z_buffered {
            command_length += 2;
        }
        return command_length;
    }
    1
}

/// `RDP::movedFromOrigin` (`src/hle/rt64_rdp.cpp:230-237`).
///
/// Shifts a horizontal coordinate by a fraction of the colour image's width,
/// selected by an extended-GBI origin code. `ori >= G_EX_ORIGIN_NONE`
/// (`0x800`) means "no origin", and the coordinate passes through untouched.
///
/// The offset is `(ori * color_image_width * 4) / G_EX_ORIGIN_RIGHT`, written
/// here in RT64's exact operand order -- `ori` times width, times four, then
/// divided by `G_EX_ORIGIN_RIGHT` (`0x400`). Two independent derivations of
/// that expression were reconciled: the literal `* 4 / 1024` form, and the
/// algebraically reduced `/ 256` form. They agree for every
/// `ori in 0..0x800` crossed with a spread of widths (asserted by the module
/// test `moved_from_origin_offset_agrees_with_its_reduced_derivation`), and
/// the literal form is the one written, per this program's "preserve
/// arithmetic order" rule.
///
/// All operands promote to `int` in C++. The product `ori * width * 4` cannot
/// overflow: its maximum is `0x7FF * 0xFFFF * 4 = 536_600_580`, well inside
/// `i32::MAX`. Both operands of the division are non-negative there, so C++'s
/// truncating division and Rust's are identical.
///
/// **DEVIATION** (disclosed in the module Nonclaims): the final `x + offset`
/// is `wrapping_add` here rather than a checked or panicking add. RT64's
/// `int32_t` addition is undefined behavior on overflow; this port refuses to
/// reproduce UB and takes the two's-complement result every mainstream C++
/// implementation actually emits. For every input where RT64's behavior is
/// defined, this is bit-identical.
pub fn moved_from_origin(x: i32, ori: u16, color_image_width: u16) -> i32 {
    if u32::from(ori) < G_EX_ORIGIN_NONE {
        let offset =
            (i64::from(ori) * i64::from(color_image_width) * 4) / i64::from(G_EX_ORIGIN_RIGHT);
        // The product is provably within i32, so this narrowing is exact; it
        // is written through i64 only so the intermediate cannot trap in a
        // debug build, never to widen the admitted result domain.
        x.wrapping_add(offset as i32)
    } else {
        x
    }
}

/// `RDP::maskAddress` (`src/hle/rt64_rdp.cpp:241-248`).
///
/// Folds a guest address into the addressable window. When RT64's extended
/// RDRAM mode is on **and** the address carries the `0x80000000` tag bit, the
/// tag is *subtracted* (not masked off) and the remaining 31 bits pass
/// through unmasked; otherwise the address is masked to
/// [`RDP_ADDRESS_MASK`]'s 24 bits.
///
/// Two facts here look like they could be tidied and are deliberately not:
///
/// - The extended branch uses `address - ExtendedMask`, a **subtraction**,
///   where `address & !ExtendedMask` would give the same answer. It gives the
///   same answer only *because* the branch already established that the bit
///   is set; the subtraction is written as RT64 writes it. The module test
///   `extended_branch_subtraction_and_bit_clear_agree_only_under_the_guard`
///   pins both that they agree under the guard and that the guard is what
///   makes them agree.
/// - The two branches mask to **different widths** -- 31 bits of pass-through
///   in the extended branch versus 24 bits in the ordinary one. That is not a
///   typo in the source: extended RDRAM is precisely the mode where the
///   24-bit window is too small.
///
/// `extend_rdram` stands in for RT64's `state->extended.extendRDRAM`. This
/// module models it as a caller-supplied `bool` rather than reaching into an
/// uncited `State`.
pub const fn mask_address(address: u32, extend_rdram: bool) -> u32 {
    if extend_rdram && (address & EXTENDED_MASK) == EXTENDED_MASK {
        address - EXTENDED_MASK
    } else {
        address & RDP_ADDRESS_MASK
    }
}

/// One chroma-key channel's centre and scale, both already normalized to
/// `[0.0, 1.0]`.
///
/// This exists only to name the two floats [`key_center_scale_r`] and
/// [`key_center_scale_gb`] return; it is not a register, not a state
/// aggregate, and carries no layout claim.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyChannel {
    /// RT64's `keyCenter.<component>`.
    pub center: f32,
    /// RT64's `keyScale.<component>`.
    pub scale: f32,
}

/// `RDP::setKeyR` (`src/hle/rt64_rdp.cpp:1013-1018`).
///
/// ```text
/// // Width is ignored until its exact purpose is understood on the chroma keying process.
/// keyCenter.x = cR / 255.0f;
/// keyScale.x = sR / 255.0f;
/// ```
///
/// Returns the red channel's `keyCenter.x` / `keyScale.x`.
///
/// **`wR` is deliberately absent from this signature.** RT64 takes it as a
/// third `uint32_t` parameter and then never reads it, with the comment
/// quoted above explaining why. Accepting a parameter this function provably
/// ignores would misrepresent the admitted behavior as consuming a value it
/// does not; the *decode* of `wR` is already owned and retained by
/// [`crate::rt64_gbi_rdp_decode::SetKeyRDecoded::w_r`], so the wire field is
/// not lost -- only its (nonexistent) effect on the key registers is refused
/// here. The same applies to `wG`/`wB` in [`key_center_scale_gb`].
///
/// The `/ 255.0f` divisor matches this crate's existing
/// [`crate::state::Color4::normalized`] exactly, and is written as a division
/// rather than a reciprocal multiply because RT64 writes a division here --
/// see [`prim_depth_normalized_rt64`] for the sibling case where RT64 writes
/// the reciprocal instead and the two are *not* interchangeable.
///
/// The operands are `uint32_t` in RT64 with no mask applied, so a value above
/// 255 normalizes above 1.0. That is reproduced, not clamped.
pub fn key_center_scale_r(c_r: u32, s_r: u32) -> KeyChannel {
    KeyChannel {
        center: c_r as f32 / 255.0,
        scale: s_r as f32 / 255.0,
    }
}

/// `RDP::setKeyGB` (`src/hle/rt64_rdp.cpp:1020-1027`).
///
/// ```text
/// // Width is ignored until its exact purpose is understood on the chroma keying process.
/// keyCenter.y = cG / 255.0f;
/// keyCenter.z = cB / 255.0f;
/// keyScale.y = sG / 255.0f;
/// keyScale.z = sB / 255.0f;
/// ```
///
/// Returns `(green, blue)` -- green is the `.y` component, blue is `.z`.
///
/// **Note RT64's assignment order is not centre/scale-paired**: it writes
/// both *centres* first (`.y` then `.z`), then both *scales* (`.y` then
/// `.z`). Since all four targets are distinct and the four right-hand sides
/// are independent, the order is not observable, and this port groups by
/// channel instead so the returned pair is usable. That regrouping is
/// recorded here rather than left silent because "preserve the source's
/// order" is this program's default and this is a deliberate, justified
/// exception -- the module test
/// `set_key_gb_channel_grouping_matches_rt64_component_assignment` pins that
/// each of the four values lands on the component RT64 assigns it to, so the
/// regrouping cannot silently transpose green and blue.
///
/// `wG` and `wB` are absent for the same reason `wR` is; see
/// [`key_center_scale_r`].
pub fn key_center_scale_gb(c_g: u32, s_g: u32, c_b: u32, s_b: u32) -> (KeyChannel, KeyChannel) {
    let green = KeyChannel {
        center: c_g as f32 / 255.0,
        scale: s_g as f32 / 255.0,
    };
    let blue = KeyChannel {
        center: c_b as f32 / 255.0,
        scale: s_b as f32 / 255.0,
    };
    (green, blue)
}

/// `RDP::setPrimDepth`'s normalization (`src/hle/rt64_rdp.cpp:961-968`),
/// reproduced **exactly as RT64 spells it** -- as a multiply by a
/// precomputed `float` reciprocal, not as a division.
///
/// ```text
/// const float Fixed15ToFloat = 1.0f / 32767.0f;
/// const float Fixed16ToFloat = 1.0f / 65535.0f;
/// primDepth.x = (z & 0x7FFFU) * Fixed15ToFloat;
/// primDepth.y = (dz & 0xFFFFU) * Fixed16ToFloat;
/// ```
///
/// # Disagreement with [`crate::state::PrimDepth`]
///
/// This crate's existing owner,
/// [`crate::state::PrimDepth::z_normalized`]/[`dz_normalized`], computes
/// `f32::from(z) / 32767.0` and `f32::from(dz) / 65535.0` -- a **division**.
/// Its own doc comment quotes RT64 as `(z & 0x7FFFU) * (1.0f / 32767.0f)`,
/// i.e. it quotes the multiply form and then implements the divide form.
///
/// These are not the same function. `1.0f / 32767.0f` is not exactly
/// representable in `f32`; rounding it to `f32` first and then multiplying
/// rounds twice, where a single division rounds once. Exhaustive comparison
/// over the whole admitted input domain (every `z` in `0..=0x7FFF`, every
/// `dz` in `0..=0xFFFF`) finds:
///
/// - **768 of 32,768 `z` values** produce different `f32` bit patterns.
/// - **512 of 65,536 `dz` values** produce different `f32` bit patterns.
///
/// Every difference is 1 ULP. The smallest disagreeing `z` is `513`
/// (multiply: `0.015655994415283203`, divide: `0.015655996277928352`); the
/// smallest disagreeing `dz` is `257`.
///
/// **Neither side is asserted to be the hardware-correct one.** The N64's RDP
/// computes primitive depth in fixed point; both float forms are host-side
/// conveniences, and which one a consumer should use depends on what it is
/// compared against. What is asserted, and pinned by
/// `prim_depth_multiply_and_divide_forms_disagree_on_known_inputs`, is that
/// (a) the two forms genuinely differ, (b) they differ on specific
/// enumerated inputs, and (c) `state.rs`'s doc comment describes the form it
/// does not implement. This function is the multiply form so that the
/// difference is executable rather than asserted in prose.
///
/// This module does **not** rewire any caller to this function. See the
/// module Nonclaims.
///
/// The masks are RT64's own and are preserved: `z` is masked to 15 bits
/// (`0x7FFF`, discarding bit 15), `dz` to the full 16 (`0xFFFF`, a no-op on a
/// `u16` input, retained because RT64 writes it).
pub fn prim_depth_normalized_rt64(z: u16, dz: u16) -> (f32, f32) {
    const FIXED15_TO_FLOAT: f32 = 1.0 / 32767.0;
    const FIXED16_TO_FLOAT: f32 = 1.0 / 65535.0;
    (
        f32::from(z & 0x7fff) * FIXED15_TO_FLOAT,
        f32::from(dz & 0xffff) * FIXED16_TO_FLOAT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Constant reconciliation -- each derived two independent ways.
    // ------------------------------------------------------------------

    #[test]
    fn rdp_address_mask_agrees_with_both_derivations() {
        // Literal reading of `#define RDP_ADDRESS_MASK 0xFFFFFF`.
        assert_eq!(RDP_ADDRESS_MASK, 0x00ff_ffff);
        // Independent derivation: the low 24 bits of a 32-bit word.
        assert_eq!(RDP_ADDRESS_MASK, (1u32 << 24) - 1);
        // Third reading: decimal.
        assert_eq!(RDP_ADDRESS_MASK, 16_777_215);
        // And it really is 24 set bits, not 23 or 25.
        assert_eq!(RDP_ADDRESS_MASK.count_ones(), 24);
        assert_eq!(RDP_ADDRESS_MASK.leading_zeros(), 8);
    }

    #[test]
    fn extended_mask_agrees_with_both_derivations() {
        // Literal reading of `constexpr uint32_t ExtendedMask = 0x80000000U`.
        assert_eq!(EXTENDED_MASK, 0x8000_0000);
        // Independent derivation: bit 31, the sign bit.
        assert_eq!(EXTENDED_MASK, 1u32 << 31);
        // Third reading: decimal.
        assert_eq!(EXTENDED_MASK, 2_147_483_648);
        // Exactly one bit, and it is the top one.
        assert_eq!(EXTENDED_MASK.count_ones(), 1);
        assert_eq!(EXTENDED_MASK.leading_zeros(), 0);
    }

    #[test]
    fn extended_mask_and_address_mask_are_disjoint() {
        // The extended tag bit is outside the ordinary 24-bit window, so the
        // two branches of `mask_address` can never both be reasoning about
        // the same bit. If this ever failed, the subtraction in the extended
        // branch would interact with the mask in the other one.
        assert_eq!(EXTENDED_MASK & RDP_ADDRESS_MASK, 0);
    }

    // ------------------------------------------------------------------
    // `rdp_command_word_length`
    // ------------------------------------------------------------------

    #[test]
    fn texrect_ids_are_the_low_six_bits_of_the_public_opcodes() {
        // Derivation 1: literal mask of the two `rt64_f3d_defines.h` values.
        assert_eq!(0xe4u8 & 0x3f, 0x24);
        assert_eq!(0xe5u8 & 0x3f, 0x25);
        // Derivation 2: read the binary digits directly. 0xe4 is 1110_0100;
        // its low six bits are 10_0100 = 0x24.
        assert_eq!(u8::from_str_radix("100100", 2).unwrap(), 0x24);
        assert_eq!(u8::from_str_radix("100101", 2).unwrap(), 0x25);
        // And those are the ids the table actually gives 2 words.
        assert_eq!(rdp_command_word_length(0x24), 2);
        assert_eq!(rdp_command_word_length(0x25), 2);
    }

    #[test]
    fn triangle_range_endpoints_come_from_the_rdptriangle_enum() {
        // `RDPTriangle::MaxValue = Base | Depth | Textured | Shaded`
        // = 0x08 | (1<<0) | (1<<1) | (1<<2).
        let max_value = 0x08u8 | (1 << 0) | (1 << 1) | (1 << 2);
        assert_eq!(max_value, 0x0f);
        // Independent derivation: base 0x08 plus a 3-bit flag field is
        // exactly 0x08..=0x0f, eight consecutive ids.
        assert_eq!(0x0fu8 - 0x08 + 1, 8);
        // Both endpoints are in the triangle branch, and the ids just
        // outside are not.
        assert_ne!(rdp_command_word_length(0x08), 1);
        assert_ne!(rdp_command_word_length(0x0f), 1);
        assert_eq!(rdp_command_word_length(0x07), 1);
        assert_eq!(rdp_command_word_length(0x10), 1);
    }

    #[test]
    fn every_triangle_length_is_hand_derived_from_the_word_constants() {
        // triangleBaseWords=4, Shade=8, Tex=8, Depth=2. Hand-computed, one
        // entry per opcode, not generated from the implementation.
        //
        //   0x08 base                       = 4
        //   0x09 base+depth                 = 4+2      =  6
        //   0x0a base+tex                   = 4+8      = 12
        //   0x0b base+tex+depth             = 4+8+2    = 14
        //   0x0c base+shade                 = 4+8      = 12
        //   0x0d base+shade+depth           = 4+8+2    = 14
        //   0x0e base+shade+tex             = 4+8+8    = 20
        //   0x0f base+shade+tex+depth       = 4+8+8+2  = 22
        let expected = [4u8, 6, 12, 14, 12, 14, 20, 22];
        for (offset, &want) in expected.iter().enumerate() {
            let id = 0x08 + offset as u8;
            assert_eq!(
                rdp_command_word_length(id),
                want,
                "command id {id:#04x} length"
            );
        }
    }

    #[test]
    fn textured_and_shaded_are_the_same_length_but_different_opcodes() {
        // 0x0a (textured) and 0x0c (shaded) both add 8 words, so they tie.
        // This is a genuine property of the constants -- triangleShadeWords
        // and triangleTexWords are both 8 -- not a transcription slip, and
        // it is pinned so a future edit that changes only one of them fails
        // loudly here.
        assert_eq!(rdp_command_word_length(0x0a), rdp_command_word_length(0x0c));
        assert_eq!(rdp_command_word_length(0x0b), rdp_command_word_length(0x0d));
    }

    #[test]
    fn every_other_command_id_is_one_word() {
        // The whole 256-entry domain, checked exhaustively: everything that
        // is not a texrect id or a triangle id must be exactly 1.
        for id in 0u8..=0xff {
            let is_texrect = id == 0x24 || id == 0x25;
            let is_triangle = (0x08..=0x0f).contains(&id);
            if !is_texrect && !is_triangle {
                assert_eq!(rdp_command_word_length(id), 1, "command id {id:#04x}");
            }
        }
    }

    #[test]
    fn the_table_covers_all_256_ids_with_the_expected_multiset() {
        // An independent whole-table check: total word count and the set of
        // distinct lengths, both hand-derived.
        //   246 ids at 1 word  = 246
        //     2 ids at 2 words =   4
        //   triangles 4+6+12+14+12+14+20+22 = 104
        //   total = 246 + 4 + 104 = 354
        let total: u32 = (0u8..=0xff)
            .map(|id| u32::from(rdp_command_word_length(id)))
            .sum();
        assert_eq!(total, 354);

        let mut distinct: Vec<u8> = (0u8..=0xff).map(rdp_command_word_length).collect();
        distinct.sort_unstable();
        distinct.dedup();
        assert_eq!(distinct, vec![1, 2, 4, 6, 12, 14, 20, 22]);
    }

    #[test]
    fn every_command_length_fits_the_u8_return_without_truncation() {
        // RT64 narrows its sum into `uint8_t`. Pin that the narrowing is
        // lossless: the maximum is 22, far below 256.
        let max = (0u8..=0xff)
            .map(rdp_command_word_length)
            .max()
            .expect("non-empty");
        assert_eq!(max, 22);
        assert!(max < u8::MAX);
    }

    #[test]
    fn triangle_branch_agrees_with_the_crates_existing_triangle_owner() {
        // Mutual corroboration: `fn64_render::raw_rdp_command_width` derives
        // the same eight triangle widths from the same public tables through
        // a completely different code path (a match on literal byte counts,
        // in a different crate), and reports them in BYTES. Multiplying this
        // module's word count by 8 must reproduce them exactly.
        //
        // That function masks its argument to six bits, which is what RT64's
        // own LLE interpreter does too (`opCode = (dl->w0 >> 24) &
        // opCodeMask`, with `opCodeMask == 0x3F` on the LLE path), so the
        // two agree across the whole reachable LLE domain.
        for id in 0x08u8..=0x0f {
            assert_eq!(
                u32::from(rdp_command_word_length(id)) * 8,
                fn64_render::raw_rdp_command_width(id).expect("triangle width is defined"),
                "command id {id:#04x}"
            );
        }
        // The two texrect ids agree as well: 2 words == 16 bytes.
        for id in [0x24u8, 0x25] {
            assert_eq!(
                u32::from(rdp_command_word_length(id)) * 8,
                fn64_render::raw_rdp_command_width(id).expect("texrect width is defined"),
                "command id {id:#04x}"
            );
        }
    }

    #[test]
    fn the_two_command_width_tables_agree_wherever_both_are_defined() {
        // The full cross-check, and the one structural difference between
        // the two owners, pinned rather than left implicit.
        //
        // `fn64_render::raw_rdp_command_width` returns None for 0x10..=0x23,
        // which it documents as deliberately-unaccepted "documented No
        // Operation" ids. RT64's table has no None: it returns 1 word for
        // every id it does not recognise. So the tables agree on every id
        // where fn64's is defined, and fn64's is strictly the narrower
        // domain.
        let mut both_defined = 0u32;
        let mut fn64_undefined = 0u32;
        for id in 0u8..=0x3f {
            match fn64_render::raw_rdp_command_width(id) {
                Some(bytes) => {
                    both_defined += 1;
                    assert_eq!(
                        u32::from(rdp_command_word_length(id)) * 8,
                        bytes,
                        "command id {id:#04x} disagrees between the two owners"
                    );
                }
                None => {
                    fn64_undefined += 1;
                    // RT64 always has an answer here, and it is always 1.
                    assert_eq!(rdp_command_word_length(id), 1);
                }
            }
        }
        // 0x10..=0x23 is 20 ids; the other 44 of the 64 are defined by both.
        assert_eq!(fn64_undefined, 0x24 - 0x10);
        assert_eq!(fn64_undefined, 20);
        assert_eq!(both_defined, 64 - 20);
        assert_eq!(both_defined, 44);
    }

    // ------------------------------------------------------------------
    // `moved_from_origin`
    // ------------------------------------------------------------------

    #[test]
    fn origin_none_and_above_pass_the_coordinate_through_untouched() {
        // The gate is `ori < G_EX_ORIGIN_NONE`, i.e. `< 0x800`. Everything
        // at or above returns x unchanged -- including the whole upper half
        // of the u16 domain.
        for ori in [0x800u16, 0x801, 0x1000, 0xffff] {
            for x in [i32::MIN, -1, 0, 1, 12_345, i32::MAX] {
                assert_eq!(
                    moved_from_origin(x, ori, 320),
                    x,
                    "ori {ori:#06x} must not move x"
                );
            }
        }
    }

    #[test]
    fn origin_boundary_is_exclusive_at_0x800() {
        // 0x7ff is inside the gate and moves; 0x800 is outside and does not.
        // Pins the strict `<` rather than `<=`.
        assert_ne!(moved_from_origin(0, 0x7ff, 320), 0);
        assert_eq!(moved_from_origin(0, 0x800, 320), 0);
    }

    #[test]
    fn hand_derived_origin_offsets() {
        // offset = ori * width * 4 / 1024. Hand-computed:
        //   ori=0x000 (LEFT),   w=320 -> 0*320*4/1024        = 0
        //   ori=0x200 (CENTER), w=320 -> 512*320*4/1024      = 640
        //   ori=0x400 (RIGHT),  w=320 -> 1024*320*4/1024     = 1280
        //   ori=0x200,          w=640 -> 512*640*4/1024      = 1280
        assert_eq!(moved_from_origin(0, 0x000, 320), 0);
        assert_eq!(moved_from_origin(0, 0x200, 320), 640);
        assert_eq!(moved_from_origin(0, 0x400, 320), 1280);
        assert_eq!(moved_from_origin(0, 0x200, 640), 1280);
        // The offset is added to x, not substituted for it.
        assert_eq!(moved_from_origin(7, 0x200, 320), 647);
        assert_eq!(moved_from_origin(-7, 0x200, 320), 633);
    }

    #[test]
    fn right_origin_offset_is_exactly_four_times_the_width() {
        // A structural property worth pinning independently of any single
        // number: G_EX_ORIGIN_RIGHT is the divisor, so ori == RIGHT makes
        // the offset exactly `width * 4` -- one full image width in the
        // quarter-pixel fixed-point space these coordinates live in.
        for width in [1u16, 2, 320, 640, 0xffff] {
            assert_eq!(
                moved_from_origin(0, 0x400, width),
                i32::from(width) * 4,
                "width {width}"
            );
        }
    }

    #[test]
    fn moved_from_origin_offset_agrees_with_its_reduced_derivation() {
        // Reconcile the literal `* 4 / 1024` against the algebraically
        // reduced `/ 256`, across the whole gated `ori` domain. If these
        // ever diverged, the literal form would be the one to trust, but
        // they must not diverge for non-negative operands.
        for ori in 0u16..0x800 {
            for width in [0u16, 1, 2, 3, 319, 320, 321, 639, 640, 0xffff] {
                let literal =
                    (i64::from(ori) * i64::from(width) * 4) / i64::from(G_EX_ORIGIN_RIGHT);
                let reduced = (i64::from(ori) * i64::from(width)) / 256;
                assert_eq!(literal, reduced, "ori {ori:#06x} width {width}");
                assert_eq!(moved_from_origin(0, ori, width), literal as i32);
            }
        }
    }

    #[test]
    fn origin_offset_product_never_overflows_i32() {
        // The bound claimed in the doc comment, asserted rather than
        // asserted-in-prose: max ori inside the gate is 0x7ff, max width is
        // 0xffff.
        let max_product = i64::from(0x7ffu16) * i64::from(u16::MAX) * 4;
        assert_eq!(max_product, 536_600_580);
        assert!(max_product <= i64::from(i32::MAX));
    }

    #[test]
    fn moved_from_origin_wrapping_add_pins_a_deviation() {
        // DEVIATION test. RT64 computes `x + offset` on int32_t; signed
        // overflow there is undefined behavior, so RT64 has no defined
        // answer for this input at all. This port takes the two's-complement
        // wrap. The test pins *our* choice; it does not claim RT64 agrees,
        // because RT64 does not have an answer to agree with.
        let offset = 640i32; // ori 0x200, width 320
        assert_eq!(
            moved_from_origin(i32::MAX, 0x200, 320),
            i32::MAX.wrapping_add(offset)
        );
        // Sanity: that really did wrap into the negatives.
        assert!(moved_from_origin(i32::MAX, 0x200, 320) < 0);
    }

    #[test]
    fn zero_width_makes_every_gated_origin_a_no_op() {
        // width == 0 zeroes the product regardless of ori, so a zero-width
        // colour image cannot move a coordinate. Pinned because it is the
        // one input where the gate is taken but has no effect.
        for ori in [0u16, 0x200, 0x400, 0x7ff] {
            assert_eq!(moved_from_origin(42, ori, 0), 42);
        }
    }

    // ------------------------------------------------------------------
    // `mask_address`
    // ------------------------------------------------------------------

    #[test]
    fn unextended_addresses_fold_into_the_24_bit_window() {
        // Hand-derived: 0x00123456 & 0xFFFFFF = 0x123456 (unchanged, already
        // inside). 0x80123456 & 0xFFFFFF = 0x123456 (tag stripped by mask).
        // 0xFFFFFFFF & 0xFFFFFF = 0xFFFFFF.
        assert_eq!(mask_address(0x0012_3456, false), 0x0012_3456);
        assert_eq!(mask_address(0x8012_3456, false), 0x0012_3456);
        assert_eq!(mask_address(0xffff_ffff, false), 0x00ff_ffff);
        assert_eq!(mask_address(0x0000_0000, false), 0);
    }

    #[test]
    fn extended_mode_subtracts_the_tag_and_keeps_31_bits() {
        // With extend_rdram on AND the tag set: 0x80123456 - 0x80000000 =
        // 0x00123456. Critically, a value that would NOT survive the 24-bit
        // mask survives here.
        assert_eq!(mask_address(0x8012_3456, true), 0x0012_3456);
        // 0x81234567 - 0x80000000 = 0x01234567, which is 25 bits wide and so
        // could not have come out of the masking branch.
        assert_eq!(mask_address(0x8123_4567, true), 0x0123_4567);
        assert!(mask_address(0x8123_4567, true) > RDP_ADDRESS_MASK);
        // 0xFFFFFFFF - 0x80000000 = 0x7FFFFFFF.
        assert_eq!(mask_address(0xffff_ffff, true), 0x7fff_ffff);
    }

    #[test]
    fn extended_mode_without_the_tag_still_masks() {
        // Both conditions are required. extend_rdram on but tag clear takes
        // the masking branch, identically to extend_rdram off.
        for address in [0x0000_0000u32, 0x0012_3456, 0x7fff_ffff, 0x0100_0000] {
            assert_eq!(
                mask_address(address, true),
                mask_address(address, false),
                "address {address:#010x} with tag clear must mask either way"
            );
            assert_eq!(mask_address(address, true), address & RDP_ADDRESS_MASK);
        }
    }

    #[test]
    fn extend_rdram_only_matters_when_the_tag_is_set() {
        // Exhaustive-in-structure: sweep the tag bit and the flag, and
        // confirm exactly one of the four combinations takes the
        // subtraction branch.
        let tagged = 0x8012_3456u32;
        let untagged = 0x0012_3456u32;
        assert_eq!(mask_address(tagged, true), 0x0012_3456); // subtraction
        assert_eq!(mask_address(tagged, false), 0x0012_3456); // mask
        assert_eq!(mask_address(untagged, true), 0x0012_3456); // mask
        assert_eq!(mask_address(untagged, false), 0x0012_3456); // mask
                                                                // Those all coincide for this address. Pick one where they do not:
        let high = 0x8123_4567u32;
        assert_eq!(mask_address(high, true), 0x0123_4567); // subtraction
        assert_eq!(mask_address(high, false), 0x0023_4567); // mask
        assert_ne!(mask_address(high, true), mask_address(high, false));
    }

    #[test]
    fn extended_branch_subtraction_and_bit_clear_agree_only_under_the_guard() {
        // RT64 writes `address - ExtendedMask`; `address & !ExtendedMask`
        // would be the "tidier" spelling. Under the guard (tag known set)
        // they agree...
        for address in [0x8000_0000u32, 0x8012_3456, 0xffff_ffff, 0x8000_0001] {
            assert_eq!(address & EXTENDED_MASK, EXTENDED_MASK, "guard precondition");
            assert_eq!(address - EXTENDED_MASK, address & !EXTENDED_MASK);
        }
        // ...and without it they do not, which is exactly why the guard is
        // load-bearing and the subtraction is not a free simplification.
        let untagged = 0x0012_3456u32;
        assert_eq!(untagged & EXTENDED_MASK, 0);
        assert_eq!(untagged & !EXTENDED_MASK, untagged);
        assert_ne!(untagged.wrapping_sub(EXTENDED_MASK), untagged);
    }

    // ------------------------------------------------------------------
    // `setKeyR` / `setKeyGB`
    // ------------------------------------------------------------------

    #[test]
    fn key_r_normalizes_both_operands_by_255() {
        // Hand-derived: 0/255 = 0, 255/255 = 1, 128/255 = 0.50196078...
        let zero = key_center_scale_r(0, 0);
        assert_eq!(zero.center, 0.0);
        assert_eq!(zero.scale, 0.0);

        let full = key_center_scale_r(255, 255);
        assert_eq!(full.center, 1.0);
        assert_eq!(full.scale, 1.0);

        let mid = key_center_scale_r(128, 64);
        assert_eq!(mid.center, 128.0f32 / 255.0);
        assert_eq!(mid.scale, 64.0f32 / 255.0);
    }

    #[test]
    fn key_r_keeps_center_and_scale_on_their_own_operands() {
        // Pins that cR feeds center and sR feeds scale, not the transpose.
        let channel = key_center_scale_r(10, 200);
        assert_eq!(channel.center, 10.0f32 / 255.0);
        assert_eq!(channel.scale, 200.0f32 / 255.0);
        assert_ne!(channel.center, channel.scale);
    }

    #[test]
    fn key_operands_above_255_are_not_clamped() {
        // RT64's parameters are uint32_t and it applies no mask, so an
        // out-of-byte-range operand normalizes above 1.0. Reproduced, not
        // defended against.
        let channel = key_center_scale_r(510, 255);
        assert_eq!(channel.center, 2.0);
        assert_eq!(channel.scale, 1.0);
        assert!(channel.center > 1.0);
    }

    #[test]
    fn set_key_gb_channel_grouping_matches_rt64_component_assignment() {
        // RT64 assigns keyCenter.y = cG, keyCenter.z = cB, keyScale.y = sG,
        // keyScale.z = sB. This port regroups by channel; the test pins that
        // the regrouping puts each operand on the component RT64 names, so
        // green and blue cannot be silently transposed.
        let (green, blue) = key_center_scale_gb(1, 2, 3, 4);
        assert_eq!(green.center, 1.0f32 / 255.0, "keyCenter.y <- cG");
        assert_eq!(green.scale, 2.0f32 / 255.0, "keyScale.y <- sG");
        assert_eq!(blue.center, 3.0f32 / 255.0, "keyCenter.z <- cB");
        assert_eq!(blue.scale, 4.0f32 / 255.0, "keyScale.z <- sB");
        // All four distinct, so no pair can be swapped without failing.
        let values = [green.center, green.scale, blue.center, blue.scale];
        for i in 0..values.len() {
            for j in (i + 1)..values.len() {
                assert_ne!(values[i], values[j], "values {i} and {j} must differ");
            }
        }
    }

    #[test]
    fn key_gb_uses_the_same_divisor_as_key_r_and_as_color4() {
        // Mutual corroboration against this crate's existing owner: the
        // /255.0 normalization here must be identical to
        // `state::Color4::normalized`'s, which was ported from the same
        // file's setEnvColor/setPrimColor/setBlendColor/setFogColor.
        for byte in 0u32..=255 {
            let via_key = key_center_scale_r(byte, byte).center;
            let via_color = crate::state::Color4::from_wire(byte << 24).normalized()[0];
            assert_eq!(via_key, via_color, "byte {byte}");
        }
        // And setKeyGB agrees with setKeyR on the same operand value.
        let (green, blue) = key_center_scale_gb(77, 77, 77, 77);
        let red = key_center_scale_r(77, 77);
        assert_eq!(green.center, red.center);
        assert_eq!(blue.center, red.center);
    }

    // ------------------------------------------------------------------
    // `setPrimDepth` -- the disagreement, made executable.
    // ------------------------------------------------------------------

    #[test]
    fn prim_depth_multiply_form_masks_z_to_15_bits() {
        // Bit 15 of z is discarded. 0x8000 masks to 0, and 0xFFFF masks to
        // 0x7FFF (the maximum, normalizing to exactly 1.0).
        assert_eq!(prim_depth_normalized_rt64(0x8000, 0).0, 0.0);
        assert_eq!(
            prim_depth_normalized_rt64(0xffff, 0).0,
            prim_depth_normalized_rt64(0x7fff, 0).0
        );
        // 0x7FFF * (1/32767) -- 32767 == 0x7FFF, so this is exactly 1.0 only
        // if the rounding cooperates. Assert what it actually is rather than
        // assuming: the reciprocal of 32767 rounds up in f32, so the product
        // is at least 1.0.
        let (z_max, _) = prim_depth_normalized_rt64(0x7fff, 0);
        assert!(
            (z_max - 1.0).abs() <= f32::EPSILON,
            "z_max was {z_max}, expected within 1 epsilon of 1.0"
        );
    }

    #[test]
    fn prim_depth_dz_mask_is_a_no_op_on_a_u16() {
        // RT64 writes `dz & 0xFFFFU`. On a u16 input that cannot remove a
        // bit; the mask is retained because RT64 writes it, and this pins
        // that retaining it changes nothing.
        for dz in [0u16, 1, 0x7fff, 0x8000, 0xffff] {
            let masked = prim_depth_normalized_rt64(0, dz).1;
            let unmasked = f32::from(dz) * (1.0f32 / 65535.0);
            assert_eq!(masked, unmasked, "dz {dz:#06x}");
        }
    }

    #[test]
    fn prim_depth_multiply_and_divide_forms_disagree_on_known_inputs() {
        // THE DISAGREEMENT. RT64 multiplies by an f32 reciprocal; this
        // crate's `state::PrimDepth` divides. They are not the same
        // function.
        //
        // Hand-identified smallest disagreeing inputs (found by exhaustive
        // sweep, restated here as fixed constants so the test does not
        // depend on the sweep): z = 513, dz = 257.
        let (z_mul, _) = prim_depth_normalized_rt64(513, 0);
        let z_div = 513.0f32 / 32767.0;
        assert_ne!(
            z_mul.to_bits(),
            z_div.to_bits(),
            "z=513 must differ between the multiply and divide forms"
        );
        // 1 ULP apart, in the direction the double-rounding predicts.
        assert_eq!((z_mul.to_bits() as i64 - z_div.to_bits() as i64).abs(), 1);

        let (_, dz_mul) = prim_depth_normalized_rt64(0, 257);
        let dz_div = 257.0f32 / 65535.0;
        assert_ne!(dz_mul.to_bits(), dz_div.to_bits(), "dz=257 must differ");
        assert_eq!((dz_mul.to_bits() as i64 - dz_div.to_bits() as i64).abs(), 1);
    }

    #[test]
    fn prim_depth_disagreement_population_is_exactly_as_measured() {
        // The counts quoted in the doc comment, recomputed here over the
        // whole admitted domain so the prose cannot drift from the code.
        let z_differences = (0u16..=0x7fff)
            .filter(|&z| {
                prim_depth_normalized_rt64(z, 0).0.to_bits() != (f32::from(z) / 32767.0).to_bits()
            })
            .count();
        assert_eq!(z_differences, 768, "z disagreement population");

        let dz_differences = (0u32..=0xffff)
            .map(|dz| dz as u16)
            .filter(|&dz| {
                prim_depth_normalized_rt64(0, dz).1.to_bits() != (f32::from(dz) / 65535.0).to_bits()
            })
            .count();
        assert_eq!(dz_differences, 512, "dz disagreement population");

        // Every difference is exactly 1 ULP -- no input diverges further.
        for z in 0u16..=0x7fff {
            let mul = prim_depth_normalized_rt64(z, 0).0.to_bits() as i64;
            let div = (f32::from(z) / 32767.0).to_bits() as i64;
            assert!((mul - div).abs() <= 1, "z {z} diverged by more than 1 ULP");
        }
    }

    #[test]
    fn prim_depth_forms_agree_on_the_vast_majority_of_inputs() {
        // The complement of the disagreement: 32768 - 768 = 32000 z values
        // agree, and 65536 - 512 = 65024 dz values agree. Stated so the
        // finding is not mistaken for "the two forms are unrelated".
        let z_agreements = (0u16..=0x7fff)
            .filter(|&z| {
                prim_depth_normalized_rt64(z, 0).0.to_bits() == (f32::from(z) / 32767.0).to_bits()
            })
            .count();
        assert_eq!(z_agreements, 32_768 - 768);
        assert_eq!(z_agreements, 32_000);
    }

    #[test]
    fn prim_depth_reciprocals_are_the_pinned_f32_bit_patterns() {
        // Derive the two reciprocal constants a second way: as exact f32 bit
        // patterns, independently of the division that produces them.
        let r15: f32 = 1.0 / 32767.0;
        let r16: f32 = 1.0 / 65535.0;
        assert_eq!(r15.to_bits(), 0x3800_0100);
        assert_eq!(r16.to_bits(), 0x3780_0080);
        // And confirm they are genuinely not the exact mathematical
        // reciprocals -- which is the whole reason the two forms differ.
        assert_ne!(f64::from(r15), 1.0f64 / 32767.0);
        assert_ne!(f64::from(r16), 1.0f64 / 65535.0);
    }
}
