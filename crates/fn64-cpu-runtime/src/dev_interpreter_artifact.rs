/// Artifact marker used by compile-and-run integration tests to distinguish
/// the development interpreter build from the production AOT-only build.
///
/// This literal deliberately lives in a feature-gated module instead of a
/// cfg-disabled item in `lib.rs`. Rust metadata can retain tokens belonging to
/// a disabled item; the production build never parses this separate module,
/// so a byte scan cannot mistake that metadata for linked interpreter support.
pub static DEV_INTERPRETER_ARTIFACT_MARKER: &[u8] = b"fn64-cpu-runtime:dev-interpreter:artifact";
