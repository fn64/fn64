//! Literal port of RT64's RSP segmented-address translation
//! (`fromSegmented`/`fromSegmentedMasked`/`fromSegmentedMaskedPD`/
//! `maskPhysicalAddress`/`setSegment`), a literal port of the permitted MIT
//! RT64 Rust-port source pinned at commit
//! `5473732a822a4423b5696e7cb18fecc425a59875`
//! (`docs/RT64-PORT-AUTHORITY.md`), `src/hle/rt64_rsp.cpp`/`.h` (SHA-256 of
//! the whole files,
//! `7dfdf40254d44d92c247d9c876bb8ca55995927ad534981bd48868bb44f1f695` /
//! `832c092bf7021ec08a46de85c95d9973b69fa7c560ca96e43215c2fb18f54d95`), plus
//! the one relevant constant from `src/shared/rt64_f3d_defines.h` (SHA-256
//! of the whole file,
//! `1c0f2dbdafeaf4329c45961b33d232eafe62c775f59245a75e3a1acd6febcd93`):
//!
//! ```text
//! // src/hle/rt64_rsp.h:28
//! #define RSP_MAX_SEGMENTS            16
//!
//! // src/hle/rt64_rsp.h:188 (State inside struct RSP)
//! std::array<uint32_t, RSP_MAX_SEGMENTS> segments;
//!
//! // src/hle/rt64_rsp.h:236-240 (declarations)
//! template<uint32_t mask> uint32_t maskPhysicalAddress(uint32_t address);
//! uint32_t fromSegmented(uint32_t segAddress);
//! uint32_t fromSegmentedMasked(uint32_t segAddress);
//! uint32_t fromSegmentedMaskedPD(uint32_t segAddress);
//! void setSegment(uint32_t seg, uint32_t address);
//!
//! // src/hle/rt64_rsp.cpp:97-132
//! constexpr uint32_t ExtendedMask = 0x80000000U;
//!
//! // Masks addresses as the RSP DMA hardware would.
//! template<uint32_t mask> uint32_t RSP::maskPhysicalAddress(uint32_t address) {
//!     if (state->extended.extendRDRAM && ((address & ExtendedMask) == ExtendedMask)) {
//!         return address - ExtendedMask;
//!     }
//!     else {
//!         return address & mask;
//!     }
//! }
//!
//! // Performs a lookup in the segment table to convert the given address.
//! uint32_t RSP::fromSegmented(uint32_t segAddress) {
//!     if (state->extended.extendRDRAM && ((segAddress & ExtendedMask) == ExtendedMask)) {
//!         return segAddress;
//!     }
//!     else {
//!         return segments[((segAddress) >> 24) & 0x0F] + ((segAddress) & 0x00FFFFFF);
//!     }
//! }
//!
//! // Converts the given segmented address and then applies the RSP DMA physical address mask.
//! // Used in cases where the RSP performs a DMA with a segmented address as the input.
//! uint32_t RSP::fromSegmentedMasked(uint32_t segAddress) {
//!     return maskPhysicalAddress<0x00FFFFF8>(fromSegmented(segAddress));
//! }
//!
//! uint32_t RSP::fromSegmentedMaskedPD(uint32_t segAddress) {
//!     return maskPhysicalAddress<0x00FFFFFC>(fromSegmented(segAddress));
//! }
//!
//! void RSP::setSegment(uint32_t seg, uint32_t address) {
//!     assert(seg < RSP_MAX_SEGMENTS);
//!     segments[seg] = address;
//! }
//!
//! // src/hle/rt64_state.h:125-134 (the `bool` this port reads as `extend_rdram`)
//! struct Extended {
//!     ...
//!     bool extendRDRAM = false;
//!     ...
//! };
//! Extended extended;
//! ```
//!
//! **Reuse, not new type.** This module does **not** reuse
//! `fn64_render_ir::PhysicalAddress` for either the segment-table entries or
//! any of the translated addresses these functions return. `PhysicalAddress`
//! is fallibly-constructed (`try_new`/`with_layout` return
//! `Result<_, ValidationError>`) and hard-bounded at
//! `RDP_PHYSICAL_ADDRESS_BYTES` (24 bits, `0x0100_0000`) --
//! `crates/fn64-render-ir/src/address.rs:29-68`. The C++ `uint32_t address`/
//! `uint32_t segments[16]` have neither property: `maskPhysicalAddress`'s
//! `address - ExtendedMask` branch (taken when `extendRDRAM` is set and the
//! top bit is set) can legally produce any value in `0..0x8000_0000`, and
//! `fromSegmented`'s `segments[i] + (segAddress & 0x00FFFFFF)` is a plain
//! wrapping 32-bit unsigned add with no bound at all -- both routinely
//! exceed the 24-bit RDP window. Reusing `PhysicalAddress` here would
//! silently inject a fallible/bounded-construction contract the source does
//! not have and force every call site to handle a `Result` the C++ never
//! produces -- exactly the divergence `rt64_frame_compatibility.rs`'s
//! `M8.12` card already found and rejected for the same reason (its
//! `ColorImageFields`/`DepthImageFields::address` fields are plain `u32`,
//! not `PhysicalAddress`, for an identical "fallible/24-bit-bounded vs. bare
//! `uint32_t`" justification). This module represents every address as a
//! plain `u32`, and represents `RSP::segments` as a fixed `[u32; 16]` array
//! (an owned `SegmentTable` struct wrapping it, since `RSP_MAX_SEGMENTS ==
//! 16` is a compile-time constant on both sides and the source itself uses
//! `std::array<uint32_t, RSP_MAX_SEGMENTS>`, not a dynamically-sized
//! container).
//!
//! ## Admitted domain
//!
//! - **`RSP_MAX_SEGMENTS` is `16`**, ported as `SEGMENT_COUNT: usize = 16`.
//!   `SegmentTable` is `[u32; SEGMENT_COUNT]`, matching
//!   `std::array<uint32_t, RSP_MAX_SEGMENTS>` field-for-field (default-
//!   initialized to all zero, matching the two `segments.fill(0)` call
//!   sites in `rt64_rsp.cpp`'s constructor/reset, both out of this port's
//!   scope -- see Nonclaims -- but the zero-fill is reproduced here via
//!   `Default`).
//! - **`fromSegmented`'s segment-table index, `((segAddress) >> 24) &
//!   0x0F`, can never be out of range for the 16-entry array.** The `>> 24`
//!   isolates `segAddress`'s top byte (any `u32` value, `0x00`..`0xFF`
//!   after the shift), and `& 0x0F` then masks that byte down to exactly
//!   `0..15` -- which is precisely `SegmentTable`'s valid index range. So
//!   for `fromSegmented`, there is no wrap/clamp/out-of-bounds case to
//!   characterize: the masking *is* the range guarantee, by construction,
//!   for every one of the 2^32 possible `segAddress` inputs. This is
//!   verified directly (index 0..15 exhaustively, plus the wraparound
//!   boundary at index 15/16 where bit 28 rolls the `& 0x0F` back to 0) in
//!   the tests below.
//! - **`setSegment`'s `seg` parameter has no such structural guarantee** --
//!   it is a caller-supplied raw index, not shifted/masked from an address.
//!   The source's only guard is `assert(seg < RSP_MAX_SEGMENTS)`, a
//!   debug-only precondition (`NDEBUG` compiles it out in release, and RT64
//!   ships release builds to players -- see `rt64_common.rs`'s established
//!   "debug-only `assert()` preconditions become `debug_assert!`"
//!   precedent, which this port follows exactly). In a *release* C++
//!   build with the assert compiled out, `segments[seg] = address` for
//!   `seg >= 16` is an out-of-bounds array write -- silent memory
//!   corruption (C++ UB), not a wrap or a clamp. This port's
//!   `set_segment` reproduces the debug-only guard as `debug_assert!(seg <
//!   SEGMENT_COUNT)` and then indexes with `self.segments[seg] = address`
//!   in both build profiles; Rust's `[T; N]` indexing is *always*
//!   bounds-checked and panics on out-of-range access (in both debug and
//!   release), which is a deliberate, admitted divergence from the C++
//!   release-mode UB -- Rust has no UB-preserving raw-array write to fall
//!   back to, and a loud release-mode panic is strictly safer than silent
//!   corruption, matching `rt64_common.rs`'s established "no UB-preserving
//!   cast exists, so this is an intentional admitted divergence" precedent
//!   for the analogous `modifyMatrix4x4Integer`/`Fraction` float-to-int
//!   cast. Because this divergence only manifests for `seg >= 16` (out of
//!   this port's exhaustive 0..15 in-range test sweep), the "one past the
//!   end" test for `set_segment` is a `#[should_panic]` test, documented
//!   as exercising the *admitted-divergent* release-mode path, not the
//!   debug-only assert.
//! - **Mask widths, exactly, both used by `fromSegmentedMasked`/PD via the
//!   `mask` const generic parameter (C++ non-type template parameter):**
//!   `fromSegmentedMasked` uses `0x00FFFFF8` (low 3 bits cleared -- 8-byte
//!   DMA alignment, top 8 bits cleared -- confines the result to the
//!   low 24 bits), `fromSegmentedMaskedPD` uses `0x00FFFFFC` (low 2 bits
//!   cleared -- 4-byte alignment, same top-8-bits-cleared 24-bit window).
//!   Both are ported as plain `u32` function parameters (`mask: u32`)
//!   rather than a Rust const generic, since the two call sites
//!   (`fromSegmentedMasked`/`fromSegmentedMaskedPD`) are both ported
//!   directly in this module with their literal mask values inlined at
//!   the call, so no generic parameterization is needed to preserve the
//!   template's behavior -- `mask_physical_address` itself stays generic
//!   over any `u32` mask value, matching the C++ template's genericity,
//!   just via a runtime parameter instead of a compile-time one (this is a
//!   representation choice with no behavior difference, since the C++
//!   template is instantiated at exactly these two literal values and
//!   Rust monomorphization vs. a runtime `u32` parameter are
//!   observationally identical for a pure masking function).
//! - **The extended-RDRAM branch, `address - ExtendedMask` /
//!   `return segAddress` unchanged**, is gated on `extend_rdram &&
//!   ((address_or_seg_address & 0x8000_0000) == 0x8000_0000)` -- i.e. bit
//!   31 set *and* the `extend_rdram` flag true. `ExtendedMask` is
//!   `0x8000_0000` (bit 31). When taken in `maskPhysicalAddress`, `address
//!   - ExtendedMask` is unsigned subtraction of a value with bit 31 set
//!   minus `0x8000_0000` -- this **cannot** underflow (the `&` check
//!   already proved bit 31 is set, so `address >= 0x8000_0000`), so it is
//!   ordinary non-wrapping subtraction in both C++ and Rust; ported as
//!   plain `address - EXTENDED_MASK` (not `wrapping_sub`, since the
//!   precondition makes wraparound structurally unreachable -- verified in
//!   the tests below at the exact boundary `address == 0x8000_0000`).
//!   When `extend_rdram` is `false`, this branch is never taken regardless
//!   of bit 31 -- ported as the same `&&` short-circuit, same order.
//! - **`fromSegmented`'s non-extended branch, `segments[i] +
//!   (segAddress & 0x00FFFFFF)`, is an unchecked 32-bit unsigned add.** In
//!   C++, unsigned integer overflow is well-defined modular (wraparound)
//!   arithmetic, not UB. Rust's `+` on `u32` panics on overflow in debug
//!   builds and silently wraps in release -- to reproduce the C++
//!   wraparound-is-defined-and-intentional semantics in *both* Rust build
//!   profiles (matching the literal behavior, not Rust's debug-only
//!   overflow-check convention), this port uses `wrapping_add` explicitly,
//!   not plain `+`. This is called out explicitly because it is the
//!   opposite choice from `rt64_common.rs`'s `scaled`'s plain `<<` (where
//!   overflow was judged to be outside the legal input domain and a debug
//!   panic was accepted as a legitimate loud trap) -- here, by contrast,
//!   the segment table plus a 24-bit offset summing past `u32::MAX` is a
//!   real, reachable, and C++-well-defined case (e.g. a segment base near
//!   `u32::MAX` with a nonzero low-24-bit offset), not a violated
//!   precondition, so silently panicking in debug would be a widened claim
//!   relative to the source, and `wrapping_add` is the literal-behavior
//!   choice.
//! - **Signedness:** every value in this module (addresses, the mask
//!   constants, the segment table, `seg` indices) is `uint32_t`/unsigned in
//!   the source; ported as `u32` throughout, with no signed type
//!   introduced anywhere.
//! - **No other C++ UB is reproduced or diverged from in this module's
//!   ported functions.** The two float-adjacent `fixedToFloat`-style casts
//!   that `rt64_common.rs` had to admit do not appear here; every operation
//!   in the five ported functions (`&`, `>>`, `|`... no, this module has no
//!   `|`; `&`, `>>`, `+`/`wrapping_add`, `-`, `==`, `<`, array index) is
//!   either C++-well-defined unsigned arithmetic (ported identically) or
//!   the one admitted array-bounds divergence in `set_segment` called out
//!   above.
//!
//! ## Nonclaims
//!
//! No GPU, WGSL, or production wiring (this module is not called from
//! anywhere yet -- dead-code warnings on the unused public surface are
//! expected and correct, matching every other characterization-first
//! module's precedent in this crate), and no RT64 visual/pixel/silicon
//! parity or performance claim. Not wired to fn64's own RDRAM addressing
//! (`fn64-render-ir`'s `PhysicalAddress`/`PhysicalMemoryLayout`, or any
//! actual RDRAM buffer) -- these functions return bare `u32` byte offsets
//! with no connection to any real backing store in this crate.
//!
//! `rt64_rsp.cpp` is 1,314 lines and is, outside the six lines this module
//! ports (the `ExtendedMask` constant plus the five segmented-address
//! functions at lines 97-132), almost entirely an orchestrator over the
//! `RSP`/`State`/`Workload`/`DisplayList` object graph -- deliberately NOT
//! ported here, matching this ticket's explicit scope note. Specifically
//! not ported, by category:
//!
//! - **RSP lifecycle/reset**: `RSP::RSP`, `RSP::reset`,
//!   `getCurrentProjectionType`, `addCurrentProjection`, `clearExtended`,
//!   `extendRDRAM(bool)` (the *setter* method -- distinct from the
//!   `extended.extendRDRAM` `bool` field this module reads as an input
//!   parameter, see below).
//! - **Matrix/transform orchestrators**: `matrixCommon`, `matrix`,
//!   `popMatrix`, `insertMatrix`, `forceMatrix`, `recalculateMatrices`
//!   (viewport/scissor/light/fog/lookAt setters and every `set*`/`clear*`
//!   RSP-state mutator not in the five named functions).
//! - **Vertex/geometry orchestrators**: `setVertexCommon`, `setVertex`,
//!   `setVertexColor`, `setVertexNormal`, `modifyVertex`,
//!   `setVertexSegmentV1`, `readExtendedVertexSegment`, `drawIndexedTri`
//!   and all triangle/rect draw-call builders.
//! - **Display-list control flow**: `runDl`/branch handling, return-address
//!   stack push/pop, and the `fromSegmentedMasked` **call sites** inside
//!   those orchestrators (lines such as 845, 857, 903, 933, 971, 1041,
//!   1045, 1049, 1217 -- this module ports the five *callee* functions
//!   themselves, not their ~20 call sites' surrounding logic).
//! - **`extendRDRAM(bool isExtended)`**: the `void` setter method
//!   (`rt64_rsp.h:302`) that presumably flips `state->extended.extendRDRAM`
//!   (and does other, unexamined extended-RDRAM setup) is not ported --
//!   this module's functions take the already-resolved `bool` flag as a
//!   plain caller-supplied parameter (`extend_rdram: bool`), the same
//!   "decision logic over caller-supplied scalar inputs" pattern
//!   `rt64_frame_compatibility.rs`'s `M8.12` card established for
//!   `matrixDifference`.
//!
//! From `src/shared/rt64_f3d_defines.h`: only the segment-count relationship
//! (`RSP_MAX_SEGMENTS`, actually defined in `rt64_rsp.h`, not this header)
//! is relevant to this module's scope; this header's own contents (`G_MW_*`/
//! `G_MWO_SEGMENT_*`/every other F3D opcode and moveword-offset constant)
//! are opcode-decode plumbing for **other** already-ported or not-yet-ported
//! modules (`rt64_gbi_f3d.rs` etc.), not consumed by any of the five
//! functions this module ports, and are not re-declared here.

