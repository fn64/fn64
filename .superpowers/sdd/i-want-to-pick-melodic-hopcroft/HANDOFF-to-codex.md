# WM2000 Playable — Handoff (Claude Code → Codex)

_Written 2026-08-23. Everything below is verified as of commit `31cca172`._

## The goal (unchanged)
Make **WWF WrestleMania 2000 (WM2000)** fully playable on the **all-fn64 Rust
stack** — `FN64_RECOMP=rs` + `FN64_RENDER=wgpu`, no C++ RT64, no C recompiler —
with no visible rendering defects and frame rate at the title's **30Hz budget
(33.3 ms per DRAWN frame)**. Secondary: exhaustive RDP hardware parity, grown
incrementally.

Plan: `/Users/jer/.claude/plans/i-want-to-pick-melodic-hopcroft.md`
SDD ledger (READ THIS for full task history): `.superpowers/sdd/i-want-to-pick-melodic-hopcroft/progress.md`

## Where the work lives
- **Worktree / cwd:** `/Users/jer/Code/fn64/.claude/worktrees/wm2000-playable`
  (a git worktree — run everything from here; do NOT cd to the main checkout).
- **Branch:** `worktree-wm2000-playable`, **727 commits ahead of origin/main**.
  Nothing on this branch is pushed. Do NOT push/merge/force-push without the
  owner's explicit go. Merge-to-main was deferred (owner wants small
  lowest-risk-first PRs, on his say-so).
- **Remote:** `origin git@github.com:fn64/fn64.git`.
- Tree is clean except intentionally-uncommitted scratch: `README.md`,
  `.superpowers/sdd/**`, `target-task6/`.

## What just shipped this session (both committed + tested, 92+3 shell tests green)
1. **`986f58ad` — tabbed settings overlay + overscan setting.**
   - F1 overlay is now three tabs: **Input** (existing bindings/stick UI
     verbatim) / **Video** / **Audio** (stub). `crates/fn64-shell/src/overlay.rs`.
   - **F-key tab shortcuts:** while the overlay is OPEN, F1/F2/F3 select
     Input/Video/Audio (shown as affordance "Input F1"…); when CLOSED they keep
     global meanings (F1 toggle overlay / F2 screenshot / F3 HUD).
   - **Overscan** is a player setting: new `crates/fn64-shell/src/video_config.rs`
     (`VideoConfig{overscan,zoom_fill,persist}`, `~/.config/fn64/video.toml`,
     mirrors InputConfig). Default `overscan=1` crops exactly the one uncovered
     stale column (WM2000 col 479). `FN64_OVERSCAN=<px>` env override read once at
     boot. `overscan=0` = raw full scanout. Display-time crop only — guest RDRAM
     and stride untouched. Driven in `main.rs` present: `target_width = stride -
     overscan` (min 1 col). The old geometry-derived `vi_visible_width` approach
     was REVERTED (col 479 is genuinely scanned → it's policy, not geometry; no
     oracle can adjudicate).
2. **`31cca172` — zoom-to-fill actually works.**
   - New `crates/fn64-shell/src/zoom_fill.rs`: `ZoomFillRenderer`, a cached
     fullscreen-triangle wgpu pipeline (uses `pixels::wgpu` / wgpu 0.19) that
     samples the frame across the whole surface via `pixels::render_with`,
     nearest sampler. Built once, bind group rebuilt only on texture resize.
   - `main.rs` present: `else if self.video.zoom_fill` branch AFTER the overlay
     branch → zoom-fill applies only when overlay/HUD CLOSED; the false branch is
     the untouched `pixels.render()` / `render_over()` letterbox (byte-identical).

