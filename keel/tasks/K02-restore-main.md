---
status: in_progress
covers: [C1, C5, B1]
pitch: "Restore the repository's declared hermetic gates so main becomes a trustworthy integration base."
---
separate real regressions from generated-doc drift and environment-qualified
tests; repair the current failures without weakening their claims.

deliverables:
- green generated-document checks
- green hermetic workspace CI

verification:
- T1

evidence:
- `fix/launch-main-gates` repairs both renderer assertions and regenerates the
  two stale derived documents
- `fn64-render-wgpu`: 4,995/4,995 passed on an adapterless Darwin host
- workspace excluding two environment/private-input tests: 8,866/8,866 passed
- remaining local exclusions are the stale out-of-tree OoT symbol oracle and
  Ghidra's fail-closed process-table memory guard; neither is a hermetic CI gate