/// `RSP_MAX_SEGMENTS`: the fixed size of the RSP segment table
/// (`src/hle/rt64_rsp.h:28`).
pub const SEGMENT_COUNT: usize = 16;

/// `constexpr uint32_t ExtendedMask = 0x80000000U;` (`rt64_rsp.cpp:97`).
pub const EXTENDED_MASK: u32 = 0x8000_0000;

/// Mask literal at `fromSegmentedMasked`'s `maskPhysicalAddress<...>` call
/// site (`rt64_rsp.cpp:122`): clears the top 8 bits (24-bit window) and the
/// low 3 bits (8-byte DMA alignment).
pub const MASK_SEGMENTED: u32 = 0x00FF_FFF8;

/// Mask literal at `fromSegmentedMaskedPD`'s `maskPhysicalAddress<...>` call
/// site (`rt64_rsp.cpp:126`): clears the top 8 bits (24-bit window) and the
/// low 2 bits (4-byte alignment).
pub const MASK_SEGMENTED_PD: u32 = 0x00FF_FFFC;

/// `RSP::segments`: `std::array<uint32_t, RSP_MAX_SEGMENTS>`
/// (`rt64_rsp.h:188`). See module doc "Reuse, not new type" for why this is
/// a plain fixed array rather than `fn64_render_ir::PhysicalAddress`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentTable {
    segments: [u32; SEGMENT_COUNT],
}

