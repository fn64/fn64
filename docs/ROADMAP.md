# Roadmap — full Rust decomp/recomp pipeline + runtime

Decided 2026-07-16 (user + session). Three phases; R and D run as **parallel
wave tracks** (disjoint crates). Render endgame this phase: **RT64 as the
faithful renderer, wgpu port deferred to Phase P**. Executor mix:
**codex-heavy implementation waves, session-model adversarial verify + merge
gate** (see DELEGATION.md).

Status legend: `[ ]` open, `[~]` dispatched/in-flight, `[x]` merged+verified
(AGENTS.md bars). Update this file in the same commit as the work it tracks.

## Phase R — close the "OoT renders faithfully" gate

The standing milestone gate (OOT-STATUS.md "Beyond OoT"). The renderer lands
faithful frames for title/attract; the open frontier is audio (R5) and the
outdoor gameplay eye-gate (R3b).

Closed-item policy: a finished item is deleted, not archived — git has the
history. What survives below is only what changes what the next session DOES:
negative results (a ruled-out cause someone would otherwise re-investigate)
and scope limits that make an open item meaningful.

- **R2 — the artifact was NOT projection** (closed 2026-07-16). Kept because
  the name misleads: root cause was the ReferenceBackend quartering
  G_LOADTLUT counts and decoding every G_LOADTILE rect from the source-image
  origin (CI palette + tiled source layout). Do not re-open a projection
  hunt. Reference is the oracle; RT64 is the faithful lane.
- **R3 eye-gate PASSED — scope: title/attract camera ONLY** (2026-07-16).
  Kept because it bounds R3b: 7 RT64-lane frames at swaps 400-1300 judged
  faithful by the user. Gameplay cameras are NOT covered by this pass.
