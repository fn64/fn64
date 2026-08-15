# Prepared WM shard source tree

Status: **verifier-ready candidate; inactive**. The 35 shard manifests still
use the current ROM-driven shared build script. The cold verifier now owns the
producer build and complete prepared projection, but no build-time or
recompilation speedup is claimed until manifest activation and its benchmarks.

The prepared tree is the Stage B boundary between one ROM-wide discovery and
35 independent Rust shard builds. The inactive one-shot producer and legacy
build now share one source generator: the producer can run discovery and code
generation once, outside the shard Cargo graph, while the legacy build selects
one package from that same measured `build.rs` implementation. The producer
streams one generated shard at a time into a unique mode-0700 sibling staging
tree, writes files with mode 0600, syncs it, then uses macOS's no-replace rename
(`renameatx_np`) or Linux's `renameat2(RENAME_NOREPLACE)` to atomically publish
the complete tree at an absent destination. It never
retains all generated sources in memory. Each future prepared shard build will
only validate its package sidecar and copy its two source files. It must never
fall back to discovery or code generation: such a fallback would silently
recreate the invalidation and memory problem. Publication fails closed on
platforms where an atomic no-replace directory rename is not implemented.
If a retry finds all 105 package files byte-identical, it may atomically replace
only `manifest.v2`; this retains the exact prepared-root environment value and
all watched artifact paths while updating authority claims. A serialized
artifact update at the same root creates private `.update.v2`, atomically
replaces only differing `runner.rs`/`metadata.rs` files, commits that package's
`identity.v1` last, and commits `manifest.v2` after all packages. Unchanged
files retain their inode and modification time. A rerun with the same target
manifest resumes any interrupted prefix; a different target fails until the
recorded update is recovered.

## Canonical v2 format

`FN64_WM_PREPARED_SHARD_ROOT` names a non-symlink directory outside git. The
producer requires an explicit absolute output path whose complete existing
component chain is non-symlink and whose parent is outside the repository:

```text
<root>/manifest.v2
<root>/<package>/identity.v1
<root>/<package>/runner.rs
<root>/<package>/metadata.rs
```

`manifest.v2` is UTF-8, LF-terminated, contains no blank or trailing lines,
and has this exact order:

```text
schema fn64.wm-prepared-shard-tree.v2
normalized_rom_sha256 <lowercase nonzero SHA-256>
generator_source_sha256 <lowercase nonzero SHA-256>
discovery_source_sha256 <lowercase nonzero SHA-256>
emitter_source_sha256 <lowercase nonzero SHA-256>
runtime_source_sha256 <lowercase nonzero SHA-256>
artifact_count 35
artifact <package> <identity.v1 SHA-256> <runner.rs SHA-256> <metadata.rs SHA-256>
```

Each `identity.v1` is independently canonical and package-specific:

```text
schema fn64.wm-prepared-shard-artifact.v1
package <exact package>
runner_sha256 <lowercase nonzero SHA-256>
metadata_sha256 <lowercase nonzero SHA-256>
```

The 35 artifact lines use the exact sorted package inventory in
`recomps/wm2000/packages/wm2000-block-shards/materializer.rs`. Extra, missing, duplicated, or
reordered lines fail closed. Each root artifact line cross-binds the exact
sidecar and both files; the sidecar repeats the two artifact digests and denies
extra or reordered fields. The manifest and sidecars contain identities only.
The rest of the private tree is ROM-derived game content: `metadata.rs` includes guest
instruction words encoded as Rust literals. The prepared tree and all
generated sources are therefore forbidden from git, logs, and receipts; only
digests and non-content geometry may leave the private boundary.

The std-only materializer rejects unknown packages, malformed or mismatched
selected-package sidecars, zero or mismatched digests, symlink roots/package
directories/artifacts, and non-regular artifacts. It watches only that
package's `identity.v1`, `runner.rs`, and `metadata.rs`; it deliberately neither
reads nor watches the root manifest. It copies only the selected package's two
sources into `OUT_DIR`, writing them only when content changes. A root
source-claim change can therefore replace only `manifest.v2` at the stable root
without changing a Cargo-watched path. A serialized same-root artifact update
replaces only the changed source and its owning sidecar, but byte locality alone
is not a Cargo invalidation result: activation still requires a
compiler-artifact benchmark proving the path/fingerprint behavior. The
materializer does not execute another process and has no discovery, codegen,
hashing-crate, or serialization dependency.

