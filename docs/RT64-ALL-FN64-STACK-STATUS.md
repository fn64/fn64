# The all-fn64 stack: measured status

**The goal** (owner, 2026-08-18): run WM2000 on **fn64's own recompiler and
fn64's own wgpu renderer** — not N64Recomp's C output, not
`fn64-render-reference`.

Everything below is **MEASURED on my own run**, dated, with the command that
reproduces it. Nothing here is inferred from a green test suite; two defects
this week were shipped *because* fn64's tests encoded them.

## Reproduce

```sh
FN64=<fn64 worktree> \
  ~/Code/recomps/wm2000/packages/wm2000-boot/rs/run-rs-lane.sh
```

The banner must read `recompiler : fn64-cpu-runtime (FN64_RECOMP=rs)` and
`renderer : wgpu`. **If it does not, the numbers are not about this stack** —
the shell defaults to the reference renderer and the C lane.

## Where it stands, 2026-08-20

| | swaps | outcome |
|---|---|---|
| 2026-08-18 | 0 | abort before the first heartbeat |
| 2026-08-20, morning | 1,887 | abort: `physical TMEM texel byte 0xa98 is invalid` |
| **2026-08-20, now** | **12,242** | **exit 0, zero panics, zero backend errors** |

Full run: 2,000,000 steps, 12,242 VI swaps, 25,200 gfx tasks, 22,452 audio
tasks. The 9 `REFUSED` lines in the log are the recompiler's **build-time**
symbol-repair guards declining unsafe function splits — not runtime failures.

## What was fixed to get here

`1f7094aa` — RDP TMEM loads copy **whole 64-bit words**. fn64 modelled a row
whose texels do not fill its last word as having an undefined tail and
CLEARED validity for those bytes, so an overlapping `LoadTile` punched holes
in an already-loaded TLUT. Four parts: padded source reads, one access per
row, full source masks, and row-local transfer-word binding. See
`RT64-WM2000-TEXEL-LOCALISATION.md` for the measurement, the hardware
citation, and the tempting fix that is wrong.

## It RENDERS

`docs/frames/wm2000-all-fn64-stack-rs-wgpu.png` is frame 443 from this
stack — fn64's own recompiler, fn64's own wgpu renderer, zero C++ objects.
It shows a wrestler mid-move with correct skin tones, brown hair, a
green-and-white patterned costume with legible detail, black trunks, an arm
tattoo, and correctly shaded 3D geometry. Real imagery, not noise.

**7,191 frames dumped, 3,770 distinct by sha256** across 7,193 VI swaps,
zero panics and zero backend errors — the scene animates rather than a
single buffer being re-presented.

Rendering holds deep into the run, not just at the start: frame 6,800 draws
a full arena crowd, ringside barriers, a wrestler carrying a detailed
championship-belt texture, and green HUD overlays. Geometry, textures and UI
all survive to the end of the budget.

**One pre-existing defect is visible in that late frame and is NOT new:** the
scene-specific colour cast recorded in `RT64-WM2000-TEXTURE-STATE.md`, where
in-match scenes read green/magenta while entrance scenes are correct. It was
confirmed pixel-identical against a run from before the `PLANE_TO_TEXEL` fix,
so it belongs to the combiner or environment-colour path rather than to
anything in this stack's own work.

**Frame-dump trap:** `run-rs-lane.sh` sets `WM2000_NO_TRACE=1`, and dumps are
gated `dumps_disabled = trace_disabled || NO_DUMP`. The flag is read with
`is_some()`, so `WM2000_NO_TRACE=0` does NOT re-enable it — the line has to
be REMOVED from a copy of the runner. `wgpu` is an accepted dump renderer
(`main.rs:1095`).

## Cross-lane agreement: the recompiler is renderer-independent

Running the SAME ROM and step budget through both renderers on the rs
recompiler gives identical guest progress:

| lane | VI swaps | gfx tasks | audio tasks | panics | backend errors |
|---|---|---|---|---|---|
| `FN64_RENDER=wgpu` | 7,193 | 15,577 | 13,198 | 0 | 0 |
| `FN64_RENDER=rt64` | 7,193 | 15,577 | 13,198 | 0 | 0 |

Identical in every counter. The rs recompiler drives both backends to the
same guest state, so CPU-side execution is deterministic and independent of
which renderer consumes its display lists. It also means the RT64 lane is
available as a same-scene oracle on this stack -- which is what settled the
"colour cast" question (see `RT64-WM2000-TEXTURE-STATE.md`): RT64 renders
the same green, so the green is the game's own arena lighting, not a defect.

To run the oracle lane, the boot binary needs the `rt64` cargo feature AND
`FN64_RT64_DIR` pointing at the RT64 source tree; without the latter the
build fails in `fn64-render-rt64`'s `build.rs`.

## What "playable" still needs — NOT yet measured

1. **A frame-for-frame comparison against the C lane.** The image above is
   read by eye and is plainly correct; it has not been diffed against
   `docs/frames/wm2000-after-byte-lane-fix-swap4090.png` at the same swap.
2. **Input.** Untested on this stack.
3. **Sync and speed.** Untested. The C lane's own perf work
   (`RT64-PERF-CEILING.md` and its correction) does not transfer.
4. **Beyond 2M steps.** Every number here is a 2M-step budget.

## Method notes that cost real time

- **State which cell a symptom came from.** Defaults give the C lane plus the
  reference renderer; symptoms from that combination say nothing about this
  stack.
- **The suite is not evidence for renderer semantics.** 19 tests asserted the
  TMEM defect above, one of them named
  `undefined_tail_bytes_are_staged_invalid_not_zero_filled_valid`. The
  authority is the pinned RT64 oracle's live loader, `src/hle/rt64_rdp.cpp` —
  not `rt64_hle_geometry.rs`'s `dumpTexture`, which is a debug heuristic.
- **A moved abort is progress and localises the next defect.** Widening the
  declared reads changed the failure from a palette byte to "source bytes are
  missing from the captured guest reads", which named the binding bug exactly.
