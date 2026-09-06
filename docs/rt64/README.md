# RT64 docs index

The RT64-to-Rust port program's documentation, moved here from `docs/` (Task
7.3 of `docs/plans/CLEANUP-2026-09.md`) because it had grown to 65 of the
repo's 120 docs. One line per doc below; the group tells you how to read it.

## Authority and method

The three docs that define what the port program measures against and how it
runs. Read these first.

- [`RT64-PORT-AUTHORITY.md`](RT64-PORT-AUTHORITY.md) — RT64 port authority
- [`RT64-PARITY.md`](RT64-PARITY.md) — RT64 parity: how closely does fn64's shipping renderer match the oracle? (date from git)
- [`RT64-ENGINEERING-LOOP.md`](RT64-ENGINEERING-LOOP.md) — How to sequence this work, and why the current loop is slow (date from git)

## Status

Machine-generated, currently-live tracking: what's ported, what's left, what's
known-broken. These change as the port progresses; do not treat them as frozen.

- [`RT64-PORT-DASHBOARD.md`](RT64-PORT-DASHBOARD.md) — RT64 port workflow dashboard (date from git)
- [`RT64-PORT-INVENTORY.md`](RT64-PORT-INVENTORY.md) — RT64 port inventory (date from git)
- [`RT64-GAP-REGISTER.md`](RT64-GAP-REGISTER.md) — RT64 Gap Register — for fn64's Rust port (date from git)
- [`RT64-PORT-PARITY.md`](RT64-PORT-PARITY.md) — RT64-to-Rust renderer parity ladder

## Evidence (frozen)

Per-slice investigation records: measurements, diagnoses, and decisions from a
point in time. Each is frozen at the date below — it records what was true then,
not a live status. A date marked "from git" is the doc's last substantive commit
date (no provenance line of its own); otherwise the date comes from the doc's own
first-page provenance line.

