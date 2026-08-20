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
