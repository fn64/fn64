# Corpus re-measurement: where indirect resolution actually pays

Measured 2026-08-01 with `diagnose_open_indirects` over all 287 ROMs after the
`TransferOpcodeMismatch` fix. **284 classified**, 3 refused (Banjo-Tooie,
Donald Duck Goin' Quackers, Rayman 2). Every prior corpus figure excluded 36
ROMs including three graded answer-key games, so these supersede them.

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

The post-fix order uses owner-proof sites, not broad sites:

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
was loaded. These two ROMs are the first load-resolution targets.

DK64 now has 68 exact owners and Banjo-Kazooie Europe has 19 while each broad
frontier still reports one open site. Both sites are candidate-only and require
no resolver. Tetrisphere remains at zero exact owners with 27 owner-proof sites
and a 742-owner counterfactual; Diddy remains at zero with 24 sites and 932.

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

## Reproduce

```
scripts/resweep.py resweep.json      # four workers, checkpoints every 40 ROMs
```

Use `--workers 1` on a memory-constrained host. The output records broad and
owner-proof open-site counts plus realized exact owners, so a scope change can
be measured without deriving promotion from a counterfactual.
