---
name: fn64-frontier
description: Measure and interpret the open-indirect ownership frontier (per-ROM diagnosis, corpus sweeps, counterfactual semantics). Use when picking targets or measuring a discovery mechanism's effect.
---

# fn64-frontier

## Measure

One ROM (seconds for most; run release):

```sh
cargo build --release -p fn64-discover --bin diagnose_open_indirects
./target/release/diagnose_open_indirects "<rom>.z64"   # JSON on stdout
```

Corpus sweep (checkpoints every batch; canonical output lives with the
analysis, e.g. the session tmp dir or crates/fn64-discover/reference/):

```sh
source .claude/local.env
python3 scripts/resweep.py out.json --rom-dir "$FN64_ROM_CORPUS_DIR" --workers 4 --timeout 900
```

`--timeout` exists because WWF No Mercy exceeded 900s under a loaded sweep.
If a single ROM diverges wildly from its class (World Tour: 1.5s;
No Mercy: >75 min), do NOT just raise the cap — that is a pathology; use
superpowers:systematic-debugging on the analysis blow-up instead.

## Interpretation rules (each of these has burned an analysis before)

- The JSON has TWO frontiers: `frontier` (all open indirect sites) and
  `owner_proof_frontier` (the subset actually blocking owner promotions).
  Mechanism work targets the second; headlines quoting the first overstate.
- **Counterfactuals are an intersection (AND), not additive.** The
  mechanism families (`load_sites`/`live_in_sites`/`other_local_sites`)
  exhaustively partition sites; `sole_owner_assessments_if_discharged`
  counts owners whose blockers fall ENTIRELY in that family. Low per-family
  numbers with a high `all_open_sites` number means owners need multiple
  families closed together — the unit of progress is the ROM, not the site
  class.
- Rank opportunities by unlockable OWNERS per ROM, never by site
  occurrences.
- `semantic_shapes` names what a resolver must prove:
  `(via_call, kind, base_definition)`. As of 2026-08 the corpus is
  dominated by call-boundary-fed shapes: (call,load,load) 2.6k,
  (call,live_in) 2.0k, (call,load,live_in) 1.7k.
- Baselines drift with every discovery change on main — before comparing
  two sweeps, confirm both came from the same binary rev, and regenerate
  rather than trust a stale JSON (stale sweeps have lied twice).

## Soundness boundary (learned the expensive way)

Enumerating a dispatch table's initial values from image bytes does NOT
close the target universe: the cells are mutable memory, and store-closure
immutability is unprovable in practice (World Tour alone: 45,117
unbounded-address stores). `resolution_from_value` keeps such sites Open by
design. Sound discharge routes: tracked-store provenance (MemoryValueSet),
sltiu-bounded static tables (JumpTable), certified call-boundary seeding
(PR #125), or the certified runtime-observation lane
(`fold_indirect_targets_into_fact_db`) — which proves coverage, never
universes. Do not weaken the static rule to make a number move.
