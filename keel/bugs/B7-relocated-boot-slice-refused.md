---
name: Relocated boot-image slice is refused as a duplicate mapping
status: fixed
fixed_by: K18
covers: [C3]
reported: 2026-09-15
---
repro: cold discovery on a ROM whose boot code copies a slice of the IPL3
image to a second VA and calls it there (7 corpus ROMs; see
docs/plans/corpus-campaign-2026-09-15.md)
expected: the slice is admitted as a Supported mapping at the relocated VA
(>= 3 proven direct-call targets land on prologues under one delta)
actual: delta-vote never votes with boot-bank calls, and the untabled
strategy drops any region overlapping the IPL3 copy in ROM space; the
recompile gate reports N `outside_all_mappings` destinations

fix: `delta_vote::relocated_slice_vote` plus a `relocated_slice_vote` step at
the end of the untabled strategy (K18). All 7 corpus ROMs now admit one
`relocated_slice_0` mapping, Supported and never Proven, at exactly the delta
the campaign report predicted; the per-ROM vote/runner-up/source counts are
in that report's "Measured outcome" section.

follow-up (separate ticket, not a regression): the recompile gate's
`unsupported` count does not move, because a Supported mapping never reaches
the execution closure -- `closure::ProgramGeometry` takes `mapped` from
`proven_bank_images()` and `prepare_snapshot_banks_with_limits` composes only
`Proven` banks. The same wall already hides every `untabled_region_*`
mapping. Lifting it means deciding what a Supported mapping means downstream
(the honest class is `mapped_not_proven_code`, i.e. interpreter-covered
`dynamic_mips`, not a release blocker) -- a `closure.rs` change affecting
every ROM and every gate.
