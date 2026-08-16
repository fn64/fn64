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

## Choosing an executor

The loop above is model-agnostic on purpose, but the choice is not arbitrary.
Match the executor to the task's *failure mode*, not to its size.

**The asymmetry that decides it:** this repo's expensive failure is a
confidently wrong "done" against a bar that did not measure what it claimed —
AGENTS.md calls a false "done" the one unforgivable sin, and Phase V exists
because that already happened. Verification is therefore never delegated to a
cheaper tier than the implementation it checks. A wave that implements cheaply
and verifies expensively is correct; the reverse is how V1a-class errors ship.

- **Frontier reasoning tier** (session model; Opus-class) — adversarial verify
  and merge gate, always. Also the *design* of any task touching an invariant:
  clean-room provenance judgements, evidence-schema changes, concurrency
  interleaving arguments, and any decision about whether a gate actually
  measures its claim. These are judgement calls where being subtly wrong is
  expensive and hard to detect downstream.
- **Strong implementation tier** (codex waves; the current default) —
  implementation-class work behind an already-specified gate: U4/U5 milestone
  items, mechanical closure against a written contract, opcode and device
  behavior with a manual citation to follow. This is the bulk of U4-U6 and
  should stay parallel and unattended.
- **Cheap/fast tier** (Sonnet- or Haiku-class) — mechanically checkable work
  with a deterministic oracle: doc regeneration, drift-lint fixes, inventory
  and census sweeps, matrix regeneration, CI workflow repair, harvesting
  citations. Anything where the gate is `scripts/lint-docs.py` or
  `check-nmr-surface.py` and the answer is objectively right or wrong.

Every delegated slice completes the same four-step loop: the writer reports a
typed ticket status and exact evidence; an independent agent reviews the
invariants and negative cases; the lead reruns the authoritative gate on the
clean branch; then the accepted commit is merged and dependent branches rebase
before taking new work. A pull request is optional, but review and
synchronization are not.

**Never delegate to any tier:** eye-gates (the user's, batched — an agent
never self-certifies render or audio output), and the sourcing/capture
blockers in `UNIVERSAL-RUNTIME-PLAN.md` §4.0, which are not model-shaped work
at all. Dispatching an agent at a sourcing blocker produces plausible prose
and no artifact.

**Parallelism is bounded by the crate graph, not by executor count.** The
`fn64-abi` serialization rule below is the real constraint: U4 and U5 run
concurrently because they touch disjoint crates, and adding executors to a
serialized chokepoint produces merge conflicts, not throughput. Fan out across
independent milestones; keep one writer per chokepoint crate.

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
