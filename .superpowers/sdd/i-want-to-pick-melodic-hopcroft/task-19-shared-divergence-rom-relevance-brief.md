# Task 19: BI_LERP_0-style ROM-relevance investigation of the shared wgpu+RT64 divergences

## Context
Fan-out Pass 1 found 7 `shared-ported-bug` cases where fn64-wgpu AND RT64 agree
with each other but BOTH diverge from bit-accurate hardware (angrylion). Two are
deliberate witnesses of the already-known BI_LERP_0 collapse
(`gen-loadblock-linear-missing-bilerp`, `gen-triangle-rgba32-missing-bilerp` —
expected to diverge, not under investigation). The other 5 group into 3 domains
and need the SAME treatment we gave BI_LERP_0: is this a real fidelity gap that a
ROM (especially WWF WrestleMania 2000) actually hits, or a corpus-only curiosity?

The 5 cases (from the parity triage, angrylion = ground truth):
- **CI4/CI8 textured-triangle via TLUT**: `gen-triangle-ci4-bilerp` (21 px differ),
  `gen-triangle-ci8-bilerp` (24 px). CI4/CI8 palette (TLUT) sampled by a triangle,
  BI_LERP_0 correctly set — yet wgpu==RT64 still diverge from angrylion.
- **Fog-color blend**: `gen-blender-fog-color-over-mem` (12 px). Blender
  P=FogColor, A=CombinedAlpha, M=Framebuffer, B=1-A with SetFogColor.
- **FORCE_BL + coverage**: `gen-coverage-all-modes-combined-one-cycle` (12 px,
  AA_EN+CVG_DST_WRAP+CLR_ON_CVG+FORCE_BL) and `gen-coverage-force-blend-one-cycle`
  (12 px, FORCE_BL with IM_RD + CVG_DST_WRAP).

## The question (READ-ONLY, no code changes, no fix)
For EACH of the 3 domains, answer:
1. **What is the hardware rule** the shared divergence violates? (Cite: fn64's own
   docs, RT64 source, angrylion behavior, the RDP references — gbi.h, the blender
   equation, TLUT lookup, coverage/blend combine.) State precisely WHAT wgpu+RT64
   do vs WHAT angrylion does. For the CI cases: is the palette lookup or the
   post-lookup filter the diverging step? For fog: rounding, or the blend-mux
   itself? For FORCE_BL+coverage: is it the coverage->alpha path or the forced
   blender?
2. **Do real ROMs hit this path** — grep the captured command corpora / census
   data in the repo. For WM2000 specifically: does it draw CI4/CI8 TLUT triangles?
   Does it use fog-color blends? Does it use FORCE_BL with CVG_DST_WRAP? Give
   evidence (grep results, census counts), not speculation. WM2000 is the current
   goal — its usage decides priority.
3. **Fix or log?** If a ROM (esp. WM2000) hits it → real shared fidelity gap,
   recommend a fix with a sketch of WHERE (the fn64-render-wgpu path). If purely
   theoretical → recommend logging as a known divergence, not fixing.

## Where to look
- Parity runner (case construction + the shared-divergence classification logic):
  `crates/fn64-render-conformance/src/bin/fn64-render-conformance-parity-runner.rs`
- wgpu blend/coverage/TLUT: `crates/fn64-render-wgpu/src/` — combiner, blender,
  coverage.rs / coverage/, tmem/ (TLUT lookup), targets/.
- Captured corpora / census: grep the repo for WM2000 capture files and any census
  data (command-frequency dumps). Look for the census infrastructure referenced by
  FN64_*_CENSUS env vars.
- Known-divergence precedent: the BI_LERP_0 memory + task-16 report
  (`.superpowers/sdd/i-want-to-pick-melodic-hopcroft/task-16-report.md` if present)
  is the template for the verdict format.

## Constraints
- READ-ONLY. No code changes. No worktree mutation. Deliver a written recommendation.
- Do NOT modify or link the angrylion tree.
- Distinguish PROVEN (grep/census evidence) from INFERRED. An inferred root cause
  loses to a proven one — say which you have.

## Deliverable
Return a concise written verdict covering all 3 domains: the hardware rule (cited),
real-ROM/WM2000 usage evidence (grep/census results), and fix/no-fix recommendation
per domain with rationale. Rank by WM2000 relevance.
