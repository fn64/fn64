# RT64 macOS certification

This is generated from `docs/rt64-macos-certification.json`. Edit the JSON and
run `python3 tools/rt64_macos_certification.py --write-doc`.

## Status

**The platform-wide `platform-macos` claim remains open.** The cases below
are closed feature-specific evidence, not a substitute for the unresolved
adapter, enhancement, GUI, full-ROM, or host-range denominator.

Denominator: Complete fn64 RT64 adapter and public enhancement behavior on a declared macOS version, architecture, and GPU support range.

Pinned RT64: `git:f0728a2520d5aa735886240de3fee75cc805f6d6` (`GitClean`); capture API: `metal-bgra8-unorm`.

Recorded host: macOS 26.5 build 25F71; Darwin 25.5.0 arm64; Apple M5 Pro.

## Feature-specific live Metal cases

| Case | Category | Example | Repeat bar | Recorded result | Closed claims |
|---|---|---|---:|---|---|
| `backend-lifecycle` | backend | [​`rt64_metal_backend_behavior`](../crates/fn64-certification/examples/rt64_metal_backend_behavior.rs) | 20 | 20 clean (2026-07-21) | `backend-metal` |
| `resolution-downsample` | resolution | [​`rt64_resolution_downsample_behavior`](../crates/fn64-certification/examples/rt64_resolution_downsample_behavior.rs) | 10 | 10 clean (2026-07-19) | `high-resolution-renderer`, `downsample-to-original-like` |
| `framebuffer-rdram-region` | framebuffer | [​`rt64_framebuffer_rdram_region_behavior`](../crates/fn64-certification/examples/rt64_framebuffer_rdram_region_behavior.rs) | 10 | 10 clean (2026-07-19) | `native-renderer-rdram-sync`, `framebuffer-detection-region-copy` |
| `framebuffer-enhancement` | framebuffer | [​`rt64_framebuffer_enhancement_behavior`](../crates/fn64-certification/examples/rt64_framebuffer_enhancement_behavior.rs) | 10 | 10 clean (2026-07-19) | `framebuffer-upscaling`, `framebuffer-reinterpretation` |
| `texture-replacements` | textures | [​`rt64_texture_replacement_behavior`](../crates/fn64-certification/examples/rt64_texture_replacement_behavior.rs) | 10 | 10 clean (2026-07-19) | `texture-pack-dds`, `texture-pack-rice-filenames`, `texture-pack-async-streaming` |
| `latency-skip-buffering` | latency | [​`rt64_latency_skip_buffering_behavior`](../crates/fn64-certification/examples/rt64_latency_skip_buffering_behavior.rs) | 10 | 10 clean (2026-07-19) | `latency-skip-buffering` |
| `latency-present-early` | latency | [​`rt64_latency_present_early_behavior`](../crates/fn64-certification/examples/rt64_latency_present_early_behavior.rs) | 20 | 20 clean (2026-07-19) | `latency-present-early` |
| `deferred-debugger` | inspection | [​`rt64_deferred_debugger_behavior`](../crates/fn64-certification/examples/rt64_deferred_debugger_behavior.rs) | 20 | 20 clean (2026-07-19) | `deferred-frame-history`, `debugger-frame-inspection` |
| `ubershader-critical-path` | pipelines | [​`rt64_ubershader_pipeline_behavior`](../crates/fn64-certification/examples/rt64_ubershader_pipeline_behavior.rs) | 20 | 20 clean (2026-07-19) | `ubershader-no-pipeline-stutter` |
| `hfr-hle-cooperation` | generated-frames | [​`rt64_hfr_interpolation_behavior`](../crates/fn64-certification/examples/rt64_hfr_interpolation_behavior.rs) | 20 | 20 clean (2026-07-21) | `hfr-60-plus-interpolation` |
| `extended-gbi-cooperation` | extended-gbi | [​`rt64_extended_gbi_enhancement_behavior`](../crates/fn64-certification/examples/rt64_extended_gbi_enhancement_behavior.rs) | 10 | 10 clean (2026-07-20) | `extended-gbi` |

A manifest result is record evidence only when its run count meets the case's
repeat bar. A shorter runner invocation is labeled `diagnostic-only` even when
every invocation exits successfully. Unavailable and skipped cases are errors.

## Open platform denominator

