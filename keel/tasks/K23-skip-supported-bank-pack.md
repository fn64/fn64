---
covers: [B11, T13, C3]
depends: [K22]
pitch: "A Supported bank no longer aborts the recompile gate, so the 27 titles that certified before K20 certify again and the relocated-slice titles keep their gain."
---
in gate_rom_recompile: emit block packs only for banks with proven blocks;
a Supported bank (relocated_slice_*, untabled_region_*) keeps its VA range
in ProgramGeometry and appears in `supported_banks`, but is skipped by
the emitter with a printed note, never a FAILED. Red test: synthetic
Supported bank with zero proven blocks must reach HEADLINE; corpus test on
Super Mario 64 (untabled_region_0) must certify at unsupported=0 as on the
first campaign, and Waialae must still reach 0.

deliverables:
- crates/fn64-discover/src/commands/gate_rom_recompile.rs
- crates/fn64-discover/tests/supported_bank_composed.rs

verification:
- cargo nextest run -p fn64-discover
- scripts/corpus-campaign.zsh --rom-dir <ntsc> --rom-ids <the 32 NoProvenBlocks ids> --skip-build
