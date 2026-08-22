# The `0x1CC` abort is not an MMIO read

Five docs record WM2000's census window as bounded by "an unmodelled `0x1CC`
MMIO read" (`RT64-WM2000-CENSUS.md:324,365`, `RT64-WM2000-CYCLE-MODES.md:156,224`,
`RT64-COVERAGE-AUDIT.md:24`, `RT64-WM2000-REPLAY.md:598`). **That
characterization is wrong, and the register it names does not exist.**

## What `0x1CC` actually is

`0x00000000000001CC` is the *complete* faulting address, not an offset from a
device base. Evaluating `fn64_mmio_proxy.h`'s own predicates on it:
`fn64_is_rcp_mmio_word` = false, `fn64_is_rdram_direct_alias` = false (0x1CC is
not in `0x80000000..0xC0000000`), and `fn64_is_unsupported_rdram_alias` = **true**.
It is a KUSEG near-null address, reached through a corrupted base register. No
SI/PI/AI/VI/DPC/SP/MI/RI register is involved, so there is nothing to model —
this doc refuses to invent one by name.

## The real defect: a lost shared-epilogue fall-through

Measured under lldb at the trap (`RUST_BACKTRACE=full`, release build):

- Faulting instruction is guest PC `0x80121A3C`, `lw $v0, 0xDC($s1)`, inside
  `func_80121764` — confirmed from the arm64 site `ldr x8, [x19, #0x88]` where
  `x19` is `recomp_context*` and `+0x88` is `r17` (`s1`), index 17 × 8 bytes.
- `s1` reads `0x000000F0`. `0xF0 + 0xDC = 0x1CC`.
- `s1` is valid at entry (its first use, `MEM_HU(s1, 0xE8)`, does not trap) and
  is assigned exactly once, so it was clobbered mid-call.

The chain, all verifiable from the generated corpus and its `symbol_addrs.txt`:

1. `func_8011F67C` (bank2_text, `size:0x7FC`) allocates `addiu $sp, $sp, -0x88`
   and **emits no epilogue** — its last instruction, `0x8011FE74`, is a `jal`
   delay slot and the body falls off the end.
2. Its epilogue is the address-contiguous next symbol, `func_8011FE78`
   (`0x8011F67C + 0x7FC = 0x8011FE78`), which restores `$ra`/`$fp`/`$s7..$s0`
   from `0x84..0x60($sp)` and does `addiu $sp, $sp, 0x88` — byte-for-byte the
   inverse of `func_8011F67C`'s prologue. This is IDO's shared-epilogue idiom;
   N64Recomp split it into a separate C function and lost the fall-through edge.
3. So `func_8011F67C` returns with `$sp` 0x88 low. Its caller `func_801200DC`
   then reloads `$s1` from `0x2C($sp)` — now the wrong frame — and passes the
   garbage up to `func_80121764`.

**Corpus scope: 13 of 2,387 generated functions (0.54%)** allocate a frame with
no matching deallocation. Eleven have an address-contiguous successor with the
split-epilogue shape; the two others (`func_80120B28`, `func_8012A1D8`) are
truncated mid-flow, same defect class.

## The fix reuses this repo's own mender

`examples/wm2000-census/build.rs` carried a hand-rolled patch pass that
normalized only N64Recomp's `jr_addend_XXXX` declarations. It now calls
`build_support::prepare_recompiled_cxx_sources_with_proven_fallthrough_repair`
— the same preparer `fn64-shell/build.rs` already uses. The mend is
section-local and structurally gated: it fires only where the generated section
table proves an address-contiguous successor *and* that successor has the
split-epilogue instruction shape, so it cannot invent a call between unrelated
bodies. On this corpus it mends 1,996 fragments.

## What it unlocks — measured

| Quantity | Before | After | Ratio |
|---|---|---|---|
| VI swaps | 383 | 1,056 | 2.76x |
| Decode entries | 219 | 2,219 | 10.1x |
| gfx tasks | 324 | 2,377 | 7.3x |
| RDP-lane commands | 142,606 | 2,636,852 | 18.5x |
| `G_TEXRECT` | 2,520 | 40,960 | 16.3x |
| Entries with triangles | 152/218 (69.7%) | 1,718/2,218 (77.5%) | — |
| Distinct texrect combiner programs | 2 | 3 | — |

The new combiner program is `rgb=(Texel0-Zero)*Primitive+Zero` (9,600
occurrences). `Shade`, `Texel1` and `Combined` remain unread by any texrect.

**Conclusions the short window weakened, re-measured over 10x the entries:**

- **Triangles** — present, and the single most frequent opcode (534,335). Still
  exactly one variant, `RDP_TRI_SHADE_TEX` (`0x0e`); no Z-variant appears.
- **Two-cycle** — still **zero** across 40,960 texrects. One-cycle is 100%.
- **`G_SETZIMG`** — still **zero**. Depth remains unexercised.
- **Coverage / `ALPHA_CVG_SEL` / `CVG_X_ALPHA`** — not decided here; the census
  counts opcodes and does not decode `G_RDPSETOTHERMODE` payload bits.
- **Scissor** — `G_SETSCISSOR` occurs in 2,218/2,218 entries, but this
  instrument records opcode counts, not rect values, so whether any scissor is
  non-identity is **not measured**.

## The next wall

The run now aborts at a different, correctly-named trap:

```
fn64_c_recompiled_function_enter: entered native callable ... was not
registered in the generated section table
```

in `func_8011EA20`, reached from `func_8011C900_bank3_text`.

> **This section's diagnosis was wrong and is retained as a disproof.** It read
> the abort as an overlay-residency problem because `func_8011EA20` lives in
> `section_4_bank3_text` while the harness marks only sections 0/1. Measurement
> showed bank swapping already works — the guest's own `osEPiStartDma` marks
> sections 2/3/4, section 4 before this abort — and that the failing symbol is
> `static_4_8011FFA4`, one of 40 section-local bodies registered nowhere. The
> trap reads an execution-evidence registry, not the resident set. See
> [`RT64-WM2000-SECTION-LOCAL.md`](RT64-WM2000-SECTION-LOCAL.md).

## Verification

- Abort reproduced before the fix at 383 swaps, exact text above.
- **Mutation test, killed:** reverting the build to the non-repairing
  `prepare_recompiled_cxx_sources` (which asserts 0 mends, so the repair is the
  only delta) reproduces the `0x1CC` abort at 385 swaps / 213 entries.
- **Determinism:** two full post-fix runs produced byte-identical census and
  texrect TSVs (SHA-256 equal).
- Workspace: 8322 passed / 14 skipped, both debug and
  `RUSTFLAGS="-C debug-assertions=off"` — unchanged, as expected for a change
  confined to an out-of-workspace example's build script.

## Nonclaims

- **Not a device model.** Nothing here models any MMIO register, and the
  `0x1CC` address is asserted to be *not* a register rather than modelled.
- **Not gameplay.** 1,056 VI fields is ~17.6s of NTSC virtual time. Whether the
  window now reaches match play is **unverified** — the framebuffers were not
  inspected and no gameplay marker was checked.
- **Not a fix to the generated corpus.** The upstream `RecompiledFuncs/*.c`
  still contains the 13 unbalanced functions; the mend is applied at build time
  in fn64 only.
- **The two truncated outliers are unverified as repaired.** They match the
  defect class structurally; no run was shown to depend on them.
- **One title.** Nothing here is evidence about other AKI titles or other ROMs.
