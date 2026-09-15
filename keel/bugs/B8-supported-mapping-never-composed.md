---
name: Supported mappings never reach the execution closure
status: fixed
fixed_by: K20
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
decision (owner, 2026-09-15): APPROVED. A Supported bank may be packed
and its words classed `mapped_not_proven_code`; the Supported/Proven
distinction stays visible in every receipt and report.
fixed (K20, 2026-09-15): the wall itself is gone. A Supported bank composes
under its own name through
`compose_materialized_banks_admitting_supported_v2_with_limits`, its VA range
enters `ProgramGeometry` as SUPPORTED-mapped, and a destination in it is
`mapped_not_proven_code`. It is never relabelled: block and owner proof stay
`BankAdmissionV1::ProvenOnly`, so it yields zero proven blocks, zero exact
owners and zero AOT bytes, and `supported_banks` sits beside `banks` in the
report. Measured on the seven K18 ROMs at the time: 4 fell (Olympic Hockey
18->9, Paperboy 17->12, NBA Showtime 12->3, F-Zero X 9->2), 2 rose and 1
stopped composing -- all three because K18's slice EXTENTS were truncated, not
because the classification was wrong.
that remainder closed (K22, same day): the extent fixed point grows a slice to
cover what its own composed code calls at the already-voted delta, and a slice
implying a delay-slot entry is refused with that entry named. FIVE of the seven
now reach unsupported == 0 (Waialae 33->0, Paperboy 17->0, NBA Showtime 12->0,
F-Zero X 9->0, Olympic Hockey 18->0); NASCAR 2000 falls 36->18 on branch-edge
and beyond-RDRAM remainders, NASCAR 99 refuses its slice. grade-all and
gate-closure unchanged throughout; both AKI ROMs stay at unsupported=0 with
supported_banks=0.
