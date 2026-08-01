# Corpus re-measurement: where indirect resolution actually pays

Measured 2026-08-01 with `diagnose_open_indirects` over all 287 ROMs after the
`TransferOpcodeMismatch` fix. **284 classified**, 3 refused (Banjo-Tooie,
Donald Duck Goin' Quackers, Rayman 2). Every prior corpus figure excluded 36
ROMs including three graded answer-key games, so these supersede them.

All owner counts below are `sole_owner_assessments_if_discharged` — the
sole-blocker metric, not entries-affected. Each is reported once per ROM.

## Headline

**47,526 owners are unlockable across 180 of 284 ROMs.**

The earlier reading — "resolving every indirect site unlocks nothing" — came
from a five-ROM spot check that landed on outliers. Mega Man 64 (0 owners from
886 sites) and WWF No Mercy (0 from 82) are real, but they are 2 of the 104
zero-unlock ROMs, not the pattern.

## The scope rule is not the binding constraint for most of the corpus

| | ROMs | owners unlockable | open sites |
|---|---|---|---|
| single-bank | 255 | **42,956 (90%)** | 23,267 |
| multi-bank | 29 | 4,570 (10%) | 5,421 |

In a single-bank ROM, bank scope and owner scope coincide — there is no wider
bank to over-block. Yet 160 of those 255 ROMs still show unlock potential. So
for 90% of the value the blocker is the **resolver**, not the scope rule.

The scope fix in `validate_indirects` remains correct and worth doing, but it
addresses the 29 multi-bank ROMs holding 10% of the value — and the two ROMs
that motivated it are among the 9 multi-bank ROMs where it measurably unlocks
nothing.

## Site count does not predict payoff

`corr(open_sites, owners_unlocked) = -0.023` over 255 single-bank ROMs.

| sites/ROM | ROMs | owners | sites | owners/site |
|---|---|---|---|---|
| 1–25 | 41 | 955 | 295 | 3.2 |
| 26–50 | 83 | 16,461 | 3,263 | **5.0** |
| 51–100 | 100 | 20,380 | 7,082 | 2.9 |
| 101–250 | 43 | 8,046 | 6,249 | 1.3 |
| 250+ | 17 | 1,684 | 11,799 | **0.1** |

**ROMs with ≤100 open sites hold 79% of unlockable owners while carrying 37%
of the sites.** The 17 largest frontiers carry 41% of the sites and return
0.1 owners each. Ranking work by frontier size targets the worst end.

Worms Armageddon: 1,983 sites, 0 owners. Donkey Kong 64: **1 site, 68 owners**
(verified live, not from cache).

## Partial progress promotes almost nothing

Summed over single-bank ROMs, discharging `all_open_sites` unlocks 42,956
owners; discharging every individual sub-mechanism unlocks **175**. A 245×
gap, and only 10 of 255 ROMs have any sub-mechanism unlock at all.

Owners are blocked by many sites at once, so the payoff lands only when a
bank's *last* open site closes. Resolve-per-site progress metrics will read as
flat until a ROM completes — plan and measure per ROM, not per site.

## Recommended target order

`targets.json` ranks the 180 nonzero ROMs by owners-per-site. Half of all
47,526 owners come from the top 52 ROMs. Best ratios:

| owners | sites | banks | ROM |
|---|---|---|---|
| 68 | 1 | 1 | Donkey Kong 64 |
| 742 | 36 | 1 | Tetrisphere |
| 932 | 47 | 1 | Diddy Kong Racing |
| 19 | 1 | 1 | Banjo-Kazooie |
| 816 | 46 | 1 | Rampage World Tour |
| 1,050 | 70 | 1 | Wayne Gretzky's 3D Hockey '98 |
| 1,109 | 92 | 1 | Xena |

DK64's single site has shape `{via_call: false, local_definition: {kind:
live_in}}` — a jump through a register defined outside the function. That
shape covers 874 sites across 276 ROMs, so a resolver for it generalizes.

## Semantic shape distribution (full corpus)

22 distinct shapes over 28,688 sites; 4 cover 85.2%. Confirms and slightly
tightens the earlier partial-sample figure (4 shapes / 83.2% of 24,456).

| sites | share | ROMs | shape |
|---|---|---|---|
| 10,336 | 36.0% | 234 | `via_call`, load of a load |
| 5,695 | 19.9% | 260 | `via_call`, live_in |
| 5,465 | 19.0% | 250 | `via_call`, load of live_in |
| 2,942 | 10.3% | 193 | `via_call`, load of add |
| 1,482 | 5.2% | 114 | `via_call`, load of immediate |
| 1,273 | 4.4% | 208 | load of add |
| 874 | 3.0% | 276 | live_in |

`via_call: true` accounts for 94.6% of all sites.

## Reproduce

```
scripts/resweep.py resweep.json      # ~40 min, checkpoints every 40 ROMs
```
