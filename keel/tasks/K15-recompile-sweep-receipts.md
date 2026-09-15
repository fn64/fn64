---
status: done
covers: [C3, T8]
depends: [K14]
parallel_with: [K16]
pitch: "Every campaign ROM gets a typed recompile receipt from the cold gate, so the second funnel stage is measured by receipts, not a headline grep."
---
write `scripts/corpus-recompile-sweep.py`: for each manifest ROM with a
discover receipt, run `gate-rom-recompile` with `FN64_RECOMPILE_REPORT`,
parse the `HEADLINE unsupported=N` / `FAILED: <Kind>` line exactly as
`scripts/review-gate.sh` does, and on `unsupported>0` run
`diagnose-cold-unsupported` for typed provenance. Map: unsupported=0 ->
passed; unsupported>0 -> frontier kind `unsupported_destinations` with the
count and reasons; `FAILED: <Kind>` -> frontier of that kind; timeout or
RSS cap -> resource_limit; any other output -> infrastructure_failure.
Each receipt binds the discover receipt id, git rev, binary sha256, wall
time, and peak RSS. Never write inside the repo; never store paths.
The gate packs before it emits, so one run also mints a `pack` receipt
(banks, pack_blocks, pack_words from the report): a `FAILED:` before any
report is a pack frontier and the recompile receipt is then not minted; a
report with pack_words>0 is a passed pack, and the recompile receipt binds
that pack receipt as its predecessor. The dashboard requires this chain.

deliverables:
- scripts/corpus-recompile-sweep.py
- scripts/test-corpus-recompile-sweep.py

verification:
- python3 scripts/test-corpus-recompile-sweep.py
- python3 scripts/test-corpus-dashboard.py
