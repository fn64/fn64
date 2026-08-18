# What actually blocks WM2000 from gameplay

A measurement card, not a plan. The question it was dispatched to answer:
*nobody had established what stands between where the emulator gets today and
a match starting*, and renderer/recompiler work had been sequenced on the
assumption that clearing the known blockers leads to gameplay.

The assumption was wrong in a specific and cheap way, and this document says
how. Every number here was measured in this worktree on this checkout; nothing
is quoted from a prior card. Companion docs:
[`RT64-WM2000-REMAINING.md`](RT64-WM2000-REMAINING.md),
[`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md),
[`RT64-PORT-CARD-BRIEF.md`](RT64-PORT-CARD-BRIEF.md) ("Measure, never
assert").

---

## 1. Headline: nothing blocks gameplay that is renderer or recompiler work

**WM2000 idles in a healthy attract loop waiting for Start, and a single
synthesized Start press exits it into the readable mode-select menu.** The
input path was already complete end to end, at every layer. It had simply
never been driven, because the headless harness had no way to press a button
and nobody had checked whether the interactive shell's keyboard path reached
the guest.

The white-frame regime is not a hang, not a blank-frame loop, and not a
waiting-on-a-device state. It is the attract sequence's own idle segment.

---

## 2. What the guest does in the white-frame loop

Measured on a neutral (un-driven) run, release profile, 6M scheduler steps,
**19,881 VI swaps** captured, every swap's framebuffer hashed:

| Swap range | Content |
|---|---|
| 2 – 1057 | AKI logo splash, then full-colour photographic intro footage |
| 1058 – 3858 | Uniform white (2,801 swaps, one repeated hash) |
| 3859 – 4505 | AKI logo again — **the sequence restarted** |
| 4506 – 8196 | White |
| 8197 – 9187 | Content |
| 9188 – 12185 | White |
| 12186 – 12744 | Content |
| 12745 – 16612 | White |
| 16613 – 17062 | Content |

The period is **~3,990 swaps, repeating five times with no drift**, and the
classification is exact: within a white span every frame hashes identically
(SHA-1 `7ea766c0…`), and the transition into it at swaps 1020 → 1040 is a
grey-to-white **fade** (dominant pixel `949494` → `ffffff`), not a stall.

That is a game cycling its attract reel, which is precisely what an AKI title
does when nobody is playing. The correction this forces on the standing
picture: the earlier reading of "uniform white, zero non-uniform frames over
45 s, a blank-frame loop" saw one white *segment* and mistook it for the end
state. Watching past it shows the loop closing.

**Nonclaim:** this does not assert the white segment is *supposed* to be
white on hardware. It is a long fade-out hold whose duration and colour were
not compared against a reference emulator. It is characterized as attract's
idle segment because the reel restarts on the other side of it, not because
its content was validated.

## 3. Is input the answer? Yes, and the plumbing was already there

### 3.1 The guest polls the controller every field

`FN64_BOOT_PROBE=1` logs every `__osSiRawStartDma`. In a 1.5M-step run the
guest issued **3,584+ SI DMAs across 965 VI swaps — roughly four per field**
(a status query pair and a read-data pair). The PIF command blocks are
textbook libultra:

```
#6  dir=1  [ff, 01, 04, 01, ff ff ff ff,  ff, 01, 04, 01, ...]   controller read, 1-byte tx / 4-byte rx, all four channels
#8  dir=0  [ff, 01, 04, 01, 00 00 00 00,  ff, 01, 84, 01, ff ff ff ff, ...]   the response
```

Channel 0 answers `00 00 00 00` — a neutral pad with **no error bit**. Channels
1–3 come back with `0x84` = `0x04 | 0x80`, the PIF no-response bit, which is
the correct answer for an empty port. `fn64-runtime`'s `PifModel` defaults port
0 to `PortState::StandardControllerNoPak` (`crates/fn64-runtime/src/si.rs:120`)
and ports 1–3 to `Absent`, and the guest sees exactly that.

So the guest was never starved of a controller. It was reading one, every
frame, and being told correctly that nothing was pressed.

### 3.2 Pressing Start exits attract

`examples/wm2000-census/` gained `WM2000_INPUT_SCRIPT` (env-gated, off by
default, neutral outside every scripted phase). One Start (`0x1000`) held for
ten swaps at swap 1100, in the middle of the white segment:

- Framebuffers diverge from the neutral run at **swap ~1105**, four swaps
  after the press.
- By **swap 1250** the frame is the mode-select menu, with legible
  **"Exhibition Match" / "Championship" / "Royal Rumble"** — WM2000's real
  main menu.
- Ten subsequent A presses (`0x8000`) at 100-swap intervals produce **ten
  distinct frame hashes**, each advancing or changing the screen: an
  "Exhibition — Single Match" submenu at swap 1520, then further screens
  including what reads as a wrestler-select at swap 2500.

The menu chain is navigable. The game responds to input the way a working
emulator's does.

**Determinism.** Two independent runs of the identical script (Start at swap
1100, 900k steps) produce **all 5,272 framebuffer PNGs byte-identical** and a
**byte-identical opcode census** — spanning the press and the state change it
causes. A scripted press is reproducible evidence, not a one-off observation.

### 3.3 The interactive shell already does this too

`crates/fn64-shell/src/main.rs:595` calls
`fn64_abi::set_controller_state(0, buttons, sx, sy)` on **every scheduling
step** from the merged keyboard+gamepad state, and `:1167` is the same call in
the `FN64_INPUT_PROBE` path. The shell's "gamepads: none connected (hotplug
supported)" line is about *gamepads specifically*; the keyboard map
(`crates/fn64-shell/src/input_map.rs`) is a complete independent source and is
wired to the same seam. A player at the window can already press Start.

**Nonclaim:** the interactive shell was not run to a menu in this card. The
claim is that the shell's input call site is the same seam the headless press
proved, not that an end-to-end windowed session was observed reaching the menu.

---

## 4. Ranked blockers, with costs

Sequenced against "implement triangle composition" (`RT64-WM2000-REMAINING.md`
V3, ~925k `RDP_TRI_SHADE_TEX`, composition target must be invented).

| # | Blocker | Cost | Evidence |
|---|---|---|---|
| **B1** | **Nothing.** Input to reach the menu is already implemented at every layer — `PifModel`, the raw-SI PIF protocol, `set_controller_state`, and the shell's per-step feed. | **Zero.** Already done. | §3 |
| **B2** | **No harness could press a button.** The headless lane could not test any post-attract state, which is why the attract loop read as terminal for so long. | **Small — now landed.** `WM2000_INPUT_SCRIPT`, 98 lines, this card. | §3.2 |
| **B2b** | **Unknown button grammar past the mid-menu plateau.** Not an emulator defect (no trap, no refusal, no panic in any run) -- just nobody has read the menu state machine. | **Small.** Read the guest's pad handler, or try the remaining button space. | §5 |
| **B3** | **Menu/gameplay raster fidelity.** Menu text is legible but **horizontally duplicated** (the same string tiled ~2.5× across the width) and heavily interlace-striped. Something in the raster or the VI field composition is wrong, and it is wrong in *attract too* — it is not a menu-specific defect. | **Unknown. Not diagnosed here.** | §5 |
| **B4** | **Triangle composition (V3).** Still the expensive renderer item, and still required for anything to look right. | Large, unchanged. | `RT64-WM2000-REMAINING.md` §3 |
| **B5** | **`FN64_RENDER=wgpu` refuses general gfx tasks** (`production.rs:1579`), so the all-fn64 target stack cannot run this path at all yet. | Large, unchanged. | Brief; not re-measured here |

**The sequencing consequence.** B1 and B2 do *not* reorder B4 — because
reaching the menu did **not** change what the renderer is asked to do. A census
taken with input driven through eighteen menu screens counts **21 distinct
opcodes and 6,526,330 RDP commands** against the neutral run's **21 distinct
opcodes and 6,166,239 commands**: the *same 21 opcodes*, same rank order,
`RDP_TRI_SHADE_TEX` still the single triangle variant at 1,475,011, still zero
Z-variants, zero two-cycle, zero `G_SETZIMG`. Menus are made of the same
commands attract is.

So the honest answer to the card's motivating question is a split one:

- **The cheap thing was real and is now done.** Gameplay was never gated on a
  syscall, a save read, a timer, or a missing controller. It was gated on a
  press.
- **It does not buy a reordering.** Getting past the menu does not reduce the
  renderer bill, because the menu draws with the same command set. Triangle
  composition stays the next expensive item.

---

## 5. What could not be established

Stated plainly, because a precise blocker is the point of this card.

- **Whether a match actually starts.** Scripted presses advance through
  several menu screens and then **plateau** on one screen (a dark arena-lit
  view of three figures, plausibly wrestler-select). **Three independent
  navigation strategies were tried and all three plateau on the same screen,
  reproducing the same frame hashes**: A-only at 100-swap intervals
  (swaps 2500-3700, oscillating between two hashes), Start-only past that
  point (to swap 4600), and alternating d-pad-right + A (to swap 4722, frozen
  on one hash from 3600 to 4400). **No frame in this card was verified to be
  in-match.** The menu chain is demonstrated; a match is not.

  The plateau is **not** an emulation failure: `grep -iE
  "unsupported|refus|trap|panic|unimplemented"` over all three run logs
  returns nothing. No shim trapped, no opcode was refused, no thread died.
  Whatever the screen wants, the emulator is not the thing withholding it.
- **The correct button grammar for each screen.** Presses were chosen by
  guess, not from the game's own input handler, and the plateau is the direct
  consequence. A card that wants a match should read the menu state machine
  (or the guest's pad-handling function) rather than brute-force button
  combinations -- that is the specific, small, named next step.
- **Why menu text is horizontally duplicated.** Recorded in §4 as B3 and left
  undiagnosed. It could be a scissor, a stride, a VI x-scale, or the raster;
  none was ruled in or out.
- **Whether the white segment's length is correct.** No reference comparison
  was run.
- **Anything about the `FN64_RENDER=wgpu` lane.** All measurement here used the
  in-repo software `ReferenceBackend` (the census harness's backend), per the
  card's own instruction to avoid the wgpu gate. Numbers here do not transfer
  to that lane without re-measuring.
- **The `fn64-boot-harness` input-schedule schema was not used.**
  `parse_controller_input_schedule` /
  `CONTROLLER_INPUT_SCHEDULE_SCHEMA` exists and is complete, but has **zero
  consumers** anywhere in the tree, and it indexes by controller-*read* ordinal
  — a clock no counter exports. The harness's script indexes by VI swap
  instead. Unifying them is a real, small, unclaimed piece of work.
