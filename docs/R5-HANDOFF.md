# R5/R8 handoff — swizzle regression blocking timing verification

Status 2026-07-17: root cause found and live-window correction visually
verified. The torn-read lead below was disproved because guest pumping and
redraw run serially on the winit event thread. The actual mismatch was
`ReferenceBackend::write_rgba5551_framebuffer`: it wrote flat big-endian
halfwords, while the OoT shell/headless decoders read N64Recomp's native-word
RDRAM storage. That producer/consumer mismatch created the bands.

The correction is not another site patch: `RdramView`, `RdramViewMut`, and
the ABI-only unsafe `RdramPtr` now own logical-address to storage translation
in `fn64-runtime`; framebuffer write/read, DMA, controller structs, audio PCM,
renderer reads, and diagnostics use them. `scripts/lint-rdram-layout.py`
rejects production manual lane XORs and raw RDRAM writes outside that owner.
Pre/post live screenshots are local evidence at `/tmp/fn64-before.png` and
`/tmp/fn64-after.png` (never committed game-derived output).

Working-tree validation: 10/10 consecutive integrated nextest runs clean;
630/630 workspace tests; strict clippy clean; layout/doc lints clean; and the
C/rs lane observation matched all 58 captured non-uniform framebuffers
through swap 60. That observation is not an authority claim: the later
whole-corpus audit in `PARITY-METHOD.md` found callable empty C bodies and made
the default lane gate fail closed. The live post-fix image was inspected, not inferred from the
heartbeat. R8 stays open in ROADMAP only because `[x]` means merged+verified.

Post-fix timing was subsequently closed at the mechanism level. The ~31 ms
pump was the priority-0 idle thread being resumed to an arbitrary step cap;
typed priority inspection now treats its second consecutive turn as guest
quiescence. A separate trace found that returning from a pump at the first VI
swap left same-retrace AudioMgr work runnable, so the next pump advanced VI
too early and OoT coalesced one queued audio notification in three.
`RetraceDrain` now makes swap observation-only and pins that invariant in a
unit test. The boot harness also requires a typed `TvType` and seeds the
IPL-owned `osTvType` global, preventing a PAL audio configuration under the
shell's NTSC VI clock.

Audio hardware feedback and host buffering are no longer conflated:
`osAiGetLength` sees only the current emulated AI DMA, while cpal keeps an
independent two-DMA jitter prebuffer and resamples 32,006 Hz guest output to
the device's 48 kHz stream. Live rs+RT64 evidence through swap 900 held 60.0
windowed retraces/sec, stable 2.6–3.1k host frames, no overflow, and zero
callback underrun samples. `/tmp/fn64-timing-audio-fixed.png` is the inspected
live-window capture. The final tree passed 10/10 consecutive whole-workspace
nextest runs (635/635 each), strict clippy, both repository lints, and matching
non-authoritative C/rs framebuffer observation through swap 60. ROADMAP R5 remains open only for
foreground/backgrounded listening confirmation.

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
   harness (`recomps/wm2000/packages/oot-boot/src/main.rs`: same constants, same `^2`) use
   identical geometry and identical decode logic. If it were a width bug
   both would show it.

4. **The headless harness's PNG dump is clean at swap 40** (reference
   backend, `OOT_MAX_SWAPS=40`, dumped via `capture_framebuffer` in
   `recomps/wm2000/packages/oot-boot/src/main.rs`). White background, blue logo shape,
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
`fn64-runtime`, and `recomps/wm2000/packages/oot-boot` — that's the sweep's scope.

## Separately, and only after the above is real: R5 audio/video timing

This is now re-measured on the corrected frame source. One wall deadline
advances exactly one VI interval; the pump drains that retrace to typed guest
quiescence even if a swap occurs; then presentation observes the completed
state. The shell heartbeat is the acceptance probe: current-window Hz,
interval/pump/present median+p95, scheduler steps, AI submissions, host depth,
and callback underrun samples. ROADMAP R5 contains the current numbers and the
remaining by-ear validation bar.

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
