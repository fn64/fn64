# fn64-certification

This crate owns executable behavioral certification gates. It is a consumer
of renderer APIs, never a renderer implementation or release authority.

The default build is pure Rust and does not compile RT64. Native gates require
the `rt64` feature and the same allowed MIT RT64 checkout used by
`fn64-render-rt64`. Evidence-only features are forwarded explicitly so a
normal game build cannot enable synthetic admission accidentally.

The compatibility examples left in `fn64-render-rt64` include these sources
temporarily so existing commands keep working. New evidence and documentation
must use this package; the wrappers will be removed with the deliberate RT64
adapter-identity v2 transition.

The non-default `f3dzex2-characterization-evidence` feature owns the local
private-input transport for exact F3DZEX2 pairs. Admission fixes the
`fn64.f3dzex2-point-light.v1` suite and its eight policy rows; manifests cannot
choose commands, expected results, or a variant. Repository code expands that
denominator into two public controls plus all six point-light hypotheses at
16-, 24-, and 32-byte candidate transfer widths. Every subcase uses fresh
RDRAM, RSP memory, and native context, exactly one FullSync, guarded synthetic
assets, and an exact task-workload-present association before pixels are
accepted. Adaptive byte-lane probes remain subordinate to the record-boundary
row.

The runner re-executes the admission policy embedded in its binary against a
staged immutable manifest, verifies the exact consumed raw-window bytes against
their descriptors, and emits no private path, digest, content, or native result
identity. No local admitted characterization pair is currently available, so
the suite has not produced a 2.06H/2.08I/2.08J behavioral result. This is
mechanism readiness only; it does not certify point-light semantics or open
production HLE admission.
