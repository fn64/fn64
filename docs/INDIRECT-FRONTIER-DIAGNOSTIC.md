# Indirect-frontier diagnostic

`diagnose_open_indirects` is a content-free opportunity-ranking tool. It runs
automatic discovery and validated snapshot composition, then classifies every
final `Open` indirect site by:

- call versus jump;
- transfer-register family;
- the nearest writer in the site's own basic block;
- for loads, the base-register family and nearest local base writer; and
- whether the resolver retained zero, one, or multiple concrete memory-source
  addresses.

Call versus jump follows the CFG's architectural rule: `jalr` is a call only
when its link register is nonzero. A link-discarding `jalr $zero, $rs` is a
computed jump, distinct from the `jr $rs` encoding but in the same semantic
class.

The tool emits one JSON line per input ROM. Output contains the normalized ROM
digest, internal header name, bank count, assessed-entry and exact-owner counts,
owner-proof open-site count, elapsed time, aggregate broad-frontier
shape counts, and owner-promotion counterfactuals. It does not emit paths, PCs,
instruction words, targets, memory addresses, or ROM bytes.

`semantic_shapes` is the primary workload distribution. It removes transfer-
and base-register allocation choices and groups loads by the base value's local
definition. The more detailed `shapes` rows retain register families and memory
source cardinality for drill-down only.

`owner_proof_frontier` is the owner-proof workload: broad-CFG `Open` sites that
also occur in the authority-only closure. `frontier` is the larger exploratory
workload and retains sites reached only from candidate roots. Both carry the
same shape and mechanism-counterfactual schema, so mechanism ranking can use
the owner-proof distribution without discarding the broad discovery census.
Their open-site difference measures sites that remain useful diagnostic
evidence but cannot execute from a proven root and therefore cannot withhold
exact ownership. This is deliberately an intersection, not the number of
`Open` states in the authority closure: candidate context can make the broad
value analysis resolve a site that remains open in the smaller authority
analysis. `exact_owners` reports realized promotion directly; do not infer it
from a counterfactual.

For a batch, an input rejected before classification is reported on standard
error and does not suppress later inputs. The process exits nonzero after the
batch if any input failed, so a partial census cannot be mistaken for a
complete one.

Run it on one or more uncompressed ROM images:

```sh
cargo run -p fn64-discover --bin diagnose_open_indirects -- ROM [ROM ...]
```

## Interpretation boundary

The local writer is not an inter-block reaching-definition proof. `live_in`
means only that the transfer register has no writer between the basic-block
start and the transfer. Shape counts describe the unresolved-site
distribution; they do not measure expected owner promotion. Do not rank the
allocator-sensitive `shapes` rows as if they were distinct mechanisms.

`mechanism_counterfactuals` closes that accounting loop. For each named shape
family, `sole_owner_assessments_if_discharged` counts an assessment only when
it has no non-indirect blocker and every unresolved-indirect blocker it carries
is an `Open` site in that family. This is the mechanism-family analogue of
`sole_blocker_assessments`; `sites` is not a substitute for it. Families
overlap and are reported independently.

The counterfactual assumes complete discharge of every matching site. It does
not claim that such a resolver exists or that resolving a subset has the same
payoff. `Bounded` indirect sites are outside this diagnostic, so they can keep
an assessment from appearing even when all of its `Open` sites match.

One run is diagnostic evidence only. Apply the `AGENTS.md` repetition bar
before making a determinism claim.
