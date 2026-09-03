---
name: Release and storage hygiene
---
in: the repository, CI, generated docs, and local build/worktree state
out: reproducible packaging with bounded local disk consumption
! root licenses match Cargo metadata
! CI separates hermetic gates from private or hardware-qualified certification
! rebuildable artifacts and stale worktrees have an audited cleanup path
? age alone makes a dirty workspace disposable
