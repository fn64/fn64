---
name: Block proof reaches nothing on the recovered NWXE overlay banks
covers: [C3, C5]
reported: 2026-09-15
---
repro: with the PRISTINE (committed) NWXE answer key, run
`fn64-discover gate-owners-overlays` at 26e375e8 and at 42307ab8
expected: the recorded result, exact_owners=46 across the four recovered
overlay banks, with block proof reaching them
actual: at 42307ab8 and on main, reached_blocks and proven_executable_bytes
are 0 on ALL FOUR banks and exact_owners collapses 46 -> 0.

|          | reached_blocks        | proven_executable_bytes   | exact_owners |
|----------|-----------------------|---------------------------|--------------|
| 26e375e8 | 4508/790/10401/12615  | 98344/19576/226796/256416 | 46           |
| 42307ab8 | 0/0/0/0               | 0/0/0/0                   | 0            |

note: roots are still produced, and there are MORE of them than before
(347/156/601/1251 versus 310/120/586/974), so the loss is in proof, not in
discovery. Every assessment reports `entry_not_authoritative`, which points
at an authority/executability change rather than the boundary rules.

First bad commit is 42307ab8 "feat(corpus): land the static-recomp
consolidation wave (#119)" -- a 127-file, ~76k-insertion squash, so it needs
its own bisect INSIDE the wave (the PR's own branch history, if it survives)
to name the authority change.

`expected_owners_overlays` in scripts/gate-determinism.sh is deliberately NOT
re-recorded: the digest is red because the result regressed, and refreshing it
would freeze the collapse as the baseline. K21 found this only after fixing
the two stale digests that had been aborting the script before this gate ran.
