# What remains: WM2000 playable on the fn64 recompiler + wgpu port

The target stack is **all-fn64**: `fn64-cpu-runtime` (the rs lane) for the CPU,
`fn64-render-wgpu` for the renderer. No N64Recomp, no RT64, no reference
backend.

Every item below is a **measured** blocker with a named site, not an estimate.
Where something is unknown it says so.

Companion docs: [`RT64-TEST-MATRIX.md`](RT64-TEST-MATRIX.md),
[`RT64-WM2000-RECOMP-LANES.md`](RT64-WM2000-RECOMP-LANES.md),
[`RT64-WM2000-VALIDATION.md`](RT64-WM2000-VALIDATION.md),
[`RT64-WM2000-CENSUS.md`](RT64-WM2000-CENSUS.md).

---

## 1. Where each half stops today

Both halves now run and fail at **different, named places**. Neither blocks the
other.

| Half | Command | Stops at |
|---|---|---|
| **Recompiler (rs lane)** | harness in `~/Code/recomps/wm2000/packages/wm2000-boot` | `lookup: no recompiled function or host shim at vram 0x80022540` = `osDriveRomInit` |
| **Renderer (wgpu)** | `FN64_RENDER=wgpu ROM=... ./target/release/fn64` | first VI present: `VI STATUS selects coverage silhouette antialiasing (AA mode 0 or 1); this scanout implements only AA mode 3` |

## 2. Recompiler gaps

**Our recompiler is not the problem.** 2,442 functions, **2,414 clean
(98.85%), zero unknown-opcode, zero ROM-range failures**, deterministic emit.

| # | Gap | Site | Size |
|---|---|---|---|
| R1 | **25 stub dispositions.** The C lane emits 57 empty *callable* bodies that return silently; ours omits the body and traps by name. 24 are direct `call_host_or_recompiled` targets (one at 27 call sites), plus `osDriveRomInit` via `lookup`. | `wm2000.toml` `[patches].stubs` | Design call: host adapter vs `force_recompile` vs named trap. Not all one answer. |
| R2 | **No `osDriveRomInit` adapter** in `fn64-abi`. A 64DD probe on a cartridge title. | — | Needs a decided return value with evidence |
| R3 | **Harness repair.** `rs/Cargo.toml` and the `fn64_shared` symlink carry dead absolute paths; `#[path = "../../crates/..."]` resolves to nothing in both harnesses (pre-existing extraction defect). | `~/Code/recomps/wm2000/` | Mechanical |
| R4 | **`oot-boot`'s 68-row `host_lookup` has zero tests.** A wrong-adapter row is silent there. WM2000's 31-row table now has a pointer-comparison test after a mutant survived. | — | Small, and the mutation lesson generalizes |

**Open question worth settling:** the config's own comments say stubs come from
an opcode scan for `cop0`/`cache`/`break`, and identified functions are meant to
"move to RENAMEs so the runtime provides them instead". That reads as *our trap
being correct and the C lane's silence being the defect* — worth confirming, and
possibly worth reporting upstream.

## 3. Renderer gaps

| # | Gap | Site | Blocks |
|---|---|---|---|
| V1 | **VI silhouette AA (modes 0/1)** — needs per-pixel coverage, which guest RDRAM RGBA16 carries in its low bit + hidden bits. The data exists; the backend does not read it. `fn64-render-reference` already implements this. | `vi_scanout.rs:67` | **The first frame.** WM2000 latches it immediately. |
| V2 | Seven other VI filters refuse by name: dither restoration, divot, gamma, gamma dither, bilinear resample, fade, repeat-line. | `vi_scanout.rs` | Unknown which WM2000 latches beyond V1 |
| V3 | **Triangles are not in the admitted set.** WM2000 issues **925,114** `RDP_TRI_SHADE_TEX`. A triangle raster declares **no journal write access at all**, so there is no declared order to compose onto — the composition target must be *invented*, not recovered. | `production.rs` `MixedFillAndTrianglePacket` | Everything past the title screen |
| V4 | **RGB dither refused**, pending hardware: the two ports disagree on both the Bayer table (8/16 cells) and the arithmetic. | `targets/texrect.rs` | Unknown |
| V5 | **Lane divergence, pinned not fixed:** `fn64-render-wgpu` refuses TLUT-over-RGBA16 (`FormatMustBeColorIndex`) where the reference now accepts it. The wgpu side's exhaustive six-cell matrix encodes the wrong model. | `tmem/` + its matrix test | Palettized sampling from triangles |
| V6 | **Noise alpha dither is unmatchable by construction.** The RDP's stream is "publicly random, but unpublished"; the reference uses SplitMix64, RT64 uses blue noise. Both invent. Comparisons control the variable instead. | — | Not a blocker; a permanent caveat |
| V7 | paraLLEl-RDP forces a **4-tap footprint whenever TLUT is on**, even point-sampled. Neither RT64 nor fn64 models this. | — | Unresolved |

