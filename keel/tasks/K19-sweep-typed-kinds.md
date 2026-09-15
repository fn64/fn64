---
status: done
covers: [C3, T8, T9]
depends: [K17]
parallel_with: [K18]
pitch: "Pack frontiers carry their typed kind and appear in the rank table, so overlay-recovery blockers are counted next to recompile ones."
---
three small fixes found by the first full run:
1. corpus-recompile-sweep.py: when the `FAILED:` line's first token is a
   phase word (recovering, building, ...), take the frontier kind from the
   first `CamelCase { ... }` token in the detail (e.g. NoUniqueAdmittedTable,
   InvalidResidentSplit); keep the phase word in `detail.phase`.
2. corpus-recompile-sweep.py: retain the diagnose-cold-unsupported JSON line
   as a private artifact (sha256 in the receipt) and copy each destination's
   incoming-edge kinds into result.unsupported (addresses only).
3. corpus-unblock-rank.py: accept `--stage pack`; corpus-campaign.zsh prints
   the pack table too.

deliverables:
- scripts/corpus-recompile-sweep.py
- scripts/test-corpus-recompile-sweep.py
- scripts/corpus-unblock-rank.py
- scripts/test-corpus-unblock-rank.py
- scripts/corpus-campaign.zsh
- scripts/test-corpus-campaign.py

verification:
- python3 scripts/test-corpus-recompile-sweep.py
- python3 scripts/test-corpus-unblock-rank.py
- python3 scripts/test-corpus-campaign.py
