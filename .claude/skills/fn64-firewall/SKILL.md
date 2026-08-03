---
name: fn64-firewall
description: Run the graded answer-key firewall (boundary grading + determinism gates) with the correct environment, and interpret results. Use before landing any discovery-mechanism change.
---

# fn64-firewall

The graded set is the project's soundness firewall: key-free discovery is
graded against external answer keys. `wrong==0` is mandatory in EVERY
configuration; recall (`matched_exact`) may only rise. A mechanism that
grades `wrong>0` anywhere is unsound as admitted — tighten or revert, never
tune per-game.

## Environment

Machine-local paths live in the gitignored `.claude/local.env` (answer-key
ROMs and dump.tomls are external to the repo and per-machine):

```sh
source .claude/local.env
```

If that file is missing, create it defining: `FN64_DISCOVER_{NWXE,NW4E,OOT}_ROM`,
`FN64_DISCOVER_{NWXE,NW4E,OOT}_DUMP`, and `FN64_ROM_CORPUS_DIR` — the
answer-key checkouts (jessetbh-derived dump.tomls beside their exact ROMs)
and the corpus ROM directory.

Traps:
- `~/Downloads/WWF No Mercy (E) (V1.1)` is PAL and is NOT the key's ROM.
  Always the `aki-recomp/games/` copies.
- Boundary grading uses per-run `FN64_DISCOVER_ROM`/`_DUMP` (+ optional
  `FN64_DISCOVER_SIG_DONOR_ROM`/`_DUMP`). Donor configurations must ALSO
  grade wrong==0 — a change can be clean donor-free and wrong>0 with a
  donor (this happened; see git history for `4797d0d`).

## Run

Full set (determinism gates + boundary grades):

```sh
bash scripts/gate-determinism.sh
```

Single grade (fast iteration):

```sh
FN64_DISCOVER_ROM=$FN64_DISCOVER_NWXE_ROM FN64_DISCOVER_DUMP=$FN64_DISCOVER_NWXE_DUMP \
  cargo run --quiet --release -p fn64-discover --bin gate_decomp_functions \
  | grep -o 'matched_exact=[0-9]*.*wrong=[0-9]*'
```

## Interpretation

- Reference baselines (update this file when they legitimately move):
  NWXE `matched_exact=698/847 wrong=0`; NW4E `835/985 wrong=0`;
  Revenge donor-free `499/689 wrong=0`.
- `wrong>0`: stop. The change is rejected regardless of recall gains.
- Pinned-hash gates (e.g. `gate_overlay_regions`): a sha mismatch means
  OUTPUT DRIFT, not necessarily breakage. Bisect whether YOUR change caused
  it (revert your files, rerun the one gate). If drift predates you, the
  baseline is stale from an upstream merge — refreshing the pinned hash is
  a separate, deliberately-reviewed commit, never a drive-by.
