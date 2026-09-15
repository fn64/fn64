---
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
