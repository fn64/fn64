---
covers: [C3, T8, T9]
depends: [K15, K16]
pitch: "One command sweeps the private corpus from a pinned clean build and ranks blockers, so a full-corpus measurement costs no model tokens."
---
write `scripts/corpus-campaign.zsh`: refuse a dirty tree unless
`--allow-dirty`; build `fn64-discover` release; record git rev, binary
sha256, and dirty flag; run rom-catalog (dedupe) -> rom-frontier ->
import-rom-frontier-campaign -> corpus-recompile-sweep -> corpus-dashboard
-> corpus-unblock-rank into `~/.cache/fn64-corpus/campaign-<date>-<rev8>/`.
Support `--limit N`, `--rom-ids <file>`, `--resume` (skip ROMs with a
receipt for this candidate), `--jobs`, and per-stage timeouts. Print a
path-free per-stage timing summary (p50/p95 wall, peak RSS).

deliverables:
- scripts/corpus-campaign.zsh
- scripts/test-corpus-campaign.py

verification:
- python3 scripts/test-corpus-campaign.py
- scripts/corpus-campaign.zsh --limit 2 --allow-dirty
