# Task 32 — WM2000 CPU-raster scanline parallelism

Date: 2026-08-23

## Result

Implemented a persistent-pool, data-parallel scanline path for depth-free raw
triangles in `fn64-render-wgpu`. Large and medium triangles retain the exact
scalar pixel body, but independent color rows execute on Rayon's persistent
work-stealing pool. The parallel iterator completes before the next draw, so
guest command order remains sequential. Depth-bearing draws and combiner
census runs stay scalar. Draws below 256 declared-range pixels stay scalar
because direct threshold measurement found that 64- and 128-pixel draws lose
to pool-dispatch overhead.

This approach was chosen over GPU promotion because it preserves the already
clean guest RGBA16 output and avoids inventing an RGBA8 RenderConfig-to-RGBA16
SetColorImage resize/requantization contract. It also keeps parity evidence on
the existing CPU executor. Rayon's persistent pool replaced an initial scoped
OS-thread prototype after the live title's many-small-triangle shape made
per-draw thread creation the wrong mechanism.

## Performance evidence

Exact retained code, release, unprofiled,
`texture_plane_raster_microbench`, 66,000 covered pixels x 400 iterations.
Four interleaved A/B pairs alternated lane order; the binary prints the exact
`FN64_PARALLEL_RASTER` value and rejects any value other than `0` or `1`.

| lane | reps, ns/covered-pixel | mean | min-of-N |
| --- | --- | ---: | ---: |
| scalar (`FN64_PARALLEL_RASTER=0`) | 523.629, 525.912, 505.721, 511.279 | **516.635** | **505.721** |
| parallel (`FN64_PARALLEL_RASTER=1`) | 82.354, 88.729, 90.473, 91.424 | **88.245** | **82.354** |

Measured speedup: **5.85x by unprofiled mean, 6.14x by min-of-N**.

The threshold sweep used the same executor and exact scalar/parallel gate:

| covered pixels | scalar ns/px | parallel ns/px | verdict |
| ---: | ---: | ---: | --- |
| 64 | 585.7 | 951.5 | keep scalar |
| 128 | 479.0 | 686.3 | keep scalar |
| 256 | 541.0 | 311.4 | parallel |
| 512 | 485.9 | 255.0 | parallel |
| 1,024 | 470.8 | 157.7 | parallel |

Evidence log: `.claude/task32/rayon-final-ab.log`; threshold logs:
`.claude/task32/rayon-threshold.log` and
`.claude/task32/rayon-threshold-small.log`.

### Drawn-frame status

The last valid same-scene rs+wgpu pump census remains Task 22's measured
**49.07 ms/drawn frame** (43.243 ms render field + 5.828 ms off field), about
20.4 fps. This task could not collect a renderer-valid after census: native
Metal is unavailable in the managed execution sandbox, and the attempted
Lavapipe window route still printed `reference-fallback`. Both invalid runs
were stopped rather than reported as wgpu.

Using the established attribution that rasterization is 62–66% of the 43.243
ms render field, applying the measured 5.85x substrate speedup to all raster
pixels would project **25.4–26.8 ms/drawn frame** (37.3–39.4 fps), inside the
33.3 ms budget. This is explicitly a conditional projection: draws below the
measured 256-pixel cutoff remain scalar, and this sandbox could not measure
their exact live pixel share. A GUI-capable rs+wgpu census is still required
before claiming the title itself hits 30 Hz.

## Correctness

- Direct scalar-versus-parallel framebuffer agreement:
  `depth_free_and_depth_present_paths_agree_on_a_depth_disabled_draw`,
  **20 consecutive release runs, 20/20 pass**. The parallel depth-free output
  is byte-identical to the scalar path; disabled depth cells remain untouched.
- Full `fn64-render-wgpu` release lib suite through the repository-documented
  Lavapipe path: **4,887 passed, 0 failed, 5 ignored**.
- Current 37-case parity runner, wgpu side through Lavapipe: **36 completed,
  1 exact expected YUV refusal; 35 hand-key matches, the exact expected
  two-cycle divergence, no new wgpu divergence**.
- The full RT64 checker could not produce a current PASS because all 37 RT64
  cases refused with `no Metal system-default device is available`. The
  immediately preceding unchanged scalar baseline is recorded in
  `task6-parity-result.log` as **PASS 33/37**. Since the current parallel path
  is directly byte-identical to that scalar output, a current Metal rerun is
  expected to pass, but that is an inference and is not labelled as a run.
- `scripts/lint-docs.py` ran and reported 14 pre-existing errors plus three
  warnings (stale SDD env-var references and generated-doc drift); none names
  a task-owned file.

## Concurrency invariant

`par_chunks_mut(row_stride)` gives each job exclusive ownership of one whole
color row. No two jobs can address the same byte. The existing scalar body is
called with that row and its guest Y base. Completing `try_for_each` is the
draw-order barrier: a later triangle cannot observe or mutate the target until
all rows of the current triangle finish. TMEM is shared read-only and now has
an explicit `Sync` bound. No unsafe code was added.

## Files

- `Cargo.toml`, `Cargo.lock`: Rayon persistent-pool dependency.
- `crates/fn64-render-wgpu/src/targets/raw_triangle.rs`: measured gate,
  row-parallel wrapper, unchanged scalar body with local-row indexing.
- `crates/fn64-render-wgpu/src/tmem/read.rs`: the read-only TMEM source carries
  its sharing invariant as a `Sync` supertrait.
- `docs/RT64-TRIANGLE-WRITEBACK.md`: architecture, threshold, measurements,
  and windowed nonclaim.

## Commit status and remaining work

No commit hash could be created in this managed session. `git add` failed
before changing the index:

```text
fatal: Unable to create '/Users/jer/Code/fn64/.git/worktrees/wm2000-playable/index.lock': Operation not permitted
```

The shared Git metadata is outside the writable workspace root. The task-owned
diff remains unstaged and scoped; the pre-existing modified
`crates/fn64-render-wgpu/README.md` and unrelated untracked artifacts were not
touched.

Remaining:

1. On a GUI/Metal-capable host, run at least two bounded, interleaved
   `FN64_PARALLEL_RASTER=0/1` rs+wgpu pump censuses with warmup 300/pumps 1200
   and report the min-of-N unprofiled drawn-frame means.
2. Rerun the full Metal RT64 parity gate and require PASS 33/37.
3. If the live triangle-size mix leaves material scalar time below 256 pixels,
   the next architectural step is packet-level tile replay (preserve draw order
   inside each tile), not lowering the measured-regressive per-triangle cutoff.
4. Commit the five task-owned production/doc/lock paths plus this report once
   the real worktree Git index is writable; do not stage the pre-existing
   README edit or unrelated artifacts.
