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
- the span has one unambiguous proven bank-image backing and complete
  proven-executable coverage;
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
- every authority-reachable indirect transfer in the bank is exhaustive, with
  the CFG target set matching one unambiguous exhaustive fact. The public
  owner-proof API has no authority-only closure and therefore conservatively
  checks every indirect in its CFG; snapshot composition supplies its
  separately built authority closure.

The two boundary-attribution rules consume heuristic claims only in the
withholding direction: a candidate claim can prevent an exact claim or
attribute a neighboring boundary, but never itself becomes an owner, and a
rejected claim stops blocking. `prove_exact_owners` therefore takes the
materialized image bytes: the trailing walk inspects words the CFG never
decoded.

The last rule is deliberately stronger than owner-local CFG reachability. An
open computed transfer elsewhere in the authority-reachable bank closure
could enter the proposed span. A site reached only through broad candidate
traversal cannot execute from a proven root and therefore cannot block an
exact owner during snapshot composition; it remains present in the broad CFG
and open-indirect diagnostic. The bank-bound owner-proof capability is built
only from the authority closure, and a missing or wrong-bank capability falls
back to checking the whole broad CFG. A typed exclusion-only target-domain
view admits one narrower case for an authority-reachable site:
a guard-bounded jump table whose complete enumerated target set was rejected
from CFG admission may discharge a bank-scoped blocker when every retained
target is outside the proposed owner extent. The domain never contributes CFG
successors or callable-entry authority. Owner-scoped sites, empty or
unconstrained domains, and initial values read from mutable memory continue to
block; those initial values are not an exhaustive runtime target domain.

Results are a tagged enum. Only `proven` carries an `ExactFunctionOwner` with
a bank-qualified VA extent and `BankBackingSpanV1`. The backing is either an
affine Physical/VROM subspan or an evaluated-image receipt identity with
output-relative offsets; no evaluated output receives fabricated ROM
coordinates. `candidate` and `ambiguous` carry proposed geometry plus sorted,
typed blockers, so a serializer or recompiler cannot accidentally consume a
guessed end address as exact metadata. Missing backing remains a candidate;
competing images and invalid backing geometry are typed ambiguities.

[`fn64-discover::coverage`](../crates/fn64-discover/src/coverage.rs) aggregates
these reports separately from entry and executable-byte coverage. A discovery
report that has not run owner proof says `function_owners.state = not_run`;
zero exact owners is never used to imply that every owner passed. Candidate
and ambiguous assessments contribute no exact bytes, and their typed blockers
remain in the coverage artifact with assessment counts. The strict
`require_all_owners_exact` gate rejects an empty run, any unresolved owner,
duplicate `(bank, entry)` assessments, cross-bank report entries, and malformed
deserialized extents before metadata can reach an emitter.

The content-free corpus diagnostic projects each unresolved assessment to its
sorted, unique set of `OwnerBlockerKind` values and counts exact combinations
separately for candidate and ambiguous assessments in each bank. Full blocker
payloads are excluded because some carry decoded instruction words. The
combination histogram is the dependency measurement: a singleton is the
immediate payoff if one blocker class is discharged, while a multi-kind row
must not be reported as independent wins for its members. Per-kind
`affected_assessments`, site `occurrences`, and `sole_blocker_assessments`
remain separate units, and ROM totals are derived by summing the bank rows.

This proof depends only on fn64's own discovery model and public MIPS-III
control-flow/delay-slot behavior already documented in
[`DISCOVER-DESIGN.md`](DISCOVER-DESIGN.md). No reference-runtime
implementation source is a behavior input.

## Function-independent block proof

`fn64-discover::block_proof` exposes a deliberately smaller
`ReachableCodeBlock` type for the `block_aot` path. A block is admitted only
when at least one independently authoritative partition-claimant root reaches
it, every word is `ProvenCode`, the terminator is neither invalid,
missing-delay, nor ran-off-end, and exactly one proven bank-image subspan backs
its bytes. `ReachableCodeBlock` retains that tagged affine or evaluated-output
subspan. The block carries the sorted nonempty set of authoritative
reachability roots, not a fabricated singular function owner. Competing
function roots do not weaken executable-byte proof; an unowned block or a
block reached only from unauthoritative traversal seeds remains a typed
candidate.

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

Snapshot's two owner passes share a bank-bound owner-proof capability built
from that same authority closure, proven entry facts, and already-vetted
cross-bank roots. It carries both callable entries and authority-reachable
indirect sites. Consequently a direct or exhaustive computed call found only
by broad candidate traversal cannot become same-bank or cross-bank callable
authority after reached executable ranges are derived, and a candidate-only
open indirect cannot withhold exactness from authority-reachable owners.

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

Snapshot wire V6 is the authority-bearing input to block-pack emission. V5 is
retained only as an affine historical schema marker and must be regenerated by
current byte-verifying composition; it is not implicitly upgraded. The Rust
`BlockPackV1` envelope emits schema V3 for admitted blocks. Its authoritative
emitter requires the opaque move-only validated composition; a deserialized
snapshot cannot mint that capability. Each block carries tagged backing,
geometry, a terminator, and a content digest. Re-materialization verifies the
normalized ROM identity, resolves affine Physical/VROM spans or exactly one
matching proven evaluated-image receipt, re-derives evaluated output under
fixed bounds, and verifies every block digest. Decoded bytes remain in memory
and are never serialized in the pack. Legacy V1 physical and V2 affine packs
remain readable; either legacy schema carrying materialized backing is
rejected. Each disjoint span is supplied separately to the sparse arbitrary-
PC emitter. No bounding-range conversion is permitted: holes have no
instruction arm, and transfers into them remain unresolved. The synthetic
gate compiles and executes hole cases; the real NWXE gate additionally
requires its 197-block / 1,039-word emitted runner to compile.

The snapshot-workspace receipt validator accepts tagged V6 backing. The
current `produce_snapshot_workspace` publisher and `stage_snapshot_bank`
external-tool bridge remain explicitly affine-only and reject evaluated
images because their artifact contracts require ROM coordinates.

`block_pack::emit_block_program_source` is the typed executable boundary over
that serialization format. It re-materializes and validates the pack before
emission, requires a bank-qualified entry and instruction budget, binds every
generated runner to the compiling host's artifact identity, and preserves
bank ambiguity as a typed fault. The companion `fn64-discover
emit-block-program` command requires an explicit no-clobber output path because
its generated Rust contains user-owned ROM-derived instruction words.
