# RT64 GPU test matrix: running the gated tests without a GPU

`crates/fn64-render-wgpu` carries **33** `#[cfg(feature = "host-gpu-tests")]`
tests across **9** files. Under a default `cargo nextest run` they are not
skipped and not reported -- they are not compiled, so they do not appear in the
count at all. That is how a lane came to describe
`wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position` as
"pre-existing red" when a default run never contained it.

This document records the software adapter that makes them runnable on a
GPU-less CI runner, the classification of what such a run does and does not
prove, and the measured software-vs-Metal results.

**Correction to the brief this work was dispatched with.** The brief stated
"50 gates across 12 files". Measured in this checkout at `a22762f3`: `grep -rn
'host-gpu-tests' crates/fn64-render-wgpu/` returns 54 lines, but 15 of those
are README prose, 1 is the `Cargo.toml` feature declaration, and 8 are code
comments. The attribute sites are 31 (some gate a `mod`, not a `fn`), and the
count that matters -- tests present with the feature and absent without it --
is **33**, measured as the difference between two `cargo nextest list` runs
(4682 with, 4649 without). Nine files contain attribute sites, not twelve.

## The adapter: Mesa Lavapipe, via the existing `vulkan` backend

| Candidate | Verdict | Why |
|---|---|---|
| **Lavapipe** (Mesa `llvmpipe`, `lvp_icd`) | **Chosen, working** | A real software rasterizer reachable through the `vulkan` backend the crate already enables. Verified enumerating on this host as `Vulkan \| llvmpipe (LLVM 22.1.8, 128 bits) \| Cpu`. |
| SwiftShader | Not needed | Would also be a software Vulkan ICD, so it solves the same problem Lavapipe already solves here, with no packaged macOS/Homebrew path and an extra build to maintain. Not refuted -- untested, because Lavapipe worked. |
| `gles` + ANGLE/SwiftShader | Rejected | The crate does not enable the `gles` wgpu feature; adopting it widens the dependency graph to gain a backend Lavapipe already covers. |
| wgpu `noop` backend | **Ruled out by inspection, as instructed** | `wgpu-hal-30.0.0/src/lib.rs:260` -- "A dummy API implementation". It records commands and produces no rasterized output, so every one of the 24 PIXEL tests below would read back zeroes. It cannot satisfy the pixel tests, and for the plumbing tests it would prove only that the CPU code ran, not that a driver accepted the pipeline. |

### The Apple gotcha, measured not assumed

On Apple targets wgpu's `vulkan` feature is a **no-op**. `wgpu-core/Cargo.toml:96`
routes `vulkan` to `wgpu-core-deps-windows-linux-android/vulkan`; only
`vulkan-portability` (`:97`) reaches `wgpu-core-deps-apple`. Measured on this
host, with the loader and ICD present and `vulkaninfo --summary` correctly
listing `llvmpipe` / `DRIVER_ID_MESA_LLVMPIPE`:

- with `features = [..., "vulkan", ...]` -- `instance.enumerate_adapters(VULKAN)`
  returned **zero** adapters.
- adding `"vulkan-portability"` -- the same call on the same host returned
  `Vulkan | llvmpipe (LLVM 22.1.8, 128 bits) | Cpu`.

`vulkan-portability` adds a backend that is otherwise unreachable on macOS and
removes none, so the default Metal path is unchanged (confirmed by the Metal
column below being identical to the pre-change Metal run).

Separately, `ash` `dlopen`s a bare `libvulkan.dylib`, which is not on the
default macOS search path. macOS SIP strips `DYLD_LIBRARY_PATH` from the
process, so that variable does not work; `DYLD_FALLBACK_LIBRARY_PATH` is the
one that survives. **A Linux CI runner needs neither** -- `libvulkan.so.1`
resolves through the normal loader path there.

## How the software path is selected, and why an env var

`FN64_WGPU_SOFTWARE_ADAPTER=1` restricts every adapter request in the crate to
`Backends::VULKAN` and requires the returned adapter to report
`DeviceType::Cpu`. The implementation is one module,
`src/device/adapter_selection.rs`, routed into all **7** adapter-request sites.

An env var rather than a cargo feature, for two reasons:

1. A feature would change which `wgpu` features compile for every consumer of
   the dependency graph, and `vulkan-portability` is a link-time unification
   knob, not a per-test switch.
2. One built test binary can serve both a hardware run and a software run.
   That is what makes the side-by-side table below a comparison of the *same
   binary* rather than of two different builds.

Default (variable unset) behavior is unchanged: the full native backend mask,
no device-type constraint.

### It cannot go quietly green

The failure mode that let 33 tests hide was silence. Two guards:

