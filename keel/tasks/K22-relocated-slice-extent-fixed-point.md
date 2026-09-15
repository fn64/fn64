---
covers: [B7, B8, C3, T10]
depends: [K20]
pitch: "A relocated slice grows to the extent its own composed code calls, so the seven K18 titles' unsupported counts finally fall instead of rising."
---
K20 made a Supported bank reach the execution closure and measured what
that exposes: the K18 extents are too small. Full numbers in
`docs/plans/corpus-campaign-2026-09-15.md`, "Measured outcome of K20".

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
