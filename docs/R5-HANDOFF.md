# R5/R8 handoff — swizzle regression blocking timing verification

Written 2026-07-17 after a session that chased this live and burned the
user's patience doing it out loud. This doc exists so the next session
doesn't repeat the same dead ends. Read it before touching code.

## Start here — the one-line prompt

> The fn64-shell window renders horizontal color-banded garbage instead of
> the game (see `docs/R5-HANDOFF.md` for a screenshot description and what's
> already ruled out). Find the actual byte-level bug, fix it, and build the
> sweep AGENTS.md asks for so this stops recurring — this is the fourth
> instance of the same bug class (ROADMAP R8). Do not report anything fixed
> until you've screenshotted the live window and looked at it yourself;
> `NON-BLANK` in the heartbeat log proved nothing (see "the false-green"
> below).

## What's broken

Running `crates/fn64-shell` (the windowed harness) against OoT shows a
corrupt frame: horizontal bands of solid colors (green/red/orange/blue/teal),
arranged in ~3 vertical strips each with a different palette. Not noise —
structured, repeating bands, which points at a stride or byte-lane bug
reading real data wrong, not uninitialized memory.

Confirmed present on `main` at commit prior to `0611d35` (this session's
pump-timing commit) — i.e. **this is not new**, it predates all of today's
work. Screenshot evidence lives only in the conversation transcript, not
committed anywhere; reproduce fresh rather than hunting for the PNG.

## What's already ruled out (do not re-investigate these)

1. **Not caused by the pump-timing fix in `0611d35`.** Bisected properly:
   stashed that commit's diff, rebuilt, ran the shell — same corruption.
   Confirmed via two full window captures, before and after.

2. **Not the `^2` byte-lane swizzle rule itself.** `framebuffer.rs`'s
   `rgba5551_to_rgba8888` uses `let at = (i * 2) ^ 2` — this is the fix R7
   already landed for the *previous* instance of this bug class, and it's
   still there, unmodified, correct for lane-order-within-a-word.

3. **Not a framebuffer geometry (width/height) mismatch.** Both the shell
   (`framebuffer.rs`: `FB_WIDTH=320, FB_HEIGHT=240`) and the headless
   harness (`examples/oot-boot/src/main.rs`: same constants, same `^2`) use
   identical geometry and identical decode logic. If it were a width bug
   both would show it.

4. **The headless harness's PNG dump is clean at swap 40** (reference
   backend, `OOT_MAX_SWAPS=40`, dumped via `capture_framebuffer` in
   `examples/oot-boot/src/main.rs`). White background, blue logo shape,
   correct colors. So the SAME decode function, on the SAME rdram-address
   source, at a similar point in boot, produces a correct frame in one
   harness and garbage in the other. **This is the load-bearing fact.** The
   bug is not in the decode math — it's in what bytes each harness is
   handing to that decode, or when.

## What was NOT finished — pick up here

The last thread, cut off mid-investigation: the shell holds its own
`rdram: Vec<u8>` (`crates/fn64-shell/src/main.rs:109`), separately allocated
via `fn64_boot_harness::new_rdram()` and passed to the executor as a raw
pointer (`rdram_ptr`, line 203). The headless harness does the same pattern.
Was about to check:

- Does the shell's `present()` (around line 378, reads via
  `fn64_abi::current_vi_framebuffer()`) read `self.rdram` at a point where
  the executor could be mid-write to the framebuffer region — i.e. a
  **torn read**, not a decode bug? The headless harness calls
  `capture_framebuffer` right after observing a swap in its drive loop; the
  shell's `present()` is called from `about_to_wait`'s wall-clock-paced path
  (`crates/fn64-shell/src/main.rs:550` calls it, block starting ~526) — a
  DIFFERENT trigger than "immediately after a swap was observed." If the
  shell can present between when the guest starts writing a new frame and
  when it finishes, that's your bug, and it would explain: (a) real,
  structured data (torn writes look like bands, not noise), (b) why headless
  never sees it (it captures synchronously right after the swap, no
  wall-clock gap), (c) why it's been happening across "three previous fixes
  at the swizzle site" that were really about the decode math, not the
  read-timing — meaning this may be a DIFFERENT bug wearing the same
  visual costume as R8's prior three, not literally R8 again. Verify which
  before generalizing.

