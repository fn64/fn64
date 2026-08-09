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

## Reproduction (run2, 2026-08-09)

After the `0x01`–`0x07` No-Op widening landed, a re-run **reproduced the frame
byte-identical** (sha256 `9794211091c53fb7dd73e52501f959843cf943566e13eac8cc637893f1731ec1`,
same task #654 / 259 tris), continued past the old `0x07` wall, and stopped a
few tasks later on the next unmapped scanner case: `raw RDP opcode 0x7f at
0x00800cf8 has no public command width` (`rsp_commit.rs:858`, non-unwinding
panic). `0x7f` exceeds the 6-bit RDP command space, so the leading suspicion is
scanner misalignment rather than stream content; under diagnosis.

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
