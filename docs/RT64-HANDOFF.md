# Handoff: WM2000 on the all-Rust stack


> **PROVENANCE WARNING.** This document's stated authority is
> angrylion-rdp-plus, which `AGENTS.md:26-45` EXCLUDES from fn64's clean-room
> protocol (`docs/DISCOVER-PLAN.md:2260` records the exclusion). Its
> observations about WM2000 and about fn64's own behaviour remain valid --
> measured facts about a ROM are explicitly allowed -- but **any claim here
> about what HARDWARE does, sourced only to angrylion, is not admissible as
> fn64 authority.** Re-ground such a claim on pinned RT64 (MIT), the public
> libultra headers, or a fresh measurement before acting on it.

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
3. **DONE. The corpus is finished and it found the root cause.** See
   `docs/RT64-WM2000-TEXEL-LOCALISATION.md`'s final sections.

   Both blockers are fixed (`3109ff66`): wgpu's `resident_bytes` refusal is
   answered from inside the packet by opening the textured command list with
   a full-extent fill (the guard was NOT widened), and the KEY ITSELF WAS
   WRONG -- texrect high edges are exclusive while `G_FILLRECT`'s are
   inclusive, so the fixture drew one fewer row and column. That was the
   handoff's undiagnosed "RT64 completes but does not match the key": a
   fixture defect, not a finding.

   **CONFIRMED ROOT CAUSE, fixed in `9168bb9a`:** deferred guest reads were
   captured as RAW ABI STORAGE bytes on both the production and conformance
   paths, while `CapturedGuestRead`'s contract is N64-LOGICAL bytes and the
   TMEM load executors index the capture linearly. Every 32-bit word of
   texture data reached the sampler byte-reversed. A 32-bit command word
   survives a raw read by accident (`^3` composed with a little-endian host
   load cancels for an aligned word), which is why this was invisible in
   command decode and fatal only for byte-granular texture data.

   Both textured corpus cases now report `identical` -- key, RT64 and wgpu
   agree byte-for-byte on all eight texels of both. Mutation-verified:
   reverting the capture to a raw `to_vec()` fails
   `a_texel0_referencing_one_cycle_texrect_reaches_guest_rdram`.

   **CONFIRMED ON SCREEN. WM2000's textures render.** The operator-scratch
   byte-lane-fix frame and the before image from the XOR4-only branch show the
   same scene under
   the previous two fixes. The contrast is not subtle:

   - **Before:** dense magenta/green/black per-pixel speckle across every
     surface; two wrestler silhouettes and yellow hair blocks discernible
     only as shapes.
   - **After:** real imagery. Skin tones and facial detail (beards,
     eyebrows), a black t-shirt with legible orange "AUSTIN 3:16" print, the
     steel-truss arena background correctly shaded, and readable text --
     "Single Match", "STEVE AUSTIN VS STEVE AUSTIN", the red "RAW IS WAR"
     logo.

   Zero panics and zero backend errors across 4,200+ dumped frames.

   So the texel-noise defect recorded in
   `RT64-WM2000-TEXTURE-STATE.md` is CLOSED. The remaining known visual gaps
   (colour bands, blocky glyphs) were never separately scoped and should be
   re-checked against a current frame before anyone scopes them -- the
   handoff's own advice was not to scope them separately until this was
   fixed.

