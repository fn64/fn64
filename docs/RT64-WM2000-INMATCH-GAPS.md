# In-match rendering gaps: hypotheses awaiting measurement

**Status: HYPOTHESIS throughout. Nothing here was measured.** These came from a
static code read by a census lane that was stood down before it ran the ROM,
because a better instrument (a wgpu runner in `fn64-render-conformance`) was
found mid-card. They are recorded so they are not re-derived at full cost, and
they are explicitly NOT results.

The observed symptom, from committed frames (`docs/frames/wm2000-swap-3000-*`,
`docs/frames/wm2000-swap-12000-*`): WM2000 plays a match, but every model is
flat-shaded, broad horizontal colour bands cross the arena, glyphs are blocky,
and crowd/arena detail is missing.

## H2 (revised): the fill drop is two layers upstream of where it was scoped

Commit `1e9b1591` made one-cycle `FillRectangle`s admitted-but-pixel-less and
recorded the nonclaim that closing it "needs the combiner-driven rectangle
executor `targets/fill.rs` names as absent". The read suggests that scoping is
wrong: `plan_fill` returns `Ok(())` without declaring a span
(`raw_dpc/mod.rs:1575`), and `production_adapter.rs:1182` then drops the command
with a bare `continue`.

If that holds, `FillExecutionError::NotFillCycle` is dead code on this path, and
a fix lane aimed at the executor would be aimed at the wrong layer.

**To settle it:** count, during an in-match window, how many fills reach
`plan_fill`, how many return without a declared span, and how many reach the
executor at all.

## A silent wrong-colour path the guard audit could not see

With `en_tlut` clear, a CI4/CI8 texel is aliased to I8 -- the palette *index* is
rendered as greyscale intensity. For CI4 that yields 0x00-0x0F: near-black.

This is the opposite signature from the loud `InvalidTexelByte` abort that fires
when `en_tlut` is set with no TLUT resident. One aborts the packet and is
visible in every log; the other silently paints the wrong colour and appears
nowhere. Only the loud one has ever been investigated.

## The terminal sink nobody counts

`production.rs:3633` returns `Ok(None)` when the schedule is empty. A packet
whose fills and texrects were all dropped upstream therefore reports success
having drawn nothing.

If real, this materially qualifies a headline number: the run that reached VI
swap 17,473 with "zero raw-DPC backend errors" may partly mean *the errors are
not errors*. A dropped-and-reported-clean packet is indistinguishable from a
correctly-empty one at this seam.

**This is the strongest argument for the differential runner.** An output diff
against `fn64-render-reference` catches a silently-empty packet; no error
counter can.

## Instrumentation asymmetry

`plan_raw_triangle` carries nine drop counters. `plan_fill` and
`plan_texture_rectangle` carry none -- so the two command types these
hypotheses concern are precisely the two that emit nothing. A separate reading
found even the triangle counters have no exit hook and no external callers.

## Why this was not measured

The card was redirected. `crates/fn64-render-conformance` already provides a
backend-neutral replay harness with runners for the reference renderer and
RT64, but none for `fn64-render-wgpu` -- the backend fn64 ships. Wiring that
runner turns "which hypothesis explains the colour bands" into "here are the
packets where wgpu and the reference disagree, ranked", and keeps working as a
regression net afterwards. That work supersedes this census.

## OPEN: is the committed lead-in reaching a match, or a stable screen?

**Status: unresolved, and it qualifies claims already made in this repo.**

A lane mapping the match state machine observed that a live boot-ladder run --
the committed two-controller lead-in -- holds constant guest cost for over
1,700 consecutive swaps: ~150 steps/swap, 3.00 gfx tasks/swap, 1.83 audio
tasks/swap, flat to within 5%. Independently confirmed here from the ladder's
own log: gfx tasks track swaps at almost exactly 3.00 from swap 8,550 through
9,881.

Animated wrestlers with a moving camera would not hold constant cost that long.
So one of two things is true, and nobody has yet measured which:

- The lead-in reaches a match and then IDLES, which is expected: the committed
  schedule stops issuing presses at swap 6000, so by swap 9,881 nothing has
  been pressed for ~4,000 swaps. A match with neither player pressing anything
  is a plausible source of a flat rate.
- The lead-in does not reproduce a match unconditionally, and the in-match
  frames captured earlier came from a run whose conditions differ from the
  committed lead-in's.

**Why this matters.** `docs/frames/wm2000-swap-12000-in-match-groundwork.png`
and its sibling are cited as evidence that fn64 renders in-match gameplay. That
evidence stands -- those frames were captured and inspected. What is NOT
established is that the committed lead-in reproduces them on demand, which is
what a regression gate needs.

**To settle it:** the same instrument the input-grammar work used -- watch the
guest's own match state rather than the frame hash, which is documented in this
repo as the wrong instrument here. `0x801589D6` is a one-byte match-state probe
(2 = live, 3 = decision). One run with `WM2000_WATCH` on that address answers
it directly.

**A related harness defect, found by the same lane and worth knowing before
anyone tests input in a match:** the harness previously MIRRORED the scripted
pad onto every plugged port, so both wrestlers made the same move on the same
frame -- a stalemate by construction. A per-port script (`WM2000_INPUT_SCRIPT_P1`)
is required for any real test of whether input reaches gameplay; mirrored input
would read as "input does nothing", a false negative on exactly that question.

