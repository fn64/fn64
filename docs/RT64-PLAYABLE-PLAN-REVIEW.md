# Plan review: playable WM2000, and a second playable ROM

A review of the existing plan against what the ROM now does, written on
2026-08-19. It corrects two stale claims, names the one gap no card covers,
and records what a second ROM needs. It adds no new measurements of its own:
every number here is cited to the run or document that produced it.

## What the ROM does today

A clean run of the real WM2000 ROM on the all-Rust stack -- `fn64-cpu-runtime`
plus `fn64-render-wgpu`, no `--features rt64` -- reaches 2,149 VI swaps and
5,967 gfx tasks, exits 0 with no panics, and terminates on the 400,000-step cap
rather than on a render wall. No guard is disabled. The captured guest
framebuffer is `docs/frames/wm2000-allguards-swap240.png`.

## Two claims in the plan were stale

**B5 is closed.** `RT64-WM2000-GAMEPLAY-GAP.md` listed "`FN64_RENDER=wgpu`
refuses general gfx tasks" as a Large blocker. No such refusal exists in
`production.rs`, and the run above disproves it directly. A blocker carried as
Large and unchanged distorts every sequencing decision made against it.

**"Seven walls" no longer describes the work.** Five were cleared, two were
upheld as correct refusals, and no wall remains in the attract loop. The scout
map that introduced the number said explicitly that seven was not a total.

## The three problems behind "playable"

Rendering 3D, correct pixels, and reaching a match are separate problems. The
plan treated the first as the whole job.

**Rendering 3D (S1).** A `RawTriangle` declares no journal write and stages no
`CompletedWrite`, so no 3D geometry reaches guest memory. See
[the S1 section](RT64-WM2000-REMAINING.md) and
[the writeback findings](RT64-TRIANGLE-WRITEBACK.md), which rule out the two
approaches that do not work. This sets the ceiling on gameplay.

**Correct pixels (B3).** The lower field is striped at a one-pixel period, and
`RT64-WM2000-GAMEPLAY-GAP.md` records menu text tiled horizontally about 2.5
times. Both appear in attract as well as menus, so one cause may explain both.

**Reaching a match.** Input is already proven end to end: scripted presses
drive eighteen menu screens. But three independent navigation strategies all
plateau on the same screen, reproducing identical frame hashes, and no frame
has been verified in-match. Menus are demonstrated; gameplay is not. Nothing
in the emulator refuses -- no trap, no panic -- so this reads as unread button
grammar rather than a defect. It is the only one of the three problems that no
card covers.

Reaching a match matters beyond itself: the scout warned that later walls are
gated behind game state rather than code paths, so in-match rendering may
surface failures no attract-loop run can reach.

## What a second playable ROM needs

WWF No Mercy (`NW4E`) is the strongest candidate: it ships with a config at
`~/Code/aki-recomp/games/NW4E/`, and it shares the AKI engine with WM2000, so
the command set the renderer must serve should be close. No document in this
repo yet measures it against the all-Rust stack.

Two things make a second ROM cheaper than the first, and one makes it harder.

Cheaper: every fix landed today was a general defect, not a WM2000 special
case. Two were the same time-travel bug in different registers, where the
planner folds a packet's state before the executor reads it; one was an
admission gate stricter than the evaluator behind it. A second ROM should
inherit all of them.

Cheaper: a census of eighteen menu screens found 21 distinct opcodes and
`RDP_TRI_SHADE_TEX` as the single triangle variant, with zero Z-variants and
zero two-cycle programs. If No Mercy's census is similar, S1's rasterizer
serves both.

Harder: nothing has run No Mercy on this stack, so its first wall is unknown.
The cheap first move is a census and a single run to see where it stops --
before any of its defects are scoped, and before assuming WM2000's fixes
transfer.

## Recommended sequence

1. S1, in stages. Flat-shaded triangles reaching guest memory first.
2. B3, in parallel. Different files, and it gates whether any output is
   trustworthy.
3. Read the menu state machine and reach a match. Cheap, and it de-risks the
   assumption that attract-loop success predicts in-match success.
4. Census No Mercy and run it once. Do this before scoping its work.

Nonclaims: this document measures nothing new. "Playable" is not demonstrated
for any ROM, and a frame is not a correct frame.