- [ ] **R3b outdoor gameplay eye-gate — blocked on the capture route, NOT on
  the renderer.** Indoor scenes (Link's house, Navi cutscene) render
  faithfully. The "missing Kokiri Forest world geometry" was investigated at
  length (R6, 2026-07-16) and the renderer is **exonerated** — do not re-open
  it as a render bug:

  - The scripted route never takes a scene/room transition. Across swaps
    4200-4300 the PlayState stays a live `Play_Main`, room 0 stays loaded and
    unchanged, `load_active=0`, and a PI-DMA trace shows no room/scene load
    after swap 4015. The apparent Kokiri view is not a loaded Kokiri room —
    the script drove Link/camera out of the Link's-house interior.
  - An independent task differential agrees: tasks 4149 (good) and 4289 (bad)
    submit the *same* populated segment-3 room graph; the bad task decodes 737
    triangles (good: 1006). There is no outdoor graph in the input to skip, so
    neither an rs-lane payload handoff nor an RT64 HLE skip can be the cause.
  - The green speckle is secondary: the bad task switches to S2DEX2 and issues
    `G_BG_COPY` from unwritten background data. No-oping just that copy turned
    speckle black without revealing geometry.

  To close: make the capture route deliberately trigger and verify an exterior
  transition, then re-gate. Evidence: `FN64_GFX_TASK_DUMP`, `OOT_STATE_TRACE`
  (see FAST-LOOP.md).
- [ ] **R4 branch hygiene — do NOT blanket-prune.** Checked 2026-07-17:
  `fix/render-combiner`, `fix/render-blend`, `fix/render-othermode`, and
  `fix/render-texfmt` are fully merged to main (0 unmerged commits — safe to
  delete). But **`fix/render-scissor` still has 1 commit not in main**; triage
  it before deleting anything. `scripts/wt.sh prune` is the safe tool (it
  refuses unmerged/dirty/live-job worktrees). ~35 remote branches exist total;
  most predate the rename.
- [ ] **R5 audio out — STILL BROKEN (static + over-speed); two causes found
  and fixed, at least one remains.** Status 2026-07-17. Do NOT treat the
  landed fixes as closing this; the user still hears static.

  FIXED + verified so far: (a) rate mismatch — the harness ladders opened
  the stream at 48 kHz while the game produces 32 kHz with no converter, so
  the ring starved ~1/3 of the time and the callback zero-fill was static
  (CpalBackend now negotiates + linear-resamples producer-side); (b) the
  AI_LEN feedback loop was severed — osAiGetLength returned a static latched
  value, so the game could not pace production (now reports live ring depth
  converted to guest-rate bytes; regression-tested).

  NOT FIXED — the open frontier: the game STILL overproduces. Live evidence
  (heartbeat `ring_frames`, rs+rt64 shell, 40 s): ring pegged at exactly
  12000 frames (its 250 ms cap) from ~swap 250 to the end of the run, with
  ~180 AI buffers per 60 VI swaps (~3 per frame, where ~1 is expected).
  The drop-oldest ring cap then skips playback continuously = the static the
  user hears. So the AI_LEN fix did NOT change the producer's behavior.
  Next probes, in order: (1) confirm the guest actually CALLS osAiGetLength
  in the rs lane (its vram 0x800D32E0 IS in the host-lookup table, but that
  proves wiring, not calls) — log call count + returned value; (2) if it is
  called and ignored, read decomp `AudioMgr_ThreadEntry`/`AudioMgr_HandleRetrace`
  for what actually gates queueing (likely an osAiGetStatus AI_STATUS_FULL
  bit, or a retrace-message cadence our VI ticker over-delivers); (3) suspect
  the VI retrace ticker itself — if retrace messages arrive faster than
  60 Hz, the audio thread runs its whole produce cycle too often, which
  would ALSO explain the user-reported over-speed feel.

  Ruled out as *sufficient* causes — each was real, each was fixed, static
  survived all of them: App Nap throttling the producer; the 48 kHz/32 kHz
  rate mismatch; the severed AI_LEN feedback loop. The pattern to resist:
  every one of these looked like the whole answer at the time. A fourth
  plausible-and-partial cause is the expected shape of the next finding, so
  do not close R5 on a single fix that merely improves it — close it by ear,
  foreground and backgrounded.

- [ ] **R8 rdram word-swizzle is a recurring bug class, not three bugs.**
  Surfaced a third time in the R7 shell work (green-tinted, noisy logo in the
  window presenter); previously in DMA and framebuffer capture. Each time it
  was fixed at the site. AGENTS.md ("mechanism over patch") wants the sweep
  that finds the rest of the class, not a fourth one-off: every host<->rdram
  byte-order boundary should be enumerated and typed so the next instance
  fails to compile. Open because the fixes landed but the mechanism did not.

## Phase D — fn64 owns discover → decomp

Today OoT symbol/section metadata comes entirely from the zeldaret decomp
via aki-recomp's Python (`import_oot_syms.py`, `gen_stubs.py`). The zeldaret
answer key (10,833 named fns) makes OoT the perfect graded target.
fn64-discover has Phases 1/2/4/5 + bounded-6; the rest is design-only
(DISCOVER-DESIGN.md).

Current grading (D1+D1.5, 2026-07-16, `gate_d1` — but see H3: these numbers
are reproducible on one machine only): OoT 90.6% precision / 72.3% recall;
NW4E 44.7%/89.0%; NWXE 36.4%/28.5%.

Two findings worth not rediscovering:
- **Recall is a Phase-2 problem, not a detector problem.** OoT recall was
  0.82% until D1.5 taught Phase 2 to discover DMA-table overlay load-images
  (OoT loads overlays via DMA tables, not descriptor tables), which alone
  took it to 72.3%. If recall stalls again, look at what Phase 2 exposes
  before tuning detectors — they can only hunt inside discovered images.
- **Table-derived candidates are an honest 0 everywhere.** Descriptor tables
  prove load-images, not entry points. Not a bug; don't "fix" it.

Still open from D1.5: resident `code`/`n64dd` destination discovery.
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

## Phase H — cut the aki-recomp tether (blocks going public)

`/Users/jer/Code/aki-recomp` is a **legacy project** (user, 2026-07-17), yet
fn64 cannot boot OoT without it. Everything the toolchain needs should live in
fn64. This phase is not "declare the dependency" — it is delete it.

Inventoried 2026-07-17. Five things are pulled from the legacy checkout; only
ONE has a permanent reason to be out-of-tree:

| What | Size | Verdict |
|---|---|---|
| `refs/N64RecompSource/include` (`recomp.h`) | 475 lines | **VENDORED 2026-07-17** at `crates/fn64-boot-harness/bridge/include/vendor/` (MIT, (c) 2024 Wiseguy, license beside it). Only `recomp.h` was ever needed — `librecomp/sections.h` is fn64's own clean-room header and always was. |
| `games/OOTU/oot.toml` | 156 lines | **Stays out-of-tree — H1's original verdict was WRONG.** Its paths are relative to its own directory and point at `oot-ntsc-1.0.z64` and `syms/dump.toml` (989K of decomp-derived symbols). In-tree it would be a config reaching back into the legacy checkout. It dies with the oracle metadata at D-gate, not before. |
| `runtime/ABI-SURFACE.md` + `abi_surface.json` | 204K | **Not needed.** Nothing consumes it; the live oracle is `fn64-abi`'s `c_smoke` link test (H2). |
| `games/*/syms/dump.toml`, `refs/oot-decomp/**/segments.csv` | — | **Dies with Phase D.** D4 emits fn64's own pack; D-gate proves it. |
| `games/OOTU/rsp-recomp/src/oot_aspmain.rs` | 186K | **Missed by the first inventory; found 2026-07-17 building H1.** ROM-derived (recompiled aspMain ucode), so it cannot be vendored. Worse than a hardcoded path: `examples/oot-boot/audio-ucode/build.rs` reaches it via `../../../../aki-recomp/...`, which assumes fn64 and aki-recomp are on-disk SIBLINGS. It breaks in any worktree (they nest deeper) and for any contributor. Needs an env var + loud skip, like H3. |
| `games/*/*.z64` | — | **Stays out-of-tree forever.** But that is a plain `ROM=` path, never `$AKI`-shaped. |

So `$AKI` is not a domain concept needing a better name — it is "the old
project's working directory", four artifacts fn64 should own plus one ROM
path. Do NOT rename it; delete the need for it.

- [x] **H1 vendor `recomp.h`** (done 2026-07-17). `crates/fn64-boot-harness/
  bridge/include/vendor/recomp.h` + `LICENSE-N64Recomp`. `RECOMP_H_DIR` now
  defaults to it and only overrides (a bogus override still fails loudly — no
  silent fallback). Verified: the C lane builds the whole generated corpus with
  `RECOMP_H_DIR` unset; override still honored; 621/621 workspace tests pass.

  Two things the original inventory got wrong, both found by doing it:
  - Only `recomp.h` was ever needed. `librecomp/sections.h` is fn64's OWN
    clean-room header (`bridge/include/librecomp/sections.h`) and always was —
    the build.rs hints saying otherwise were wrong, and are fixed.
  - `oot.toml` must NOT move in-tree (see table above).
- [ ] **H1b the audio-ucode sibling-path assumption.** `examples/oot-boot/
  audio-ucode/build.rs` hardcodes `../../../../aki-recomp/games/OOTU/rsp-recomp/
  src/oot_aspmain.rs` — it assumes fn64 and aki-recomp are on-disk siblings, so
  it breaks in ANY worktree and for any contributor. The file is ROM-derived
  (186K of recompiled aspMain) so it cannot be vendored; it needs an env var
  plus a loud, named skip, exactly like H3. `--no-default-features` (or
  `OOT_SKIP_AUDIO_UCODE`) is the current workaround.
- [x] **H2 `ABI-SURFACE.md` — RESOLVED 2026-07-17: fn64 does not need it, and
  the clean-room question never had to be answered.** It was AGENTS.md
  read-order item 3 (mandatory) while living in a legacy repo, so a fresh clone
  could not satisfy its own contract. The fix was neither copying it in nor
  demoting it — it was noticing the entry had become cargo cult:
  - **Nothing consumes `abi_surface.json`.** Zero hits across all `.rs`/`.sh`/
    `.toml`/`.py`. The `nm`-based completeness gate DESIGN.md §4 describes is
    aspirational; the one `nm` mention in code is a comment about a past
    observation.
  - **The live oracle is a test that runs.** `crates/fn64-abi/tests/c_smoke.rs`
    compiles a real C caller against the staticlib and executes it, proving the
    extern symbols link and are callable exactly as generated `RecompiledFuncs`
    would call them. Its own doc comment calls it "the mechanical check that the
    ABI-SURFACE.md shape is honored". 62/62 pass.
  - **The ~45 citations are provenance, not lookups.** They record which allowed
    source a claim came from, per the clean-room protocol. They stay honest
    whether or not the file is reachable — a footnote does not break when the
    library closes.

  It WAS a live oracle during transcription (`rdram.rs:12` and `lib.rs:617`
  both record waves mistranscribing it), which is why the read-order entry
  existed and why it outlived its job. Read order now points at `fn64-abi` and
  its tests. Kept as a closed item — the reasoning is the point; delete once
  Phase H lands.
- [ ] **H3 `fn64-discover` gates cannot run off the author's machine.** Not
  env vars with defaults — compile-time `const`s: `gate_b1.rs:18/20-22`,
  `gate_d1.rs:18-19/24-27`, `gate_b2.rs:39/51` hold
  `/Users/jer/Downloads/...z64` and `/Users/jer/Code/aki-recomp/...`. The
  D1/B1 grading numbers cited in Phase D are reproducible by exactly one
  person. Fix: env var + a loud, named skip when unset (never a silent pass —
  that is the "silent shrug" AGENTS.md bans).

Nothing here blocks R5/Phase D on the author's machine, which is why it has
survived this long.

## Phase P — pure-Rust endgame (after R-gate + D-gate)

## Phase P — pure-Rust endgame (after R-gate + D-gate)

- [ ] **P1**: retire the C lane to CI-oracle-only (DESIGN.md M3); relicense
  checkpoint.
- [ ] **P2**: wgpu render backend (RENDER-WGPU-PORT-PLAN.md), eye-gated
  against the then-verified RT64 output.
- [ ] **P3**: shell polish — input, audio device handling, windowing.
