//! One module per former `fn64-discover` `[[bin]]` target. Each module's
//! `run` is the former bin's `main` body, moved here unchanged apart from
//! the `fn main()` -> `pub fn run(...)` rename, so a subcommand dispatch in
//! `main.rs` can call it directly instead of linking a separate binary
//! crate per gate. See docs/plans/CLEANUP-2026-09.md Task 2.3.

pub mod attribute_known_functions;
pub mod candidate_cfg_probe;
pub mod classify_callerless;
pub mod compare_computed_flows;
pub mod corpus_index;
pub mod diagnose_cold_unsupported;
pub mod diagnose_open_indirects;
pub mod fn64_discover_run;
pub mod gate_asm_roundtrip;
pub mod gate_b1;
pub mod gate_b2;
pub mod gate_callgraph_match;
pub mod gate_closure;
pub mod gate_content_consumer;
pub mod gate_corpus_homology;
pub mod gate_coverage;
pub mod gate_d1;
pub mod gate_d1_oot_overlays;
pub mod gate_d1_overlays;
pub mod gate_decomp_functions;
pub mod gate_decomp_reference;
pub mod gate_delta_vote;
pub mod gate_gp_base;
pub mod gate_homology;
pub mod gate_keys;
pub mod gate_loaders;
pub mod gate_oot_reference;
pub mod gate_overlay_generalize;
pub mod gate_overlay_regions;
pub mod gate_owners_overlays;
pub mod gate_recompiler_lint;
pub mod gate_recover_boundaries;
pub mod gate_regions;
pub mod gate_reloc_accuracy;
pub mod gate_rom_rebuild;
pub mod gate_rom_recompile;
pub mod gate_selector;
pub mod gate_static_closure;
pub mod gate_timing_diff;
pub mod gate_tool_jsonl;
pub mod gate_trace;
pub mod headless_bridge;
pub mod ingest_tool_claims;
pub mod produce_snapshot_workspace;
pub mod profile_overlay_regions;
pub mod rom_identity;
pub mod run_wm_writer_audit;
pub mod stage_snapshot_bank;
pub mod validate_candidate_receipts;
pub mod validate_executable_image_group;
pub mod validate_training_workspace;