## 4. What is proven, and how narrowly

WM2000's real captured frame 0 executes end-to-end through `WgpuBackend` and
publishes to guest RDRAM, at **0 differing pixels of 115,200** against *both*
`fn64-render-reference` and RT64, with `alpha_dither` controlled to `Disabled`
identically on all three. The prim-colour control moves exactly 114,481 pixels
on all three, so they are not agreeing by failing to draw.

**Scope, stated at the right strength:** one packet, one decode entry, **zero
triangles**, one dither mode, **two distinct output halfwords**. Three renderers
agreeing on a flat-primitive blend plus a fill is much weaker than agreement on
a textured, depth-tested scene. And all of it came through the **C lane** — the
rs lane has produced no stream, so the recompiler axis collapse is on paper
only.

## 5. Ordered path to "playable"

1. **V1** — VI silhouette AA. One refusal from a visible frame.
2. **R1 + R2 + R3** — stub dispositions and harness repair, to boot on our own recompiler.
3. **Wire-diff the two lanes** at command-word level, once both produce a stream. This is the regression gate that makes the C lane retirable.
4. **V3** — triangles. The largest remaining renderer item, and everything past the title screen depends on it.
5. **V5** — resolve the TLUT lane divergence.
6. Extend the census past attract into gameplay, then re-measure the five flat deltas.

## 6. The five flat deltas — a real property, not a short-run artifact

Across a ~500x window expansion (5,792 to 109,041 decode entries; 108,040
texrects), **none of these ever appeared**: Z-variant triangles **0**, two-cycle
texrects **0**, `G_SETZIMG` **0**, `Shade`/`Texel1`/`Combined` **unread**,
distinct combiner programs **3**, opcodes **21** — nothing added or lost.

If that holds through gameplay, the port's required scope is **materially
narrower** than a general RDP implementation. It is the single most useful thing
to re-check once the window reaches real play.

## 7. Known-unknowns

- **No hardware comparison has ever been made.** No claim here is silicon-validated.
- **Gameplay has never been reached** on either lane. The C lane's deep window is uniform white — a blank-frame loop, not content.
- Whether the two recompiler lanes emit identical RDP streams is **unmeasured**.
- Whether WM2000 latches VI filters beyond V1 is **unmeasured** — the census counts opcodes and does not decode `G_RDPSETOTHERMODE` payload bits or scissor rect values.

## S1: GPU triangles never reach guest memory

This comes from reading the crate, not from inference. Near `stage_and_report`,
`production.rs` states it directly: the missing RDRAM writeback for the GPU
raster path "is a separate, pre-existing gap that this arm never closed."

A `RawTriangle` pushes no `ResourceAccess`, so it declares no journal write and
stages no `CompletedWrite`. Its raster lands in `triangle_draw_output`, which
`present` refuses to scan out by name: "one submission's readback, not a
VI-sampled framebuffer."

Texrects reach guest memory; raw triangles do not. WM2000's title and HUD are
texrects (2,520 measured), so the ROM renders a real frame while no 3D geometry
appears at all. A capture from a clean run with every guard live confirms this:
see `docs/frames/wm2000-swap240-true-geometry-480x237.png`, which shows flat 2D
rectangles and no geometry.

S1 sets the ceiling on playable gameplay. Closing it requires a design change
rather than a guard fix, so it does not parallelize with the guard cards.

To close S1, write a CPU triangle rasterizer inside `fn64-render-wgpu`. Reading
the GPU result back does not work: `TriangleDrawOutput.color_rgba8` is
`Rgba8Unorm` against an RGBA16 guest framebuffer, and its extent comes from
`RenderConfig` rather than `SetColorImage`. Rastering into the color target
reduces to the same CPU work, because `ColorTargetRegistry`'s `device_bytes` is
a CPU `Vec<u8>`. For the full analysis, see
[the triangle writeback findings](RT64-TRIANGLE-WRITEBACK.md).

`fn64-render-reference` already has a rasterizer, but it is a dev-dependency
whose functions are `pub(super)`. Promoting it to a production dependency puts
the software reference renderer back into the stack that this goal excludes, so
the rasterizer has to be written fresh in-crate.
