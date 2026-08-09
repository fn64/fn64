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

The run stops shortly after this frame, and **not on anything title-specific**:
the raw-RDP scanner rejects opcode `0x07` ("has no public command width",
`crates/fn64-render/src/rdp_completion.rs:96-101`). Opcodes `0x01`–`0x07` are
absent from its Table 11 map. Revenge's microcode emits one; WM2000's never
does. Widening that map is the next step for a longer route.