- **Every one of the 33 already panics on a missing adapter** -- audited across
  all 9 files; there is no `return;`-style skip and no `eprintln`-and-continue
  anywhere in the gated set. A runner with no adapter of any kind gets a named
  panic (`required host GPU evidence unavailable: typed no-adapter for ...`),
  never a pass.
- **`assert_expected_adapter`** additionally fails the run when the software
  flag is set but a *hardware* adapter answers. Without it, a CI runner that
  happened to have a GPU would silently report hardware results in the software
  column of this table.

## CI command

```sh
# Linux runner, no GPU. Mesa's Lavapipe ICD + the Vulkan loader:
#   apt-get install -y mesa-vulkan-drivers libvulkan1
FN64_WGPU_SOFTWARE_ADAPTER=1 \
  cargo nextest run -p fn64-render-wgpu --features host-gpu-tests
```

On macOS (`brew install mesa vulkan-loader`) the loader is not on the default
`dlopen` path, so two more variables are needed:

```sh
FN64_WGPU_SOFTWARE_ADAPTER=1 \
VK_ICD_FILENAMES=/opt/homebrew/share/vulkan/icd.d/lvp_icd.aarch64.json \
DYLD_FALLBACK_LIBRARY_PATH=/opt/homebrew/lib \
  cargo nextest run -p fn64-render-wgpu --features host-gpu-tests
```

**Wall clock**, measured on this host, whole crate suite (4685 tests), three
consecutive runs each: software **4.51 / 4.49 / 4.53 s**; Metal **3.97 / 3.99 /
4.08 s**. Lavapipe costs roughly half a second on this suite -- the gated tests
draw 8x8-scale fixtures, so the software rasterizer is not the bottleneck.
Determinism: 10 consecutive software runs, all `4685 tests run: 4682 passed, 3
failed`.

## What software-green proves, and what it does not

Lavapipe is a **fourth implementation** alongside the N64 RDP, fn64's own CPU
oracles, and the host's Metal/DX12/Vulkan driver. It has its own rounding, its
own interpolation precision, and its own edge tie-breaking.

- **PLUMBING (8 tests)** -- assertions are entirely CPU-side: admission
  decisions, pipeline caching and reset, extent bookkeeping, TMEM slot
  identity. The GPU is present only so the code path can execute. These become
  **genuinely CI-covered** under Lavapipe; a green here means the same thing it
  means on hardware.
- **PIXEL (24 tests)** -- assert rasterized output values against an oracle.
  Green under Lavapipe is **evidence about Lavapipe**. It is a real regression
  net -- it would catch a shader that stopped compiling, a binding that came
  loose, a WGSL change that altered results on every rasterizer -- but it is
  **not hardware validation**, and CI-green here must never be cited as
  Metal-green or as RDP parity.
- **PIXEL-SELF (1 test)** -- `an_invalid_draw_after_two_valid_triangles_preserves_the_prior_output`
  compares `color_rgba8` to *its own* earlier value rather than to an oracle.
  It is rasterizer-independent in principle (any implementation that is
  self-consistent passes), but it does read back rasterized bytes, so it is
  listed separately rather than folded into PLUMBING. **This one is the
  ambiguous case**; it is called out rather than guessed at.

## Results: all 33, software vs Metal

Both columns are the same test binary on the same host, differing only in
`FN64_WGPU_SOFTWARE_ADAPTER`.

