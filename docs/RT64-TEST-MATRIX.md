# fn64 test matrix: which configuration produced which evidence

Every measurement in this project comes from one cell of a matrix, and three
claims have already had to be corrected because the cell was not recorded with
the number. This document exists so a future reader can ask "which
configuration is this evidence from?" and get an answer.

**Rule this document exists to enforce:** a measurement without its cell is not
a measurement. Quote the cell with the number.

Companion docs: [`RT64-WM2000-VALIDATION.md`](RT64-WM2000-VALIDATION.md),
[`RT64-WM2000-THREE-WAY.md`](RT64-WM2000-THREE-WAY.md),
[`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md),
[`RT64-PORT-CARD-BRIEF.md`](RT64-PORT-CARD-BRIEF.md).

---

## 1. The axes

| Axis | Values | Selected by |
|---|---|---|
| **Recompiler** | N64Recomp **C lane** / fn64's own **rs lane** (`fn64-cpu-runtime`) | `FN64_RECOMP=rs` vs `RECOMPILED_DIR` (`crates/fn64-shell/build.rs:53`) |
| **Renderer** | `fn64-render-reference` / `fn64-render-rt64` / `fn64-render-wgpu` | `FN64_RENDER` (shell), or direct construction in tests |
| **Profile** | debug / release (`-C debug-assertions=off`) | `RUSTFLAGS` |
| **GPU** | real adapter / none | host, plus `--features host-gpu-tests` |
| **Title** | WM2000 today; other AKI titles later | `ROM` |

The naive cross-product is 2x3x2x2 = 24 before titles. **Most cells are not
configurations anyone can run**, so occupancy matters more than completeness.

## 2. Occupancy — measured, unmeasured, or not-a-configuration

| Cell | State | Evidence |
|---|---|---|
| C lane x all three renderers, WM2000 frame 0 | **MEASURED** | 0 differing pixels of 115,200, all three pairs, `alpha_dither` controlled to `Disabled` on all sides. `RT64-WM2000-THREE-WAY.md` |
| C lane x reference, extended window | **MEASURED** | 4,454 VI swaps / 5,792 decode entries / 5,406,193 RDP commands / 230,240 texrects |
| **rs lane x anything** | **UNMEASURED** | The census harness has no `FN64_RECOMP` branch. This is the lane we ship. |
| `fn64-render-wgpu` in a shipping binary | **NOT WIRED** | No `FN64_RENDER=wgpu` arm; `present` gated it until recently |
| GPU-gated wgpu tests, no adapter | **ABSENT, not skipped** | 50 `#[cfg(feature = "host-gpu-tests")]` gates across 12 files |
| debug vs release | **MEASURED** | Every card runs both; 4 `should_panic` tests invert under `-C debug-assertions=off` |

## 3. The corrections this table would have prevented

- **"N64Recomp emits ..."** read as if it were fn64's recompiler. It is the C
  lane. `fn64-recomp` is ours and emits nothing of that shape.
- **`texture_rectangle_at` "pre-existing red"** — it is
  `#[cfg(feature = "host-gpu-tests")]` and therefore *absent* from a default
  run. A test that does not exist in a cell cannot be red in it.
- **"byte-identical to an independent rasterizer"** — `fn64-render-reference`
  shares fn64's lineage (both derive from public SGI docs and this project's
  reading). Agreement is corroboration, not validation. RT64 is the separate
  lineage; silicon is the absent authority.

## 4. Oracle ranking

1. **Hardware** — absent. No measurement in this project has been compared to
   silicon, and no hardware-correctness claim is supportable.
2. **RT64** (`fn64-render-rt64`) — separate authors, separate tree, real GPU,
   eyes-verified against games. Builds here with `--features rt64` and
   `FN64_RT64_DIR`. **The default comparison target.**
3. **`fn64-render-reference`** — same lineage as the port, and carries
   deliberately invented behavior (see below). A third voice and the
   CI-runnable fallback, **not the authority**.

**Where 2 and 3 disagree, RT64 wins absent hardware evidence.**

## 5. The reference's invented noise, and why it stays

`crates/fn64-render-reference/src/raster/mod.rs:112-120` implements the RDP's
noise as SplitMix64, documented as "publicly random, but unpublished" and
supplying a stream "without pretending to be the RDP's unknown polynomial".

- **Removing it is wrong.** WM2000 latches `alpha_dither = Noise`; a
  deterministic reference would be wrong on a mode the game uses, in a way that
  looks right.
- **Matching RT64's is not validation.** RT64 uses blue noise
  (`src/shaders/BlueNoise.hlsli`) — a rendering-quality choice, not a silicon
  reproduction. Both implementations invent a sequence, differently.
- **So it is controlled, not matched.** The three-way comparison rewrites
  `alpha_dither` to `Disabled` in the words fed to **all** sides. Both
  implementations receive identical input; this is variable control, not
  tuning one side.

**Never read a noise-controlled comparison as covering the noise term.**

## 6. What a green cell does not prove

- A **software-adapter** pass (Lavapipe/SwiftShader) covers CPU-side plumbing.
  For pixel assertions it is a *fourth* implementation with its own rounding;
  CI-green does not imply hardware-green.
- A **reference-only** agreement is same-lineage corroboration.
- A **frame-0** agreement covers one packet, one decode entry, **zero
  triangles**, one dither mode, and two distinct output halfwords. Three
  renderers agreeing on a flat-primitive blend plus a fill is much weaker than
  agreement on a textured, depth-tested scene.
