# M2 wgpu Metal headless capability probe

`metal_caps` is the macOS/Metal half of the fn64 renderer port's M2 feasibility
gate. It is a standalone workspace and intentionally does not share the
shell's `pixels`/wgpu 0.19 dependency graph. This probe pins wgpu 30.0.0 with
only its `metal` and `wgsl` crate features.

The same locked standalone workspace also contains four execution probes. They
remain separate binaries because an adapter advertisement is not evidence that
the corresponding shader or format path executed:

- `metal_semantics` runs exact `u32` mask/shift/wrapping arithmetic and a
  bounded 4 KiB packed TMEM-like address calculation. Its divergent storage-
  buffer binding-array arm runs only when all three required array/indexing
  features are advertised. Its offscreen dual-source blend runs only when the
  feature and target usages are advertised. The packed-buffer binding and
  integer manual-blend fallbacks always run and must produce the same fixed
  expected bytes as their native counterparts. The blend vector uses distinct
  fractional factors for every channel and requires the exact rounded RGBA8
  result rather than a 0/255 selector case.
- `metal_formats_io` uploads two rows of big-endian guest RGBA5551 sentinel
  bytes through an `R8Uint` texture, explicitly converts them to
  `Rgba8Unorm`, copies the result through a 256-byte-row staging buffer, and
  compares only the logical row bytes. It checks every required advertised
  usage and storage access before creating the native textures. A packed-buffer
  conversion fallback always runs. A validation error scope must reject an
  unlisted `R32Uint` view of an `Rgba8Unorm` texture. Staging geometry appears
  only in a typed `passed` outcome after copy, map, padded-row extraction, and
  logical-byte validation complete; unsupported or failed native paths cannot
  fabricate successful staging evidence.
