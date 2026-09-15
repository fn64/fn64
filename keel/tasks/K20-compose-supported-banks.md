---
status: done
covers: [B8, C3, T10]
depends: [K18]
pitch: "Code reached only through a Supported mapping runs in the interpreter lane instead of failing the gate, so untabled recoveries finally count."
---
B8 approved 2026-09-15. Let closure treat words
inside a Supported bank as `mapped_not_proven_code` (dynamic_mips) and let
snapshot composition pack Supported banks under their own name, with the
Supported/Proven distinction preserved in every receipt and report. Red
test on the seven K18 ROMs (unsupported must fall), AKI ROMs must stay at
unsupported=0, grade-all and gate-determinism must not change.

deliverables:
- crates/fn64-discover/src/closure.rs
- crates/fn64-discover/src/snapshot_workspace.rs
- crates/fn64-discover/tests/relocated_slice_vote.rs

verification:
- cargo nextest run -p fn64-discover
- scripts/grade-all.sh
- scripts/corpus-campaign.zsh --rom-dir <ntsc> --rom-ids <the 7 ids>

## Implemented and measured 2026-09-15

The semantics are in and verified; the numeric claim is NOT met, so this
stays open behind K22. Full numbers and diagnosis:
`docs/plans/corpus-campaign-2026-09-15.md`, "Measured outcome of K20".

Landed:
- `closure::ProgramGeometry` carries a SUPPORTED-mapped range set beside its
  proven one. A concrete destination inside it is `mapped_not_proven_code`
  (`dynamic_mips`); one inside no bank is still `outside_all_mappings`;
  a proven owner/block still wins. `facts::BankAdmissionV1` names the two
  admission levels, `FactDb::supported_bank_images()` is a SEPARATE accessor
  from `proven_bank_images()`, and
  `compose_materialized_banks_admitting_supported_v2_with_limits` is a new
  entry point the recompile gate alone uses. Every other composer caller --
  graders, `cold_sweep`, gate-determinism -- keeps `ProvenOnly` byte for byte.
- Block proof and owner proof still resolve backing `ProvenOnly`, so a
  Supported bank contributes zero proven blocks, zero exact owners, and zero
  exact-AOT/block-AOT bytes. Nothing is relabelled Proven.
- `fn64.rom-recompile-report.v1` gains `supported_banks` (serde default)
  beside `banks`; the gate prints `proven_banks=`/`supported_banks=` and one
  line per Supported bank. `diagnose-cold-unsupported` reports
  `supported_bank_count`.

Verified: `cargo nextest run -p fn64-discover` 1103/1103 pass; with
FN64_K18_ROM_A/B set, all 5 tests in `tests/supported_bank_composed.rs` pass.
grade-all byte-identical to HEAD (nwxe-donor 779/1, nwxe-solo 726/0,
nw4e-donor 925/0, nw4e-solo 873/0, revenge-solo 597/0). gate-closure
765e6349. gate_overlay_regions still drifts at dc7d29a8 exactly as at HEAD.
Discovery admits zero Supported banks on WM2000, No Mercy and OoT.

Not met at the time: `unsupported` fell on 4 of 7 (Olympic Hockey 18->9,
Paperboy 17->12, NBA Showtime 12->3, F-Zero X 9->2), ROSE on 2 (NASCAR 2000
36->38, Waialae 33->50) and NASCAR 99 stopped composing
(`UnsupportedControlDelayEntry` in the boot bank). On Waialae all 33 prior
refusals retired and 50 new ones appeared, every one at a ROM offset past the
slice's end under the same delta -- the K18 extent was truncated.

**Closed by K22 the same day.** The extent fixed point grows a slice to cover
what its own composed code calls at the already-voted delta, and the delay-slot
case refuses with the entry named instead of throwing. Five of seven now reach
`unsupported == 0` (Waialae 33->0, Paperboy 17->0, NBA Showtime 12->0, F-Zero X
9->0, Olympic Hockey 18->0); NASCAR 2000 falls 36->18 and NASCAR 99 refuses its
slice. See "Measured outcome of K22" in the campaign plan.
