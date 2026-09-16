---
name: Owner proof reaches nothing on the recovered NWXE overlay banks
covers: [C3, C5]
reported: 2026-09-15
status: root-caused; recorded expectation is stale, disposition needs an owner call
---
repro: with the PRISTINE (committed) NWXE answer key, run
`fn64-discover gate-owners-overlays` at 26e375e8 and at 42307ab8
expected (as recorded): exact_owners=46 across the four recovered overlay
banks, with block proof reaching them
actual: at 42307ab8 and on main, reached_blocks and proven_executable_bytes
are 0 on ALL FOUR banks and exact_owners collapses 46 -> 0.

|          | reached_blocks        | proven_executable_bytes   | exact_owners |
|----------|-----------------------|---------------------------|--------------|
| 26e375e8 | 4508/790/10401/12615  | 98344/19576/226796/256416 | 46           |
| 42307ab8 | 0/0/0/0               | 0/0/0/0                   | 0            |

## Bisect

The pre-squash history SURVIVES as `origin/agent/static-recomp-wave-checkpoint`
(36 commits; merge-base 26e375e8 == 42307ab8^). `git bisect` with an
exact_owners==46 predicate narrows to `38f74860` / `f060a742` / `dce648c0`.
The middle two are not independently buildable or relevant -- 38f74860's
lib.rs declares modules whose files only arrive in dce648c0, and f060a742
touches only Python tooling -- so the tree-level first bad commit is
`dce648c0` "checkpoint static recomp discovery wave".

## Mechanism (exact, and NOT a discovery or boundary change)

Both revisions produce an IDENTICAL pre-composition fact DB (probed):

  boot                    hardware_entries=1  proven_entries=1
  recovered_overlay_0..3  hardware_entries=0  proven_entries=0

What changed is how `compose_materialized_banks_v1` seeds the closure that
owner proof consumes.

  26e375e8 (snapshot.rs):
      roots = input.seed_roots UNION proven_function_entries(bank)
      closure = build_cfg_value_set_closed(.., &roots)   <- owner proof used this

  main (snapshot/mod.rs ~693):
      traversal_roots = roots                            <- diagnostic only now
      authority_roots = proven_hardware_function_entries(bank)
      semantic = derive_semantic_callable_argument_roots(.., &authority_roots)
      authorized_callable_roots = semantic_callable_root_set(semantic)
      authority_closure = build_cross_bank_authority_closure(
          .., &authorized_callable_roots, ..)

`proven_hardware_function_entries` matches ONLY `CandidateDetector::
HardwareEntrypoint` + `FunctionEntryEvidence::RomHeaderEntrypoint`, which by
construction exists only for the boot bank; and
`derive_semantic_callable_argument_roots` early-returns empty when
`hardware_roots.is_empty()`. So a recovered overlay bank's authority root set
is now unconditionally empty, every assessment reports
`entry_not_authoritative`, and no block is reached.

Composing NWXE boot alone on main still yields 196 owner assessments and
38,700 proven executable bytes from its single hardware entry, so the closure
machinery does run. But the effect is NOT confined to banks that lack a
hardware entrypoint: `gate_b2` shows OoT's boot bank -- which HAS one -- going

  26e375e8  ProgramSnapshot v1: block proof=301/306 blocks (6744 bytes)
            owner proof: exact=32 candidate=10 ambiguous=3 (45 assessed)
  main      ProgramSnapshot v6: block proof=807/807 blocks (16980 bytes)
            owner proof: exact=0  candidate=82 ambiguous=5 (87 assessed)

More blocks are proven and more bytes are executable than before, yet exact
owners fall 32 -> 0, with `unresolved_indirect` the sole blocker on 73 of 85
assessments. So the tightening moved exact-owner admission across the board,
not just on recovered overlay banks, and it traded owner exactness for block
coverage.

## Why this is a tightening, not a bug

The gate's seed roots are SEEDS, not proof. `lib.rs:178` states the principle
outright ("Seeds remain distinct from authority"), the empty-hardware-roots
early return is a deliberate guard, the gate prints `roots=N (proven=M)` with
the two kept apart, and the snapshot schema has since advanced to V6. The old
46 owners rested on treating unproven seed roots as authority -- exactly what
the firewall posture exists to refuse. The recorded 46 is therefore a stale
expectation, not a lost capability.

## Open -- deliberately NOT actioned, needs an owner decision

`expected_owners_overlays` stays UNCHANGED. Re-recording a digest whose
headline result is 0 exact owners retires a published number, which is an
owner call rather than a drive-by. Either:

  (a) accept the tightening and re-record, stating plainly that exact_owners
      went 46 -> 0 and what it now takes to earn authority on a recovered
      bank; or
  (b) give recovered overlay banks a sound authority source. The cross-bank
      rule already exists -- a direct `jal` in proven boot code authorizes the
      overlay entry it targets -- and boot's authority closure is non-empty
      (38,700 bytes), so the question is why no authority-reachable boot call
      lands on an overlay entry. That is the substantive follow-up and is its
      own ticket.

Same-cause collateral, all first-bad at 42307ab8 and held by the same
decision: `gate_asm_roundtrip` fails "owner proof admitted zero exact
functions"; `gate_coverage` and `gate_b2` digests moved.
