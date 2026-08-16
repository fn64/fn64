# In-flight wave status

This inventory describes the uncommitted working tree captured on 2026-07-31.
It covers only this wave. Private ROMs, capture receipts, and the disposable
ares checkout are not commit candidates.

“Verified” means the named gate actually ran against this tree. Deterministic
claims meet the `AGENTS.md` ten-run bar only where a 10/10 count is stated.

## Whole-program forward/reverse transfer index

**FILES:** `crates/fn64-discover/src/program_transfer_index.rs`; the module
exports in `crates/fn64-discover/src/lib.rs`; the migration note in
`docs/DISCOVER-STORAGE.md`.

**STATE:** Done as an interim in-memory view. It indexes authority-rooted
intra-bank CFG edges in both directions and exact cross-bank direct or
exhaustively-resolved calls. Exact-owner projections expose callers, callees,
source blocks, and call sites. It does not claim to be the persistent CSR
database specified by `docs/DISCOVER-STORAGE.md`.

**VERIFICATION:**

- `cargo test -q -p fn64-discover --lib program_transfer_index --
  --test-threads=1`: 10 consecutive clean runs; five tests per run.
- `cargo test -p fn64-discover --lib -- --test-threads=2`: one clean run;
  778 passed and two ignored.
- `cargo fmt --all -- --check`: clean.
- `git diff --check`: clean.
- `scripts/lint-docs.py`: run, not green. It reports the 30-second NMR
  checker timeout, stale `docs/BASE-RENDERER-BEHAVIOR-MATRIX.md`, and the
  existing `semantic.rs:241` unsupported-recorder bypass.
- Not run: ten full-package runs, full workspace tests, clippy, nextest ABI,
  lane parity, a persistent-index benchmark, or a production consumer profile.

**FRONTIER:** Cross-bank jumps are absent because composition retains typed
cross-bank call facts but no equivalent jump-authority record. The omission is
explicit in `program_transfer_index.rs`; the test
`cross_bank_jump_is_omitted_without_typed_jump_authority` fixes that boundary.
Construction now pre-indexes exact call authority by bank/site/target/class,
so it no longer linearly scans a source CFG per cross-bank call. No profile
shows whether the remaining `Vec`/`BTreeMap` rebuild cost is material.

**COMMIT SAFETY:** Safe to commit independently with its `lib.rs` export and
`docs/DISCOVER-STORAGE.md` paragraph. It does not depend on the TLB diagnostic.

## Static boot-TLB write and alias diagnostics

**FILES:** `crates/fn64-discover/src/{resolve.rs,boot_tlb_alias.rs}`;
`crates/fn64-discover/src/bin/fn64_discover.rs`; the related module export in
`crates/fn64-discover/src/lib.rs`; `crates/fn64-cpu-runtime/src/{runtime.rs,lib.rs}`;
`docs/DESIGN.md`; `tools/mupen-trace/README.md`.

**STATE:** Done as a diagnostic-only mechanism. The analyzer carries COP0 TLB
writes through the CFG, samples EntryHi/ASID after the transfer delay slot,
and derives only aliases intersecting independently proven physical backing.
The CLI does not mint `RomMapping` facts. Production translation retains loud
traps for invalid PageMask encodings and undefined multiple matches.

**VERIFICATION:**

- `cargo test -q -p fn64-discover --lib boot_tlb_alias --
  --test-threads=1`: 10 consecutive clean runs.
- `cargo test -q -p fn64-discover --lib tlb -- --test-threads=1`: 10
  consecutive clean runs.
- `cargo test -q -p fn64-cpu-runtime --lib
  instruction_translation_diagnostic -- --test-threads=1`: 10 consecutive
  clean runs.
- `cargo check -p fn64-discover --bin fn64-discover`: one clean run.
- `cargo test -p fn64-discover --lib -- --test-threads=2`: one clean run;
  778 passed and two ignored.
- `cargo test -p fn64-cpu-runtime -- --test-threads=2`: one clean run; 372
  passed and one ignored across unit, integration, and doc-test targets.
- `cargo fmt --all -- --check`: clean.
- `git diff --check`: clean.
- `scripts/lint-docs.py`: run, not green, with the same three repository-wide
  failures recorded above.
- Not run: ten full-package runs, full workspace tests, clippy, nextest ABI,
  lane parity, or a production discovery-delta gate.

**FRONTIER:** GoldenEye's diagnostic transfer at `0x800004b4` targets
`0x70000510`; Perfect Dark's at `0x8000109c` targets `0x700016cc`. Each
path-invariant indexed write describes a one-MiB alias of proven boot backing,
but both retain `InitialTlbStateUnproven { known_entries: 1 }`. The actual
instruction-bytes to CFG to analyzer to alias regression proves the blocker is
not bypassed. No production mapping was admitted and no cold score changed.

**COMMIT SAFETY:** Safe with the runtime diagnostic translator and the two
docs named above. It is independent of the transfer index except for adjacent
`lib.rs` module-export lines.

## Initial boot-TLB certificate / ares authority route

**FILES:** No certificate code remains in the working tree. The negative result
is recorded in `docs/DESIGN.md`; `tools/mupen-trace/README.md` records why the
public mupen debugger's opaque `void *` TLB pointer cannot be cast into an
architectural wire. Disposable evidence remains under
`/private/tmp/fn64-ares-tlb-spike` and private mode-0600 receipt directories.

**STATE:** Abandoned. Review found that the proposed certificate accepted a
free-form producer string and could let fabricated JSON erase the initial-state
blocker. That wire, binder, exports, CLI flags, and tests were removed.

**VERIFICATION:** The disposable ares checkout at upstream `b80f67d3` built
successfully after one compile-only API correction. One GoldenEye capture and
one Perfect Dark capture each reached the exact header entry with 32 indexed
records. Both successful-TLBWI/TLBWR-since-power masks were `0x00000000`, and
all captured entries were zero. The capture patch passes `git diff --check`;
its private patch SHA is retained outside the repository. These were one-shot
diagnostics, not ten-run deterministic gates. No hardware capture ran.

**FRONTIER:** Ares initializes the TLB to zero, while its write mask proves that
neither boot overwrote any entry before game entry. The captured values are
therefore emulator policy, not hardware initial-state evidence. The public
mupen API supplies no versioned entry layout. Those two producer routes were
ruled out by build output and the two receipts; hardware initial state remains
unproved.

**COMMIT SAFETY:** Only the negative-result documentation is committed with the
diagnostic. The disposable ares patch and receipts must remain outside git.

| thrust | state | verified? | blocking? | safe to commit alone? |
|---|---|---|---|---|
| Forward/reverse transfer index | done, interim view | focused 10/10; package once | no; cross-bank jumps are an explicit omitted scope | yes |
| Static boot-TLB diagnostic | done, diagnostic only | focused 10/10; packages once | no production admission; initial state remains open | yes, with runtime translator |
| Certificate / ares authority | abandoned | two one-shot captures; no hardware gate | yes for TLB-backed production mapping | no code remains; docs travel with diagnostic |