| Test | Module | Class | Lavapipe | Metal |
|---|---|---|---|---|
| `required_host_fragment_fn_matches_cpu_oracle_across_frozen_fixtures` | `alpha_compare` | PIXEL | PASS | PASS |
| `required_host_fragment_fn_matches_cpu_oracle_across_frozen_fixtures` | `blend` | PIXEL | PASS | PASS |
| `required_host_fragment_fn_matches_cpu_oracle_across_frozen_fixtures` | `coverage` | PIXEL | PASS | PASS |
| `required_host_executes_exact_fill_full_sync_to_receipted_gpu_completion` | `device` | PIXEL | PASS | PASS |
| `required_host_negative_uv_floors_toward_negative_infinity_not_truncation` | `production` | PIXEL | PASS | PASS |
| `required_host_textured_triangle_wgsl_sampling_matches_the_cpu_tmem_oracle` | `production` | PIXEL | PASS | PASS |
| `two_ordinary_triangles_in_one_call_both_survive_into_one_output` | `production` | PIXEL | PASS | PASS |
| `wgpu_backend_draws_a_real_admitted_triangle_matching_the_combiner_oracle` | `production` | PIXEL | PASS | PASS |
| `wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position` | `production` | PIXEL | FAIL | FAIL |
| `wgpu_backend_draws_a_real_texture_rectangle_flip_at_the_same_wire_position` | `production` | PIXEL | FAIL | FAIL |
| `direct_texel_decode_native` | `shader_manifest` | PIXEL | PASS | PASS |
| `required_host_executes_exact_native_fill_through_guest_and_target_commit` | `targets` | PIXEL | PASS | PASS |
| `required_host_alpha_compare_none_always_writes_and_threshold_discards_below_blend_color` | `targets` | PIXEL | PASS | PASS |
| `required_host_depth_test_accepts_a_nearer_second_triangle_in_the_same_target` | `targets` | PIXEL | PASS | PASS |
| `required_host_depth_test_both_clear_draws_over_the_first_and_does_not_write_depth` | `targets` | PIXEL | PASS | PASS |
| `required_host_depth_test_both_set_reduces_to_the_prior_less_write_always_pipeline` | `targets` | PIXEL | PASS | PASS |
| `required_host_depth_test_rejects_a_farther_second_triangle_in_the_same_target` | `targets` | PIXEL | PASS | PASS |
| `required_host_depth_test_z_cmp_clear_lets_a_farther_second_triangle_draw_over_the_first` | `targets` | PIXEL | PASS | PASS |
| `required_host_depth_test_z_upd_clear_does_not_write_depth_for_a_third_draw` | `targets` | PIXEL | PASS | PASS |
| `required_host_framebuffer_color_blend_matches_the_rust_oracle_at_nonzero_row` | `targets` | PIXEL | PASS | PASS |
| `required_host_general_divide_blend_draw_matches_the_rust_oracle_with_no_memory` | `targets` | PIXEL | PASS | PASS |
| `required_host_ordinary_vs_alpha_coverage_select_writes_distinct_alpha` | `targets` | PIXEL | PASS | PASS |
| `required_host_rasterizes_covering_triangle_and_matches_combiner_and_depth_oracles` | `targets` | PIXEL | PASS | PASS |
| `required_host_draws_a_real_admitted_triangle_matching_the_combiner_oracle` | `targets` | PIXEL | PASS | PASS |
| `an_invalid_draw_after_two_valid_triangles_preserves_the_prior_output` | `production` | PIXEL-SELF | PASS | PASS |
| `a_failed_triangle_draw_leaves_the_prior_successful_output_untouched` | `production` | PLUMBING | PASS | PASS |
| `draw_admitted_triangles_admits_a_framebuffer_color_only_blend_cycle` | `production` | PLUMBING | PASS | PASS |
| `draw_admitted_triangles_rejects_a_blend_cycle_that_reads_the_framebuffer` | `production` | PLUMBING | PASS | PASS |
| `create_requests_a_real_metal_adapter_and_stores_the_triangle_pipeline` | `production` | PLUMBING | FAIL | PASS |
| `repeated_create_calls_reset_the_triangle_pipeline_each_time` | `production` | PLUMBING | PASS | PASS |
| `mixed_load_and_triangle_plan_uses_the_real_successor_route_not_preserving` | `production` | PLUMBING | PASS | PASS |
| `repeated_create_with_a_changed_extent_updates_pipeline_and_extent_together` | `production` | PLUMBING | PASS | PASS |
| `triangle_only_plan_completes_via_preserving_physical_and_never_flips_the_slot` | `production` | PLUMBING | PASS | PASS |
### Summary

| | Lavapipe | Metal |
|---|---|---|
| Passed | 30 / 33 | 31 / 33 |
| Failed | 3 | 2 |

## Findings

### The two previously-red tests fail on **both**, and neither failure is a GPU failure

`wgpu_backend_draws_a_real_texture_rectangle_at_its_own_wire_position` and
`wgpu_backend_draws_a_real_texture_rectangle_flip_at_the_same_wire_position`
were reported red on a hardware adapter. They are red under Lavapipe too, and
the panic text is **byte-identical** in both runs:

```
panicked at crates/fn64-render-wgpu/src/production.rs:4080:14:
fixture stays inside the admitted state+rect subset: Backend {
  backend: "render-wgpu/raw-dpc-execute",
  reason: "this packet declares a TextureRectangle but completed no TMEM load,
           so there is no pending TMEM post-image for it to sample" }
```

This is **not a pixel mismatch**. It is `WgpuRawDpcExecutionError::TexrectWithoutTmemLoad`
(`production.rs:1439-1442`), a CPU-side admission refusal raised by
`execute_raw_dpc` before any draw is recorded. The rasterizer is never reached,
which is exactly why the two implementations agree to the byte.

