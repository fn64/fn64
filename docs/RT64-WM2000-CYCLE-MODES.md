# WM2000's texrect cycle modes: the measurement that sizes the remaining work

Every `G_TEXRECT` WWF WrestleMania 2000 (NWXE) issues over its
boot-through-attract window, with the RDP cycle mode latched at the moment it
was issued and the color-combiner program in effect. Produced by running the
real recompiled game; nothing here is estimated.

This answers a question [`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md) could
not: that census records an unordered bag of opcodes per decode entry, so it
proves texrect and TMEM-load co-occurrence but carries no operand data. The
cycle mode is operand data, and it decides whether the remaining
texture-rectangle work is a blit or a per-fragment combiner.

Companion docs: [`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md),
[`RT64-WM2000-GAP.md`](RT64-WM2000-GAP.md),
[`RT64-PORT-CARD-BRIEF.md`](RT64-PORT-CARD-BRIEF.md) ("Measure, never assert").

---

## 1. Headline: 2,520 of 2,520 texrects are one-cycle. Zero are Copy.

| Cycle mode | Texrects | Share |
|---|---|---|
| Fill | 0 | 0% |
| Copy | 0 | 0% |
| **1-Cycle** | **2,520** | **100%** |
| 2-Cycle | 0 | 0% |

Every texture rectangle in the window carries the same other-mode high word,
`0x0000acef`, whose `G_MDSFT_CYCLETYPE` field (bits 21:20) is `0` — one-cycle.
There is no variation to report: one word, one mode, 2,520 times.

The reverted Copy/Fill gate at
`crates/fn64-render-wgpu/src/raw_dpc/mod.rs:1590-1592` was reverted for the
right reason and would have been wrong in the other direction too. Its comment
records that "no measurement in this repo establishes which mode WM2000's
title-screen texrects use." This is that measurement, and the answer is the one
the current executor refuses.

**The live executor admits exactly the mode WM2000 never uses.**
`execute_texture_rectangle` (`crates/fn64-render-wgpu/src/targets/texrect.rs:511-515`)
refuses any cycle type but Copy, by name, with `UnsupportedCycleType`. Against
this window that refusal fires on 2,520 of 2,520 rectangles. The `fill + TMEM
load + texrect` composition landed in `35113da4` is real and its refusal is
honest, but it cannot draw a WM2000 rectangle as it stands.

---

## 2. The combiner is doing real work, and there are exactly two programs

Copy and Fill cycle bypass the color combiner entirely; one- and two-cycle run
it per fragment. Since every rectangle here is one-cycle, the combiner runs on
every one of them, and what it computes is the cost.

| Combiner class | Texrects |
|---|---|
| Not evaluated (Copy/Fill) | 0 |
| Texel passthrough | 0 |
| **Real work** | **2,520** |

Zero rectangles are a passthrough blit. But "real work" turns out to be far
narrower than that class name suggests: across all 2,520 rectangles there are
only **two distinct combiner programs**.

| Count | Entries | RGB | Alpha |
|---|---|---|---|
| 2,100 | 2–85 | `(Environment − Texel0) * Primitive + Texel0` | `(Texel0 − Zero) * Primitive + Zero` |
| 420 | 1–7 | `(Zero − Zero) * Zero + Primitive` | `(Zero − Zero) * Zero + Primitive` |

The first is a texel-to-environment-color lerp weighted by primitive color —
the standard tint/fade a title screen uses. The second is a flat primitive-color
rectangle that ignores the texture entirely.

**The inputs those two programs need are a strict subset of what the crate
already models.** Between them they read `Texel0`, `Primitive`, `Environment`,
`Zero`, and `One`. They read **no** `Shade`, no `Texel1`, no `Combined`, no LOD
fraction, no noise, and no chroma key. `CombinerInputs`
(`crates/fn64-render-wgpu/src/combiner.rs:519-532`) already carries all five,
and `run_one_cycle` (`:698`) already evaluates them.

