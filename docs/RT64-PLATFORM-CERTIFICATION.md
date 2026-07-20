# RT64 cross-platform certification

Generated from `docs/rt64-platform-certification.json` by
`tools/rt64_platform_certification.py`; edit the JSON, not this file.

## Status

**Every platform row remains open.** A build-capability advertisement is not
actual-hardware certification. Blocked and skipped states never count as passes.
The existing eleven-case macOS/Metal evidence is preserved exactly from
`docs/rt64-macos-certification.json`; post-legacy cases carry their own retained
macOS evidence below. No Linux, Windows, Vulkan, or D3D12 hardware result is
inferred from either source.

Pinned RT64 source: `git:f0728a2520d5aa735886240de3fee75cc805f6d6` (`GitClean`).

Preserved macOS host: macOS 26.5 build 25F71; Darwin 25.5.0 arm64; Apple M5 Pro; Metal.

Denominator: The same thirteen fn64 RT64 adapter cases on every supported platform/API target, plus seven platform-wide blockers that remain open until actual-hardware evidence closes them.

## Platform/API targets

| Target | Platform claim | API | Capture | Cases | Blockers | Exact frontier |
|---|---|---|---|---:|---:|---|
| `macos-metal` | `platform-macos` (`open`) | `metal` | `metal-bgra8-unorm` | 13 pass / 0 blocked / 0 skipped | 7 open | Every currently required Metal case retains its applicable 10/20-run evidence, including HFR and Extended-GBI cooperation; user-controls/rebuild has a separate 20-clean-run race-fix result and enhancement/emulator controls retain bounded causal results on the same macOS 26.5 arm64 Apple-M5-Pro host. Both S2DEX controls additionally pass twenty fresh processes with complete load-program and exact pixel/RDRAM evidence. All seven non-shrinking platform blockers remain open pending production full-ROM and wider platform certification. |
| `linux-vulkan` | `platform-linux` (`open`) | `vulkan` | `vulkan-bgra8-rgba8-unorm` | 0 pass / 13 blocked / 0 skipped | 7 open | The backend-neutral post-VI hook and Vulkan image-to-buffer path compile on macOS and retain static ABI/copy/fence tests, but no retained Linux/Vulkan actual-hardware result exists. |
| `windows10-d3d12` | `platform-windows-10` (`open`) | `d3d12` | `d3d12-bgra8-rgba8-unorm` | 0 pass / 13 blocked / 0 skipped | 7 open | The backend-neutral post-VI hook retains a static D3D12 placed-footprint copy/fence seam, but it has not been Windows-compiled and no retained Windows 10/D3D12 actual-hardware result exists. |
| `windows10-vulkan` | `platform-windows-10` (`open`) | `vulkan` | `vulkan-bgra8-rgba8-unorm` | 0 pass / 13 blocked / 0 skipped | 7 open | The backend-neutral post-VI hook and Vulkan image-to-buffer path compile on macOS and retain static ABI/copy/fence tests, but no retained Windows 10/Vulkan actual-hardware result exists. |
| `windows11-d3d12` | `platform-windows-11` (`open`) | `d3d12` | `d3d12-bgra8-rgba8-unorm` | 0 pass / 13 blocked / 0 skipped | 7 open | The backend-neutral post-VI hook retains a static D3D12 placed-footprint copy/fence seam, but it has not been Windows-compiled and no retained Windows 11/D3D12 actual-hardware result exists. |
| `windows11-vulkan` | `platform-windows-11` (`open`) | `vulkan` | `vulkan-bgra8-rgba8-unorm` | 0 pass / 13 blocked / 0 skipped | 7 open | The backend-neutral post-VI hook and Vulkan image-to-buffer path compile on macOS and retain static ABI/copy/fence tests, but no retained Windows 11/Vulkan actual-hardware result exists. |

## Non-shrinking case matrix

| Case | Repeat bar | macOS/Metal | Linux/Vulkan | Win10/D3D12 | Win10/Vulkan | Win11/D3D12 | Win11/Vulkan |
|---|---:|---|---|---|---|---|---|
| `backend-lifecycle` | 20 | pass: 20 clean (2026-07-19) | blocked | blocked | blocked | blocked | blocked |
| `resolution-downsample` | 10 | pass: 10 clean (2026-07-19) | blocked | blocked | blocked | blocked | blocked |
| `user-controls-rebuild` | 20 | pass: 20 clean (2026-07-20) | blocked | blocked | blocked | blocked | blocked |
| `enhancement-emulator-controls` | 10 | pass: 10 clean (2026-07-19) | blocked | blocked | blocked | blocked | blocked |
| `framebuffer-rdram-region` | 10 | pass: 10 clean (2026-07-19) | blocked | blocked | blocked | blocked | blocked |
| `framebuffer-enhancement` | 10 | pass: 10 clean (2026-07-19) | blocked | blocked | blocked | blocked | blocked |
| `texture-replacements` | 10 | pass: 10 clean (2026-07-19) | blocked | blocked | blocked | blocked | blocked |
| `latency-skip-buffering` | 10 | pass: 10 clean (2026-07-19) | blocked | blocked | blocked | blocked | blocked |
| `latency-present-early` | 20 | pass: 20 clean (2026-07-19) | blocked | blocked | blocked | blocked | blocked |
| `deferred-debugger` | 20 | pass: 20 clean (2026-07-19) | blocked | blocked | blocked | blocked | blocked |
| `ubershader-critical-path` | 20 | pass: 20 clean (2026-07-19) | blocked | blocked | blocked | blocked | blocked |
| `hfr-hle-cooperation` | 20 | pass: 20 clean (2026-07-20) | blocked | blocked | blocked | blocked | blocked |
| `extended-gbi-cooperation` | 10 | pass: 10 clean (2026-07-20) | blocked | blocked | blocked | blocked | blocked |