- `metal_submission` prewarms one raster ubershader, two finite raster PSOs,
  and one compute pipeline before moving the device behind an armed execution
  interface that exposes no pipeline-construction operation. The consumed
  pipeline factory issues the typed arm witness; its zero is scoped to
  application-level pipeline creation through the probe owner and says nothing
  about hidden driver compilation. The probe submits three command buffers to
  the one device queue. Each command buffer owns its callback, each returned
  `SubmissionIndex` is waited exactly, and a nonblocking receive observes
  callback delivery after that wait. Separately, the native wgpu-core contract
  for [`wgpu-types` 30.0.0 `PollType::Wait`](https://docs.rs/wgpu-types/30.0.0/wgpu_types/enum.PollType.html#variant.Wait),
  also present in the pinned crate's `src/lib.rs`, guarantees callback
  invocation before the wait returns. Submission two is not enqueued until
  callback one is observed ready. The second command buffer
  executes compute, render, texture-to-buffer copy, and—when the base
  `TIMESTAMP_QUERY` feature is advertised—four descriptor-bound pass
  timestamps. Only after its exact wait and callback-ready check does the third
  command buffer resolve and copy those queries, following wgpu's documented
  portable query-availability ordering. Exact output bytes, callback/wait
  order, zero post-arm application pipeline creation, nonzero ordered
  timestamp ticks, and a positive timestamp period are separate typed receipt
  fields. This is lifecycle and execution evidence, not a frame-time or
  throughput measurement.
- `metal_coverage_fallback` evaluates the public eight N64 checkerboard sample
  positions `(1,1), (5,1), (3,3), (7,3), (1,5), (5,5), (3,7), (7,7)` in
  eighth-pixel units in a compute shader. Twelve all/none, axis, single-corner,
  and complementary-diagonal edge fixtures are graded against independently
  fixed masks. The positions and checkerboard topology follow Ultra64
  Programming Manual Chapter 15, “Coverage Values,” as recorded in
  [`docs/RDP-SILICON-VECTORS.md`](../../docs/RDP-SILICON-VECTORS.md) and the
  existing reference-renderer contract. This is only the documented
  sample-mask primitive; it does not claim the full top-left/coverage pipeline
  or unpublished silicon edge-accumulator arithmetic.
  A separate four-sample hardware-MSAA render/resolve arm is always labeled
  `executed_non_exact` (or typed optional unsupported): host MSAA positions are
  never substituted for the exact documented sample-mask primitive.

Run an observational probe:

```sh
cargo run --locked -- --iterations 1
```

Run the semantic and format execution probes:

```sh
cargo run --locked --bin metal_semantics
cargo run --locked --bin metal_formats_io
```

Run the submission/timestamp and coverage-fallback probes:

```sh
cargo run --locked --bin metal_submission
cargo run --locked --bin metal_coverage_fallback
```

Require the whole candidate RT64 hard-capability matrix:

```sh
cargo run --locked -- --require --iterations 1
```

The program writes one compact canonical JSON receipt to stdout. It never
records the executable path, source path, user name, home directory, host name,
or ambient command line. `binary_sha256` identifies the exact executable and
`source_sha256` covers the manifest, lockfile, build script, Rust sources, and
this README. `canonical_sha256` covers the canonical receipt before that
digest field is inserted. Machine receipts are evidence outputs and must
remain out of git.

Limit rows bind the exact `wgpu::Limits::default()` minimums used for device
creation as well as the unbucketed adapter values; they do not invent a larger
renderer-specific threshold that the current M2 contract has not established.

For `metal_caps`, exit status remains stable: 0 means the probe executed
successfully (including an observational run that reports unsupported
requirements), 69 means no Metal adapter was available, 78 means `--require`
found an advertised requirement missing, and 2 means device execution or
validation failed.

For `metal_semantics` and `metal_formats_io`, 0 means every native and fallback
subtest executed and matched exact bytes, 2 means a semantic comparison,
required fallback, device operation, or validation expectation failed, 69
means no Metal adapter was available, and 78 means at least one native subtest
was explicitly unsupported after all applicable native subtests and all
fallbacks succeeded. Native support is decided from the adapter's advertised
features/usages first and then separately proven by execution; advertisement
alone never produces a passing arm.

For `metal_submission`, 0 means an exhaustive positive predicate accepted the
exact prewarm counts, exact lifecycle events, compute/render/copy bytes, zero
errors, and `Passed` timestamps paired with an advertised timestamp feature.
Exit 78 means that same core predicate passed while the feature was not
advertised and the timestamp arm was explicitly `Unsupported`. Every
`NotRun`, failure, mismatch, inexact passed receipt, or advertisement/outcome
contradiction exits 2. `metal_coverage_fallback` exits 0 only when an exact
`CoverageArm::Passed`, zero errors, and either
`HardwareMsaaArm::ExecutedNonExact` or its typed optional unsupported outcome
all satisfy their positive predicates. Every other arm exits 2. Missing
optional hardware MSAA does not weaken or mark unsupported the exact compute
fallback. Both binaries use exit 69 for no Metal adapter.

Each binary writes one typed, compact, canonical JSON receipt. The execution
schemas include only canonical command/probe identity, source and executable
digests, path-free adapter/device/driver identity, target and exact rustc
release/commit identity, boolean advertisements, typed arm outcomes, and byte
counts/digests. The submission and coverage receipts additionally serialize
their complete static probe configuration. Hostile tests mutate that
configuration, callback order, timestamp values, coverage masks, and readback
geometry to prove those changes cannot retain a passing receipt. Receipts
exclude paths, user/home/host names, ambient arguments, and raw backend error
text. Receipts and Cargo target output are machine evidence and must not be
committed.

## wgpu 30 API audit and result boundaries

The implementation was written against the downloaded official wgpu 30.0.0
and wgpu-types 30.0.0 crate sources. Material differences from this
repository's inherited wgpu 0.19 surface include:

- `InstanceDescriptor` is constructed by value and includes `display`, memory
  thresholds, and backend options; the probe selects only `Backends::METAL`.
- `request_adapter` returns `Result<Adapter, RequestAdapterError>` rather than
  an `Option`, adds explicit limit-bucketing control, and `request_device`
  takes one `DeviceDescriptor` argument.
- `DeviceDescriptor` now names experimental features, memory hints, and a
  `Trace` value. Error scopes are `ErrorScopeGuard` values whose `pop` method
  returns the future.
- `Device::poll` takes `PollType`, including a timeout and optional submission
  index. The old `Maintain` spelling is not used, and mapped-range acquisition
  is now fallible.
- Storage-texture read-only, write-only, and read-write support is reported in
  `TextureFormatFeatureFlags`, separately from the `STORAGE_BINDING` usage.

The feature, limit, and format rows are adapter advertisements. The semantic
section is deliberately separate: it proves device creation plus an exact GPU
buffer copy/readback for every requested iteration while capturing validation
and uncaptured errors. It does not claim that every advertised feature has
been exercised. Storage read-only and write-only access are hard rows; the
matrix still records read-write support, but does not require in-place storage
texture mutation because the port can preserve those transitions with
separate source and destination bindings. Encoder- and pass-interior timestamp
writes are enumerated but marked optional because base `TIMESTAMP_QUERY`
supports descriptor-bound pass timestamps; absence of the more permissive
write locations is not a hard renderer requirement. M2's integration lead
owns the portable/fallback/blocked classification; this probe supplies a
candidate platform observation, not a renderer parity or performance claim.
