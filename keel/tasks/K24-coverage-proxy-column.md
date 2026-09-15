---
status: done
covers: [C3, T14]
depends: [K19]
pitch: "Every campaign row shows how much of a ROM's code is mapped and recompiled, so a certified ROM with 41 reachable words is not read as a cracked game."
---
add two proxy ratios per ROM to corpus-unblock-rank.py (a new
`--coverage` table, also written to rank-<ts>.txt by corpus-campaign.zsh)
and to the dashboard JSON rows: mapped = pack_words*4 / code_run_bytes;
recompiled = (exact_aot_bytes + block_aot_bytes) / code_run_bytes, where
code_run_bytes is copied by import-rom-frontier-campaign.py from the
catalog row (joined by normalized sha256) into the discover receipt's
result, so the dashboard keeps deriving from receipts alone and never
opens the private catalog. Print per ROM: id, stage status,
mapped %, recompiled %; then a summary: median of each, and the certified
ROMs whose recompiled ratio is below 50% (the "certified but not covered"
list), descending by code_run_bytes. Values are path-free. Missing
receipt -> not_run; code_run_bytes == 0 -> undefined, listed separately.

deliverables:
- scripts/import-rom-frontier-campaign.py
- scripts/corpus-unblock-rank.py
- scripts/test-corpus-unblock-rank.py
- scripts/corpus-dashboard.py
- scripts/test-corpus-dashboard.py
- scripts/corpus-campaign.zsh
- scripts/test-corpus-campaign.py

verification:
- python3 scripts/test-corpus-unblock-rank.py
- python3 scripts/test-corpus-dashboard.py
- python3 scripts/test-corpus-campaign.py