- [`RT64-ALL-FN64-STACK-STATUS.md`](RT64-ALL-FN64-STACK-STATUS.md) — frozen 2026-08-18 — The all-fn64 stack: measured status
- [`RT64-ASPECT-EVIDENCE.md`](RT64-ASPECT-EVIDENCE.md) — frozen 2026-07-20 (date from git) — RT64 aspect-ratio evidence
- [`RT64-COVERAGE-AUDIT.md`](RT64-COVERAGE-AUDIT.md) — frozen 2026-08-18 (date from git) — Coverage audit: nonclaims, unreachable refusals, surviving mutants
- [`RT64-EXTENDED-GBI-SEAM.md`](RT64-EXTENDED-GBI-SEAM.md) — frozen 2026-07-21 (date from git) — RT64 Extended-GBI fixture and evidence seam
- [`RT64-FILL-PARTIAL-SEED.md`](RT64-FILL-PARTIAL-SEED.md) — frozen 2026-08-19 (date from git) — The partial-fill seed: where untouched pixels come from
- [`RT64-FORCE-BRANCH-EVIDENCE.md`](RT64-FORCE-BRANCH-EVIDENCE.md) — frozen 2026-07-21 (date from git) — RT64 force-branch enhancement evidence
- [`RT64-GPU-TEST-MATRIX.md`](RT64-GPU-TEST-MATRIX.md) — frozen 2026-08-18 (date from git) — RT64 GPU test matrix: running the gated tests without a GPU
- [`RT64-GUARD-AUDIT.md`](RT64-GUARD-AUDIT.md) — frozen 2026-08-21 (date from git) — RT64 / fn64-render-wgpu refusal-guard audit
- [`RT64-GUI-ASSESSMENT.md`](RT64-GUI-ASSESSMENT.md) — frozen 2026-08-17 (date from git) — RT64 GUI and ImGui-backend port assessment
- [`RT64-HANDOFF.md`](RT64-HANDOFF.md) — frozen 2026-08-21 (date from git) — Handoff: WM2000 on the all-Rust stack
- [`RT64-LANE-BRIEF-CHECKLIST.md`](RT64-LANE-BRIEF-CHECKLIST.md) — frozen 2026-08-19 (date from git) — Dispatching a lane: what to put in the brief
- [`RT64-LANE-DIVERGENCES.md`](RT64-LANE-DIVERGENCES.md) — frozen 2026-08-19 (date from git) — Lane divergences: `fn64-render-reference` vs `fn64-render-wgpu`
- [`RT64-M6-M7-SCOPING.md`](RT64-M6-M7-SCOPING.md) — frozen 2026-09-06 (date from git) — M6 and M7 architecture scoping
- [`RT64-MACOS-CERTIFICATION.md`](RT64-MACOS-CERTIFICATION.md) — frozen 2026-08-03 (date from git) — RT64 macOS certification
- [`RT64-PERF-CEILING.md`](RT64-PERF-CEILING.md) — frozen 2026-08-22 — WM2000 performance: the 30 Hz budget and the open post-fix question
- [`RT64-PLATFORM-CERTIFICATION.md`](RT64-PLATFORM-CERTIFICATION.md) — frozen 2026-08-03 (date from git) — RT64 cross-platform certification
- [`RT64-PLAYABLE-PLAN-REVIEW.md`](RT64-PLAYABLE-PLAN-REVIEW.md) — frozen 2026-08-19 — Plan review: playable WM2000, and a second playable ROM
- [`RT64-PORT-CARD-BRIEF.md`](RT64-PORT-CARD-BRIEF.md) — frozen 2026-08-17 (date from git) — RT64 port-card standing brief
- [`RT64-PORT-ORCHESTRATION.md`](RT64-PORT-ORCHESTRATION.md) — frozen 2026-08-16 (date from git) — RT64 Rust-port orchestration
- [`RT64-PUBLIC-FEATURE-INVENTORY.md`](RT64-PUBLIC-FEATURE-INVENTORY.md) — frozen 2026-09-06 (date from git) — RT64 public feature inventory
- [`RT64-REFERENCE-SHADER-ARTIFACTS.md`](RT64-REFERENCE-SHADER-ARTIFACTS.md) — frozen 2026-08-16 (date from git) — RT64 reference shader artifacts
- [`RT64-REFUSAL-AUDIT.md`](RT64-REFUSAL-AUDIT.md) — frozen 2026-08-17 (date from git) — RT64 refusal audit
- [`RT64-RENDER-MEASUREMENT.md`](RT64-RENDER-MEASUREMENT.md) — frozen 2026-08-16 (date from git) — RT64 render measurement report contract
- [`RT64-RUNTIME-CONTROLS.md`](RT64-RUNTIME-CONTROLS.md) — frozen 2026-08-14 (date from git) — RT64 runtime-control boundary
- [`RT64-RUNTIME-SHADER-CORPUS.md`](RT64-RUNTIME-SHADER-CORPUS.md) — frozen 2026-08-16 (date from git) — RT64 runtime shader corpus
- [`RT64-S2DEX-ENHANCEMENT-EVIDENCE.md`](RT64-S2DEX-ENHANCEMENT-EVIDENCE.md) — frozen 2026-07-20 (date from git) — RT64 S2DEX enhancement evidence
- [`RT64-S2DEX-OBJECT-EVIDENCE.md`](RT64-S2DEX-OBJECT-EVIDENCE.md) — frozen 2026-07-24 (date from git) — RT64 S2DEX2 object-rectangle evidence
- [`RT64-SHADER-ARTIFACTS.md`](RT64-SHADER-ARTIFACTS.md) — frozen 2026-08-16 (date from git) — RT64 shader artifacts
- [`RT64-TEST-MATRIX.md`](RT64-TEST-MATRIX.md) — frozen 2026-08-18 (date from git) — fn64 test matrix: which configuration produced which evidence
- [`RT64-TEXTURE-LOD-EVIDENCE.md`](RT64-TEXTURE-LOD-EVIDENCE.md) — frozen 2026-07-20 (date from git) — RT64 texture-LOD scale enhancement evidence
- [`RT64-TRIANGLE-WRITEBACK.md`](RT64-TRIANGLE-WRITEBACK.md) — frozen 2026-08-23 (date from git) — Raw triangle -> guest RDRAM: design record
- [`RT64-UPSTREAM-OBSERVATIONS.md`](RT64-UPSTREAM-OBSERVATIONS.md) — frozen 2026-08-17 (date from git) — RT64 upstream observations
- [`RT64-WGPU-SHADER-ASSESSMENT.md`](RT64-WGPU-SHADER-ASSESSMENT.md) — frozen 2026-08-16 (date from git) — RT64 wgpu shader ingestion assessment
- [`RT64-WM2000-0X1CC-DIAGNOSIS.md`](RT64-WM2000-0X1CC-DIAGNOSIS.md) — frozen 2026-08-18 (date from git) — The `0x1CC` abort is not an MMIO read
- [`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md) — frozen 2026-08-18 (date from git) — WM2000's command census: what the game actually asks for
- [`RT64-WM2000-COMBINER-CENSUS.md`](RT64-WM2000-COMBINER-CENSUS.md) — frozen 2026-08-21 (date from git) — WM2000 flat-shading: the combiner census
- [`RT64-WM2000-CYCLE-MODES.md`](RT64-WM2000-CYCLE-MODES.md) — frozen 2026-08-18 (date from git) — WM2000's texrect cycle modes: the measurement that sizes the remaining work
- [`RT64-WM2000-FRAME-RATE-MEASURED.md`](RT64-WM2000-FRAME-RATE-MEASURED.md) — frozen 2026-08-25 (date from git) — WM2000 frame rate: where the time actually goes
- [`RT64-WM2000-GAMEPLAY-GAP.md`](RT64-WM2000-GAMEPLAY-GAP.md) — frozen 2026-08-24 (date from git) — What actually blocks WM2000 from gameplay
- [`RT64-WM2000-GAP.md`](RT64-WM2000-GAP.md) — frozen 2026-08-25 (date from git) — WM2000 through the Rust port: the measured gap
- [`RT64-WM2000-HARNESS-TRAPS.md`](RT64-WM2000-HARNESS-TRAPS.md) — frozen 2026-08-20 (date from git) — WM2000 harness traps
- [`RT64-WM2000-INMATCH-GAPS.md`](RT64-WM2000-INMATCH-GAPS.md) — frozen 2026-08-24 (date from git) — In-match rendering gaps: hypotheses awaiting measurement
- [`RT64-WM2000-INPUT-GRAMMAR.md`](RT64-WM2000-INPUT-GRAMMAR.md) — frozen 2026-08-24 (date from git) — WM2000's own input grammar, read from the ROM
- [`RT64-WM2000-MATCH-COMPLETION.md`](RT64-WM2000-MATCH-COMPLETION.md) — frozen 2026-08-19 (date from git) — Can WM2000 play a match to completion on fn64's all-Rust stack?
- [`RT64-WM2000-MATCH-GRAMMAR.md`](RT64-WM2000-MATCH-GRAMMAR.md) — frozen 2026-08-19 (date from git) — WM2000's in-match grammar, and the search for a match-end signal
- [`RT64-WM2000-MATCH-LIVE.md`](RT64-WM2000-MATCH-LIVE.md) — frozen 2026-08-20 (date from git) — CONFIRMED: WM2000 reaches a live match on the all-Rust stack
- [`RT64-WM2000-MATCH-RUN-BUDGET.md`](RT64-WM2000-MATCH-RUN-BUDGET.md) — frozen 2026-08-19 — What a "run the match to the end" run actually costs
- [`RT64-WM2000-RECOMP-LANES.md`](RT64-WM2000-RECOMP-LANES.md) — frozen 2026-09-06 (date from git) — WM2000's recompiler lanes: which one every measurement came from
- [`RT64-WM2000-REMAINING.md`](RT64-WM2000-REMAINING.md) — frozen 2026-08-24 (date from git) — What remains: WM2000 playable on the fn64 recompiler + wgpu port
- [`RT64-WM2000-REPLAY.md`](RT64-WM2000-REPLAY.md) — frozen 2026-08-18 (date from git) — Replaying a real WM2000 packet through the Rust port
- [`RT64-WM2000-SCOUT.md`](RT64-WM2000-SCOUT.md) — frozen 2026-08-24 (date from git) — WM2000 all-Rust lane: scout report
- [`RT64-WM2000-SECTION-LOCAL.md`](RT64-WM2000-SECTION-LOCAL.md) — frozen 2026-08-18 (date from git) — The section-table abort was not an overlay-swap problem
- [`RT64-WM2000-TEXEL-LOCALISATION.md`](RT64-WM2000-TEXEL-LOCALISATION.md) — frozen 2026-08-21 (date from git) — Localising the wrong texel value
- [`RT64-WM2000-TEXTURE-STATE.md`](RT64-WM2000-TEXTURE-STATE.md) — frozen 2026-08-21 (date from git) — LOOKED AT THE SCREEN after the texel-scale fix
- [`RT64-WM2000-THREE-WAY.md`](RT64-WM2000-THREE-WAY.md) — frozen 2026-08-18 (date from git) — WM2000 frame 0, three ways: does an independent lineage agree?
- [`RT64-WM2000-UNCOVERED-ENTRIES.md`](RT64-WM2000-UNCOVERED-ENTRIES.md) — frozen 2026-08-19 (date from git) — 0x801226A0 is not a bank-overlap case: it is an uncovered entry in bank 4
- [`RT64-WM2000-VALIDATION.md`](RT64-WM2000-VALIDATION.md) — frozen 2026-08-24 (date from git) — Validating WM2000's frame 0 against an independent oracle
- [`RT64-WM2000-VERSUS-PLATEAU.md`](RT64-WM2000-VERSUS-PLATEAU.md) — frozen 2026-08-19 (date from git) — The versus-screen plateau is guest state 18, waiting on a player-ready check
