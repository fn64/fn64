# Task 30: WM2000 rendering performance regression diagnosis

## Verdict

`fcd48b7c` (`feat(render-wgpu): bind z-image ... and wire depth test`) is a
real, still-present CPU-raster regression on WM2000's depth-disabled triangle
path. An exact commit-versus-parent A/B of the headless raster benchmark measured
**492.639 -> 502.699 ns/covered-pixel**, or **+10.060 ns/px (+2.04%)**.

This is not the initially hypothesized worst case: WM2000 does **not** execute
the expensive Z compare/update math. The commit correctly gates allocation of
the depth accumulator on `Z_CMP || Z_UPD`, and gates `relations`/depth writes on
the per-draw flags. The regression is the smaller but still per-pixel cost left
in the ostensibly depth-disabled fast path: every covered pixel computes a
linear pixel index, matches `(depth.as_ref(), fragment_depth)`, checks the result,
then later matches `(depth.as_mut(), fragment_depth)` again. With `depth == None`
both matches fall through, but neither disappears at runtime.

No other suspect produced a measured raw-triangle regression. In particular,
the exact `435dbbab` coverage commit A/B overlapped noise and was slightly faster
on average. The remaining suspects are either outside the dominant raw-triangle
loop, per-load/texrect-only, exit-only, already compensated, or test-only.

## Actual WM2000 depth state

The existing real-ROM census is decisive, not an inference from the command
names. Across an attract-loop run and an 18-menu-screen run it observed
2,655,652 `G_RDPSETOTHERMODE` writes and found:

- `Z_CMP`: 0
- `Z_UPD`: 0
- primitive Z source: 0
- non-opaque Z mode: 0

The opcode census also found no alternative OtherMode writer, and five earlier
censuses found zero `G_SETZIMG` and zero Z-variant triangles. See
`docs/RT64-TRIANGLE-WRITEBACK.md:1283-1333`, especially the tallies at
`:1305-1312` and coverage argument at `:1319-1329`.

Therefore `stage_color_commands` evaluates `wants_depth == false` and keeps
`depth_accum == None` on this measured WM2000 path. The direct production gate
is `crates/fn64-render-wgpu/src/production.rs:3777-3796`: it scans triangle
snapshots for `depth_compare_enabled() || depth_update_enabled()` and allocates
only when one is set.

## Exact regressing mechanism

The added code is partly gated and partly unconditional:

1. Once per draw, `fragment_depth` is derived through `depth.as_ref()` outside
   the pixel loop (`crates/fn64-render-wgpu/src/targets/raw_triangle.rs:434-451`).
   WM2000 gets `None`.
2. Every covered pixel unconditionally computes `pixel` and executes the
   `match` at `raw_triangle.rs:681-713`. Only its expensive
   `depth_mode::relations` arm at `:683-708` is gated by `Some(depth)` and
   `d.compare`.
3. Every passing pixel then unconditionally enters the second `if let` shape at
   `raw_triangle.rs:728-739`. Only the codec and depth-cell store are gated by
   `Some(depth)` and `d.update`.

Thus the direct answer to the brief's verification question is: **the actual Z
compare/update calculations do not run unconditionally, but the new depth-path
dispatch/bookkeeping does run unconditionally per covered pixel even when both
Z bits are clear and no depth accumulator exists.** WM2000 pays that bookkeeping
for its hot 0x0e shaded+textured triangle loop despite never arming depth.

## Microbenchmark A/B

Renderer/lane: pure CPU `fn64-render-wgpu` raw-triangle executor used by the
all-fn64 rs+wgpu lane. Benchmark: ignored
`texture_plane_raster_microbench`, release optimized, unprofiled, 66,000 covered
pixels x 400 iterations per repetition. The test-only benchmark from `9ffa4e69`
was transplanted into isolated scratch clones; the parent copy removed only the
new trailing `depth: None` argument required by `fcd48b7c`'s API. No production
source was changed for either measured side.

Command class:

```sh
cargo test -q -p fn64-render-wgpu --lib --release \
  texture_plane_raster_microbench -- --ignored --nocapture
```

Exact comparison:

- Before: `e98ade37` (`fcd48b7c^`)
- After: `fcd48b7c`
- Scratch root: `/tmp/fn64-task30.M3b8wW` (not evidence that needs to persist)