So the answer to "do they pass under software and fail on Metal?" is **no --
they fail identically on both, and the adapter is irrelevant to why**. The
cause is a stale test, not a driver: both tests were introduced in `f61b5cfe`,
and `TexrectWithoutTmemLoad` was introduced later by the four-commit sequence
`99bde6a3` / `87b2f5b0` / `a19c3ff4` / `3a1a6a73`, which tightened the
admission rule. Each test re-declares its tile binding across a fresh
`execute_raw_dpc` call, but the fixture's TMEM load happened in the *previous*
call, so under the current rule the packet carries a texrect with no load of
its own.

**Nonclaim: this document does not fix them and does not assert which side is
wrong.** Whether the admission rule should accept a published-TMEM texrect, or
the tests should carry their own load, is a behavioral question for the texrect
owner. What is settled here is that it is a CPU-side admission question with no
GPU content -- and that a lane calling them "pre-existing red" from a default
run was reading a test that the default run did not contain.

### The one real software-vs-Metal divergence is correct behavior

`create_requests_a_real_metal_adapter_and_stores_the_triangle_pipeline` passes
on Metal and fails under Lavapipe with:

```
this test qualifies real Metal execution specifically, not merely some adapter
-- got AdapterInfo { name: "llvmpipe (LLVM 22.1.8, 128 bits)",
   device_type: Cpu, backend: Vulkan, ... }
  left: Vulkan
 right: Metal
```

The test asserts `backend == Metal` by name, on purpose (`production.rs:7625`
documents `host-gpu-tests` as "this crate's real-Metal evidence"). Under a
software adapter that assertion is *correctly* false. This is the one row where
the software column must be read as "not applicable on this runner" rather than
as a defect -- and its failure text names the reason, which is the behavior
this document asks for everywhere else.

It also doubles as the proof that the software path really is engaged: the
panic prints `device_type: Cpu, backend: Vulkan`, so the run genuinely
exercised Lavapipe and not a silently-substituted Metal adapter.

**No PIXEL test diverged between Lavapipe and Metal.** All 24 agree. That is a
mildly encouraging signal about the fixtures' tolerance to rasterizer
differences, and it is *not* evidence that the shaders are correct on either.

## Nonclaims

- Software-green is not hardware-green. The 24 PIXEL rows above prove
  Lavapipe's rasterizer agrees with fn64's oracles at these fixtures; they
  prove nothing about Metal, DX12, or any hardware Vulkan driver, and nothing
  about N64 RDP parity.
- No claim that Lavapipe is bit-identical to any hardware rasterizer. It was
  not compared beyond these 33 tests.
- SwiftShader was not tested. It is listed as an untried alternative, not a
  refuted one.
- No behavior change to the renderer. `adapter_selection` is inert unless
  `FN64_WGPU_SOFTWARE_ADAPTER=1` is set; the Metal column of the table is
  unchanged from the pre-change Metal run.
- The two texrect failures are diagnosed, not fixed, and this document takes no
  position on which side of the disagreement is correct.
- No `repr(C)`, size, alignment, or ABI claim is made about any type touched
  here.

## Verification

Measured in this worktree at `a22762f3`, baselines taken before any change in a
second clean worktree at the same commit (not quoted from the brief):

| | Before | After |
|---|---|---|
| Workspace, debug | 8339 passed / 13 skipped | 8342 passed / 13 skipped |
| Workspace, `-C debug-assertions=off` | 8339 passed / 13 skipped | 8342 passed / 13 skipped |
| `fn64-render-wgpu`, default | 4649 | 4652 |
| `fn64-render-wgpu`, `--features host-gpu-tests` | 4682 | 4685 |
| `lint-docs.py` | 1 error, 3 warnings | 1 error, 3 warnings |
| Dead code, lib-only | 1197 | 1197 |
| Dead code, all-targets | 1213 | 1213 |

The `+3` is `adapter_selection`'s own tests. The test count is identical across
debug and release profiles. The `lint-docs` error is
`RT64-WM2000-VALIDATION.md:360`, pre-existing and deliberately not touched.

**Mutation testing**, 3 mutants, **3 kills**, source confirmed byte-identical
after restore:

| Mutant | Killed by |
|---|---|
| `backends_for_request` returns `native` unconditionally (rewrite no-op'd) | `every_mask_collapses_to_vulkan_under_the_software_flag` |
| `assert_expected_adapter` returns before its check (guard disabled) | `create_requests_a_real_metal_adapter_and_stores_the_triangle_pipeline` -- it stops reporting the Vulkan-vs-Metal mismatch |
| flag matches `Ok(_)` instead of `Ok("1")` | `the_flag_reads_exactly_one_and_nothing_else`, run with `FN64_WGPU_SOFTWARE_ADAPTER=nope` |
