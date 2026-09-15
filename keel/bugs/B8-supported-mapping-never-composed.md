---
name: Supported mappings never reach the execution closure
covers: [C3]
reported: 2026-09-15
---
repro: any ROM whose untabled strategy admits a Supported mapping
(`untabled_region_*`, `relocated_slice_*`); run gate-rom-recompile
expected: destinations inside a Supported bank classify as
`mapped_not_proven_code` (interpreter-covered `dynamic_mips`), not
`outside_all_mappings`; the seven K18 ROMs move toward unsupported=0
actual: `closure::ProgramGeometry` builds `mapped` from
`proven_bank_images()` and `prepare_snapshot_banks_with_limits` composes
only `Proven` banks, so every Supported mapping is invisible to the gate;
Paperboy selects `untabled_delta_vote` yet measures proven_bank_count == 1
decision needed (owner): whether a Supported bank may be packed and its
words classed `mapped_not_proven_code`. This changes what the release
gate counts for every ROM, so it is not dispatched without sign-off.