A "texel passthrough" classification would have been the cheapest possible
answer and it is not what the ROM does. But two fixed programs over five
already-modelled inputs is the second-cheapest.

---

## 3. Distribution across the window: uniform, with one structural split

The mode does not vary. The combiner program varies in exactly one regular way.

- **Cycle mode:** identical for every rectangle in every entry. One-cycle,
  other-mode high `0x0000acef`, from the first rectangle to the last.
- **Texrects appear in decode entries 1–85 only.** The window's remaining 134
  entries issue none. That is consistent with the census's per-frame finding
  that 133 frames carry triangles but no `G_TEXRECT` — the texrect-bearing
  frames are the early logo/title screens, exactly the frames the owner's bar
  names.
- **The flat-primitive program runs in entries 1–7, 60 rectangles per entry
  (420 total).** The environment-lerp program runs in entries 2–85, 25
  rectangles per entry (2,100 total). Entries 2–7 carry both. The counts are
  exactly regular: 7 × 60 + 84 × 25 = 2,520.

So the earliest frames are 60 flat rectangles plus 25 tinted ones; from entry 8
on it is 25 tinted rectangles a frame and nothing else.

Neither program's appearance changes the cycle mode. There is no frame in this
window where a Copy-cycle path would have drawn anything.

---

## 4. How this was produced

**The tooling**, committed and re-runnable as a burndown:

- `crates/fn64-render-reference/src/gbi/census.rs`, module `texrect` — an
  env-gated per-rectangle probe hooked at the decoder's `G_TEXRECT` arm
  (`crates/fn64-render-reference/src/gbi/stream.rs`, in the
  `G_TEXRECT | G_TEXRECTFLIP` match). It reads the cycle type through
  `OtherMode::cycle_type` (`gbi/types.rs:364`) — the crate's own accessor, the
  same one the decoder uses to build the rectangle three lines later — so a
  census row and a decode cannot disagree about `G_MDSFT_CYCLETYPE`. Armed by
  `FN64_GBI_TEXRECT_CENSUS`, independently of the opcode census, because the
  row vector grows once per rectangle.
- `examples/wm2000-census/` — the same headless harness the opcode census used,
  carrying zero game content. It emits the probe on the same incremental
  cadence as the opcode census, for the same reason: the run ends in a
  non-unwinding abort, so an end-of-run report would never be reached.

**The exact command.**

```sh
cd examples/wm2000-census
RECOMPILED_DIR="$HOME/Code/aki-recomp/games/NWXE/RecompiledFuncs" \
RECOMP_H_DIR="$HOME/Code/wm2000-run/recomp-h-clean" \
ROM="$HOME/Code/aki-recomp/games/NWXE/wm2000.z64" \
  cargo build --release

FN64_GBI_CENSUS=1 \
FN64_GBI_CENSUS_PER_TASK=1 \
FN64_GBI_CENSUS_OUT=<scratch>/census.tsv \
FN64_GBI_TEXRECT_CENSUS=1 \
FN64_GBI_TEXRECT_CENSUS_OUT=<scratch>/texrect.tsv \
WM2000_MAX_STEPS=20000000 \
ROM="$HOME/Code/aki-recomp/games/NWXE/wm2000.z64" \
  ./target/release/wm2000-census
```

**The ROM.** `/Users/jer/Code/aki-recomp/games/NWXE/wm2000.z64`. Its SHA-1 was
verified with `shasum -a 1` to match the `rom_sha1` recorded at
`aki-recomp/games/NWXE/profile.toml:14` — the same check and the same ROM the
opcode census used. This doc does not restate the digest, since no test here
gates it.

**The window.** 383 VI fields, 219 decoded display-list entries, 142,606
commands — byte-for-byte the same window the opcode census covered, ending at
the same unrelated `0x1CC` MMIO abort
(`generated-C direct-device read at 0x00000000000001CC used unsupported mapped
address with width 4`). About 6.4 seconds of NTSC virtual time: boot, logo,
title, attract.

