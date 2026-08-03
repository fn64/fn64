---
name: rust-module-split
description: Split an oversized Rust source file into modules behavior-preservingly. Trap checklist distilled from splitting 47 files (~205k lines) across this workspace.
---

# rust-module-split

Target shape: `file.rs` -> `file/mod.rs` + concern submodules, every
resulting file under the repo's line limit INCLUDING tests. Behavior-
preserving means: no public API, error-enum text, or serialized-digest
change; external `use crate::file::X` keeps working via mod.rs re-exports.
For multi-file campaigns, drive it with
superpowers:subagent-driven-development (one implementer per file, tests
as the reviewer's gate).

## Pre-flight (do ALL before moving anything)

1. **Self-hashing manifests**: `grep -rn 'include_bytes!\|include_str!\|include!(' <crate>/src/`
   Some crates embed their own sources for build receipts
   (fn64-recomp-rs and fn64-recomp-rs-codegen lib.rs). Splitting a listed
   file REQUIRES enumerating the new paths in the manifest in the same
   commit. Also: coverage TESTS may hardcode path lists
   (rt64 `adapter_source_identity`) even when the hash walker recurses.
2. **`include!`/`include_str!` are file-relative** — every such path in a
   moved file gains one `../` per directory level.
3. **Guard tests that read source**: `include_str!("lib.rs")`-style tests
   must keep pointing at the file that still contains their target region.
4. **Test module layout**: a trailing `#[cfg(test)] mod tests` is the cheap
   win (extract to `file/tests.rs` with `use super::*;`). Mid-file test
   modules, multiple `#[cfg(test)]` attributes on items, and production
   code AFTER the test module all mean a concern split, not a tail split.
5. **Doc couplings (this repo)**: after splitting under
   crates/fn64-render-reference run
   `python3 tools/check_base_renderer_matrix.py --write-doc`; after moving
   fn64-abi shims run `python3 scripts/check-nmr-surface.py --write-doc`;
   always finish with `python3 scripts/lint-docs.py`. CI fails on stale
   file:line citations even when tests are green.

## Mechanics that bite

- Extracting across a NEW module boundary needs visibility bumps on:
  top-level items, struct fields (INCLUDING tuple fields), inherent-impl
  methods, thread_local statics — but NOT trait-impl methods (E0449).
  Child modules see parent private items, so parent-owned state can stay
  private if children reach it via `super`.
- `macro_rules!` used across submodules must be defined in the parent
  BEFORE the child `mod` declarations.
- `derive(Ord)/PartialOrd` variant order can be load-bearing (sorts).
  Move type definitions byte-for-byte.
- Test bodies contain multi-line raw strings whose leading whitespace is
  content — never blind-dedent; brace-counting must ignore braces inside
  string literals and match the module's own closing brace (nested
  `mod x {}` inside tests defeats "last brace" heuristics).
- Doc comments + attributes travel WITH their item; a cut between
  `/// ...` and its `fn` produces "expected item after attributes".
- Integration tests: a child `mod` of `tests/foo.rs` resolves into
  `tests/`, where cargo would treat the file as a new test target — use
  `#[path = "foo/child.rs"] mod child;`.
- One split at a time, `cargo nextest run -p <crate>` between each; the
  pass count must not drop. Do NOT trust `cargo fix` on files with
  cfg-gated imports or glob re-export chains — it removes imports the
  other cfg needs; rustc's unused-import lint is self-contradictory across
  `use super::*` chains (check accepts what fix removes, and the removal
  breaks the build via silently-rebinding pattern constants).
- Commit with explicit pathspecs when any other agent shares the index.
