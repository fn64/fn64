---
status: done
covers: [B7, B8, C3, T10]
depends: [K20]
pitch: "A relocated slice grows to the extent its own composed code calls, so the seven K18 titles' unsupported counts finally fall instead of rising."
---
K20 made a Supported bank reach the execution closure and measured what
that exposes: the K18 extents are too small. Full numbers in
`docs/plans/corpus-campaign-2026-09-15.md`, "Measured outcome of K22".

K18's extent rule is right for the VOTE's own evidence -- low end is the
first voted entry exactly (page-rounding it down manufactured a boot-bank
overlap on NASCAR 99), high end rounds one page past the last voted entry --
and measurably smaller than the region the copy installs. Once the slice
composes, its OWN calls land past both ends at the SAME delta:

- Waialae: all 33 prior refusals retire, 50 new ones appear, every one at
  ROM >= 0x72000 (the slice ends there) under delta 0x800b90c0.
- NASCAR 2000: 25 of 36 retire, 21 of 27 new ones sit at ROM
  0x9c164..0x9e608, just BELOW the slice start at 0x9e710.
- NASCAR 99: composition FAILS --
  `UnsupportedControlDelayEntry { bank: "boot", entry: 0x800fcad4 }`. A
  cross-bank call the slice implies lands in a delay slot, so at least one
  target its delta implies is not a function entry.

do:
1. After admitting a slice, iterate to a fixed point: compose it, take its
   in-slice calls that land outside every mapping under the ALREADY-VOTED
   delta, widen the extent to cover them, repeat until it stops growing.
   Every added byte is justified by a call from already-admitted code at a
   delta the vote already carried, so this adds no new hypothesis and no new
   delta. Keep the whole extent inside the ROM and VA-disjoint from every
   existing mapping, exactly as K18 requires.
2. Refuse the whole slice when the fixed point implies an authority root in
   a delay slot, with a recorded `Open` measurement naming the entry --
   never let it reach composition and fail the gate.
3. Bound the iteration (count and total bytes) and record the count, so a
   pathological ROM refuses rather than runs.

verification:
- the 7 K18 ROMs' `unsupported` must all FALL from 153, 36, 18, 17, 12, 33, 9
- `cargo nextest run -p fn64-discover`, including
  `tests/supported_bank_composed.rs` with FN64_K18_ROM_A/B set
- scripts/grade-all.sh unchanged; discovery must still admit zero Supported
  banks on WM2000, No Mercy and OoT

## Done 2026-09-15

All three deliverables landed.
`delta_vote::grow_relocated_slice_extent` is the pure growth step (one
iteration, no composer); `lib.rs::grow_relocated_slice_to_fixed_point`
iterates it against a scratch fact database, so nothing is written until the
extent settles. Bounds: `MAX_SLICE_GROWTH_ITERATIONS = 16`,
`MAX_SLICE_GROWN_BYTES = 4 MiB`; the iteration count is recorded in the
slice's own evidence note whenever the extent moved. The delay-slot case
refuses the whole slice with the entry and its control word named.

Measured: **five of seven reach unsupported == 0** (Waialae 33->0, Paperboy
17->0, NBA Showtime 12->0, F-Zero X 9->0, Olympic Hockey 18->0). NASCAR 2000
falls 36->18 and NASCAR 99 stays at 153 with its slice refused. Neither was
tuned; both remainders are reported with their placement in the plan:

- NASCAR 2000's 18 are 13 destinations at or above 0x80800000 (the value-set
  imprecision class the funnel already counts as not-missing-code) plus 5 on
  `BranchTaken` edges, never `Call`. Growth consumes calls only, because this
  mechanism's premise is that the destination is a function ENTRY; growing on
  a branch target would weaken that rule to move a number.
- NASCAR 99's slice is refused at delta 0x8006b520 because composing it
  implies an authority entry at 0x800fcad4, the delay slot of the control
  word at 0x800fcad0. That is strictly better than K20's behaviour, where
  composition threw and the gate reported nothing.

Verified: `cargo nextest run -p fn64-discover` 1110/1110; all 6 corpus tests
pass with FN64_K18_ROM_A/B and FN64_K22_ROM_C set; grade-all byte-identical
to HEAD (779/1, 726/0, 925/0, 873/0, 597/0); gate-closure 765e6349; both AKI
ROMs `unsupported=0` with `supported_banks=0`.
