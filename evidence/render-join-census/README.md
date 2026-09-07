# Render-join overlap census — WM2000 3,000-pump lane

Task 6.2 Step 1 of `docs/plans/CLEANUP-2026-09.md`. Instrumentation only.

`wm2000-3000-pump-census.txt` is the verbatim summary emitted at process exit
by `crates/fn64-abi/src/task_dispatch/render_join_census.rs`.

## How to reproduce

Build the rs-lane shell, then run with the census armed:

```
zsh scripts/play-wm2000.sh --print-config   # builds crates/fn64-shell/rs/target/release/fn64

FN64_RENDER_JOIN_CENSUS=1 zsh scripts/benchmark-wm2000-render.zsh \
  --rom <path>/wm2000.z64 \
  --bin crates/fn64-shell/rs/target/release/fn64 \
  --output-dir <tmp> --label census --warmup 300 --pumps 3000 \
  -- --boot-context <path>/wm2000-boot-context.json
```

The summary lines land on stderr, captured in `<tmp>/census-01.log`. The
benchmark script refuses to run while any `cargo`/`rustc` process exists, so
wait for a build-free gap first.

## What the numbers say

Joins vary run to run (a live 3,000-pump run on a contended machine): observed
1,134 / 1,144 / 1,281 across runs. The classification is stable.

**The headline is the per-cause split, not the total.** Every join on this lane
is `cause=dmem_dependency` with `rdram_only=false`; there are zero
`later_graphics` joins. The joining task is `task_type == M_AUDTASK` (2) on
every occurrence, so `is_gfx` — and therefore the `later_graphics` predicate —
is false throughout. The population is an audio task arriving while a render
batch is in flight, whose rspboot waits on the shared DMEM command buffer.

The 100%-disjoint RDRAM verdict is correctly computed but describes joins whose
dependency these RDRAM ranges do not model. It must **not** be read as "1,144
skippable joins".

## Controls

- **Negative control (committed as a test).**
  `a_batch_compared_against_itself_always_overlaps` pins that the pipeline can
  report a non-degenerate OVERLAP. The live form of the same control — comparing
  each real batch to itself over this lane — reported 1,140 joins / 1,140
  overlap / 0 disjoint, the exact inverse of the real run.
- **A bounds defect was found and fixed by checking.** An earlier revision
  passed `host.runtime_rdram_len` (the runtime allocation, `0x2900_0000` — 82x
  the console's 8 MB) as the RDRAM bound, inflating every render-target extent
  to 688 MB. Fixed to `min(installed RDRAM, RDP 24-bit ceiling)`; pinned by
  `the_color_image_extent_is_bounded_by_the_rdp_address_ceiling`.