3b. **The rest of the corpus: PARTLY DONE.** It was 10 fill-rectangle cases;
   it is now **17**, and the textured half of it found and confirmed the
   byte-lane defect above.

   Landed since:

   - **`textured-rect-wide-line-two`** -- the first tile `line` other than 1.
     An 8x2 RGBA16 tile puts two 64-bit words in a TMEM row, so the row
     stride is `line * t`; a wrong multiplier is INVISIBLE at `line = 1`
     because any multiplier times row 0 is still row 0. `identical`, both
     backends match the key. Mutation-verified: forcing `line = 1` leaves the
     backends agreeing with each other but makes BOTH stop matching the key.
   - **`textured-triangle-point-sampled`** -- the first raw triangle, and the
     first case on the path WM2000 actually draws through. **wgpu matches the
     hand-derived key on all twelve covered pixels; RT64 writes nothing.**
   - **`textured-rect-ci4-tlut`** -- the first colour-indexed case. CI4
     indices in low TMEM, an RGBA16 palette in high TMEM via `LoadTlut`,
     and `en_tlut` switching the sampler onto the lookup. `identical`, both
     backends match the key across a non-identity index permutation. Note
     the index image must be LOADED through a 16-bit form and then
     redescribed as CI4 -- fn64 refuses a direct four-bit load by name, and
     that is what real N64 code does anyway. This clears the palette as a
     suspect for CI4 point sampling.
   - **A texel that aliased the background.** `TEXTURE_TEXELS[3]` was
     `0xffff` = `STALE`, so a skipped pixel and a correctly drawn one were
     the same observation. Now `0x7fff`. This was live: the triangle case
     under-reported 9 differing pixels of 12 because of it.

   **RESOLVED, and it turned into a real wgpu fix.** The raw-triangle case
   went through three wrong readings before landing; the resolved chain is:

   - RT64 **does** rasterize raw triangles. "Writes no pixels" came from a
     triangle-ONLY list; with a texrect in the same packet it draws both.
   - The same edge words mean a RECTANGLE to fn64 and a RIGHT TRIANGLE to
     RT64, because RT64 derives `v1 = (XH at YH)`, `v2 = (XH at YL)`,
     `v3 = (XL, YM)` -- `v1`/`v2` share the H edge's X, so one command
     cannot be a box. The case emits TWO triangles that tile it.
   - RT64 evaluates S at three VERTICES and only `v3` carries `Dx`, so with
     `De = 0` each half's `base` must be the S of its OWN H edge. Authored
     that way, at the hardware scale, **RT64 reproduces the key exactly**.
   - The hardware scale, derived from angrylion (now cloned durably at
     `/Users/jer/Code/angrylion-rdp-plus`): `ss = s >> 16` to S10.5
     (`rasterizer.c:479`), `tcdiv_nopersp` applies NO scale
     (`tcoord.c:1024`), `*S = locs >> 5` to texels (`tcoord.c:143`). One
     texel = `2^21` plane units.
   - **That exposed a live wgpu defect.** `PLANE_TO_TEXEL` was `2^21` and
     its result is consumed as S10.5, so the sampler applied the `>>5`
     AGAIN -- the `2^5` counted twice. Fixed to `2^16`. The case is now
     `identical` and `Rt64Authoritative`; the corpus went 14 -> 15.
     Confirmed on screen: a full ROM run renders frame 4090 identically to
     the committed pre-fix image, zero panics, zero backend errors.

   Note the four `rdp_harness` tests had been asserting the DEFECT's output
   -- they passed with the bug and failed on the fix -- which is why the
   corpus was needed to find it at all.

   Still missing: other combiner programs, other triangle shapes, and CI8
   / bilerp / two-cycle variants. Also still missing is a **captured-from-ROM** case: the reader for
   it is already built (`mod captured`, driven by `FN64_WM2000_PACKET_TSV`),
   but the dump is produced by `fn64-render-reference`'s GBI census under
   `FN64_GBI_PACKET_DUMP`, so it needs a reference-lane ROM run to generate
   and is deliberately never committed (game content).

   **One open corpus case, and it is an RT64 difference:**
   `scissor-narrower-than-rect`. RT64 paints all 38,400 pixels the scissor
   excludes; wgpu leaves them and matches the key. The scissor's encoding
   was verified against angrylion's `rdp_set_scissor` first and found
   wrong in the fixture -- the bounds split across BOTH words and the
   corpus packed them into word 0 -- but correcting that did NOT change the
   outcome, so it is a real difference on a correctly formed command.
   **NOT the perspective path**, which is untouched: fn64's `(S/W) * 32768`
   on raw planes versus angrylion's reciprocal-table divide on the shifted
   `ss` is a structural difference inspection cannot settle, and guessing
   is how that constant got its earlier wrong value of `1024`.
4. **DONE, and the texel noise is CLOSED.** The cause was the deferred
   guest-read capture handing the renderer raw ABI storage bytes where
   `CapturedGuestRead`'s contract is N64-logical, so every 32-bit word of
   texture reached the sampler byte-reversed. Fixed in `9168bb9a` on both
   the production and conformance paths; confirmed on screen (see
   `docs/RT64-WM2000-TEXTURE-STATE.md`): skin tones, facial detail, a
   legible "AUSTIN 3:16" shirt, readable text.

   **Still open and separate: a scene-specific colour cast.** At in-match
   swap 644 skin reads green and the WWF logo magenta. CONFIRMED
   pre-existing -- the same swap from the run before the `PLANE_TO_TEXEL`
   fix is pixel-identical -- and not global, since the entrance scene in
   the same run is correct. Likely the combiner or environment-colour path.
   The comparison MUST use the same swap from both runs; comparing against
   a different scene blames the wrong change.
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
