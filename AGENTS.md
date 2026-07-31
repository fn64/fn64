# AGENTS.md — the fn64 operating contract

You're an agent (or a human — same rules) landing in an agent-forward codebase.
Most of this project is built by AI agents in orchestrated waves; the reason
that works is that the norms are explicit and nobody gets to skip them,
including you. This file is short on purpose. Read it all.

## Read order

1. `README.md` — what this is and why.
2. `docs/DESIGN.md` — architecture, threading model, A/B migration plan.
3. `crates/fn64-abi` and its tests — the extern surface recompiled code links
   against. `tests/c_smoke.rs` is the live oracle: it compiles a real C caller
   against the staticlib and runs it, so the ABI shape is proven by a test that
   runs, not by prose. Read the code, not a description of it.
4. The docs referenced by whatever you're touching. Docs here are load-bearing:
   if you change behavior, you change its doc in the same commit.

`ABI-SURFACE.md` (in the legacy aki-recomp checkout) is cited ~45 times across
this repo as clean-room provenance — "this claim came from that allowed
source." Those citations are history and stay honest whether or not you can
reach the file. It was a live oracle while the surface was being transcribed;
it is not one now, and you do not need it to work here. Don't go hunting for
it (see ROADMAP Phase H).

## Clean-room protocol (non-negotiable)

fn64 is a clean-room reimplementation. Allowed sources:

- Our own design docs and behavioral specs in `docs/` (they cite their
  provenance — keep it that way).
- The public libultra manuals (cite section names).
- The MIT recompiler source ([fn64/n64recomp](https://github.com/fn64/n64recomp))
  and the C it generates — that's the ABI we serve.
- Public hardware documentation.
- Permissively-licensed N64 references, licenses verified from their LICENSE
  files 2026-07-18 (`docs/DISCOVER-PLAN.md` "Research intake" records the
  verification): ares (ISC), paraLLEl-RDP (MIT — its Angrylion reference
  lineage is unlicensed and stays excluded), n64-systemtest (MIT), libdragon
  (Unlicense). Same precedent as reading MIT RT64. MAME remains excluded
  (GPL-2.0+ as a whole; its documented Lua/debugger interfaces are fine for
  black-box use). ddisasm is AGPL: cite its paper's concepts, never its code.
- By project-owner decision 2026-07-28, N64LoaderWV may be executed, reviewed,
  and maintained in a separate fork despite its repository having no declared
  license. That exception is for loader/tool engineering, not N64 behavioral
  authority: its mappings and analysis remain candidate evidence, and its code
  does not enter fn64's MIT/Apache distribution.
- m2c is excluded from this toolchain: do not install, invoke, vendor, read, or
  build an adapter for it.

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
- **Differential evidence.** Behavior changes get diffed, and you attach the
  diff (or its absence) to your claim. Use a differential that actually runs:
  - `scripts/lane-parity.sh N` — first mechanically audits the c/rs generated
    callable-body sets, then compares per-swap framebuffer SHAs only if that
    authority precondition holds. The current legacy C lane has callable empty
    bodies that fn64 recompiles correctly, so default mode rejects it as an
    arbiter from swap zero. `scripts/lane-parity.sh --observe N` retains a
    labeled, non-authoritative framebuffer comparison; no observed matching
    horizon proves the missing bodies were irrelevant. See
    `docs/PARITY-METHOD.md`.
  - `cargo nextest run -p fn64-abi` — `c_smoke` links a real C caller against
    the staticlib, so the ABI shape is proven by a test, not by prose.
  - `crates/fn64-recomp-rs/tests/oracle.rs` + friends — per-instruction
    differential of fn64's emitted bodies against N64Recomp's C. Note its
    blind spot: it compares CODEGEN, so anything applied above codegen (a
    config stub, a patch) is invisible to it.

  Diffing against the reference *runtime* (ultramodern et al.) is NOT a
  mechanism this repo has — `fn64-diff` implements a comparator but the
  savestate-transplant path it needed cannot work (see DESIGN.md: functions are
  the smallest resumable unit). Do not cite a differential you did not run.
- **Types before audits.** If an invariant can live in the type system —
  ownership of a queue, the single-runnable-thread token, an rdram address
  newtype — put it there. An invariant enforced by review is a bug with a
  delay timer.
- **Mechanism over patch.** If you fix an instance of a bug class, build the
  sweep that finds the rest of the class. One-off fixes to recurring shapes
  get bounced in review. Doc drift is such a class and now has its sweep:
  `scripts/lint-docs.py` (dangling refs, README crate coverage, phantom env
  vars, a blind regen recipe). Run it alongside the tests; if you change a
  doc's shape, teach the linter rather than exempt the doc.

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
