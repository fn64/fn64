---
covers: [C2, C5, B6]
depends: [K07, K09]
pitch: "Archive unique dirty state, then remove stale worktrees and stashes while retaining a recoverable audit trail."
---
clean build products first; worktree or stash removal follows its WIP
disposition and archive receipt.

deliverables:
- bounded local workspace inventory

verification:
- T6