Producer update and Cargo consumption are explicitly serialized. While
`.update.v2` exists, every materializer fails by name before accepting the
projection and checks again before and after copying. This closes every
documented crash prefix under the verifier-owned single-producer contract; it
is not authority for concurrent Cargo and producer execution.

## Authority boundary and activation gates

Manifest identities are producer claims, not authority. Generated-build v3
implements the inactive cold-verifier path: it builds the producer with frozen
metadata in a guarded private target, stages and hashes the exact producer
binary, invokes it against the staged ROM, and independently measures the
normalized ROM, generator/discovery/emitter/runtime/materializer sources,
producer manifest/lock/source graph, exact root/package topology, directory and
file modes, 105 package files, sidecars, root manifest, and complete-tree and
descriptor digests. It remeasures the retained projection before Cargo, after
Cargo, and after the identity child, then retains the stable root and producer
inside the move-only build capability.

The v3 identity states its source mode explicitly. Today the only repository
mode is `legacy_with_prepared_candidate`: the selected binary's existing Cargo
source attestation remains authoritative for the legacy shard build, while the
prepared tree is a separately bound candidate and is not represented as a
compiled source. A future all-35-manifest switch derives `prepared_consumed`
only from the exact manifest inventory and changes the measured shard source
domain from `build.rs` to `prepared_build.rs` plus `materializer.rs`. Mixed,
missing, or extra manifest inventories fail closed. A warm shard build or
successful materialization remains diagnostic only.

The producer requires explicit nonzero lowercase SHA-256 claims for the
generator, discovery, emitter, and runtime source domains. It does not mint or
infer source authority. Its only standard output is the schema, normalized-ROM
digest, and prepared-manifest digest; it never prints ROM paths, output paths,
or generated content. The published root and package directories remain mode
0700 and every manifest/source file remains mode 0600. An exact existing tree makes a retry
idempotently succeed. A partial, extra, symlinked, or different destination
fails without replacement.

The parity, invalidation, and single-shard profile entrypoints default to one
Cargo job, a 2048 MiB aggregate process-group RSS ceiling, and a 40% system-free
floor. The generated-build v5 verifier keeps producer compilation and
publication serial, but binds the selected-runner build to exactly two Cargo
jobs under one fixed 4096 MiB / 40% owned-build envelope. Both `-j2` and
`CARGO_BUILD_JOBS=2` are set after clearing the child environment, and the job
count is part of the verifier-owned build authority digest; ambient overrides
cannot weaken it. The v4 attempt reached every shard and the root before the
old 2048 MiB guard killed the group at 2050 MiB with 77% system memory free;
the 4096 MiB cap covers the separately measured 3194 MiB full-graph peak. No
v5 authority receipt exists until the verifier completes. Run
`scripts/lint-compiler-memory-safety.py --selftest` to sweep these defaults and
reject unguarded Cargo compiler commands in the prepared-shard production paths.

Activating `prepared_build.rs` in the 35 manifests requires all of the
following in the same migration:

1. Real-ROM byte parity between every legacy one-package output and the shared
   producer's corresponding output, plus atomic-publication negative evidence.
2. An independently measured cold-verifier input for every source-identity
   field and every prepared artifact; the verifier must not trust values merely
   because they appear in `manifest.v2`.
3. Exact pre- and post-build tree enumeration so extra files, symlink swaps,
   or mutations fail closed.
4. The implemented generated-build v5 schema/capability binding the complete
   tree and producer identities must pass its guarded cold-build exercise.
5. Guarded synthetic, real-ROM cold, warm-invalidation, and negative tests.

Until those authority inputs are specified and implemented, the current build
path remains active. The prepared materializer is deliberately not selectable
by a hidden fallback or ambient auto-detection.