- Concretely: read `fn64-abi/src/vi.rs`'s `current_vi_framebuffer` and
  `osViSwapBuffer_recomp` (around lines 110-150, 285-296) to understand
  exactly when `ViState::current_framebuffer` updates relative to when the
  guest is done writing pixel data into that buffer. Compare against when
  `present()` is actually invoked in the winit event loop. If there's a
  window where "swap pointer updated" precedes "pixel writes finished,"
  that's the mechanism.

## The false-green — fix this regardless of root cause

`fn64-shell`'s heartbeat logs `(NON-BLANK)` based on
`framebuffer::is_uniform(region)` — this only detects "all pixels
identical," which the corrupt frame trivially passes (it's not uniform, it's
wrong). This let the bug ship invisibly through every heartbeat log this
session. Before closing this out, replace or supplement that check with
something that can actually fail on wrong-but-non-uniform content — e.g. a
per-channel histogram sanity check, or (better) a mechanical comparison
against the headless PNG dump for the same swap index, since that path is
proven correct. AGENTS.md's "loud traps, no silent shrugs" applies here: a
heartbeat that says NON-BLANK for garbage is a silent shrug wearing a log
line.

## Then, build the sweep (AGENTS.md: mechanism over patch)

This is documented as the fourth instance of an rdram-byte-order bug class
(ROADMAP R8: DMA, framebuffer capture, the R7 presenter, now this). Once the
actual bug is found and fixed, do not stop at the instance. R8 already asks
for: enumerate every host<->rdram byte-order boundary in the codebase and
either (a) type them so a lane/stride mismatch fails to compile, or (b) add
a shared, tested conversion helper that all four sites (and any others found
during the sweep) are migrated to use, so a fifth instance isn't possible by
construction. Search for every place that does raw `rdram[...]` indexing
combined with a manual `^ 2` or similar swizzle, in `fn64-abi`, `fn64-shell`,
`fn64-runtime`, and `examples/oot-boot` — that's the sweep's scope.

## Separately, and only after the above is real: R5 audio/video timing

The pump-timing fix in `0611d35` (deriving VI retrace from the wall-clock
pump instead of from work done) is believed correct in principle but is
UNVERIFIED — every measurement gathered this session was taken against a
window that was already rendering garbage from the bug above, so the
numbers (retrace_hz readings, ring_frames behavior) should be distrusted
until re-measured on a corrected frame source. Re-run the same measurement
protocol (`docs/ROADMAP.md`'s R5 section has the heartbeat log format) once
the swizzle bug is fixed, and only then decide whether the retrace-rate fix
actually holds up. There is also a known-but-unverified second timing bug
noted in `0611d35`'s commit message: the clock advance was moved to the top
of `pump_one_frame` specifically because placing it after the loop was
skipped by any pump that produced a frame (an early `return true`) — that
fix landed but was never re-measured before this session got derailed by
the framebuffer corruption.

## Ground rules for this investigation (learned expensively this session)

- **Screenshot the live window before claiming anything about render
  correctness.** Log lines like `NON-BLANK`, "no render error", or a clean
  headless PNG from a DIFFERENT code path are not evidence about what's on
  screen. This session made this mistake at least three times in a row.
- **Bisect before theorizing.** One `git stash` + rebuild settled "is this
  my change's fault" in under two minutes, after ~20 minutes of plausible
  wrong theories (byte-lane rule, geometry mismatch, VI mode).
- State what you've ruled out AND how you ruled it out, every time, so the
  next reader doesn't redo the work.
