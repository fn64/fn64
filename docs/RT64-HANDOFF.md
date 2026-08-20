# Handoff: WM2000 on the all-Rust stack

Written for a session picking this up cold. Everything below is either measured
or explicitly marked as unverified. Where a prior claim of mine was refuted,
the refutation is stated rather than the claim quietly dropped.

## 1. Where things actually are

**WM2000 is playable in a window, with a controller, on fn64's own recompiler
and renderer.** No N64Recomp C bodies, no RT64 C++ adapter.

```sh
cd /private/tmp/fn64-play-window && ./scripts/play-wm2000.sh
```

F1 gamepad rebinding · F2 screenshot · F3 stack+FPS HUD · F11 fullscreen ·
Esc exit. `FN64_SKIP_EMIT=1` skips the ~2s recompile on relaunch.

CONFIRMED by the owner, hands-on: **basic input works**. CONFIRMED by
measurement: the guest reaches a **live match** (state byte `0x801589D6` = 2 at
VI swap 6,336) with its **clock advancing in minutes** (`0x8016F0AC`), and runs
to swap 53,485 with zero panics and zero raw-DPC backend errors.

**What it is not:** models render flat/untextured, there are horizontal colour
bands, glyphs are blocky, and it runs at ~26-37 Hz. No match has been played to
completion -- and per the disassembly a *script* cannot force one (no button
test gates a pin anywhere; the time limit is 60 game-minutes or unlimited).
A human on a pad is the only cheap route to that.

## 2. Branch state

**`port/rt64-conveyor` is pushed, verified, and green at 575 commits.**
Workspace **8681 passed / 13 skipped**; `-p fn64-render-wgpu --features
host-gpu-tests` **4914 passed / 3 skipped**. `main` is untouched. There is no
unverified work outstanding.

Integration worktree: `/private/tmp/fn64-rt64-main-integration`. Playable
worktree: `/private/tmp/fn64-play-window`.

Verify before any push:

```sh
cargo nextest run --workspace --offline                                  # 8681 / 13 skipped
cargo nextest run -p fn64-render-wgpu --features host-gpu-tests --offline # 4914 / 3 skipped
python3 scripts/lint-docs.py    # 1 known error at RT64-WM2000-VALIDATION.md:360
```

## 2b. Resuming in a new session

From `/Users/jer/Code/fn64`, start a session and paste:

> Read `docs/RT64-HANDOFF.md` on branch `port/rt64-conveyor` and continue from
> section 4. The integration worktree is `/private/tmp/fn64-rt64-main-integration`.
> Do not work in `/Users/jer/Code/fn64` -- it is dirty. One ROM run at a time.

That is enough: the handoff carries the state, the priorities, and the
operational rules. If the worktrees are gone (they live in `/private/tmp` and
do not survive a reboot), recreate one with
`git worktree add /private/tmp/fn64-work port/rt64-conveyor` from the main
checkout -- everything is on the branch, nothing of value lives only in a
worktree.

To see the game immediately:
`cd /private/tmp/fn64-play-window && ./scripts/play-wm2000.sh`
(if that worktree is gone, the script is at `scripts/play-wm2000.sh` on the
branch and needs `FN64` pointed at a worktree of it).

## 3. Findings that refute things this repo previously believed

Each of these overturned a claim someone was acting on. They are the most
valuable thing in this handoff.

**a. The renderer IS the cost -- the "infinitely fast renderer" figure is
stale.** `docs/RT64-PERF-CEILING.md` (which I wrote) cites
`perf-method.md:3234` for "host side alone is 21.55 ms = 1.29x, so a renderer
fix cannot reach 60 fps." That figure's dominant row was the mirror boundary at
8.85 ms, **fixed since by `8109435`**. Measured on the live shell route:
`exec_mirror_ns` = **0.001 ms**, host-side with graphics zeroed = **12.35 ms =
0.37x budget (~81 fps)**. **`RT64-PERF-CEILING.md` needs correcting** -- it is
currently misleading in the direction of "don't bother optimising the
renderer."

**b. WM2000 renders at 30 Hz.** Budget is **33.333 ms**, not 16.667. Measuring
against 60 Hz overstates every gap by 2x.

**c. The rasterizer is the bottleneck, measured.** Per drawn frame: graphics
82.6% (execute/CPU-rasterizer **67.4%**, plan 15.0%), VI presentation 14.7%,
non-graphics host 2.7%. Total **2.13x budget**. Three plausible targets were
refused on evidence: VI presentation (flat across fast/slow pumps), the
whole-RDRAM staging copy (`dpc_calls=0.00`, path not taken), and the RSP
recompiler (0.315 ms here vs 5.09 on the block lane).

