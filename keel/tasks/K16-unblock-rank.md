---
covers: [C3, T9]
depends: [K14]
parallel_with: [K15]
status: done
pitch: "One short table names which frontier cluster blocks the most ROMs, so mechanism work is chosen from receipts, not from a model reading logs."
---
write `scripts/corpus-unblock-rank.py`: read a campaign's manifest, receipts,
and dashboard JSON; group non-passed receipts per stage by (outcome kind,
frontier kind, unsupported-destination reason set, destination-count bucket
1 / 2-3 / 4+); print rows descending by ROM count with the ROM ids; print
resource_limit and infrastructure_failure as their own rows; exit nonzero
if the row counts do not reconcile with the dashboard's counts. Discover
receipts that passed with `no_candidate_table_found` are not blocked.

deliverables:
- scripts/corpus-unblock-rank.py
- scripts/test-corpus-unblock-rank.py

verification:
- python3 scripts/test-corpus-unblock-rank.py