| pair | parent ns/px | fcd48b7c ns/px | delta |
|---:|---:|---:|---:|
| 1 | 491.254 | 497.675 | +6.421 |
| 2 | 482.878 | 496.987 | +14.109 |
| 3 | 486.973 | 499.129 | +12.156 |
| 4 | 499.459 | 496.682 | -2.777 |
| 5 | 488.803 | 499.918 | +11.115 |
| 6 | 477.354 | 482.112 | +4.758 |
| 7 | 497.278 | 504.528 | +7.250 |
| 8 | 492.913 | 519.398 | +26.485 |
| 9 | 502.035 | 525.718 | +23.683 |
| 10 | 507.440 | 504.840 | -2.600 |
| **mean** | **492.639** | **502.699** | **+10.060 (+2.04%)** |

The depth commit was slower in 8/10 interleaved pairs. This establishes a
microbenchmark regression, but it should not be inflated into an invented
whole-frame delta: the available 49.1 ms/drawn-frame anchor was not collected
at both exact commits, and the microbenchmark isolates only covered-pixel CPU
raster work. A reported night-to-night improvement much larger than a few
percent of raster time would still require checking lane/configuration or
window/compositor variance.

## Other suspects

### `435dbbab` coverage carry path: not a measured regression

The commit changes `blend_and_write_pixel` from a short-circuit expression to
an eagerly computed `coverage_carry` decision (`targets/texrect.rs:2698-2729`).
That helper is also called by raw triangles, so it was worth measuring even
though the commit is named for texrect coverage.

An exact commit-versus-parent benchmark transplant, five interleaved pairs:

- `435dbbab^` mean: **489.723 ns/px**
- `435dbbab` mean: **487.402 ns/px**
- delta: **-2.321 ns/px (-0.47%)**, with heavily overlapping distributions

This is noise/null, not evidence of a regression.

### `1d8c0d11` LoadBlock DxT: static exclusion

Its production work computes a footprint and marks validity during LoadBlock
staging (`production.rs` and `tmem/execute/load_block.rs`), once per load/word
span. It adds no covered-pixel raster work and cannot explain the measured
raw-triangle ns/px increase.

### `c8ba2cb5` TexRectFlip: secondary, unmeasured texrect-only possibility

It adds `TexrectDraw::coordinates_at` with a `flipped_axes` branch at
`targets/texrect.rs:358-369`, called per texrect pixel at `:1862-1865`. It does
not run in `raster_triangle`, so the dominant WM2000 triangle benchmark cannot
measure it. Existing evidence does not establish whether live WM2000 uses
opcode 0x25; this remains a possible small texrect-only cost, not the identified
triangle regression.

### Shell/present commits and test-only commit

- `5120e619` added a once-per-present VI blanking check, not per-pixel raster
  work. Its initial full-MMIO access was replaced by `c6eacd65`'s direct
  `vi_registers[0]` field read; current code is `fn64-abi/src/vi.rs:236-252` and
  `fn64-runtime/src/device/fabric.rs:717-724`. It is not a remaining current-HEAD
  hot-loop regression.
- `924effda` changes process-exit sealing and diagnostics. It is not on the
  steady-state frame path.
- `c6eacd65` is explicitly a performance improvement.
- `9ffa4e69` changes only test files.

## Proposed fix direction (not implemented)

Make depth absence a structural fast-path choice **outside** the covered-pixel
loop. When `depth.is_none()` (which production already derives from
`Z_CMP || Z_UPD`), run a depth-free loop/body that contains no pixel index for
Z, no `Option` matches, and no compare/update flag checks. For the depth-present
case, preferably specialize compare/update combinations once per draw outside
the loop as well.

Merely adding another test of `z_compare_en`/`z_update_en` inside the existing
per-pixel block would preserve the regression. The kill evidence should be the
same exact parent/changed microbench with `depth=None`, plus the existing depth
parity/unit tests for the `Some(depth)` path and framebuffer byte identity for
the no-depth path. This report intentionally implements none of that.

## Worktree hygiene

No checkout, tracked source edit, or build was performed in the shared checkout.
It began at `a6d4a85572a6`; while this diagnosis was running, another session
advanced it to `dc297b69960b` with a two-line progress-document commit only.
That concurrent commit did not touch the cited runtime files, and the
depth-disabled per-pixel work remains present at the same lines. The checkout
was already dirty on entry, including a modified
`crates/fn64-render-wgpu/README.md` and unrelated untracked artifacts; all were
preserved. This report is this diagnosis's only shared-worktree write.
