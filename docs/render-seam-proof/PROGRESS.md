# Render seam: progress (2026-07-14)

## Delivered

1. **`fn64-render`** (new crate): the `RenderBackend` trait per
   `docs/DECOUPLING.md` -- `create`/`process_task`/`present`/`resize`/
   `supported_ucodes`, plus `OsTask`, `UcodeId`, `RenderConfig`,
   `FrameStatus`, `RenderError`. Zero backend, zero dependency on any other
   workspace crate (`#![forbid(unsafe_code)]`). 6 tests (dyn-safety,
   not-ready gating, unsupported-ucode trapping, bounds checking).

2. **`fn64-render-rt64`** (renamed from `fn64-rt64` per `DECOUPLING.md` step
   3 -- `git mv`, all references/Cargo.tomls updated):
   - `Rt64Backend`: a named, loud stub. Every method returns a `RenderError`
     naming the exact blocker (fork not vendored; gfx task handoff signature
     unresolved -- `osSpTaskLoad`/`osSpTaskStartGo` still ABSENT per
     `docs/COMPLETENESS.md`). **RT64 FFI is NOT live. Do not read this as
     "RT64 works."**
   - `ReferenceBackend`: a real, headless, pure-Rust software rasterizer.
     `gbi.rs` decodes a small F3DEX2-family display-list subset (`G_VTX`,
     `G_TRI1`/`G_TRI2`, `G_ENDDL` -- public SDK wire encoding); `raster.rs`
     is a textbook barycentric scanline rasterizer into an RGBA8888
     framebuffer; `png_dump.rs` is a from-scratch, dependency-free PNG
     encoder (stored/uncompressed DEFLATE, valid zlib per RFC 1951).
   - `tests/fixture_replay.rs`: builds a hand-constructed (NOT ROM-captured
     -- see that file's doc comment for why) 3-vertex display list, runs it
     through `ReferenceBackend::process_task`, asserts a non-clear frame,
     dumps a PNG. **Green.**

3. **Executor gfx-task seam wired**: `fn64-abi`'s `osSpTaskYielded_recomp`
   now routes `M_GFXTASK` submissions through a registered `dyn
   RenderBackend` (`fn64_abi::set_render_backend`), the same
   thread-local-registration pattern the audio path (`set_audio_ucode_fn`)
   already used. `fn64-abi` depends only on the `fn64-render` trait crate,
   never on a concrete backend (dev-dependency only, for the seam test).
   New test `os_sp_task_yielded_routes_gfx_tasks_through_the_registered_render_backend`
   proves the FULL path: real `extern "C"` shim call -> registered
   `ReferenceBackend` -> real decode+rasterize -> non-clear frame. **Green.**

4. **`fixture_triangle.png`** (this directory): the first non-clear frame
   this project has ever rendered -- a red/green/blue gradient triangle,
   64x64, decoded from a real (if hand-built) F3DEX2 display list. See
   `first_frame: true` in the workflow's structured output.

## Gates (all green, scoped to the crates this wave touched)

- `cargo build -p fn64-render -p fn64-render-rt64 -p fn64-abi -p fn64-shell` — clean.
- `cargo test` (same scope) — 6 + 8 + 2 + 28 + 1 = 45 tests, 0 failures.
- `cargo clippy --all-targets` (same scope) — 0 warnings attributable to
  any file this wave touched.
- `cargo fmt --check` (same scope) — clean after one `cargo fmt` pass.

**Known unrelated noise**: `fn64-runtime/src/mmio.rs` (a concurrent
session's in-flight work, landed mid-wave) has pre-existing
`dead_code`/`identity_op` clippy warnings not touched by this wave -- left
alone, not this crate's scope.

## What's honestly NOT done

- RT64 itself is not vendored, built, or linked. `Rt64Backend` is a stub.
- The gfx task handoff signature (`osSpTaskLoad`/`osSpTaskStartGo`) has
  still not been observed from real generated code in either game's
  corpus -- `fixture_triangle.png` is proof the SEAM works, not proof any
  specific game's real content renders yet.
- `fn64-shell` does not call `set_render_backend` yet (no window/ROM intake
  exists to feed it) -- documented as the concrete next step in
  `fn64-shell/src/main.rs`'s module doc.
- `gbi.rs`'s decoder is intentionally tiny: no matrix stack, no texturing,
  no lighting, no clipping. It exists to prove the seam, not to be a
  faithful F3DEX2 reimplementation.
