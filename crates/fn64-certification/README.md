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
private-input transport smoke for exact F3DZEX2 pairs. It forwards the
renderer adapter's evidence-only API, re-executes the admission policy embedded
in the compiled runner against a staged immutable manifest, verifies the exact
consumed raw-window bytes against their descriptors, and emits no private
path, digest, or content. This is mechanism evidence only; it does not certify
point-light semantics or open production HLE admission.
