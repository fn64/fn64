# fn64-render-rt64

This crate is fn64's thin adapter over the pinned MIT RT64 renderer. It owns
the only C++ interop boundary in the workspace and implements
`fn64_render::RenderBackend` without exposing RT64 or C++ types to the
runtime, ABI, or recompiler crates.

The deterministic software renderer is a separate package:
`fn64-render-reference`. Microcode admission and other backend-neutral
mechanisms live in `fn64-render`.

## What belongs here

- `Rt64Backend` and the typed Rust-to-C ABI mappings.
- The C++ shim that converts fn64 task, RDRAM, RSP, VI, and policy state to
  RT64's API.
- Build and source-identity checks for the pinned RT64 checkout.
- Native evidence hooks and narrowly scoped behavior fixtures.
- Extended-GBI wire definitions used by the native adapter.

Software display-list decoding, rasterization, TMEM emulation, depth, and VI
filter emulation do not belong here. The production RT64 task path consumes
the immutable `TaskAdmissionPlan` produced by `fn64-render`; it does not
invoke `fn64-render-reference`. The reference crate is a dev-dependency only,
for differential examples.

The boundary is thin in ownership, not a trivial single-function binding.
RT64 exposes task execution, presentation, resizing, runtime policy, texture
replacement, queue synchronization, capture, and evidence surfaces. The Rust
and C++ halves map and validate that complete surface explicitly. Unsafe Rust
is quarantined to `src/ffi.rs`; C++ is quarantined to `ffi/`.

## Feature boundary

The default build is pure Rust and does not configure CMake, link RT64, or
require a GPU. Creating `Rt64Backend` without the `rt64` feature returns a
named `RenderError::Backend`; it never pretends a renderer exists.

Enable the native adapter with:

```sh
cargo build -p fn64-render-rt64 --features rt64
```

By default, the build expects RT64 at
`../no-mercy-recompiled/third_party/rt64`. An allowed pinned checkout can be
selected explicitly:

```sh
FN64_RT64_DIR=/path/to/rt64 cargo build -p fn64-render-rt64 --features rt64
```

`build.rs` verifies the RT64 source identity and MIT license, configures the
crate-local CMake project, and links the static renderer. The GPL mupen64plus
subtree is neither compiled nor linked. On macOS the adapter owns the hidden
SDL/Metal surface currently required by RT64 initialization.

## Runtime behavior

`RenderRuntimePolicy` composes typed user, enhancement, emulator, and texture
replacement settings. Settings that RT64 can change live are applied live;
setup-owned changes return `RestartRequired`. Invalid enum tags, non-finite
numbers, unsupported device values, and failed native updates are named
errors. A failed update invalidates release identity instead of retaining a
stale policy digest.

Geometry tasks are admitted before native entry from immutable RDRAM/RSP
inputs. Entry and self-loaded microcode generations remain ordered and
content-addressed. The schema-v2 plan carries a behavior-bearing identity;
F3DZEX2 requires its exact 2.06H, 2.08I, or 2.08J classifier result and native
preflight checks the corresponding NoN/point-lighting capability without
opening HLE admission. Unknown or incompatible generations return
`FrameStatus::NeedsLle` before interpreter mutation. A native generation
mismatch after execution starts poisons the context and fails loudly. The
adapter snapshots both guest-memory resources before native execution and
publishes them only after the complete task-result schema, plan identity, and
exact FullSync count have been validated. Any other native error restores
RDRAM and RSP byte-for-byte, destroys the unrollbackable RT64 context, and
clears its active release identity; a schema-valid precommit `NeedsLle` must
also prove that neither memory image changed before the context is retained.
Raw RDP submissions use the same RDRAM rollback/context-destruction boundary.

Presentation temporarily lends RT64 the live physical-RDRAM allocation and
the vblank-latched VI image. Native queues are synchronized before the Rust
borrow returns. Resize and capture paths validate workload/present identity so
stale output cannot satisfy release evidence.

## Validation

Fast package checks:

```sh
cargo test -p fn64-render-rt64
cargo test -p fn64-render-rt64 --features rt64
cargo clippy -p fn64-render-rt64 --all-targets --features rt64 -- -D warnings
```

GPU-backed behavior gates and the RT64 public-feature denominator live in
`fn64-certification` and
[`docs/RT64-PUBLIC-FEATURE-INVENTORY.md`](../../docs/RT64-PUBLIC-FEATURE-INVENTORY.md).
The base/silicon frontier is tracked separately in
[`docs/BASE-RENDERER-BEHAVIOR-MATRIX.md`](../../docs/BASE-RENDERER-BEHAVIOR-MATRIX.md).

For read-only native transition diagnostics, set
`FN64_RT64_PRESENT_DIAGNOSTICS=1`. Diagnostic output is not release evidence.
