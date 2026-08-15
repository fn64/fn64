# Coverage corpus — the ~26 ROMs fn64 tooling should focus on

Drafted 2026-08-15. Answers a recurring question ("what should discovery/recomp
work actually target?") that no doc previously answered — `CORPUS-INDIRECT-PRIORITY.md`
ranks by indirect-jump owner-payoff only; this layers recompile-quality and
engine/toolchain breadth on top, per two explicit goals:

1. **Close discovery-tool gaps** — pick ROMs where resolving indirect sites
   unlocks real exact-owner payoff, so resolver work has a measurable target.
2. **Prove fast, high-quality recompilation at scale** — pick ROMs spanning
   distinct engines/toolchains, not just the highest-payoff ones, so the
   pipeline isn't overfit to one codebase (Zelda-only, AKI-only, etc).

Source data: full 287-ROM resweep, 2026-08-15 (`docs/CORPUS-INDIRECT-PRIORITY.md`,
287/287 classified, 0 errors). Numbers below are pulled from that sweep, not
guessed or carried over from the earlier 5-ROM spot check.

## Tier 0 — highest discovery-gap payoff

Ranked by `sole_owner_assessments_if_discharged` (owners unlocked if every
owner-proof site in the ROM were resolved). These are where a resolver
investment pays off most.

| owners | owner-proof sites | ROM |
|---:|---:|---|
| 1,099 | 41 | Xena: Warrior Princess — The Talisman of Fate |
| 1,042 | 37 | Wayne Gretzky's 3D Hockey '98 |
| 923 | 15 | Diddy Kong Racing |
| 898 | 45 | Hydro Thunder |
| 816 | 22 | Rampage: World Tour |

## Tier 1 — cheap per-site load-resolution targets

Small owner-proof site counts (2-18), so a single resolver shape closes the
whole ROM. Lower total payoff than Tier 0 but the fastest wins per unit of
resolver work.

| owners | owner-proof sites | ROM |
|---:|---:|---|
| 260 | 4 | Wave Race 64 |
| 90 | 2 | Pilotwings 64 |
| 740 | 18 | Tetrisphere |
| 35 | 1 | Pokemon Stadium 2 (France) |

## Tier 2 — already resolved / regression anchors

No further discovery payoff available (owner-proof frontier is empty or
candidate-only), but useful as gates that must stay green.

| ROM | status |
|---|---|
| Donkey Kong 64 | 68 owners realized, 1 dead candidate-only site left |
| Banjo-Kazooie | 19 owners realized, same dead-end pattern (already gated) |
| Banjo-Tooie | 31 owners realized, 1 dead-end site, 0 further payoff |
| WWF No Mercy | real zero-owner case (0 owners / 82 sites) — a confirmed negative, not a bug |

## Tier 3 — already-gated flagships (keep the pipeline honest end-to-end)

Existing `FN64_DISCOVER_*_ROM` gate vars in `crates/fn64-discover`. These
prove the full discover -> recomp -> run pipeline on titles the project
already depends on for other work (renderer, audio, answer-key grading).

- Ocarina of Time — most mature title in the repo (Phase R renderer work)
- Majora's Mask
- Super Mario 64
- GoldenEye 007
- Perfect Dark — known boundary case (KUSEG TLB + gzip code); keep as the
  "doesn't fit the current model" canary, not a near-term target
- WWF WrestleMania 2000 — only playable AKI title today
- WCW/nWo Revenge — has a local donor checkout
- WCW vs. nWo World Tour

## Tier 4 — engine/toolchain breadth (recompile-at-scale goal)

Not chosen for discovery payoff — chosen so the corpus isn't all
Nintendo-EAD/Rare/AKI. Wire up `FN64_DISCOVER_*_ROM` gates for these; none
are gated today.

- Mario Kart 64 — MIO0-heavy asset pipeline, different from anything gated today
- Super Smash Bros.
- F-Zero X
- Kirby 64: The Crystal Shards
- Lylat Wars (PAL Star Fox 64) — confirmed present in corpus under this title

## Deprioritized, not deferred

**Rayman 2: The Great Escape** and **Donald Duck: Goin' Quackers** now
classify cleanly (PR #119 fix) with real owner-proof frontiers (45 and 41
sites) but **zero** counterfactual payoff — every open site is a shared
blocker, not sole-owner. They are not discovery-gap targets. If included at
all, it should be for Tier 4-style engine/toolchain breadth (different,
non-Nintendo toolchain), not because there's a resolver payoff waiting.

## Reproduce / update this doc

```
scripts/resweep.py resweep.json --rom-dir <corpus-dir>
```

Re-derive the tier tables from the output the same way
`docs/CORPUS-INDIRECT-PRIORITY.md`'s "Recommended target order" section does.
Update both docs together — this one tracks *what to build toward*,
`CORPUS-INDIRECT-PRIORITY.md` tracks *the raw measurement*.