**d. ROOT CAUSE FOUND: `PERSPECTIVE_TEXEL_SCALE` is 2^10; angrylion says
2^15.** A 32x error collapses texture coordinates, so **87% of textured
triangles sample exactly one texel** -- the sampler runs, reads real TMEM, and
every pixel lands on the same texel. That is precisely the "flat/untextured"
symptom, and it is why every earlier diagnostic said textured-admitted-sampled
while the screen said otherwise.

It also explains why three prior hypotheses died: binding was fine (255,654 of
255,654 admitted triangles take the binding arm and call `sample_point`),
`Texel0` was not discarded (the fog program samples the texture), and
CI-without-TLUT aliasing matched angrylion byte for byte. The bug was one
constant, downstream of all of them.

**Note the pattern:** this is the second scale-factor defect today. A prior
lane recorded two empirical constants -- `x2^10` perspective and `/2^21`
non-perspective -- with the instruction "cite these; do not re-derive." One of
them was wrong. Constants carried forward on citation still need an oracle
check.

**e. CI-without-TLUT aliasing matches angrylion byte for byte** -- REFUTED as
the glyph explanation.

**f. In one-cycle mode wgpu is right and `fn64-render-reference` is wrong.**
**RT64 is the oracle for this port.** `fn64-render-reference` is a second fn64
implementation, not an authority -- it has happened to be right where wgpu was
wrong several times, which made it a useful cross-check, but this case is the
counter-example: here wgpu is right and the reference is wrong. Cross-check
against it freely; do not promote it to oracle.

## 4. What to do next, in order

The reasoning is in `docs/RT64-ENGINEERING-LOOP.md`; the short form:

1. **Verify and push the 15 commits** (§2). Cheap, and everything else builds
   on them.
2. **Correct `RT64-PERF-CEILING.md`** per §3a. It currently argues against the
   work the measurement says to do.
3. **Extend the parity corpus to triangles, textures, and combiner programs.**
   It is 10 hand-authored **fill-rectangle** cases today -- it cannot see any
   open bug. This is the highest-leverage item: the fast layer does not cover
   the code under change.
4. **DONE -- see `docs/RT64-WM2000-TEXTURE-STATE.md`.** The screen was checked
   after the fix. Surfaces went from flat solid colour to dense per-pixel
   variation, confirming the fix at the pixel level, but the variation is
   **noise, not imagery**. Geometry and shading are visibly correct; the texel
   VALUES are wrong. The defect is now well-bounded: coordinates are right,
   the fetch at those coordinates is not -- so suspect the TMEM address
   computation, the tile descriptor's format/size/line fields, the palette, or
   the byte-lane mapping. Colour bands and blocky glyphs plausibly share this
   cause; do not scope them separately yet.
5. **Then optimise the rasterizer.** ~150 cycles/pixel at 8x overdraw where
   20-30 should do; `pixel_coverage` and `attribute_sample` each rescan the
   same subsamples, and every admitted triangle samples a texture per pixel.
   Needs ~3x, not 20%. Note the fix above may *raise* cost per pixel by making
   sampling do real work, so re-measure rather than assuming the 67.4% holds.
6. **Wire angrylion as a conformance runner.** No longer a theoretical
   argument: **it is what caught the texel-scale defect** (§3d). Reading its C
   by hand found a 32x constant error that every in-tree instrument reported as
   healthy. It is cycle-accurate, currently only read by humans, and wiring it
   as a runner would catch this class automatically instead of relying on
   someone thinking to check. It would also make coverage/AA/dither adjudicable
   instead of permanently UNKNOWN in the guard audit, since RT64 is silent
   there.
7. **Gate parity and the boot ladder.** Both instruments exist; neither is
   wired to anything, so a regression ships silently.

## 5. Hard-won operational rules

Full list in `docs/RT64-WM2000-HARNESS-TRAPS.md`. The ones that cost the most:

- **One ROM run at a time.** Concurrent runs have twice produced false results.
  Never `pkill -f wm2000-boot` -- other sessions share the binary.
- **Ask for the fewest swaps that answer the question.** ~53k emulator steps
  per wall-clock minute; a match is live by swap ~6,336 (~2M steps). A 40M-step
  budget is 12+ hours and was once scheduled to answer what 2M answers.
- **Build+recompile is ~46s of a 30-minute cycle.** Caching it saves 2%.
  Measured, after I proposed it. Don't.
- **`FN64_RENDER` is not exported by the benchmark script** -- it silently
  defaults to the software rasterizer. Print `RENDERER:` in every perf report.
- **Watch guest state, not frames.** The frame hash has given the wrong answer
  twice here.
- **Don't trace unless you need frames.** A traced run buffered every executor
  event in memory *and* wrote a 996 MB sink, running ~3x slower.
- **When a question is cheap for a person and expensive for a script, ask the
  person.** Three scripted input experiments cost ~90 minutes and answered
  nothing; the owner settled it in thirty seconds with a pad.
