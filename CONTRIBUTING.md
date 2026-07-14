# Contributing to fn64

Contributions are welcome — from humans, from agents, and from humans driving
agents. This project is itself built agent-forward, so we're the last people
who'd gatekeep on *how* your patch got written. We gatekeep on whether it's
good.

## Agentic contributions: explicitly welcome

If your PR was written partly or entirely by an AI agent, that's fine — say so
if you like, or don't. It changes nothing about the bar:

- **You own it.** "The agent wrote it" is not a review response. If you can't
  explain what your patch does and why it's correct, it isn't ready — same as
  any code you didn't write carefully enough.
- **It meets the same guidelines as everything else.** Tests, clean lint,
  docs, evidence. Agents are great at exactly these; there's no excuse for an
  agentic PR arriving without them.
- **Read `AGENTS.md` first** — it's the operating contract for agents and
  humans alike, and PRs that ignore it (silent fallbacks, unverified "fixed,"
  GPL-derived code) get closed regardless of authorship.

## The bar, concretely

- **Tests.** Behavior changes come with a check that fails without the change.
  Runtime behavior claims come with differential-trace evidence against the
  reference (see `AGENTS.md`). Race fixes state their run counts (20+).
- **Lint & format.** `cargo fmt` clean, `cargo clippy` clean (no fresh
  `#[allow]`s without a comment defending each one).
- **Docs.** If you changed behavior, the doc that describes it changed in the
  same PR. New surface gets doc comments. Load-bearing design decisions go in
  `docs/`, not in the PR description where they'll be lost.
- **Evidence-cited commits.** What you verified, how, and the run counts.
  "Not verified" is an honest label we accept; discovering a false "done" in
  review is how a PR dies.
- **Clean room.** No GPL runtime code — not read, not referenced, not
  paraphrased. Provenance questions get asked in review; have answers.
- **No game content.** Ever. ROMs, ROM-derived bytes, recompiled game output —
  none of it enters this repository, in any form, in any commit.

## Practicalities

- Small, reviewable PRs beat heroic ones. If your agent produced a 4,000-line
  diff, make it produce a stack of coherent commits instead.
- CI must be green. CI is not advisory here.
- License: by contributing you agree your work lands under MIT OR Apache-2.0.

That's it. Bring us something sharp.
