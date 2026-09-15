---
covers: [C3, T8]
pitch: "The catalog, importer, and dashboard land with tests in CI, so a corpus sweep yields validated receipts instead of a one-off report."
---
commit the campaign receipt chain that already exists untracked, and wire
its tests into the CI docs job per the checker-must-be-wired-into-ci rule.

deliverables:
- scripts/rom-catalog.py
- scripts/test-rom-catalog.py
- scripts/corpus-dashboard.py
- scripts/test-corpus-dashboard.py
- scripts/import-rom-frontier-campaign.py
- docs/plans/corpus-operations-dashboard.md
- .github/workflows/ci.yml

verification:
- python3 scripts/test-corpus-dashboard.py
- python3 scripts/test-rom-catalog.py
- python3 scripts/lint-docs.py