impl SegmentTable {
    /// All-zero table, matching the two `segments.fill(0)` call sites in
    /// the (unported) `RSP::RSP`/`RSP::reset` constructors.
    pub const fn new() -> Self {
        Self {
            segments: [0; SEGMENT_COUNT],
        }
    }

    /// `RSP::setSegment(seg, address)` (`rt64_rsp.cpp:129-132`). C++
    /// `assert(seg < RSP_MAX_SEGMENTS)` is debug-only -- ported as
    /// `debug_assert!` (see module doc "Admitted domain"). For `seg >=
    /// SEGMENT_COUNT` this panics via Rust's always-bounds-checked array
    /// indexing, an admitted divergence from the C++ release-mode
    /// out-of-bounds write (UB) -- see module doc.
    pub fn set_segment(&mut self, seg: usize, address: u32) {
        debug_assert!(seg < SEGMENT_COUNT);
        self.segments[seg] = address;
    }

    /// Reads segment table entry `seg` directly (not part of the C++ public
    /// surface, but needed to observe `set_segment`'s effect from tests
    /// without invoking `fromSegmented`).
    pub fn get(&self, seg: usize) -> u32 {
        self.segments[seg]
    }

    /// `RSP::fromSegmented(segAddress)` (`rt64_rsp.cpp:110-117`).
    /// `extend_rdram` stands in for the caller-resolved
    /// `state->extended.extendRDRAM` `bool` field (see module doc
    /// "Nonclaims").
    pub fn from_segmented(&self, seg_address: u32, extend_rdram: bool) -> u32 {
        if extend_rdram && ((seg_address & EXTENDED_MASK) == EXTENDED_MASK) {
            seg_address
        } else {
            let index = ((seg_address >> 24) & 0x0F) as usize;
            self.segments[index].wrapping_add(seg_address & 0x00FF_FFFF)
        }
    }

