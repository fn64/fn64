# WCW/nWo Revenge reference frames

Normalized ROM `d8c097f8880032fc63a73a78ad2fcabac8f4b5938b69e615c0689c57c5fdc3c3`
(WCW/nWo Revenge - Starrcade Edition (USA) (v1.01)). Filed under the ROM
digest, not the title string: three distinct "Revenge" images were in play this
session and no two were the same.

A dumped frame is evidence and belongs in the repository, for the same reason
`reference/wm2000-routes/*.schedule` are committed — the run that first reached
WM2000's match-setup screen was written to `/tmp`, macOS reaped it, and it
survived only as prose.

## `first-boot-arena-1800collect.png`

**The first frame any non-WM2000 title has rendered through fn64.** Graphics
task #654, 259 triangles, reported NON-CLEAR by the reference rasterizer. Shows
the ringside arena with the 1-800-COLLECT sponsor board and the nWo logo.

Produced with **no controller schedule at all** — `FN64_CONTROLLER_SCHEDULE` is
an `Option` in `main.rs`, not a requirement, so every port reads neutral. Boot
needs no route; this frame is reached by the game's own attract sequence.

    binary    examples/revenge-block-boot (headless)
    renderer  reference (software rasterizer — NOT rt64)
    steps     ~200,000 of a 1,500,000 request
    at 200k   gfx_submits=507  audio_submits=1035  render_error=None
    overlay   entered generation [0x80090000,0x800c5ad0) at step 15,987

The run stopped shortly after this frame, and **not on anything
title-specific**: the raw-RDP scanner rejected opcode `0x07` ("has no public
command width"). That was the first of five apparent walls which turned out to
share a single cause — the guest legally declares a command range far longer
than its display list, and we were decoding the leftover bytes. See below.

## Reproduction (run2-run5, 2026-08-09) — one root cause, five masks

Every re-run **reproduced the frame byte-identical** (sha256
`9794211091c53fb7dd73e52501f959843cf943566e13eac8cc637893f1731ec1`, same task
#654 / 259 tris), with task #656 the last to decode. Each fix advanced the run
a few commands further, to a new-looking wall:

| # | stop | spec verdict |
|---|---|---|
| 1 | scanner: `0x07` "no public command width" | `0x01`-`0x07` are all *No Operation*, one word each |
| 2 | scanner: `0x7f` same message | the command field is bits 61:56; `0x7f` masks to `0x3f`. Only the `0xc0` spelling of each state command was accepted |
| 3 | decoder: `G_NOOP reserved first-word payload must be zero` (`w0=0x000a0000`) | No Operation marks every bit don't-care but `command[5:0]` |
| 4 | backend: `G_TEXRECT in Fill cycle is invalid` | "In FILL mode this behaves identically to Fill Rectangle, the texturing properties are ignored" |
| 5 | backend: `G_SETCIMG format=0 size=0 is unsupported` | `G_SETCIMG` is a latch, like `G_SETTIMG`; format matters at the draw |

**These are five real spec corrections and each stands on its own.** But they
were not five bugs. Dumping the command stream (`RSP_TRACE_DPC_WORDS`) showed
**one** root cause wearing five masks.

### The root cause: residue past `SyncFull`

Task #657's display list ends at byte 3,280 of a **65,376-byte** submitted
range. `SyncFull` sits at offset `0xcc8` — 5.0% in. The remaining 62,096 bytes
are the *previous* frame's list, still resident in a reused command buffer.
Every one of walls 1, 2, 3 and 5 was our decoders reading that residue:

    0x0cc8  SyncFull            <- the real end of the list
    0x0ce8  "NoOp 0x07"         <- wall 1     (float 2.04688)
    0x0cf0  "NoOp" payload      <- wall 3     (0x000a0000)
    0x0cf8  "SetColorImage"     <- walls 2, 5 (0x7ff80000 is float NaN)
    0x0d28  clean commands resume — last frame's list

The tell was there at wall 2 and I missed it: that "Set Color Image" decodes
as `width=2049` at `addr=0xf80000`, for a game whose three real framebuffers
are all `width=480` at `0x147000`/`0x17f400`/`0x1b7800`. A **valid command ID
with an implausible payload** should have triggered a payload check
immediately. The transferable rule: *when a rejection moves after a fix,
validate the plausibility of the payload, not just the legality of the
opcode* — a wall that relocates by a few commands is evidence you are decoding
data, not commands.

### Not our bookkeeping — and not a reason to stop at `SyncFull`

The DPC range is exactly what the guest programmed. Every range starts at
`0x3eefd0`; if `dp_current` were stale, #657 would start at #656's *end*. It
does not, and #655 is a 51-command range immediately after an 8,165-command
one, so there is no high-water mark. `[dp_current, dp_end)` is also the
documented model — N64brew *RDP Interface* describes incremental transfers
where "the DPC_END register can be updated ... and the DMA will continue
running until the new end pointer is reached".

Hardware does **not** stop at `SyncFull` either: it fetches until
`CURRENT == END`, so the residue *is* consumed. Revenge ships and works on
hardware because consuming it is harmless — the stale `G_SETCIMG` points at
15.5 MiB, and per N64brew *RDRAM Interface* ("Accesses outside of mapped RDRAM
chips") no Rambus device answers: reads return zero and **"writes will be
ignored"**, explicitly not mirrored into low memory. The page's stray-value
caveat is scoped "at least during reads", so the write rule is unqualified.

So the fix is to execute the residue with that tolerance rather than to stop
early: a colour target outside installed RDRAM latches, draws through it
execute, and their writes are discarded. Stopping at `SyncFull` was rejected
as unfaithful — it would also have broken the legal two-list incremental case
and silently discarded whatever the residue does.

**Strictness is unchanged for addresses that exist.** An unsupported format at
a backed address is still a loud error; only the unbacked case is tolerated,
because only there is the write unobservable.

The exact invocation — recorded because the original session documented the
outcome but not the command, which cost a full re-diagnosis. Run from
`examples/revenge-block-boot` (standalone workspace; `-p` from the repo root
fails). **`FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1` is load-bearing**: without it
the binary stops at first overlay entry (step 15,987) and exits 0 having
rendered nothing.

    cd examples/revenge-block-boot
    export ROM=<the d8c097f8… ROM>
    export FN64_BOOT_CONTEXT=<capture binding d8c097f8…>
    export FN64_EXECUTABLE_IMAGE_GROUPS=FN64_EXECUTABLE_IMAGE_GENERAL_EXCEPTION
    export FN64_EXECUTABLE_IMAGE_GENERAL_EXCEPTION=<run-1>/image.json:<run-2>/image.json:<run-3>/image.json
    export FN64_ABSENT_N64DD=1
    export FN64_RENDER=reference
    export FN64_BLOCK_CONTINUE_AFTER_OVERLAY=1
    export FN64_BLOCK_MAX_STEPS=1500000
    export FN64_RENDER_DUMP_DIR=<somewhere>   # dump prefix is hardcoded
    export FN64_RENDER_DUMP_LIMIT=4000        #   "fn64-wm2000-block" for every
    cargo run --release --bin revenge-block-boot  # title; rename before filing
