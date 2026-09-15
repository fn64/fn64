---
name: Gate fails on a Supported bank's empty block pack
status: fixed
fixed_by: K23
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
fixed (K23, 2026-09-15): the gate now asks `block_pack::snapshot_has_emittable_blocks`
before emitting, and skips a composed bank that holds no Proven and no
Installed block with a printed note (`<state> bank <name>: no proven blocks,
mapping only`). The skipped bank keeps its VA range in ProgramGeometry's
`supported_mapped` set, is still counted in `supported_banks`, and is still
listed in the report -- it just produces no pack. The emitter's own
`NoProvenBlocks` rule is unchanged. Re-running the 32 affected ROMs against
campaign-20260915-fa3c5051: pack 32/32 passed (was 0/32), recompile 25/32
passed. Corpus funnel pack 161 -> 193 of 213, recompile 134 -> 159 of 193, so
net certified is 159 -- above the first campaign's 149, which is the K18/K20/K22
relocated-slice gain finally collectable. Five of the seven relocated-slice
ROMs reach unsupported=0 and NASCAR 2000 (18) / NASCAR 99 (153) reproduce K22
exactly. The repro line's "27 of them ... certified on the first campaign" was
an estimate; measured against that campaign's own receipts it is 15. Numbers in
`docs/plans/corpus-campaign-2026-09-15.md`, "Measured outcome of K23".
