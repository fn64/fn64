# Corpus re-measurement: where indirect resolution actually pays

Measured 2026-08-01 with `diagnose_open_indirects` over all 287 ROMs after the
`TransferOpcodeMismatch` fix. At that time 284 classified, 3 refused
(Banjo-Tooie, Donald Duck Goin' Quackers, Rayman 2). Every prior corpus figure
excluded 36 ROMs including three graded answer-key games, so these superseded
them.

**Update 2026-08-15:** PR #119 (merged 2026-08-03) fixed the classifier that
refused those 3 ROMs. Re-swept 2026-08-15 with the same tool: **287/287
classified, 0 errors.** Banjo-Tooie now resolves to 31 exact owners with 1
remaining candidate-only site (same shape as DK64/Banjo-Kazooie — no payoff
left). Rayman 2 and Donald Duck: Goin' Quackers both classify with real
owner-proof frontiers (45 and 41 sites) but **zero counterfactual payoff at
either ROM** — every open site there is a shared/multi-owner blocker, not a
sole-owner site, so neither is a load-resolution target despite the large
site counts. The "3 refused" framing below is resolved; the target-order
table is refreshed to the current sweep. Other tables/prose in this doc are
the original 2026-08-01 measurement and have not been rechecked line-by-line
against the new run (some drift is expected — see `## Reproduce`).

Counterfactual owner counts below are
`sole_owner_assessments_if_discharged` — the sole-blocker metric, not
entries-affected. Realized `exact_owners` are identified separately.

## Headline

**The baseline counterfactual reported 47,526 unlockable owners across 180 of
284 ROMs.** This is not a post-fix forecast: it counted every broad-CFG open
site as an owner blocker, including sites reached only from candidate roots.

The authority-scope remeasurement assessed **395,749 entries** and produced
**594 exact owners across 40 ROMs**. It separates two workloads:

| workload | open sites |
|---|---:|
| broad exploratory CFG | 28,786 |
| broad-open and authority-reachable | **7,285** |
| candidate-only difference | **21,501 (74.7%)** |

Candidate-only sites remain in the discovery census but cannot execute from a
proven root and no longer withhold exact ownership. Thirty-seven ROMs with no
remaining owner-proof open site produce 583 exact owners; three ROMs with a
remaining owner-proof frontier produce another 11.

The earlier corpus at commit `1b1d145` produced 2 exact owners from 391,188
assessments. That is not a clean A/B baseline for all 592 additional owners:
the current tree also includes later overlay recovery. Of the 29 current
exact-owner ROMs present in the old indirect target list, 28 match their prior
all-open sole-blocker payoff exactly, totaling 470 owners. This agreement is
evidence that the scope correction realized the measured payoff, but it is not
a substitute for a same-tree control run.

The earlier reading — "resolving every indirect site unlocks nothing" — came
from a five-ROM spot check that landed on outliers. Mega Man 64 (0 owners from
886 sites) and WWF No Mercy (0 from 82) are real, but they are 2 of the 104
zero-unlock ROMs, not the pattern.

## The original bank-scope reading was wrong

| | ROMs | owners unlockable | open sites |
|---|---|---|---|
| single-bank | 255 | **42,956 (90%)** | 23,267 |
| multi-bank | 29 | 4,570 (10%) | 5,421 |

That table measured bank count, not authority reachability. Single-bank ROMs
still contain broad candidate roots that cannot execute from a proven root.
The post-fix intersection removes 74.7% of broad-open sites from owner proof,
so the single-bank/multi-bank split did not isolate the binding scope rule.

## Partial progress remains clustered

Across the post-fix owner-proof frontier, complete discharge of all 7,285 sites
has a 67,518-owner counterfactual. Individual families expose much less:

| family | owner-proof sites | sole-owner payoff |
|---|---:|---:|
| all open sites | 7,285 | 67,518 |
| loads | 5,220 | 4,532 |
| live-in values | 2,030 | 113 |
| other local definitions | 35 | 25 |
| loads with retained memory sources | 695 | 19 |

Families overlap; these rows must not be summed.

Owners are blocked by many sites at once, so the payoff lands only when a
bank's *last* open site closes. Resolve-per-site progress metrics will read as
flat until a ROM completes — plan and measure per ROM, not per site.

## Recommended target order

**Update 2026-08-15:** the 2026-08-01 table below was drawn from a 5-ROM spot
check, not the full corpus. The 2026-08-15 resweep computed the same
`sole_owner_assessments_if_discharged` (`all_open_sites` mechanism) for all
287 ROMs; 230 have nonzero payoff. Top 15 by owner payoff:

| owners | owner-proof sites | broad sites | ROM |
|---:|---:|---:|---|
| 1,099 | 41 | 82 | Xena - Warrior Princess: The Talisman of Fate |
| 1,042 | 37 | 61 | Wayne Gretzky's 3D Hockey '98 |
| 923 | 15 | 38 | Diddy Kong Racing |
| 898 | 45 | 104 | Hydro Thunder |
| 816 | 22 | 37 | Rampage: World Tour |
| 808 | 55 | 115 | Virtual Pool 64 |
| 780 | 37 | 68 | Wayne Gretzky's 3D Hockey |
| 750 | 17 | 36 | Nightmare Creatures |
| 747 | 38 | 92 | Rampage 2: Universal Tour |
| 740 | 18 | 27 | Tetrisphere |
| 708 | 20 | 40 | Mission Impossible |
| 692 | 73 | 139 | Gex 3: Deep Cover Gecko |
| 683 | 37 | 121 | Bio F.R.E.A.K.S. |
| 665 | 59 | 123 | Tigger's Honey Hunt |
| 631 | 20 | 47 | Fox Sports College Hoops '99 |

Diddy's counterfactual grew from 932 (5-ROM spot check, 24 sites) to 923 across
the full sweep at a lower site count (15) — small deltas between spot check
and full sweep are expected from tree drift, not a regression. Tetrisphere
(740/18) and Wave Race/Pilotwings remain in-family but no longer top the
table once measured against the full 287, not a 5-ROM sample.

The original 2026-08-01 spot-check table, for reference:

| owners | owner-proof sites | broad sites | ROM |
|---:|---:|---:|---|
| 264 | 4 | 35 | Wave Race 64 |
| 90 | 2 | 51 | Pilotwings 64 |
| 355 | 8 | 40 | International Superstar Soccer '98 |
| 342 | 8 | 21 | Bottom of the 9th |
| 932 | 24 | 47 | Diddy Kong Racing |
| 35 | 1 | 78 | Pokemon Stadium 2 (France) |
| 742 | 27 | 36 | Tetrisphere |

Wave Race's four sites are all loads: two computed calls through a load whose
base was loaded, and two computed jumps through a load whose base was formed
by add. Pilotwings' two sites are both computed calls through a load whose base
was loaded. Both remain cheap load-resolution targets (small site counts) even
though they don't lead the full-corpus table by raw payoff.

DK64 now has 68 exact owners and Banjo-Kazooie Europe has 19 while each broad
frontier still reports one open site. Both sites are candidate-only and require
no resolver. Banjo-Tooie matches this same pattern post-fix: 31 exact owners
realized, 1 remaining candidate-only site, 0 further payoff. Tetrisphere
remains at zero exact owners with 18 owner-proof sites (full sweep) and a
740-owner counterfactual; Diddy remains at zero with 15 sites and 923.

Rayman 2 (45 owner-proof sites) and Donald Duck: Goin' Quackers (41 sites) are
now classifiable but both carry **zero** sole-owner payoff — their open sites
are all shared blockers. They belong in the discovery census, not the
resolver target list.

DK64's remaining broad site has shape `{via_call: false, local_definition:
{kind: live_in}}` — a jump through a register defined outside the function.
That shape covers 874 broad-frontier sites across 276 ROMs, but the DK64 result
shows semantic shape alone does not establish resolver priority; authority
reachability must be measured first.

## Owner-proof semantic shape distribution

The broad distribution remains available in `semantic_shapes`. Resolver
priority uses `owner_proof_semantic_shapes`: 17 shapes over 7,285 sites, with
the first three covering 86.8%.

| sites | share | shape |
|---:|---:|---|
| 2,641 | 36.3% | `via_call`, load of a load |
| 2,010 | 27.6% | `via_call`, live-in |
| 1,670 | 22.9% | `via_call`, load of live-in |
| 306 | 4.2% | `via_call`, load of immediate |
| 276 | 3.8% | `via_call`, load of add |
| 202 | 2.8% | jump, load of add |
| 109 | 1.5% | `via_call`, load of register copy |

`via_call: true` accounts for 96.7% of owner-proof sites.

**Update 2026-08-15 (full 287-ROM sweep):** 6,688 owner-proof sites, 66,983
total sole-owner counterfactual payoff, 628 exact owners already realized —
consistent with the 2026-08-01 spot-check headline (7,285 sites / 67,518
payoff) within tree drift. Shape mix is essentially unchanged:

| sites | share | shape |
|---:|---:|---|
| 2,624 | 39.2% | `via_call`, load of a load |
| 1,680 | 25.1% | `via_call`, live-in |
| 1,382 | 20.7% | `via_call`, live-in (no load) |
| 310 | 4.6% | `via_call`, load of immediate |
| 286 | 4.3% | `via_call`, load of add |
| 220 | 3.3% | jump, load of add |
| 112 | 1.7% | `via_call`, load of register copy |

## Reproduce

```
scripts/resweep.py resweep.json --rom-dir <corpus-dir>   # four workers, checkpoints every 40 ROMs
```

Use `--workers 1` on a memory-constrained host. The output records broad and
owner-proof open-site counts plus realized exact owners, so a scope change can
be measured without deriving promotion from a counterfactual. Last run:
2026-08-15, 287/287 classified, 0 errors, 666s wall time, 4 workers.