The `user-controls-rebuild` race fix passed twenty consecutive watchdog-bounded
full process exits with exact policy and pixel digests. The bounded failures at
`/tmp/fn64-rt64-control-89452.sample.txt` and
`/tmp/fn64-rt64-control-86646.sample.txt` showed `Application::end` joining a
raster-shader worker after its delayed startup overwrote the destructor's stop
predicate and slept after the only notification. Exact-source overlay
`fn64:raster-shader-start-stop:v1` publishes the predicate before launch and
leaves teardown as its only post-launch writer. The 20 retained run logs are
`/tmp/fn64-rt64-user-controls-overlay-run-{1..20}.log`.

The `enhancement-emulator-controls` pass uses isolated fresh contexts and
hard per-process watchdogs. Its retained note preserves discarded exploratory
capture, cross-profile-contamination, and non-mechanism copy observations, then
binds the final two-workload fixture to exclusive GPU-tile-copy versus ordinary
RDRAM/TMEM-upload paths through a read-only completed-workload seam.

## Non-shrinking blocker denominator

Every target carries all seven blockers below. Removing a case, blocker, or target
fails static validation; closing one requires a retained, integrity-checked result
from matching hardware and does not close any other row.

| Blocker | Related claims | Frontier |
|---|---|---|
| `recognized-hle-and-extended-gbi` | — | Public Extended command behavior is closed through guarded non-ROM HLE-dialect evidence while production hash recognition stays strict. User-owned production-recognized microcode and full-ROM coverage remain incomplete. |
| `aspect-and-generated-frames` | — | Widescreen, ultrawide, and HFR renderer behavior are closed on Metal through public non-ROM HLE-dialect fixtures while production hash recognition stays strict. The HFR gate binds required Extended-v1 negotiation, typed 60 Hz refresh and transform-group cooperation, live Manual 120 Hz selection, source/current workload and present IDs, exact midpoint/endpoint pixels, and post-sleep present-call cadence across twenty fresh processes. This platform blocker remains for equivalent D3D12/Vulkan coverage and physical compositor/scanout timestamps. |
| `remaining-user-controls` | — | Metal now isolates Manual refresh targeting, hardware resolve under MSAA4x, idle work, and developer mode with exact active-policy, post-VI, present, and source-resource continuity through live apply and restoration. Existing cases cover MSAA rebuild, filtering, 2D upscale, display buffering, internal color format, post-blend noise, render-to-RAM, and an exclusive GPU-tile-copy-versus-ordinary-RDRAM/TMEM-upload mechanism. Keep this blocker open for broader mixed-control combinations, recognized-HLE workloads, physical refresh/resolve-path evidence, and every other platform/API target. |
| `remaining-enhancement-controls` | — | Metal has causal evidence for every pinned EnhancementConfiguration field: force-branch, texture-LOD scale, both S2DEX controls, latency, and the remaining scalar controls. The S2DEX bilerp matrix proves point/point and bilerp/bilerp invariance plus the mismatched three-load to exact point two-load transition and restoration. Other platform/API targets retain this blocker until the same bounded behavior suite runs there; this does not close recognized-game cooperation. |
| `inspector-gui` | — | Backend-specific Inspector GUI construction and interaction is not certified across the matrix. |
| `full-adapter-rom-coverage` | `base-rendering-accuracy` | Full-HLE/full-ROM fixed-cycle zero-unsupported coverage is not established. |
| `declared-host-range` | — | Minimum OS versions, architectures, GPU families, and driver ranges are not certified. |

## CI and actual-hardware commands

GPU-free validation and planning:

```sh
python3 tools/rt64_platform_certification.py --check
python3 tools/rt64_platform_certification.py --selftest
python3 tools/rt64_platform_certification.py --list
python3 tools/rt64_platform_certification.py --plan linux-vulkan
python3 tools/rt64_platform_certification.py --verify-result path/to/result.json
```

A matching actual-hardware runner retains one integrity-bound result:

```sh
python3 tools/rt64_platform_certification.py \
  --run macos-metal:backend-lifecycle --gpu 'Apple M5 Pro' \
  --rt64-dir /absolute/path/to/clean/pinned/rt64 \
  --result artifacts/macos-metal-backend-lifecycle.json
```

A shorter `--runs 1` result is `diagnostic-only`. A host/target mismatch is
retained as `skipped`; a matching target whose case is not runnable is retained
as `blocked`. Neither can satisfy a repeat bar. Results bind source identity,
OS product/version/build/kernel, architecture, GPU, graphics API, every process
exit code, run count, status, reason, and a canonical SHA-256. Live execution
requires an explicit RT64 directory; tooling never guesses an out-of-tree path.
Every live case invocation fails if it exceeds the shared 60-second
per-process watchdog.
Skipped/blocked execution exits 2 so CI cannot mistake it for a passing run.