**Two independent counters agree on the denominator.** The opcode census's
cumulative RDP-lane `G_TEXRECT` count is 2,520; its per-entry deltas sum to
2,520; the probe's own `texrects_seen` is 2,520 and it recorded all 2,520. A
disagreement between those would have meant the probe was hooked somewhere the
histogram was not.

**Determinism: two full runs produced byte-identical output.** The two
`texrect.tsv` files compared equal under `diff` and hashed identically under
`shasum -a 256`; the opcode census files matched too. The counts are
deterministic, not a sample. This holds the bar the prior census set. (The
digest is not restated here for the same reason the ROM's is not: no test gates
it, and re-running the command is the check.)

---

## 5. Sizing verdict: medium, not large — but it is combiner work, not a blit

**The remaining texrect work is NOT the small Copy-only case.** Copy cycle is
0% of this title's rectangles, so no amount of polishing the existing Copy
executor draws a WM2000 frame. The `UnsupportedCycleType` refusal is the
binding constraint on 100% of them.

**Nor is it the full general per-fragment combiner.** The large version of this
card would be "evaluate an arbitrary combiner program per fragment for an
unknown distribution of programs." The measured distribution is two programs,
fixed across the entire window, reading five inputs the crate already models
and already evaluates.

Concretely, what stands between here and a drawn WM2000 rectangle:

1. **Admit one-cycle in `execute_texture_rectangle`.** Today it hard-refuses
   (`targets/texrect.rs:511`). That refusal is correct while no combiner runs;
   it becomes the thing to replace.
2. **Evaluate the combiner per texel.** `run_one_cycle`
   (`combiner.rs:698`) already does this, is already public, and is already
   used by the triangle pipeline
   (`targets/triangle_pipeline/tests.rs`). The texrect executor writes the
   sampled RGBA8888 straight to the destination
   (`targets/texrect.rs:50-54` says so in its own module doc); the change is to
   route that texel through the combiner instead.
3. **Supply `Primitive` and `Environment` to the executor.** Both are already
   decoded — `G_SETPRIMCOLOR` (2,691 occurrences) and `G_SETENVCOLOR` (3,397)
   are both ADMITTED per the opcode census — so this is plumbing latched state
   to a call site, not new decode.

What is measurably NOT needed for this window: `Shade` (no texrect program
reads it), `Texel1`, two-cycle mode, the `Combined` carry, LOD fraction, noise,
and chroma key. Those are the parts of a general combiner that make it large,
and this title's texrects touch none of them.

**Verdict: medium.** Larger than "wire up Copy," because the combiner genuinely
has to run per fragment and cannot be skipped. Smaller than "port the general
combiner," because the arithmetic already exists in-crate, the two programs are
fixed and narrow, and every input they read is already modelled and already
decoded. The card is a plumbing card over existing arithmetic, not a new
evaluator.

---

## 6. Limits on how far this generalizes

Stated plainly, because the window is the constraint on every claim above.

- **The window stops before gameplay.** It ends at the `0x1CC` MMIO abort, ~6.4
  seconds in, covering boot, logo, title, and attract. A match could use
  different cycle modes and different combiner programs. Nothing here speaks to
  that, and the opcode census records the same limit. Extending the window
  needs that unrelated defect fixed first.
- **This is one title.** WM2000 is not evidence about the other AKI titles or
  any other ROM. The probe is re-runnable against any of them; none has been
  run.
- **The probe records what the reference decoder dispatched**, which is the
  right proxy for what a backend must admit, but is not itself a `WgpuBackend`
  run. The §1 claim about `execute_texture_rectangle`'s refusal is read from
  that function's source against the measured cycle modes, not observed from a
  `WgpuBackend` execution — no such run was made.
- **"Real work" is a classification of the programmed combiner, not of the
  pixels.** The flat-primitive program `(Zero − Zero) * Zero + Primitive`
  evaluates to a constant, and a sufficiently clever implementation could
  constant-fold it into a fill. The classifier deliberately does not do that:
  it reports what a fragment shader would have to compute from the wire
  program, because folding is an optimization decision and this doc is a
  measurement.
