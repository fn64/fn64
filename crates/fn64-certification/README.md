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
