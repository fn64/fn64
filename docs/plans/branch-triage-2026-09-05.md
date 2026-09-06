# Branch triage 2026-09-05

**Method:** patch-equivalence via `git cherry -v HEAD <branch>` (novel = commits with no
patch-equivalent already on `HEAD`, total = commits above merge-base). For the three
squash-merged PR-130-era branches, a tree-diff test restricted to the files they touched
(`git diff HEAD <branch> -- $(git diff --name-only $(git merge-base HEAD <branch>) <branch>)`)
was used instead, since squash-merging changes patch-ids. Live-worktree branches were
identified from `git worktree list`. Base for `git cherry` is this integration branch's
`HEAD` (976f2d33), not `origin/main`, per controller ruling.

**Date:** 2026-09-05

No branch was deleted by this task; deletion requires owner approval of this table.

201 branches carry unmerged commits above HEAD. Three additional named branches are
fully merged into HEAD (0 unmerged commits) and are out of this table's scope: the two
live worktrees `worktree-agent-a9e1453004efb0edc` and `worktree-moonlit-munching-russell`,
and `perf/wm2000-audio-lockfree-land` (recorded below as landed per Task 0.1 — it carries
no unmerged commits because its content is already on HEAD).

## Verdict table

| novel/total | last date | branch | verdict | live worktree | notes |
|---:|---|---|---|---|---|
| 464/469 | 2026-08-14 | `integration/pr130-finalize` | land |  | squash-merge patch-id mismatch; tree-diff vs actual touched files is 85k-94k lines (not ~200), pre-restructuring snapshot spanning fn64-discover/fn64-abi/fn64-render-reference/fn64-render-rt64/fn64-certification/fn64-recomp-rs/fn64-boot-harness -- needs its own landing task |
| 463/467 | 2026-08-14 | `fix/overlay-stride-aliases` | land | **yes** | squash-merge patch-id mismatch; tree-diff vs actual touched files is 85k-94k lines (not ~200), pre-restructuring snapshot spanning fn64-discover/fn64-abi/fn64-render-reference/fn64-render-rt64/fn64-certification/fn64-recomp-rs/fn64-boot-harness -- needs its own landing task |
| 431/434 | 2026-08-10 | `fix/frame-census-spread` | land |  | squash-merge patch-id mismatch; tree-diff vs actual touched files is 85k-94k lines (not ~200), pre-restructuring snapshot spanning fn64-discover/fn64-abi/fn64-render-reference/fn64-render-rt64/fn64-certification/fn64-recomp-rs/fn64-boot-harness -- needs its own landing task |
| 56/56 | 2026-08-25 | `worktree-feat+conker-decompress-discover` | land |  | keeper per controller ruling 5 |
| 48/51 | 2026-09-03 | `integrate/recompiler-coverage` | land | **yes** | keeper per controller ruling 5 |
| 36/36 | 2026-08-02 | `agent/static-recomp-wave-checkpoint` | superseded (pre-extraction checkpoint) |  |  |
| 36/36 | 2026-08-02 | `backup/pre-rebase-checkpoint-20260808` | superseded (pre-extraction checkpoint) |  |  |
| 34/34 | 2026-08-02 | `agent/discovery-materialization-pipeline` | superseded (pre-extraction checkpoint) |  |  |
| 31/293 | 2026-08-18 | `scout/wall-preview` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 30/30 | 2026-08-02 | `agent/static-recomp-wave-checkpoint-pre-ci-fix` | superseded (pre-extraction checkpoint) |  |  |
| 28/32 | 2026-07-24 | `feature/wm2000-audio-hle-20260723` | superseded (pre-extraction; audio/runtime rebuilt since) |  |  |
| 25/29 | 2026-07-24 | `integration/wm2000-v28-gate-reviewfix-20260724` | superseded (pre-extraction; audio/runtime rebuilt since) |  |  |
| 23/705 | 2026-08-24 | `integrate/preplayable-tranche` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 22/663 | 2026-08-22 | `port/rt64-conveyor` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 22/764 | 2026-08-24 | `worktree-wm2000-playable` | superseded | **yes** | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 19/520 | 2026-08-19 | `lane/wm2000-match-completion` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 17/591 | 2026-08-20 | `lane/texel-corpus` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 17/17 | 2026-07-21 | `wm2000-abi-rebase` | superseded (wm2000-boot crate renamed fn64-boot-harness; peripheral ABI surface save.rs/gbpak.rs/voice.rs already on HEAD) |  |  |
| 16/418 | 2026-08-19 | `diag/wm2000-textures` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 15/452 | 2026-08-19 | `fix/fill-triangle-guard` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 14/444 | 2026-08-19 | `fix/fillrect-cycle-type` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 14/540 | 2026-08-20 | `perf/wm2000-framerate` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 14/419 | 2026-08-19 | `probe/wm2000-versus` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 13/13 | 2026-07-31 | `agent/runtime-unified-resume` | superseded (pre-extraction checkpoint) |  |  |
| 13/13 | 2026-08-10 | `feat/profile-and-frame-census` | superseded (FN64_PROFILE already on HEAD) |  |  |
| 13/492 | 2026-08-19 | `fix/fill-scissor-partial` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 13/555 | 2026-08-20 | `lane/combiner-tally` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 13/459 | 2026-08-19 | `lane/rdp-scissor-texrect` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 13/480 | 2026-08-19 | `lane/rt64-parity-metric` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 13/475 | 2026-08-19 | `lane/wgpu-conformance-runner` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 13/535 | 2026-08-20 | `lane/wm2000-playable-window` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 13/14 | 2026-08-14 | `resolve/pr140-merge-main` | superseded (FN64_PROFILE already on HEAD) |  |  |
| 12/451 | 2026-08-19 | `audit/rt64-guard-audit` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 12/13 | 2026-08-14 | `feat/rmlui-rs-adapter` | salvage-blocked: commit 2/12 (7fe0e70c) conflicts on Cargo.lock/Cargo.toml workspace registration; sequence aborted, harmless 1st commit reverted |  |  |
| 12/444 | 2026-08-19 | `fix/texrect-shadealpha` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 12/457 | 2026-08-19 | `lane/rt64-guard-a2a3` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 11/11 | 2026-08-02 | `agent/static-recomp-wave-foundation` | superseded (pre-extraction checkpoint) |  |  |
| 11/441 | 2026-08-19 | `fix/wm2000-bank-dispatch` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 11/26 | 2026-07-21 | `wm2000-ladder-3` | superseded (wm2000-boot crate renamed fn64-boot-harness; WM2000 boot far exceeded by later work) |  |  |
| 10/430 | 2026-08-19 | `probe/wm2000-ready-check` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 9/292 | 2026-08-19 | `lane/rt64-tri-writeback` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 9/410 | 2026-08-19 | `lane/wm2000-match-drive` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 8/397 | 2026-08-19 | `fix/wm2000-1900-abort` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 8/10 | 2026-07-21 | `wm2000-ladder-2` | superseded (wm2000-boot crate renamed fn64-boot-harness; WM2000 boot far exceeded by later work) |  |  |
| 7/302 | 2026-08-19 | `feat/rt64-triangle-writeback` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 7/7 | 2026-07-23 | `integration/wm2000-rt64-audio-lle-20260722` | superseded (pre-extraction; audio/runtime rebuilt since) |  |  |
| 7/389 | 2026-08-19 | `lane/wm2000-match-run` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 6/382 | 2026-08-19 | `card/rt64-depth-scope` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 6/392 | 2026-08-19 | `card/symdump-jal-crosscheck` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 6/385 | 2026-08-19 | `card/wm2000-othermode-census` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 6/374 | 2026-08-19 | `diag/wm2000-frozen-frame` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 6/379 | 2026-08-19 | `fix/rt64-raw-tri-gpu-tile` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 6/386 | 2026-08-19 | `sweep/frozen-value-defects` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 5/5 | 2026-07-29 | `agent/loader-claim-bridge` | superseded (pre-extraction checkpoint) |  |  |
| 5/356 | 2026-08-19 | `lane/tri-texture-rung` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 5/5 | 2026-08-15 | `perf/eliminate-hot-debug-work` | superseded (landed as 7fae15ea PR #145, same subject) |  |  |
| 5/348 | 2026-08-19 | `probe/b3-menu-duplication` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 5/361 | 2026-08-19 | `probe/wm2000-input-grammar` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 4/7 | 2026-07-16 | `merge/render-final` | superseded (RT64 backend FFI + scissor/perspective-ST fixes on HEAD) |  |  |
| 4/339 | 2026-08-19 | `port/rt64-harness` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 3/309 | 2026-08-19 | `lane/vi-interlace-stripes` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 3/3 | 2026-08-27 | `worktree-feat+gen-host-lookup` | superseded (host_lookup.rs autogen + div-guard-all-funcs already on HEAD) |  |  |
| 2/2 | 2026-08-15 | `feat/netplay-foundation` | salvage |  |  |
| 2/2 | 2026-07-22 | `feat/recompiler-lint` | superseded (gate_recover_boundaries already on HEAD) |  |  |
| 2/6 | 2026-09-04 | `feat/wm-block-external-symbol-ref` | landed (Task 0.2) | **yes** |  |
| 2/2 | 2026-08-27 | `integrate/generated-host-lookup` | superseded (host_lookup.rs autogen + div-guard-all-funcs already on HEAD) |  |  |
| 2/2 | 2026-09-03 | `integrate/generated-host-lookup-only` | superseded (landed as ec3a45b3, same subject) |  |  |
| 2/2 | 2026-07-22 | `integration/u6-audio-lle-policy-20260722` | superseded (pre-extraction; audio/runtime rebuilt since) |  |  |
| 2/277 | 2026-08-18 | `lane/tmem-walls-345` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 2/333 | 2026-08-19 | `lane/tri-cpu-raster` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 2/3 | 2026-07-16 | `merge/render-scissor-artifact` | superseded (scissor/perspective-ST fix already on HEAD) |  |  |
| 2/2 | 2026-07-22 | `perf/reference-lazy-texel-context` | superseded (pre-extraction reference-renderer diagnostic; rewritten since) |  |  |
| 2/2 | 2026-08-10 | `perf/vi-boundary-observation-count` | superseded (VI/present pacing rebuilt since; frame_census covers this) |  |  |
| 2/2 | 2026-08-29 | `perf/wm2000-w01-w05-integration` | superseded (raw-DPC batch/admission tracing superseded by current raw_dpc module) |  |  |
| 2/2 | 2026-08-16 | `port/production-t1-planner-v11` | superseded (RawDpcCoordinator already on HEAD under current raw_dpc architecture) |  |  |
| 2/2 | 2026-08-15 | `worktree-corpus-resweep` | superseded (docs/CORPUS-INDIRECT-PRIORITY.md carries current numbers) |  |  |
| 2/2 | 2026-07-17 | `worktree-h3-gate-config` | superseded (see c4cf739a docs: add H3 gate-config spec, marked SUPERSEDED) |  |  |
| 1/1 | 2026-09-02 | `chore/launch-status` | superseded (landed as 064bf912, same subject) |  |  |
| 1/1 | 2026-07-16 | `chore/split-emit-wip` | superseded (explicit wip:, no follow-up) |  |  |
| 1/3 | 2026-07-23 | `combined-audio-test` | superseded (AKI ucode coverage probe with WM2000 audible path already on HEAD) |  |  |
| 1/1 | 2026-07-25 | `docs/aki-recomp-handoff` | superseded (docs/plans/aki-recompile-certification.md is the current AKI doc) |  |  |
| 1/2 | 2026-08-27 | `feat/rdram-dump-tooling` | land |  | keeper per controller ruling 5 |
| 1/1 | 2026-07-16 | `feat/rt64-backend-ffi` | superseded (feature-gated RT64 backend FFI already on HEAD) |  |  |
| 1/3 | 2026-07-20 | `feature/certification-denominator-v5` | superseded (pre-extraction checkpoint) |  |  |
| 1/5 | 2026-07-20 | `feature/nextest-temp-isolation` | superseded (pre-extraction checkpoint) |  |  |
| 1/6 | 2026-07-20 | `feature/private-release-runner-v1` | superseded (pre-extraction checkpoint) |  |  |
| 1/7 | 2026-07-20 | `feature/release-consumption-evidence-v1` | superseded (pre-extraction checkpoint) |  |  |
| 1/4 | 2026-07-20 | `feature/release-report-v16` | superseded (pre-extraction checkpoint) |  |  |
| 1/1 | 2026-07-16 | `fix/render-artifact` | superseded (OoT render-artifact fix folded into current raster pipeline) |  |  |
| 1/1 | 2026-07-16 | `fix/render-scissor` | superseded (scissor/perspective-ST fix already on HEAD) |  |  |
| 1/273 | 2026-08-18 | `fix/rt64-wall67-color-registers` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 1/296 | 2026-08-19 | `fix/texrect-tmem-tile` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 1/1 | 2026-08-15 | `handoff/session-2026-08-15` | superseded (session notes; RT64 port wave since found inert, see memory) |  |  |
| 1/1 | 2026-09-03 | `integrate/div-guard-all-functions` | land |  | keeper per controller ruling 5 |
| 1/3 | 2026-08-27 | `integrate/wt-recognizers` | superseded (identical subject landed as 7190edb4/bdcad858) |  |  |
| 1/1 | 2026-09-03 | `integrate/wt-recognizers-core` | land |  | keeper per controller ruling 5 |
| 1/13 | 2026-07-24 | `integration/cop0-authority-main-20260724` | superseded (pre-extraction; COP0 authority rebuilt since) |  |  |
| 1/9 | 2026-07-24 | `integration/cop1-exact-main-20260724` | superseded (pre-extraction; COP1 rebuilt since) |  |  |
| 1/28 | 2026-07-24 | `integration/v28-substrate-port-20260724` | superseded (pre-extraction checkpoint) |  |  |
| 1/302 | 2026-08-19 | `lane/combined-input` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 1/286 | 2026-08-19 | `lane/tmem-ia4-tlut` | superseded |  | all fix/feat content (FillNotDeclared admission, execute_raw_triangle CPU raster, tmem lane-0 TLUT read, audit_undispatchable_call_targets) already on HEAD under current names |
| 1/1 | 2026-08-15 | `perf/pgo-workflow` | superseded (docs/PGO-WORKFLOW.md + scripts/pgo-release.py already on HEAD) |  |  |
| 1/1 | 2026-08-15 | `perf/runtime-measurements` | superseded (docs sizing note superseded by later perf docs) |  |  |
| 1/6 | 2026-08-16 | `port/m2-metal-semantics` | superseded (docs/rt64/RT64-PORT-DASHBOARD.md already on HEAD) |  |  |
| 1/1 | 2026-08-16 | `port/production-t0-neutral` | superseded (RawDpcCoordinator already on HEAD under current raw_dpc architecture) |  |  |
| 1/2 | 2026-08-16 | `port/render-ir-integration` | superseded (docs edits folded into current RT64-PORT-DASHBOARD.md) |  |  |
| 1/4 | 2026-08-16 | `port/rt64-dashboard` | superseded (docs/rt64/RT64-PORT-DASHBOARD.md already on HEAD) |  |  |
| 1/2 | 2026-08-16 | `port/rt64-formats-dither` | superseded (RT64 port wave found inert, see memory rt64-ported-modules-are-inert) |  |  |
| 1/1 | 2026-08-16 | `port/rt64-fragment-combiner-wiring` | superseded (fragment constant registers already on HEAD under raw_dpc/triangle_draw_data.rs) |  |  |
| 1/1 | 2026-08-16 | `port/rt64-triangle-vertices` | superseded (triangle_vertices.rs already on HEAD) |  |  |
| 1/151 | 2026-08-17 | `rt64-honest-inventory` | superseded (RT64 port wave found inert, see memory rt64-ported-modules-are-inert) |  |  |
| 1/1 | 2026-07-24 | `tools/mupen-core-pin` | superseded (explicit wip, UNTESTED, not for PR) |  |  |
| 1/4 | 2026-08-27 | `worktree-agent-a938f9dbb7c71662d` | superseded (FN64_RDRAM_DUMP_AT_STEP already on HEAD) |  |  |
| 1/1 | 2026-08-16 | `worktree-rt64-depth-mode-port` | superseded (depth_strict_less.rs already on HEAD) |  |  |
| 1/1 | 2026-08-16 | `worktree-rt64-fragment-registers-impl` | superseded (SetFogColor/SetPrimColor/SetBlendColor already on HEAD) |  |  |
| 1/2 | 2026-08-16 | `worktree-rt64-texture-gen` | superseded (TextureRectangle landed per its own commit text) |  |  |
| 1/1 | 2026-07-20 | `worktree-wm2000-discover-gates` | superseded (peripheral ABI surface save.rs/gbpak.rs/voice.rs already on HEAD) |  |  |
| 1/3 | 2026-08-27 | `worktree-wt-recognizers` | superseded (FN64_RDRAM_DUMP_AT_STEP already on HEAD) | **yes** |  |
| 0/17 | 2026-07-24 | `conflict-probe` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `coverage-fragment-fn-rebase` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-07-25 | `docs&#47;fix-stale-and-false-claims` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-07-23 | `docs&#47;foundation-specs` | superseded (patch-equivalent) | **yes** |  |
| 0/1 | 2026-08-09 | `docs&#47;marketing-site` | superseded (patch-equivalent) |  |  |
| 0/2 | 2026-07-20 | `feat/aki-cross-donor` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-07-22 | `feat/aki-quick-wins` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-15 | `feat/input-settings-ui` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-07-25 | `feat/mupen-core-build-pin` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-17 | `feat/rt64-production-coverage-node1` | superseded (patch-equivalent) |  |  |
| 0/2 | 2026-08-15 | `feat/rt64-render-hook-multiplex` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-07-24 | `final-compose` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-07-23 | `fix/aki-audio-imem-base` | superseded (patch-equivalent) |  |  |
| 0/2 | 2026-09-02 | `fix/launch-main-gates` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-29 | `fix/native-4x3-presentation` | superseded (patch-equivalent) |  |  |
| 0/16 | 2026-08-05 | `fix/overlay-stride-aliases-only` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-24 | `fix/wm2000-task-compute-disposition` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-27 | `integrate/hidden-coverage` | superseded (patch-equivalent) |  |  |
| 0/3 | 2026-08-24 | `integrate/main-validation-baseline` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-17 | `integrate/metal-blend-gate` | superseded (patch-equivalent) |  |  |
| 0/5 | 2026-08-27 | `integrate/render-foundations-1` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-17 | `integrate/rt64-derived-port-state` | superseded (patch-equivalent) |  |  |
| 0/55 | 2026-08-17 | `integrate/rt64-portable-tranche` | superseded (patch-equivalent) |  |  |
| 0/2 | 2026-08-17 | `integrate/rt64-tmem-budget` | superseded (patch-equivalent) |  |  |
| 0/91 | 2026-08-24 | `integrate/wm2000-01-rt64-foundation` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-17 | `integrate/wm2000-rt64-common` | superseded (patch-equivalent) |  |  |
| 0/21 | 2026-07-24 | `integration/audio-hle-main-20260724` | superseded (patch-equivalent) |  |  |
| 0/4 | 2026-07-23 | `integration/fpu-main-transplant-20260724` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `m2-4-validator-profiles-staged` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `m2-5a-reference-corpus` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `m2-5b-wgpu-assessment` | superseded (patch-equivalent) |  |  |
| 0/6 | 2026-08-03 | `measure/corpus-certification-sweep` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-24 | `perf/wm2000-cycle-key-census` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-24 | `perf/wm2000-task-cpu-census` | superseded (patch-equivalent) |  |  |
| 0/3 | 2026-08-16 | `plan/rt64-wgpu-backend-triangle-integration` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m0-3-measurement` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m2-5b-exclusive-witness` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m2-5b-profile-consumer` | superseded (patch-equivalent) |  |  |
| 0/2 | 2026-08-16 | `port/m2-metal-caps` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m2-metal-submission` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m4-1-tmem-wire-state` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m4-2a-physical-state` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m4-2b-loadtile` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m4-2c-loadblock` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m4-2d-packet` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m4-3-1-loadtlut-plan` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m4-3-1b-tlut-destination-mask` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m4-3-3b-indexed-decode` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/m4-3-3c-physical-texel-reader-clean` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/production-t2-abi` | superseded (patch-equivalent) |  |  |
| 0/3 | 2026-08-16 | `port/render-ir-spine` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/rt64-combiner-slice2` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/rt64-combiner-slice3` | superseded (patch-equivalent) |  |  |
| 0/2 | 2026-08-16 | `port/rt64-endian-swap` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/rt64-inventory-v2` | superseded (patch-equivalent) |  |  |
| 0/2 | 2026-08-16 | `port/rt64-production-triangle-draw` | superseded (patch-equivalent) |  |  |
| 0/2 | 2026-08-16 | `port/rt64-texture-rectangle` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/rt64-triangle-composition-precursor` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `port/rt64-triangle-decode` | superseded (patch-equivalent) |  |  |
| 0/21 | 2026-07-24 | `pr91-land` | superseded (patch-equivalent) |  |  |
| 0/12 | 2026-07-24 | `pr91-verify` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-07-24 | `pr92-verify` | superseded (patch-equivalent) |  |  |
| 0/7 | 2026-08-15 | `refactor/game-neutral-runner-build` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-14 | `refactor/typed-boundary-units` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-agent-aa72b9718f7bfbc53` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-agent-abbfc4bfde304e999` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-agent-af4b508c693ca3128` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-blend-selector-cycle-semantics` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-m2-5-3b-three-nearest-wgsl` | superseded (patch-equivalent) | **yes** |  |
| 0/1 | 2026-08-16 | `worktree-m2-5b-decoration-index-repair` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-m2-5b-taxonomy-two-classes` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-m4-3-1c-tlut-wrap` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-m4-3-2-loadtlut-finalize` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-m4-3-3a-direct-texel` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-production-t3-phase-b` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-rt64-alpha-compare-slice` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-rt64-combiner-slice1` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-rt64-coverage-semantics` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-rt64-parity-cooldown-flake` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-rt64-raster-vs-lane` | superseded (patch-equivalent) |  |  |
| 0/2 | 2026-08-15 | `worktree-wave-b-transferpak-overlay` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-wgsl-three-nearest-final-integration` | superseded (patch-equivalent) |  |  |
| 0/1 | 2026-08-16 | `worktree-wgsl-three-nearest-integration` | superseded (patch-equivalent) |  |  |

## Salvage landed on this integration branch

- `feat/netplay-foundation` -> cherry-picked as `175a211b` (`feat(runtime): add deterministic netplay input foundation`) and `2258ee03` (`fix(input): reject invalid raw controller ports loudly`). Both applied cleanly; `cargo check -p fn64-runtime` passed after each.

## Salvage blocked

- `feat/rmlui-rs-adapter` (novel=12): first commit (C shim header, isolated, no wiring) applied cleanly but was reverted (commit `cd3e2878`) once the second commit (`7fe0e70c`, adds the `fn64-rmlui` crate) conflicted on `Cargo.lock`/`Cargo.toml` workspace-member registration. The conflict is resolvable but is real workspace-surgery, not a clean cherry-pick, so this branch is left for its own landing task rather than resolved here.

## Land (not cherry-picked here; each gets its own landing task per the brief)

- `worktree-feat+conker-decompress-discover`, `integrate/recompiler-coverage`, `integrate/wt-recognizers-core`, `integrate/div-guard-all-functions`, `feat/rdram-dump-tooling` — keepers per controller ruling 5.
- `integration/pr130-finalize`, `fix/overlay-stride-aliases`, `fix/frame-census-spread` — squash-merge patch-id mismatches; the tree-diff test (restricted to touched files) is 85k-94k lines against 214-257 files spanning fn64-discover/fn64-abi/fn64-render-reference/fn64-render-rt64/fn64-certification/fn64-recomp-rs/fn64-boot-harness, far over the ~200-line explainable-remainder bar, so these are `land` rather than `superseded (squash-merged)`. This is a pre-restructuring snapshot predating the current fn64-discover/fn64-abi layout; a landing task should diff each against the actual PR #130 squash commit on main (not this branch's stale merge-base) to find true novel content — that analysis is out of this task's scope. `fix/overlay-stride-aliases` is also in a live worktree (`rom-corpus-catalog`).

## Already landed (recorded, not re-landed)

- `feat/wm-block-external-symbol-ref` — landed (Task 0.2).
- `perf/wm2000-audio-lockfree-land` — landed (Task 0.1).

## Proposed deletions

186 branches: verdict `superseded*` and not checked out in any live worktree. Ready to run after owner approval:

```sh
git branch -D agent/static-recomp-wave-checkpoint
git branch -D backup/pre-rebase-checkpoint-20260808
git branch -D agent/discovery-materialization-pipeline
git branch -D scout/wall-preview
git branch -D agent/static-recomp-wave-checkpoint-pre-ci-fix
git branch -D feature/wm2000-audio-hle-20260723
git branch -D integration/wm2000-v28-gate-reviewfix-20260724
git branch -D integrate/preplayable-tranche
git branch -D port/rt64-conveyor
git branch -D lane/wm2000-match-completion
git branch -D lane/texel-corpus
git branch -D wm2000-abi-rebase
git branch -D diag/wm2000-textures
git branch -D fix/fill-triangle-guard
git branch -D fix/fillrect-cycle-type
git branch -D perf/wm2000-framerate
git branch -D probe/wm2000-versus
git branch -D agent/runtime-unified-resume
git branch -D feat/profile-and-frame-census
git branch -D fix/fill-scissor-partial
git branch -D lane/combiner-tally
git branch -D lane/rdp-scissor-texrect
git branch -D lane/rt64-parity-metric
git branch -D lane/wgpu-conformance-runner
git branch -D lane/wm2000-playable-window
git branch -D resolve/pr140-merge-main
git branch -D audit/rt64-guard-audit
git branch -D fix/texrect-shadealpha
git branch -D lane/rt64-guard-a2a3
git branch -D agent/static-recomp-wave-foundation
git branch -D fix/wm2000-bank-dispatch
git branch -D wm2000-ladder-3
git branch -D probe/wm2000-ready-check
git branch -D lane/rt64-tri-writeback
git branch -D lane/wm2000-match-drive
git branch -D fix/wm2000-1900-abort
git branch -D wm2000-ladder-2
git branch -D feat/rt64-triangle-writeback
git branch -D integration/wm2000-rt64-audio-lle-20260722
git branch -D lane/wm2000-match-run
git branch -D card/rt64-depth-scope
git branch -D card/symdump-jal-crosscheck
git branch -D card/wm2000-othermode-census
git branch -D diag/wm2000-frozen-frame
git branch -D fix/rt64-raw-tri-gpu-tile
git branch -D sweep/frozen-value-defects
git branch -D agent/loader-claim-bridge
git branch -D lane/tri-texture-rung
git branch -D perf/eliminate-hot-debug-work
git branch -D probe/b3-menu-duplication
git branch -D probe/wm2000-input-grammar
git branch -D merge/render-final
git branch -D port/rt64-harness
git branch -D lane/vi-interlace-stripes
git branch -D worktree-feat+gen-host-lookup
git branch -D feat/recompiler-lint
git branch -D integrate/generated-host-lookup
git branch -D integrate/generated-host-lookup-only
git branch -D integration/u6-audio-lle-policy-20260722
git branch -D lane/tmem-walls-345
git branch -D lane/tri-cpu-raster
git branch -D merge/render-scissor-artifact
git branch -D perf/reference-lazy-texel-context
git branch -D perf/vi-boundary-observation-count
git branch -D perf/wm2000-w01-w05-integration
git branch -D port/production-t1-planner-v11
git branch -D worktree-corpus-resweep
git branch -D worktree-h3-gate-config
git branch -D chore/launch-status
git branch -D chore/split-emit-wip
git branch -D combined-audio-test
git branch -D docs/aki-recomp-handoff
git branch -D feat/rt64-backend-ffi
git branch -D feature/certification-denominator-v5
git branch -D feature/nextest-temp-isolation
git branch -D feature/private-release-runner-v1
git branch -D feature/release-consumption-evidence-v1
git branch -D feature/release-report-v16
git branch -D fix/render-artifact
git branch -D fix/render-scissor
git branch -D fix/rt64-wall67-color-registers
git branch -D fix/texrect-tmem-tile
git branch -D handoff/session-2026-08-15
git branch -D integrate/wt-recognizers
git branch -D integration/cop0-authority-main-20260724
git branch -D integration/cop1-exact-main-20260724
git branch -D integration/v28-substrate-port-20260724
git branch -D lane/combined-input
git branch -D lane/tmem-ia4-tlut
git branch -D perf/pgo-workflow
git branch -D perf/runtime-measurements
git branch -D port/m2-metal-semantics
git branch -D port/production-t0-neutral
git branch -D port/render-ir-integration
git branch -D port/rt64-dashboard
git branch -D port/rt64-formats-dither
git branch -D port/rt64-fragment-combiner-wiring
git branch -D port/rt64-triangle-vertices
git branch -D rt64-honest-inventory
git branch -D tools/mupen-core-pin
git branch -D worktree-agent-a938f9dbb7c71662d
git branch -D worktree-rt64-depth-mode-port
git branch -D worktree-rt64-fragment-registers-impl
git branch -D worktree-rt64-texture-gen
git branch -D worktree-wm2000-discover-gates
git branch -D conflict-probe
git branch -D coverage-fragment-fn-rebase
git branch -D docs\/fix-stale-and-false-claims
git branch -D docs\/marketing-site
git branch -D feat/aki-cross-donor
git branch -D feat/aki-quick-wins
git branch -D feat/input-settings-ui
git branch -D feat/mupen-core-build-pin
git branch -D feat/rt64-production-coverage-node1
git branch -D feat/rt64-render-hook-multiplex
git branch -D final-compose
git branch -D fix/aki-audio-imem-base
git branch -D fix/launch-main-gates
git branch -D fix/native-4x3-presentation
git branch -D fix/overlay-stride-aliases-only
git branch -D fix/wm2000-task-compute-disposition
git branch -D integrate/hidden-coverage
git branch -D integrate/main-validation-baseline
git branch -D integrate/metal-blend-gate
git branch -D integrate/render-foundations-1
git branch -D integrate/rt64-derived-port-state
git branch -D integrate/rt64-portable-tranche
git branch -D integrate/rt64-tmem-budget
git branch -D integrate/wm2000-01-rt64-foundation
git branch -D integrate/wm2000-rt64-common
git branch -D integration/audio-hle-main-20260724
git branch -D integration/fpu-main-transplant-20260724
git branch -D m2-4-validator-profiles-staged
git branch -D m2-5a-reference-corpus
git branch -D m2-5b-wgpu-assessment
git branch -D measure/corpus-certification-sweep
git branch -D perf/wm2000-cycle-key-census
git branch -D perf/wm2000-task-cpu-census
git branch -D plan/rt64-wgpu-backend-triangle-integration
git branch -D port/m0-3-measurement
git branch -D port/m2-5b-exclusive-witness
git branch -D port/m2-5b-profile-consumer
git branch -D port/m2-metal-caps
git branch -D port/m2-metal-submission
git branch -D port/m4-1-tmem-wire-state
git branch -D port/m4-2a-physical-state
git branch -D port/m4-2b-loadtile
git branch -D port/m4-2c-loadblock
git branch -D port/m4-2d-packet
git branch -D port/m4-3-1-loadtlut-plan
git branch -D port/m4-3-1b-tlut-destination-mask
git branch -D port/m4-3-3b-indexed-decode
git branch -D port/m4-3-3c-physical-texel-reader-clean
git branch -D port/production-t2-abi
git branch -D port/render-ir-spine
git branch -D port/rt64-combiner-slice2
git branch -D port/rt64-combiner-slice3
git branch -D port/rt64-endian-swap
git branch -D port/rt64-inventory-v2
git branch -D port/rt64-production-triangle-draw
git branch -D port/rt64-texture-rectangle
git branch -D port/rt64-triangle-composition-precursor
git branch -D port/rt64-triangle-decode
git branch -D pr91-land
git branch -D pr91-verify
git branch -D pr92-verify
git branch -D refactor/game-neutral-runner-build
git branch -D refactor/typed-boundary-units
git branch -D worktree-agent-aa72b9718f7bfbc53
git branch -D worktree-agent-abbfc4bfde304e999
git branch -D worktree-agent-af4b508c693ca3128
git branch -D worktree-blend-selector-cycle-semantics
git branch -D worktree-m2-5b-decoration-index-repair
git branch -D worktree-m2-5b-taxonomy-two-classes
git branch -D worktree-m4-3-1c-tlut-wrap
git branch -D worktree-m4-3-2-loadtlut-finalize
git branch -D worktree-m4-3-3a-direct-texel
git branch -D worktree-production-t3-phase-b
git branch -D worktree-rt64-alpha-compare-slice
git branch -D worktree-rt64-combiner-slice1
git branch -D worktree-rt64-coverage-semantics
git branch -D worktree-rt64-parity-cooldown-flake
git branch -D worktree-rt64-raster-vs-lane
git branch -D worktree-wave-b-transferpak-overlay
git branch -D worktree-wgsl-three-nearest-final-integration
git branch -D worktree-wgsl-three-nearest-integration
```
