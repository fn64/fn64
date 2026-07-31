# Exact function-owner proof

Status: implemented conservative proof boundary, 2026-07-17; boundary
attribution rules and reached-code executable derivation, 2026-07-18.

`fn64-discover::partition` computes useful candidate ownership from CFG
reachability. Candidate ownership is not emitter-ready metadata. The
`owner_proof` pass exposes an `ExactFunctionOwner` only when all of these are
mechanically established:

- the bank has a proven load-image identity;
- the entry is authoritative: a proven function-entry fact, direct call from
  proven code, or exhaustive computed call. Multi-bank composition carries
  both call forms across bank boundaries only when the source instruction and
  delay slot are proven code; a computed call additionally requires one
  unambiguous exhaustive analysis whose target set exactly matches the CFG;
- the owner's blocks form one aligned, gap-free span;
- each instruction and every transfer delay slot is proven code;
- the span has one unambiguous ROM backing and complete proven-executable
  coverage;
- no competing root, overlapping owner, foreign incoming edge, observed
  interior entry, or interior callable root exists;
- no unrefuted candidate/supported function-entry claim lies strictly inside
  the span (`interior_candidate_entry`): the span may cover multiple
  historical functions, e.g. fallthrough after a call to a non-returning
  callee smearing into the next function's prologue, until the claim is
  proven or rejected;
- the bytes immediately after the span are attributed
  (`trailing_unattributed_code` otherwise): the walk from the proposed end
  must reach proven code, a function-entry claim site, or the image end
  crossing only zero padding. Unreached non-zero words there are plausible
  code no mechanism owns; NWXE measurement found byte-identical trailing
  `jr $ra; nop` neighborhoods whose ground-truth attribution differs (a
  dead tail of the previous function at `0x80031810`, a standalone stub
  function at `0x80036770`), so no content rule can decide the boundary and
  exactness is withheld;
- no path runs off the decoded image; and
- every indirect transfer in the bank is exhaustive, with the CFG target set
  matching one unambiguous exhaustive fact.

The two boundary-attribution rules consume heuristic claims only in the
withholding direction: a candidate claim can prevent an exact claim or
attribute a neighboring boundary, but never itself becomes an owner, and a
rejected claim stops blocking. `prove_exact_owners` therefore takes the
materialized image bytes: the trailing walk inspects words the CFG never
decoded.

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
when at least one independently authoritative partition-claimant root reaches
it, every word is `ProvenCode`, the terminator is neither invalid,
missing-delay, nor ran-off-end, and exactly one proven ROM mapping backs its
bytes. The block carries the sorted nonempty set of authoritative reachability
roots, not a fabricated singular function owner. Competing function roots do
not weaken executable-byte proof; an unowned block or a block reached only
from unauthoritative traversal seeds remains a typed candidate.

Snapshot composition also projects reachability from the separately built
authority-only CFG onto broad exploratory blocks. The projection is
crate-private, derives its roots only from the authority closure's own
partition, and retains a broad successor only when authority contains the
exact same source site, destination, and transfer kind. The sole compatible
refinement is a consecutive plain fallthrough wholly inside the ordinary prefix
of one authority block; it cannot cross a control/terminal boundary. A bank-end
fallthrough is instead an exact typed `ran_off_end_fallthrough` authority edge.
Before broad traversal, a non-authoritative root that lands on an authority
delay slot is omitted from the traversal set while its candidate fact remains
recorded. An exact transfer to an ordinary instruction at that address is a
typed delay-entry alias: direct entry executes that word and continues at the
next word, while the predecessor CFG block retains its inseparable control and
delay pair. Call-derived aliases carry callable authority; branch and tail
aliases carry reachability only. A control-shaped delay entry remains rejected.
In this mode only
projected roots count: broad candidate-owner roots cannot feed back into
executable authority. Proving the block does not promote its candidate
function owner.

Snapshot's two owner passes share a bank-bound callable-entry capability built
from that same authority closure, proven entry facts, and already-vetted
cross-bank roots. Consequently a direct or exhaustive computed call found only
by broad candidate traversal cannot become same-bank or cross-bank callable
authority after reached executable ranges are derived.

Cross-bank direct and exhaustive-resolved calls advance one monotone authority
fixed point only when an exact target VA identifies one target bank. A VA in
several overlapping generations confers neither reachability nor semantic
callback-argument authority: choosing any of those bytes requires a future
typed activation-compatibility capability. Unique late-wave roots can still
expose another exact direct or exhaustive-resolved call in the next wave.
Authority records remain uniquely counted under the composition cap and
ordered canonically.
Bank names are the fixed-point identity key, so multi-bank composition rejects
duplicate input names before preparing or cloning bank state.

A target also contained by the source bank is not projected onto overlapping
sibling banks: runtime generation segments can replace only part of an image,
so VA containment alone proves neither that execution stays in the source
generation nor that a sibling is active at the target. This is a deliberate
completeness loss until composition has typed activation-compatibility
evidence; choosing a sibling by address alone would be unsound.

This does not manufacture an executable section boundary and does not produce
an `ExactFunctionOwner`. It proves only the bank-qualified block bytes, their
authoritative reachability, and their control exit. Thus local data, padding,
split assembly, and uncertain historical function boundaries can block
function AOT without blocking an already-reached code block. Unresolved
indirect exits remain visible for the bank-qualified dispatcher; block proof
does not claim their target set is closed.

## Reached-code executable derivation

`block_proof::conclude_reached_executable_ranges` turns the proven reached
blocks into typed, evidence-carrying `ExecutableRange` facts (conclusion rule
`reached_proven_code_closure`): a word proven reachable by CFG closure from an
authoritative entry is demonstrably executed under the proven mapping, so it
is proven executable. Exactly the reached bytes are claimed — adjacent proven
blocks merge into one interval, gaps between reached blocks are never
bridged, and region scores or content statistics play no role (a
score-threshold promotion rule was measured and rejected; see
`DISCOVER-PLAN.md`). A range subject already `Rejected`/`Conflict` is not
silently promoted; the new reachability evidence is recorded and the
conclusion surfaces as `Conflict`. `snapshot` composition runs this
derivation between an authority-only owner pass and the final owner pass, so
the former `not_proven_executable` blocker is discharged exactly where an
assessment's full extent lies inside reached proven code.

The Rust `BlockPackV1` type's schema-v2 wire is the serialization boundary for
admitted blocks. Its authoritative emitter requires the opaque move-only
validated composition produced by byte-verifying snapshot construction; a
deserialized snapshot cannot mint that capability. The pack carries only bank
identity, geometry, ROM address space, terminators, and content digests. Re-
materialization verifies the normalized ROM identity and every block digest,
then supplies each disjoint span separately to the sparse arbitrary-PC
emitter. No bounding-range conversion is permitted: holes have no instruction
arm, and transfers into them remain unresolved. The synthetic gate compiles
and executes hole cases; the real NWXE gate additionally requires its
197-block / 1,039-word emitted runner to compile.

`block_pack::emit_block_program_source` is the typed executable boundary over
that serialization format. It re-materializes and validates the pack before
emission, requires a bank-qualified entry and instruction budget, binds every
generated runner to the compiling host's artifact identity, and preserves
bank ambiguity as a typed fault. The companion `fn64-discover
emit-block-program` command requires an explicit no-clobber output path because
its generated Rust contains user-owned ROM-derived instruction words.
