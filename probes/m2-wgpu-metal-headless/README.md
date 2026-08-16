# M2 wgpu Metal headless capability probe

`metal_caps` is the macOS/Metal half of the fn64 renderer port's M2 feasibility
gate. It is a standalone workspace and intentionally does not share the
shell's `pixels`/wgpu 0.19 dependency graph. This probe pins wgpu 30.0.0 with
only its `metal` and `wgsl` crate features.

Run an observational probe:

```sh
cargo run --locked -- --iterations 1
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

Exit status is stable: 0 means the probe executed successfully (including an
observational run that reports unsupported requirements), 69 means no Metal
adapter was available, 78 means `--require` found an advertised requirement
missing, and 2 means device execution or validation failed.

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
