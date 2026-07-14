# AGENTS.md — the fn64 operating contract

You're an agent (or a human — same rules) landing in an agent-forward codebase.
Most of this project is built by AI agents in orchestrated waves; the reason
that works is that the norms are explicit and nobody gets to skip them,
including you. This file is short on purpose. Read it all.

## Read order

1. `README.md` — what this is and why.
2. `docs/DESIGN.md` — architecture, threading model, A/B migration plan.
3. `docs/ABI-SURFACE.md` — the extern surface recompiled code expects
   (mechanically extracted; regenerate rather than hand-edit).
4. The docs referenced by whatever you're touching. Docs here are load-bearing:
   if you change behavior, you change its doc in the same commit.

## Clean-room protocol (non-negotiable)

fn64 is a clean-room reimplementation. Allowed sources:

- Our own design docs and behavioral specs in `docs/` (they cite their
  provenance — keep it that way).
- The public libultra manuals (cite section names).
- The MIT recompiler source ([fn64/n64recomp](https://github.com/fn64/n64recomp))
  and the C it generates — that's the ABI we serve.
- Public hardware documentation.

Disallowed: reading GPL runtime implementation code (ultramodern/librecomp
internals, or any GPL runtime). Not for "inspiration," not to "check one
thing." If a behavior is only knowable from that code, the answer is a
differential experiment against the reference runtime as a black box — trace
it, don't read it. Every design claim states which allowed source it came from.

## Validation bars

- Deterministic bug fix: **10 consecutive clean runs** before you say "fixed."
- Concurrency/race fix: **20+ consecutive clean runs**, and name the exact
  interleaving your fix closes, in a comment, at the fix site.
- One green run proves nothing. Don't report it as if it does.
- "Not verified" is an acceptable, respectable status. A false "done" is the
  one unforgivable sin here. When in doubt, report the doubt.

## Behavior rules

- **Loud traps, no silent shrugs.** Unimplemented ABI surface panics with the
  symbol name and call context. Never emit a silent no-op, a defensive
  null-guard that hides corruption, or a fallback that masks a missing
  feature. If you're tempted to guard, you haven't found the bug yet.
- **Differential evidence.** Runtime behavior changes emit the shared event
  trace and get diffed against the reference runtime over identical recompiled
  code. Attach the diff (or its absence) to your claim.
- **Types before audits.** If an invariant can live in the type system —
  ownership of a queue, the single-runnable-thread token, an rdram address
  newtype — put it there. An invariant enforced by review is a bug with a
  delay timer.
- **Mechanism over patch.** If you fix an instance of a bug class, build the
  sweep that finds the rest of the class. One-off fixes to recurring shapes
  get bounced in review.

## Scope & hygiene

- Match the surrounding idiom. Comments state constraints the code can't —
  never narrate what the next line does, never argue the change is correct.
- Commits are evidence-cited: what you verified, how, and the run counts.
- No game content, no ROM bytes, no recompiled-game output ever enters git.
  Check what you're staging.
- If your task boundary says a path belongs to someone else, it does — even
  if your fix "would only take a second."

## When you're stuck

Report the precise frontier — the failing invariant, the trace, what you ruled
out — and stop. A well-documented dead end is a deliverable; thrashing isn't.
The next wave (maybe you, with fresh context) starts from your evidence
instead of your optimism.