| Blocker | Related open claims | Exact frontier |
|---|---|---|
| `recognized-hle-and-extended-gbi` | — | The public Extended command-set behavior is closed by guarded non-ROM HLE-dialect gates with strict production-recognition negative controls. A user-owned production-recognized microcode/full-ROM run remains required for release certification and is not inferred from synthetic admission. |
| `aspect-and-generated-frames` | — | The public HFR renderer-API row is closed by twenty fresh Metal processes that bind a required Extended-v1 handshake, typed 60 Hz refresh and transform-group cooperation, a live Manual-120-Hz transition, workload/present identities, exact midpoint/endpoint pixels, and post-sleep present-call cadence while production recognition stays strict. macOS platform certification remains open because API-call return is not a physical compositor/scanout timestamp. |
| `remaining-user-controls` | — | The expanded user-control gate isolates Manual refresh targeting, hardware resolve under MSAA4x, idle work, and developer mode with exact active-policy, post-VI, present, and source-resource continuity through live apply and restoration. The blocker remains open for broader mixed-control combinations, recognized-HLE workloads, and physical refresh/resolve-path evidence. |
| `remaining-enhancement-controls` | — | Every individual pinned enhancement control now has bounded causal Metal evidence, including a twenty-fresh-process S2DEX bilerp predicate matrix and framebuffer-fast-path differential. The non-shrinking platform blocker remains until those controls and combinations are certified through supported recognized HLE tasks rather than only isolated synthetic fixtures. |
| `metal-inspector-gui` | — | The backend-independent debugger host API is certified, but pinned RT64's ImGui Inspector constructor supports D3D12/Vulkan only and asserts for Metal. |
| `full-adapter-rom-coverage` | `base-rendering-accuracy` | The live suite is synthetic and predominantly raw-DPC. A twenty-phase native Metal gate binds exact nondefault post-VI pixels, causal/restorable gamma and seeded gamma-dither, coverage-gated horizontal divot, and DITHER_FILTER RGBA16 restoration. Divot changes twelve exact componentwise-median pixels and restores exactly. Restoration changes eighteen exact full-coverage pixels, leaves twenty-four non-full controls and six flat full-coverage controls byte-identical, preserves alpha, and restores exactly when disabled. A separate adapter-capture integration test plus eleven-phase gate preserve hardware mode 0 versus compatibility Unspecified across the wire and native callback, makes modes 0/1 match an independent Figure-11 coverage-four oracle at RGB [132,78,99], restores modes 2/3 exactly, and proves AA-before-divot order. The context-reuse gate asserts recreated and compatibility identities 1/1 and 1/2, filter workload 2 with presents 3 through 22, and selector workload 3 with presents 23 through 33; the changed overlay and setup-registration path passed the official watchdog-bounded lifecycle runner in 20/20 fresh Darwin 25.5.0 arm64 processes on 2026-07-21. Evidence is limited to pinned-Metal nearest/progressive synthetic RGBA16 and deliberately generated code 4 with opaque code-7 controls; pinned RT64's code-7 alias, untested partial codes, code-0/save, natural/imported hidden coverage, wider filtering/scaling lattices, MSAA/downsample, other APIs, full-ROM, silicon, and analog behavior remain uncertified. Complete HLE microcode, fixed-cycle device, and zero-unsupported coverage are not established on macOS. |
| `declared-host-range` | — | Evidence currently names one macOS 26.5 arm64 Apple-M5-Pro host; the minimum macOS version, architecture set, GPU families, and CI support range are not declared and certified. |

## Validation and execution

```sh
python3 tools/rt64_macos_certification.py --check
python3 tools/rt64_macos_certification.py --list
python3 tools/rt64_macos_certification.py --run backend-lifecycle
python3 tools/rt64_macos_certification.py --run backend-lifecycle --runs 1
```

The first run command uses the manifest repeat bar. The second is deliberately
diagnostic-only. `--run all` executes every case at its own repeat bar unless
`--runs` supplies a common diagnostic or repeat count. Execution requires
Darwin and an exact, clean pinned RT64 source tree selected by `FN64_RT64_DIR`
or the default sibling checkout. Every live case invocation fails if it exceeds
the shared 60-second per-process watchdog. Fresh
processes are spaced by 10 seconds so WindowServer
can reclaim each hidden Metal surface before the next case invocation.
