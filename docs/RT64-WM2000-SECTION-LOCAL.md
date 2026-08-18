# The section-table abort was not an overlay-swap problem

`RT64-WM2000-0X1CC-DIAGNOSIS.md` closed with the run aborting in
`func_8011EA20` and named the next card "overlay bank swapping", reasoning that
`func_8011EA20` lives in `section_4_bank3_text` while the harness marks only
sections 0/1 loaded. **That premise is wrong on both halves, and this doc
records the disproof before the measurement it unlocked.**

## Overlay swapping already worked, and the guest already drives it

WM2000 loads its banks through `func_80000744` (`games/NWXE/overlays.json`'s
`load_fn`), which chunks each bank into `osEPiStartDma` transfers of at most
0x200 bytes. `fn64_abi::note_dma_overlay_load` is already wired into the PI
completion path (`crates/fn64-abi/src/pi/timing.rs:658`) and already keys off
the guest's own DMA. **Measured** by probing every ROM-to-RDRAM DMA in a full
run: of 52,000 observed, exactly three match a section start, and all three
mark the correct bank —

| DMA `rom_addr` | dest vram | section marked |
|---|---|---|
| `0x0004C160` | `0x800E1B90` | 2 (`bank1_text`) |
| `0x00073390` | `0x8011C900` | 3 (`bank2_text`) |
| `0x000809D0` | `0x8011C900` | 4 (`bank3_text`) |

Section 4 was marked resident *before* the abort, not after it. Sections 3 and
4 sharing link base `0x8011C900` is handled correctly and needs no harness
convention: residency follows the guest's DMA.

## The real defect: section-local bodies are registered nowhere

The abort is `fn64_c_recompiled_function_enter`, which reads
`native_destination_by_pointer` — an execution-evidence registry, unrelated to
section residency. The faulting symbol is `static_4_8011FFA4`.

N64Recomp emits `static_<section>_<vram>` bodies for game symbols that had
file-local linkage. They are never indirect-call targets, so they appear in no
`recomp_overlays.inl` `FuncEntry` table — **measured: 0 of 40 appear in it** —
yet `build_support.rs`'s instrumenter injects the entry observer into every
`RECOMP_FUNC` body, statics included. The first one entered therefore aborts.
The corpus has **40** such bodies, 19 in section 4 and 21 in section 5; all 40
are declared in `funcs.h` (external linkage, so registerable), and all 40 parse
to a link VRAM inside their own named section.

The fix discovers them from the generated sources at build time, reconciles
each against the generated `section_table[]`, and emits a registration TU. They
are registered for execution evidence **only** — never published to
`get_function`, because a file-local symbol is not a dispatch target and
publishing one would invent an edge the ROM does not have.

## What it unlocks — measured

| Quantity | Prior | Now | Ratio |
|---|---|---|---|
| VI swaps | 1,055 | 4,454 | 4.2x |
| Decode entries | 2,219 | 5,792 | 2.6x |
| gfx tasks | 2,377 | 6,146 | 2.6x |
| RDP-lane commands | 2,636,852 | 5,406,193 | 2.05x |
| `G_TEXRECT` | 40,960 | 230,240 | 5.6x |
| `G_FILLRECT` | 59,580 | 260,160 | 4.4x |
| `G_SETSCISSOR` | 5,019 | 21,480 | 4.3x |

Prior figures re-measured in a clean `ff20e5ce` worktree, not quoted.

**No opcode appears that did not appear before, and none disappears.** The five
questions this card was asked to answer, over a 2.6x larger window:

- **Z-variant triangles: still zero.** `RDP_TRI_SHADE_TEX` (`0x0e`, 925,114) is
  again the only triangle variant of any kind.
- **Two-cycle texrects: still zero**, now across 230,240 texrects. One-cycle is
  100%.
- **`G_SETZIMG`: still zero.** Depth remains unexercised.
- **Combiner inputs**: `Shade`, `Texel1` and `Combined` remain unread. The
  inputs observed are `Texel0`, `Primitive`, `Environment`, `Zero`.
- **Distinct combiner programs: 3, the same three**, in the same rank order
  (214,920 / 11,120 / 4,200 occurrences).

## Where the run now stops

Not in the CPU or the section table. It reaches gfx task #6146 and aborts in
the *renderer*: `crates/fn64-render-reference/src/lib.rs:143`,
"texture-LUT sampling of a 0-coded 2b tile at TMEM word 0 is unsupported" — a
CI-oracle gap reached only now that `G_LOADTLUT` traffic (103,297 commands)
drives palettized sampling. That is the next wall.

## Does it reach gameplay? No — attract mode, with evidence

Framebuffer inspection over the new window shows the AKI logo splash
(`/tmp/fn64-fb-4200.png`, the yellow/blue/red "K" mark with legible text) and
then full-colour photographic intro footage (`fn64-fb-4300`, `fn64-fb-4350`),
field-interlaced. That is the title/attract sequence. **No gameplay marker was
observed, and this doc does not claim gameplay** — the deeper window buys the
intro, not match play. 4,454 VI fields is ~74s of NTSC virtual time.

## Verification

- Determinism: **four** full runs produced byte-identical census and texrect
  TSVs (SHA-256 equal), spanning a reformat and a harness edit.
- Workspace: 8332 -> **8339 passed / 13 skipped** (+7 new tests), identical
  under `RUSTFLAGS="-C debug-assertions=off"`. Baseline measured in this
  worktree, not quoted. 10 consecutive focused runs identical.
- Dead code unchanged: 1218 all-targets, 1198 `fn64-render-wgpu` lib-only.
- `scripts/lint-docs.py`: 1 error + 3 warnings before and after, the same
  pre-existing `RT64-WM2000-VALIDATION.md:360` error, untouched.
- **Mutation: 4 run, 3 killed, 1 proven equivalent.**
  - *Wrong section index* (emit `0` for every static) — **survived the first
    design**, which checked only the parsed name at build time rather than the
    emitted value. That survivor was a real defect: it exposed that nothing
    validated what was actually written. Fixed by checking containment at
    registration against the populated registry; now killed with a named abort.
  - *Skip all registration* — killed; reproduces the original abort exactly.
  - *`link_vram + 4`* (in-section shift) — survives the harness, which never
    reads the evidence record, so it is killed by a unit test pinning the exact
    recorded destination instead.
  - *Relax the 8-hex-digit VRAM guard* — **equivalent on this corpus**: every
    one of the 40 names has exactly an 8-digit field, so the guard is dead
    here. It is retained for corpora where it would not be.

## Nonclaims

- **Not gameplay.** Attract mode is established; match play is not, and the
  framebuffers were checked rather than assumed.
- **Not an overlay-swap change.** No residency logic was modified; the prior
  doc's overlay premise is disproved, not implemented.
- **No new dispatch edge.** Section-local bodies are execution evidence only
  and stay unresolvable through `get_function`.
- **Not a renderer claim.** The new wall is a `ReferenceBackend` texture-LUT
  gap; nothing here implements or excuses it.
- **The census cannot decode rect values or `G_RDPSETOTHERMODE` payload bits.**
  Whether any scissor is non-identity, and the coverage/`ALPHA_CVG_SEL`
  question, remain **not measurable by this instrument** — unchanged, and not
  inferred from the larger window.
- **One title.** Nothing here is evidence about other AKI titles or ROMs.
