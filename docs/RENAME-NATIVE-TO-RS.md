# Rename plan: retire ambiguous "native" -> "-rs" (Rust) / "recompiled" (role)

`native` currently means THREE unrelated things (313 occurrences). The `-rs`
suffix is the Rust-ecosystem convention for "the Rust implementation of X"
(serde-rs, tree-sitter-rs, ...). This plan renames per-MEANING — a blind
`s/native/rs/` would corrupt the byte-order sense.

## Sense C — DO NOT TOUCH (different meaning entirely)
`native-endian`, `native_endian`, `from_ne`, "native-endian-word layout" —
these are BYTE ORDER, unrelated to the recompiler. 15 files. Leave verbatim.
The rename passes below are scoped to exact identifiers, never a bare `native`.

## Sense A — the Rust recompiler (crate/tool/types) -> `-rs` / `rs`
The from-scratch Rust MIPS->Rust recompiler (vs N64Recomp's C output).
| old | new |
|---|---|
| crate `fn64-recomp-native` | `fn64-recomp-rs` |
| module path `fn64_recomp_native` | `fn64_recomp_rs` |
| `NativeRecompiler` (struct) | `RsRecompiler` |
| dir `crates/fn64-recomp-native/` | `crates/fn64-recomp-rs/` |
Internal types INSIDE that crate that say "Native" but mean "the recompiler's":
prefer dropping the qualifier since the crate name already says it:
`NativeContext` -> `RecompContext` (if not already), `NativeFunc` -> `RecompFunc`
(already exists in fn64-abi — reconcile), `NativeLookup` -> `Lookup`.

## Sense B — the recompiled-game LANE (vs the C-file lane)
The runtime path that runs fn64's Rust-recompiled game module (vs N64Recomp C).
Here "native" really means "the recompiled game code" -> use **recompiled**,
and for the lane SELECTOR name both lanes by tech.
| old | new |
|---|---|
| env `FN64_NATIVE_RECOMP=1` | `FN64_RECOMP=rs` (value; C lane = `FN64_RECOMP=c`) |
| env `NATIVE_RECOMPILED_DIR` | `RECOMP_RS_DIR` |
| env `NATIVE_RECOMP_PROFILE` | `RECOMP_RS_PROFILE` |
| crate `oot-native-funcs` | `oot-recompiled` |
| `mod native_funcs` (oot-boot) | `mod recompiled` |
| `cfg(fn64_native_recomp)` | `cfg(fn64_recomp_rs)` |
| `NATIVE_SECTION_GEOMETRY` | `RECOMPILED_SECTION_GEOMETRY` |
| `native_rdram_len`, `native_symbols`, `native_dir` | `recompiled_*` |

## Sense D — runtime "host vs native" dispatch
`call_host_or_native` means "call the host shim OR the recompiled fn" — the
real axis is host-vs-recompiled. -> `call_host_or_recompiled`,
`native_host_lookup` -> `recompiled_or_host_lookup`, `native_lookup` ->
`recompiled_lookup`, `register_native_section` -> `register_recompiled_section`,
`pause_active_native_thread` -> `pause_active_recompiled_thread`.

## Render backend (separate, name by tech not "native")
`NativeBackend` (the FUTURE pure-Rust renderer) -> `WgpuBackend` (it will be
wgpu-based). Not yet built, so this is just fixing the plan docs.

## Execution
1. `git mv` the crate dirs; update workspace members + all path deps.
2. Per-sense sed of the EXACT tokens above (never bare `native`), file-by-file.
3. Rename env vars in the `oot` runner, build.rs, native manifest, docs.
4. Gate: `cargo nextest run --workspace` green + `cargo build --workspace` +
   a C-lane + rs-lane smoke boot. Rename is behavior-preserving; any test
   change = a missed reference.
Do it as ONE reviewable mechanical pass; the `-rs` convention makes it durable.
