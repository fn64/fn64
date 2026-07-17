# Roadmap — full Rust decomp/recomp pipeline + runtime

Decided 2026-07-16 (user + session). Three phases; R and D run as **parallel
wave tracks** (disjoint crates). Render endgame this phase: **RT64 as the
faithful renderer, wgpu port deferred to Phase P**. Executor mix:
**codex-heavy implementation waves, session-model adversarial verify + merge
gate** (see DELEGATION.md).

Status legend: `[ ]` open, `[~]` dispatched/in-flight, `[x]` merged+verified
(AGENTS.md bars). Update this file in the same commit as the work it tracks.

## Phase R — close the "OoT renders faithfully" gate

The standing milestone gate (OOT-STATUS.md "Beyond OoT") is unmet on main;
the work that closes it is rotting on stale pre-rename branches.

- [ ] **R1 salvage**: re-apply the combiner/blender/scissor/perspective-ST
  deltas from the `merge/render-final` stack (merge-base `f252c1e`, tip
  `e7ebf22`; the load-bearing delta is `raster.rs` +44, `gbi.rs` +13) onto
  post-rename main as fresh commits. Must preserve the G_DL tail-jump
  semantics landed in fn64#2. Gate: render-rt64 tests + snapshot + bounded
  reference-backend boot, 10 clean runs.
- [ ] **R2 projection artifact**: root-cause the Hyrule Field large-world
  title-camera projection artifact (OOT-STATUS.md "render fidelity" #1; the
  raw-eye-matrix hypothesis is already falsified — don't re-test it).
- [ ] **R3 eye-gate (user)**: batched side-by-side — RT64 frames + reference
  frames vs mupen64plus ground truth at fixed swaps, native res. No agent
  self-certifies this.
- [ ] **R4 branch hygiene**: after R1 lands, close/prune the five stale
  render worktrees+branches (they are then strictly-worse duplicates).
- [ ] **R5 audio out (user)**: physical cpal playback verification (needs
  ears; CoreAudio fails before stream creation on this machine so far).

## Phase D — fn64 owns discover → decomp

Today OoT symbol/section metadata comes entirely from the zeldaret decomp
via aki-recomp's Python (`import_oot_syms.py`, `gen_stubs.py`). The zeldaret
answer key (10,833 named fns) makes OoT the perfect graded target.
fn64-discover has Phases 1/2/4/5 + bounded-6; the rest is design-only
(DISCOVER-DESIGN.md).

- [x] **D1 Phase-3 candidate harvesting** (merged 2026-07-16): three
  deterministic providers (jal/jalr-target, prologue patterns,
  table-derived) feeding the proof-state model, graded via `gate_d1`.
  OoT combined 62.3% precision / 0.82% recall; NW4E 44.7%/89.0%;
  NWXE 36.4%/28.5%. STRUCTURAL FINDING: OoT recall is bounded by Phase 2 —
  only the boot bank is a discovered load-image (OoT overlays load via DMA
  tables, not descriptor tables), so detectors had ~nothing to hunt in.
  Table-derived is an honest 0 everywhere (descriptor tables prove
  load-images, not entry points).
- [ ] **D1.5 Phase-2 load-image discovery for DMA-table overlays** (the OoT
  shape): required before OoT detector recall can be meaningful; feeds D2.
- [ ] **D2 Phase-6 completion**: jump tables + value-set analysis for
  indirect targets (the bounded HI/LO case already works).
- [ ] **D3 Phases 7-8**: targeted dynamic probes; assembly/relink
  verification.
- [ ] **D4 pack emission**: emit fn64-owned `dump.toml`-equivalent
  (Decomp Pack) from discover output, replacing the aki-recomp Python for
  metadata production.
- [ ] **D-gate**: `recompile_rom` consumes the fn64-discovered pack for OoT
  and the boot is **byte-identical** (framebuffer SHA at fixed swaps) to the
  decomp-metadata build. Then produce the WM2000 pack with zero
  game-specific code.

## Phase P — pure-Rust endgame (after R-gate + D-gate)

- [ ] **P1**: retire the C lane to CI-oracle-only (DESIGN.md M3); relicense
  checkpoint.
- [ ] **P2**: wgpu render backend (RENDER-WGPU-PORT-PLAN.md), eye-gated
  against the then-verified RT64 output.
- [ ] **P3**: shell polish — input, audio device handling, windowing.