---

## MEASURED, in the interactive shell: triangles are NOT being dropped

**Status: CONFIRMED.** Measured 2026-08-20 on the branch that first made the
windowed shell buildable on the all-Rust stack (see `scripts/play-wm2000.sh`),
against the real ROM, with `FN64_TRI_DROP_STATS=1` -- the instrument this
document's "instrumentation asymmetry" section says has "no exit hook and no
external callers". It has one now: the shell.

Four consecutive 100,000-decision ticks, `FN64_RENDER=wgpu`, rs lane:

```
[fn64-tri-drop] tick=400000 total=400000
[fn64-tri-drop]   no_covered_rows = 58850
[fn64-tri-drop]   ADMITTED = 341150
[fn64-tri-drop]   admitted_target 0x0038f800 = 169518
[fn64-tri-drop]   admitted_target 0x003c7c00 = 171632
```

**85.3% of raw triangles are ADMITTED**, and the proportion is stable across
all four ticks (82.8 / 83.3 / 85.2 / 85.3%). The ONLY non-admitting reason that
fires at all is `no_covered_rows` -- a triangle covering no scanline, which is
a degenerate or fully-offscreen triangle and is correct to drop. **Seven of the
nine drop reasons are ZERO**, including every one that would indicate a
texture, tile, or TMEM problem.

Two further facts from the same run:

- **The two admitted targets alternate almost exactly evenly** (169,518 vs
  171,632, a 0.6% split). That is a double-buffer flip, and it means triangles
  are landing in the buffers the VI actually scans out -- not into an orphan
  surface nobody presents. This retires the question `note_address` was added
  to answer.
- **The run logged zero refusals and zero errors.** `grep -icE
  'refus|unbound|unsupported|error|panic|out of scope'` over the whole session
  log returns **0**. In particular `TexrectUnboundTile` -- the refusal a
  textured triangle with an unresolved tile binding MUST produce
  (`production.rs:4203`) -- never fires.

### What this refutes

**The first hypothesis in this document is wrong as stated.** "Every model is
untextured/flat-shaded ... this is likely a TMEM/tile *binding* problem, the
same class already fixed for texrects" predicts a binding failure. A binding
failure is not silent in this code: `production.rs:4198-4212` refuses by name
with `TexrectUnboundTile` rather than defaulting to a zeroed tile, and
`raw_triangle.rs:158-166` refuses `TriangleTextureBindingDisagreesWithOpcode`
if the opcode's textured bit and the binding ever disagree. Neither fired once
in 400,000 decisions.

So the triangles reaching the rasterizer are being admitted, bound, and drawn
into the presented buffer. Whatever makes the models look flat is **downstream
of admission** -- in the sampled value or the combiner -- or the triangles
carry no texture bit in the first place. Those are different investigations
from the one this document scoped, and the cheap next measurement is a count of
`triangle.flags().textured()` true-vs-false among ADMITTED triangles, which
distinguishes them in one run.

**Method note.** This is the first of these hypotheses tested against the real
ROM rather than read from source, and it took one run of a committed script
because the shell now builds on this stack. That is the cheaper instrument this
document was waiting for.

### Follow-up, same instrument: every admitted triangle IS textured

**Status: CONFIRMED.** The measurement named above as "the cheap next
measurement" was built (`raw_triangle_drop_stats::note_textured`) and run.
Three consecutive ticks on the real ROM:

```
[fn64-tri-drop] tick=100000  of ADMITTED: textured = 82806,  untextured = 0
[fn64-tri-drop] tick=200000  of ADMITTED: textured = 166654, untextured = 0
[fn64-tri-drop] tick=300000  of ADMITTED: textured = 255654, untextured = 0
```

**Not one untextured triangle in 255,654.** Every admitted triangle carries
the wire opcode's texture bit, so every one of them takes the `Some(binding)`
arm at `raw_triangle.rs:447` and calls `sample_point`. Combined with the zero
refusals above -- and `TriangleTextureBindingDisagreesWithOpcode`
(`raw_triangle.rs:159`) refuses loudly if the opcode's bit and the binding ever
disagree -- the texture path is not merely reached, it is reached for 100% of
drawn triangles with a resolved tile binding behind it.

**This closes the hypothesis as stated.** "Every model is untextured" is false
at the wire and false at the binding. The remaining candidates are strictly
downstream of `sample_point`'s inputs:

1. the **sampled texel value** -- TMEM contents, tile addressing, or format
   decode returning a wrong-but-valid colour (note this is the same class as
   the `en_tlut`/CI-aliasing path recorded above, which is silent by
   construction and would look exactly like flat shading), or
2. the **combiner**, selecting something other than `Texel0` into the output,
   in which case a correct texel is fetched and then discarded.

Those are distinguishable, and the distinguishing measurement is again cheap:
histogram the returned texel values against the combiner's selected inputs for
one packet. A texel histogram with one or two distinct values indicts (1); a
varied texel histogram with a flat output indicts (2).

**What NOT to do next.** Do not widen any guard on this path. Nothing here
refused anything -- the counters, the binding equality check, and the tile
resolution all report success, which is precisely why the defect is somewhere
that reports success.
