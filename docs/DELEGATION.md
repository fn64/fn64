# Delegation workflow — codex implementation waves, verified merges

How work on ROADMAP.md gets delegated, gated, and merged. Reuses the
existing fast-loop infra; adds only a dispatch wrapper.

## The loop (per task)

1. **Dispatch** (`scripts/dispatch.sh <name> <prompt-file> [codex args...]`):
   creates `../fn64-<name>-wt` on branch `wave/<name>`, prepends the
   AGENTS.md contract to the task card, and runs
   `codex exec -C <worktree> --sandbox workspace-write` in the background,
   logging to `<worktree>/.dispatch.log`. (`codex exec -C` direct — the
   plugin wrapper sandboxes to its own cwd and silently no-ops cross-repo.)
2. **Supervise**: the dispatcher polls the codex PID; a dead PID with a
   "running" claim is treated as dead, log + `git -C <wt> status` checked
   for partial work before redispatch.
3. **Verify (session model, adversarial)**: read the diff against the task
   card; run the gates yourself — never trust the job's own claim:
   - per-crate: `cargo test -p <crate>` + `cargo clippy -p <crate> --all-targets`
   - workspace: `cargo nextest run --workspace` (the authoritative gate)
   - invariants: `scripts/lint-docs.py` + `scripts/lint-rdram-layout.py`
   - task-specific probe (e.g. bounded `./oot run` with warning greps)
   - AGENTS.md bars: 10 consecutive clean runs for deterministic fixes,
     20+ named-interleaving for concurrency.
4. **Merge**: no PRs for now — merge the wave branch to `main` locally
   (`--no-ff`, evidence-cited message), push main, prune the worktree
   (`scripts/wt.sh prune`).
5. **Record**: tick ROADMAP.md and touch the affected doc **in the same
   merge** (docs are load-bearing).

## Rules that shape every task card

- Card must instruct: read `AGENTS.md` first and obey it (clean-room GPL
  ban, loud traps, evidence-cited commits, no game content in git, docs
  same-commit).
- One focused task per job; on a blocker, report the frontier and stop —
  no rabbit-holes.
- **Serialize any wave that edits `fn64-abi`** (shared chokepoint; parallel
  edits have left the tree non-compiling before). R-track and D-track are
  parallel because they touch disjoint crates.
- Jobs export the fast loop: `CARGO_TARGET_DIR=/tmp/fn64-shared-target`,
  `RECOMP_RS_DIR="$(scripts/native-emit.sh)"` for oot-boot work
  (FAST-LOOP.md). The fn64-audio lockfile caveat applies to worktree
  oot-boot C-lane builds; prefer the rs manifest in worktrees.
- Eye-gates are the user's, batched (R3/R5); an agent never self-certifies
  a render or audio output.