    /// `RSP::fromSegmentedMasked(segAddress)` (`rt64_rsp.cpp:121-123`).
    pub fn from_segmented_masked(&self, seg_address: u32, extend_rdram: bool) -> u32 {
        mask_physical_address(
            self.from_segmented(seg_address, extend_rdram),
            MASK_SEGMENTED,
            extend_rdram,
        )
    }

    /// `RSP::fromSegmentedMaskedPD(segAddress)` (`rt64_rsp.cpp:125-127`).
    pub fn from_segmented_masked_pd(&self, seg_address: u32, extend_rdram: bool) -> u32 {
        mask_physical_address(
            self.from_segmented(seg_address, extend_rdram),
            MASK_SEGMENTED_PD,
            extend_rdram,
        )
    }
}

impl Default for SegmentTable {
    fn default() -> Self {
        Self::new()
    }
}

/// `template<uint32_t mask> uint32_t RSP::maskPhysicalAddress(uint32_t
/// address)` (`rt64_rsp.cpp:100-107`). The C++ non-type template parameter
/// `mask` is ported as a plain runtime `mask: u32` parameter (see module doc
/// "Admitted domain" -- observationally identical for a pure masking
/// function, since the template is instantiated at exactly two literal
/// call-site values in this port's scope).
pub fn mask_physical_address(address: u32, mask: u32, extend_rdram: bool) -> u32 {
    if extend_rdram && ((address & EXTENDED_MASK) == EXTENDED_MASK) {
        address - EXTENDED_MASK
    } else {
        address & mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- mask_physical_address: non-extended branch (plain `address & mask`) ---

    #[test]
    fn mask_physical_address_zero_address_is_zero() {
        assert_eq!(mask_physical_address(0, MASK_SEGMENTED, false), 0);
    }

    #[test]
    fn mask_physical_address_extend_rdram_false_ignores_bit_31() {
        // Bit 31 set, but extend_rdram is false: plain `& mask` path taken.
        let addr = 0x8012_3456;
        assert_eq!(
            mask_physical_address(addr, MASK_SEGMENTED, false),
            addr & MASK_SEGMENTED
        );
    }

    #[test]
    fn mask_physical_address_max_address_segmented_mask() {
        assert_eq!(
            mask_physical_address(u32::MAX, MASK_SEGMENTED, false),
            u32::MAX & MASK_SEGMENTED
        );
        assert_eq!(u32::MAX & MASK_SEGMENTED, 0x00FF_FFF8);
    }

    #[test]
    fn mask_physical_address_max_address_segmented_pd_mask() {
        assert_eq!(
            mask_physical_address(u32::MAX, MASK_SEGMENTED_PD, false),
            0x00FF_FFFC
        );
    }

    #[test]
    fn mask_physical_address_clears_low_three_bits_at_segmented_mask_boundary() {
        // 0x0010_0007's low 3 bits (0x07) are entirely below the mask's
        // clear boundary -- they vanish, leaving the 0x8-aligned base.
        let addr = 0x0010_0007;
        assert_eq!(
            mask_physical_address(addr, MASK_SEGMENTED, false),
            0x0010_0000
        );
        // 0x0010_0008 is already 8-byte aligned -- masking is a no-op.
        let addr2 = 0x0010_0008;
        assert_eq!(
            mask_physical_address(addr2, MASK_SEGMENTED, false),
            0x0010_0008
        );
    }

    #[test]
    fn mask_physical_address_clears_low_two_bits_at_pd_mask_boundary() {
        // 0x0010_0003's low 2 bits (0x03) are entirely below the PD mask's
        // clear boundary -- they vanish, leaving the 0x4-aligned base.
        let addr = 0x0010_0003;
        assert_eq!(
            mask_physical_address(addr, MASK_SEGMENTED_PD, false),
            0x0010_0000
        );
        // 0x0010_0004 is already 4-byte aligned -- masking is a no-op.
        let addr2 = 0x0010_0004;
        assert_eq!(
            mask_physical_address(addr2, MASK_SEGMENTED_PD, false),
            0x0010_0004
        );
    }

    #[test]
    fn mask_physical_address_clears_top_eight_bits_one_bit_above_24_bit_window() {
        // Bit 24 set (0x0100_0000), one bit above the 24-bit window --
        // must be masked away by both MASK_SEGMENTED and MASK_SEGMENTED_PD
        // when extend_rdram is false, proving masking (not corruption).
        let addr = 0x0100_0000;
        assert_eq!(mask_physical_address(addr, MASK_SEGMENTED, false), 0);
        assert_eq!(mask_physical_address(addr, MASK_SEGMENTED_PD, false), 0);
    }

    #[test]
    fn mask_physical_address_top_of_24_bit_window_survives_masking() {
        // The highest byte still inside the 24-bit window: 0x00FF_FFFF.
        let addr = 0x00FF_FFFF;
        assert_eq!(
            mask_physical_address(addr, MASK_SEGMENTED, false),
            0x00FF_FFF8
        );
        assert_eq!(
            mask_physical_address(addr, MASK_SEGMENTED_PD, false),
            0x00FF_FFFC
        );
    }

    // --- mask_physical_address: extended branch (`address - ExtendedMask`) ---

    #[test]
    fn mask_physical_address_extended_branch_at_exact_boundary() {
        // address == ExtendedMask exactly: subtraction lands on zero, no
        // underflow.
        assert_eq!(
            mask_physical_address(EXTENDED_MASK, MASK_SEGMENTED, true),
            0
        );
    }

    #[test]
    fn mask_physical_address_extended_branch_max_address() {
        assert_eq!(
            mask_physical_address(u32::MAX, MASK_SEGMENTED, true),
            u32::MAX - EXTENDED_MASK
        );
        assert_eq!(u32::MAX - EXTENDED_MASK, 0x7FFF_FFFF);
    }

    #[test]
    fn mask_physical_address_extended_branch_ignores_mask_entirely() {
        // Extended branch result is `address - ExtendedMask`, completely
        // bypassing the `mask` parameter (proves the mask has zero effect
        // once the extended branch is taken).
        let addr = EXTENDED_MASK | 0x0000_0007; // low 3 bits set
        assert_eq!(
            mask_physical_address(addr, MASK_SEGMENTED, true),
            0x0000_0007
        );
    }

    #[test]
    fn mask_physical_address_extended_branch_can_exceed_24_bit_window() {
        // Proves maskPhysicalAddress CAN produce an address outside the
        // installed-RDRAM/24-bit window when extend_rdram is set: the
        // result 0x7FFF_FFF8 is far larger than any MASK_SEGMENTED output.
        let addr = EXTENDED_MASK | 0x7FFF_FFF8;
        let result = mask_physical_address(addr, MASK_SEGMENTED, true);
        assert_eq!(result, 0x7FFF_FFF8);
        assert!(result > 0x00FF_FFFF);
    }

    #[test]
    fn mask_physical_address_bit_31_clear_with_extend_rdram_true_still_masks() {
        // extend_rdram is true, but bit 31 is NOT set: falls through to the
        // plain `& mask` branch, same as extend_rdram == false.
        let addr = 0x7FFF_FFFF;
        assert_eq!(
            mask_physical_address(addr, MASK_SEGMENTED, true),
            addr & MASK_SEGMENTED
        );
    }

    // --- from_segmented: segment index masking (0..15, wraparound at 16) ---

    #[test]
    fn from_segmented_every_segment_index_zero_through_fifteen() {
        // Exhaustively prove every 4-bit segment index resolves to its own
        // table entry, matching `((segAddress >> 24) & 0x0F)` exactly.
        let mut table = SegmentTable::new();
        for seg in 0..SEGMENT_COUNT {
            table.set_segment(seg, 0x1000 * (seg as u32 + 1));
        }
        for seg in 0..SEGMENT_COUNT {
            let seg_address = (seg as u32) << 24;
            let expected = table.get(seg) + 0; // offset bits are all zero
            assert_eq!(
                table.from_segmented(seg_address, false),
                expected,
                "seg={seg}"
            );
        }
    }

    #[test]
    fn from_segmented_index_wraps_at_bit_28_one_past_the_last_segment() {
        // Segment index 16 does not exist as a distinct case: bit 28 set
        // (segAddress >> 24 == 0x10) masked by & 0x0F wraps back to index 0,
        // proving the "one past the end" input still resolves in-range
        // (wrap, not out-of-bounds read) -- see module doc "Admitted
        // domain".
        let mut table = SegmentTable::new();
        table.set_segment(0, 0xAAAA_0000);
        table.set_segment(15, 0xBBBB_0000);
        let seg_address = 0x10_00_00_00u32; // (0x10 >> 0) & 0x0F == 0
        assert_eq!(table.from_segmented(seg_address, false), table.get(0));
    }

    #[test]
    fn from_segmented_offset_bits_are_added_to_segment_base() {
        let mut table = SegmentTable::new();
        table.set_segment(3, 0x0010_0000);
        let seg_address = (3u32 << 24) | 0x0000_1234;
        assert_eq!(table.from_segmented(seg_address, false), 0x0010_1234);
    }

    #[test]
    fn from_segmented_offset_is_masked_to_low_24_bits_before_adding() {
        // segAddress's own top byte selects the segment; only the low 24
        // bits are added as offset, even though the raw seg_address's
        // upper byte is nonzero there too (it's the segment selector, not
        // part of the offset).
        let mut table = SegmentTable::new();
        table.set_segment(5, 0);
        let seg_address = (5u32 << 24) | 0x00FF_FFFF;
        assert_eq!(table.from_segmented(seg_address, false), 0x00FF_FFFF);
    }

    #[test]
    fn from_segmented_zero_address_zero_segment_zero() {
        let table = SegmentTable::new();
        assert_eq!(table.from_segmented(0, false), 0);
    }

    #[test]
    fn from_segmented_max_address_wraps_via_wrapping_add() {
        // Proves the wraparound-is-defined semantics: segment base near
        // u32::MAX plus a nonzero offset wraps rather than panicking.
        let mut table = SegmentTable::new();
        table.set_segment(0, u32::MAX);
        let seg_address = 0x0000_0005u32; // segment 0, offset 5.
        assert_eq!(table.from_segmented(seg_address, false), 4); // wraps.
    }

    #[test]
    fn from_segmented_max_seg_address_all_segment_bits_and_offset_bits_set() {
        let mut table = SegmentTable::new();
        table.set_segment(15, 0);
        assert_eq!(table.from_segmented(u32::MAX, false), 0x00FF_FFFF);
    }

    // --- from_segmented: extended branch (returns segAddress unchanged) ---

    #[test]
    fn from_segmented_extended_branch_returns_input_unchanged_ignoring_table() {
        let mut table = SegmentTable::new();
        table.set_segment(7, 0xDEAD_BEEF); // would be selected by seg 7, but should be bypassed.
        let seg_address = EXTENDED_MASK | (7u32 << 24) | 0x0000_0001;
        assert_eq!(table.from_segmented(seg_address, true), seg_address);
    }

    #[test]
    fn from_segmented_extended_branch_requires_flag_true() {
        // Same bit-31-set address, but extend_rdram false: falls through to
        // the segment-table lookup instead.
        let mut table = SegmentTable::new();
        table.set_segment(0, 0x1000_0000);
        let seg_address = EXTENDED_MASK; // segment index (0x80 >> 0)... top byte 0x80 & 0x0F = 0.
        let via_extended = table.from_segmented(seg_address, true);
        let via_table = table.from_segmented(seg_address, false);
        assert_eq!(via_extended, seg_address);
        assert_eq!(via_table, 0x1000_0000);
        assert_ne!(via_extended, via_table);
    }

    #[test]
    fn from_segmented_extended_branch_exact_boundary_address() {
        let table = SegmentTable::new();
        assert_eq!(table.from_segmented(EXTENDED_MASK, true), EXTENDED_MASK);
    }

    // --- from_segmented_masked / from_segmented_masked_pd ---

    #[test]
    fn from_segmented_masked_composes_lookup_then_mask() {
        let mut table = SegmentTable::new();
        table.set_segment(2, 0x0020_0000);
        let seg_address = (2u32 << 24) | 0x0000_1237; // offset has low 3 bits set.
                                                      // Lookup: 0x0020_0000 + 0x0000_1237 = 0x0020_1237.
                                                      // Mask (0x00FFFFF8): clears low 3 bits -> 0x0020_1230.
        assert_eq!(table.from_segmented_masked(seg_address, false), 0x0020_1230);
    }

    #[test]
    fn from_segmented_masked_pd_uses_four_byte_alignment_not_eight() {
        let mut table = SegmentTable::new();
        table.set_segment(2, 0x0020_0000);
        let seg_address = (2u32 << 24) | 0x0000_1237;
        // Mask (0x00FFFFFC): clears low 2 bits -> 0x0020_1234, distinct
        // from the 8-byte-aligned from_segmented_masked result above.
        assert_eq!(
            table.from_segmented_masked_pd(seg_address, false),
            0x0020_1234
        );
    }

    #[test]
    fn from_segmented_masked_zero_address_zero_table_is_zero() {
        let table = SegmentTable::new();
        assert_eq!(table.from_segmented_masked(0, false), 0);
        assert_eq!(table.from_segmented_masked_pd(0, false), 0);
    }

    #[test]
    fn from_segmented_masked_max_address_zero_table() {
        let table = SegmentTable::new();
        // segment index 15, offset 0x00FFFFFF, table entry 0 -> lookup =
        // 0x00FFFFFF, then masked.
        assert_eq!(
            table.from_segmented_masked(u32::MAX, false),
            0x00FF_FFFF & MASK_SEGMENTED
        );
    }

    #[test]
    fn from_segmented_masked_extended_branch_bypasses_both_lookup_and_mask() {
        let mut table = SegmentTable::new();
        table.set_segment(0, 0xFFFF_FFFF);
        let seg_address = EXTENDED_MASK | 0x0000_0007; // low 3 bits set, would be cleared by MASK_SEGMENTED.
        assert_eq!(table.from_segmented_masked(seg_address, true), 0x0000_0007);
    }

    #[test]
    fn from_segmented_masked_pd_extended_branch_bypasses_both_lookup_and_mask() {
        let mut table = SegmentTable::new();
        table.set_segment(0, 0xFFFF_FFFF);
        let seg_address = EXTENDED_MASK | 0x0000_0003;
        assert_eq!(
            table.from_segmented_masked_pd(seg_address, true),
            0x0000_0003
        );
    }

    // --- set_segment ---

    #[test]
    fn set_segment_every_index_zero_through_fifteen_is_independently_addressable() {
        let mut table = SegmentTable::new();
        for seg in 0..SEGMENT_COUNT {
            table.set_segment(seg, 0x1_0000 + seg as u32);
        }
        for seg in 0..SEGMENT_COUNT {
            assert_eq!(table.get(seg), 0x1_0000 + seg as u32, "seg={seg}");
        }
    }

    #[test]
    fn set_segment_overwrites_previous_value() {
        let mut table = SegmentTable::new();
        table.set_segment(4, 0x1111_1111);
        table.set_segment(4, 0x2222_2222);
        assert_eq!(table.get(4), 0x2222_2222);
    }

    #[test]
    fn set_segment_zero_address_is_a_legal_value() {
        let mut table = SegmentTable::new();
        table.set_segment(0, 0xFFFF_FFFF);
        table.set_segment(0, 0);
        assert_eq!(table.get(0), 0);
    }

    #[test]
    fn set_segment_max_address_value() {
        let mut table = SegmentTable::new();
        table.set_segment(15, u32::MAX);
        assert_eq!(table.get(15), u32::MAX);
    }

    #[test]
    #[should_panic]
    fn set_segment_one_past_the_end_panics_admitted_divergence_from_cpp_release_ub() {
        // seg == SEGMENT_COUNT (16) is exactly one past the last valid
        // index (0..15). In a release C++ build (assert compiled out) this
        // is an out-of-bounds array write (UB); Rust's array indexing is
        // always bounds-checked and panics instead -- see module doc
        // "Admitted domain".
        let mut table = SegmentTable::new();
        table.set_segment(SEGMENT_COUNT, 0x1234_5678);
    }

    // --- SegmentTable::new / Default ---

    #[test]
    fn segment_table_new_is_all_zero() {
        let table = SegmentTable::new();
        for seg in 0..SEGMENT_COUNT {
            assert_eq!(table.get(seg), 0, "seg={seg}");
        }
    }

    #[test]
    fn segment_table_default_matches_new() {
        assert_eq!(SegmentTable::default(), SegmentTable::new());
    }
}