## ⚠️ NOT YET VERIFIED — do this first
Both features pass unit tests and are correct by construction, but **neither has
been checked on screen** (the agents that wrote/verified them can't run Metal):
- Does `overscan=1` (default) actually hide col 479 in a live intro frame, with
  kept columns identical to `overscan=0`?
- Does `zoom_fill=true` visibly fill the window (stretched, no matte)?

**How to verify on-screen** (needs a real Metal window):
```
cd /Users/jer/Code/fn64/.claude/worktrees/wm2000-playable
FN64_RECOMP=rs FN64_RENDER=wgpu ./scripts/play-wm2000.sh
```
- Confirm the banner says `renderer: wgpu` and the rs recomp lane (anything else
  silently gives the C lane / reference renderer — WRONG stack).
- `FN64_SKIP_EMIT=1` reuses an existing emitted crate (much faster; the script
  prints the scratch dir).
- Open the overlay (F1), Video tab (F2), toggle zoom-fill and drag overscan.
- For a bounded/headless frame capture: `FN64_FRAME_DUMP=<dir>` writes each
  tripwire frame as a 480-wide PNG; pair with `FN64_PUMP_CENSUS=1
  FN64_PUMP_CENSUS_WARMUP=<n> FN64_PUMP_CENSUS_PUMPS=<n>` so the run EXITS.
  Inspect cols 476–479 to confirm the overscan crop.
- macOS has **no `timeout`**; the GUI/Metal init occasionally stalls — kill and
  rerun. Never leave a GUI loop running.

## In flight (background Codex task, may already be done when you read this)
- **Framerate + audio-chop eval** (task-41). Report will be at
  `/Users/jer/.claude/jobs/297000b1/tmp/task-41-report.md` (that jobs dir is
  Claude-Code-session-scoped and may be gone under your Codex harness — if the
  file isn't there, just re-run the eval yourself; the brief is at
  `.../tmp/task-41-perf-audio-eval-brief.md`, or reconstruct from the "measure"
  rules below). It measures ms/DRAWN-frame (attract + in-match) and attributes
  the audio chop (downstream underrun vs independent audio defect).

## Open work (from the plan / ledger)
- **Perf to 30Hz:** parallel raster (rayon, default-ON — `FN64_PARALLEL_RASTER`
  unset ⇒ true, in `crates/fn64-render-wgpu/src/targets/raw_triangle.rs:455`)
  was projected to bring gameplay from ~52.79 ms/field into ~25–27 ms/frame.
  **Confirm live** whether that holds in a real match (task-41 answers this).
- **Audio chop:** likely a symptom of the framerate miss (producer late →
  callback underruns), but that needs evidence, not assertion (task-41).
- **#20** CI4/CI8 triangle S-plane texcoord addressing — glyph-artifact
  candidate; still pending (needs HW/spec or a booted-ares oracle).
- 8 wgpu-refused RDP gaps (two-cycle TEXEL1, LOD/mipmap, alpha-dither) — any-ROM
  breadth, not WM2000 blockers.
- **Merge to main** — deferred, owner-gated. Small lowest-risk-first PRs only.

## Hard-won rules (violate these and you'll waste a day — from the ledger + memory)
- **Perf measurement:** WM2000 is 30Hz → 1 drawn frame = 2 pumps (a pump is a
  60Hz field). Report ms/DRAWN-frame vs 33.3ms, not pump ms. Use the census
  **phase counters**, NOT leaf/`sample` profiles (the guest runs on coroutines
  that profile as idle-at-85%-CPU). Guard against **thermal drift** (baseline
  drifts 478→530 ns/px within a session) — any A/B must be interleaved min-of-N,
  never a single sequential before/after. Big byte counts ≠ bottlenecks;
  counters ≠ outcomes.
- **Oracles:** RT64 (C++ HLE) and angrylion (bit-accurate, but MAME-licensed →
  OUTPUT-only, NEVER link into the tree) are the parity targets.
  **`fn64-render-reference` is UNVALIDATED — never use it as a comparison
  oracle.** Corpus cases must be HAND-DERIVED, never captured game packets.
- **Verify before completion:** every fix gets mutation-tested (revert it, the
  gate must go red). A green gate with shallow coverage is worse than a red one
  that caught a real defect.
- **Shared git index hazard:** other worktrees share the index — commit with
  `git commit -- <explicit paths> -m` OR `git add <paths>` then `git commit`
  (note `git commit -- <p> -m "msg"` mis-parses; the `--` swallows `-m`, so
  prefer `git add` then `git commit -m`). Don't commit the dirty
  README/scratch/sdd/target-task6.
- The game `present()` path is `#[cfg(fn64_game_linked)]` — it only compiles+links
  with a ROM emit (`RECOMPILED_DIR`/`ROM` set via `scripts/play-wm2000.sh`). The
  content-free `cargo test -p fn64-shell` does NOT type-check that module; verify
  the `game` module by inspection or a full emit build.

## Verification suites (the "fn64 verification set")
Six suites gate this repo; skipping any has shipped red gates before. Key ones:
`cargo test -p fn64-shell`, the wgpu lib suite, and the RT64 parity gate
`python3 scripts/check_rt64_parity.py` (expect **PASS 33/37** — 4 accounted
exceptions). Run the parity gate after any render change; run boot-harness too.

## Memory pointers (Claude-Code auto-memory, may not carry to Codex — the facts are here + in the ledger)
Most relevant: `wm2000-live-render-path`, `wm2000-boot-play-goal`,
`wm2000-all-fn64-stack-goal`, `perf-measure-before-dispatching`,
`wm2000-perf-gpu-draw-is-unpresented`, `rt64-rust-renderer-unverified-vs-oracle`,
`shell-interactive-probe`.
