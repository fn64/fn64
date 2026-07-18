# Exact function-owner proof

Status: implemented conservative proof boundary, 2026-07-17.

`fn64-discover::partition` computes useful candidate ownership from CFG
reachability. Candidate ownership is not emitter-ready metadata. The
`owner_proof` pass exposes an `ExactFunctionOwner` only when all of these are
mechanically established:

- the bank has a proven load-image identity;
- the entry is authoritative: a proven function-entry fact, direct call from
  proven code, or exhaustive computed call;
- the owner's blocks form one aligned, gap-free span;
- each instruction and every transfer delay slot is proven code;
- the span has one unambiguous ROM backing and complete proven-executable
  coverage;
- no competing root, overlapping owner, foreign incoming edge, observed
  interior entry, or interior callable root exists;
- no path runs off the decoded image; and
- every indirect transfer in the bank is exhaustive, with the CFG target set
  matching one unambiguous exhaustive fact.

The last rule is deliberately stronger than ordinary CFG reachability. An
open computed transfer elsewhere in the active bank could enter the proposed
span. The current fact model cannot express an exhaustively excluded target
domain, so the pass retains every such owner as `candidate`. A future bounded
target-domain fact may discharge that blocker for unrelated owners without
requiring global indirect closure.

Results are a tagged enum. Only `proven` carries an `ExactFunctionOwner` with
bank-qualified VA and ROM extents. `candidate` and `ambiguous` carry proposed
geometry plus sorted, typed blockers, so a serializer or recompiler cannot
accidentally consume a guessed end address as exact metadata.

[`fn64-discover::coverage`](../crates/fn64-discover/src/coverage.rs) aggregates
these reports separately from entry and executable-byte coverage. A discovery
report that has not run owner proof says `function_owners.state = not_run`;
zero exact owners is never used to imply that every owner passed. Candidate
and ambiguous assessments contribute no exact bytes, and their typed blockers
remain in the coverage artifact with assessment counts. The strict
`require_all_owners_exact` gate rejects an empty run, any unresolved owner,
duplicate `(bank, entry)` assessments, cross-bank report entries, and malformed
deserialized extents before metadata can reach an emitter.

This proof depends only on fn64's own discovery model and public MIPS-III
control-flow/delay-slot behavior already documented in
[`DISCOVER-DESIGN.md`](DISCOVER-DESIGN.md). No reference-runtime
implementation source is a behavior input.

## Function-independent block proof

`fn64-discover::block_proof` exposes a deliberately smaller
`ReachableCodeBlock` type for the `block_aot` path. A block is admitted only
when its partition owner has an authoritative entry, every word is
`ProvenCode`, the terminator is neither invalid, missing-delay, nor
ran-off-end, and exactly one proven ROM mapping backs its bytes. Ambiguous or
unowned blocks and unauthoritative traversal seeds remain typed candidates.

This does not manufacture an executable section boundary and does not produce
an `ExactFunctionOwner`. It proves only the bank-qualified block bytes and
their control exit. Thus local data, padding, split assembly, and uncertain
historical function boundaries can block function AOT without blocking an
already-reached code block. Unresolved indirect exits remain visible for the
bank-qualified dispatcher; block proof does not claim their target set is
closed.

`BlockPackV1` is the serialization boundary for admitted blocks. It carries
only bank identity, geometry, terminators, and content digests. Re-
materialization verifies the normalized ROM identity and every block digest,
then supplies each disjoint span separately to the sparse arbitrary-PC
emitter. No bounding-range conversion is permitted: holes have no instruction
arm, and transfers into them remain unresolved. The synthetic gate compiles
and executes hole cases; the real NWXE gate additionally requires its
197-block / 1,039-word emitted runner to compile.
