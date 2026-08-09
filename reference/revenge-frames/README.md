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
command width"). Opcodes `0x01`–`0x07` were absent from its Table 11 map;
Revenge's microcode emits one and WM2000's never does. That has since been
fixed, along with four more of the same kind — see the run2 table below.

## Reproduction (run2, 2026-08-09)

Every re-run since has **reproduced the frame byte-identical** (sha256
`9794211091c53fb7dd73e52501f959843cf943566e13eac8cc637893f1731ec1`, same task
#654 / 259 tris), with task #656 the last to decode. Each re-run then advanced
one wall further:

| # | stop | verdict |
|---|---|---|
| 1 | scanner: `0x07` "no public command width" | ours — `0x01`–`0x07` are all *No Operation*, one word each |
| 2 | scanner: `0x7f` same message | ours — the command field is bits 61:56; `0x7f` masks to `0x3f` (Set Color Image). Only the `0xc0` spelling of every state command was accepted |
| 3 | decoder: `G_NOOP reserved first-word payload must be zero` (`w0=0x000a0000`) | ours — the No Operation table marks every bit don't-care but `command[5:0]`; the rule was written for the GBI `gDPNoOp` and applied to the raw lane |
| 4 | backend: `G_TEXRECT in Fill cycle is invalid` | ours — "In FILL mode this behaves identically to Fill Rectangle, the texturing properties are ignored" |
| 5 | backend: `G_SETCIMG format=0 size=0 is unsupported` | ours — `G_SETCIMG` is a latch; validation now defers to the first draw through the target |

**Five walls, five ours.** Not one was a defect in Revenge's stream: each was a
faithfulness rule written against what WM2000's F3DEX2 macros emit rather than
against what the RDP accepts. `0x7f` looked like scanner misalignment when
first seen — it was not; the field extraction was correct and the classifier
was reading eight bits of a six-bit field.

**Open question for the next run.** Wall 5 was fixed by deferring, which is
right whether or not a draw follows the latch. But whether Revenge's stream
*does* draw through that `format=0 size=0` target is **not yet established** —
the console log does not show it. If the next run fails at a draw naming
`format=0 size=0`, that is a genuinely new case (a 4-bit colour image is not a
real framebuffer configuration) and wants reporting, not invented semantics.
`FN64_XBUS_STREAM_DUMP_DIR` / `_SKIP` dump the captured command stream if the
question needs settling directly.

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
