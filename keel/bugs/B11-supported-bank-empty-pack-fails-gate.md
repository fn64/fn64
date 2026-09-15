---
name: Gate fails on a Supported bank's empty block pack
covers: [C3]
reported: 2026-09-15
---
repro: second full campaign (rev fa3c5051): 32 ROMs fail pack with
`FAILED: emitting block pack for <bank>: NoProvenBlocks`, where <bank> is
`relocated_slice_0` or `untabled_region_0`; 27 of them packed and
certified on the first campaign (e.g. Super Mario 64, Starcraft 64).
expected: a Supported bank contributes its VA range to closure geometry
(K20) and no block pack; the gate emits packs for proven banks only and
still reports the ROM's headline
actual: K20 composes every Supported bank and the emitter refuses a bank
with zero proven blocks, so the whole ROM fails before HEADLINE.
funnel effect: recompile frontiers fell 44 -> 27 but pack frontiers rose
20 -> 52; net certified 149 -> 134. Must be fixed before any merge.
