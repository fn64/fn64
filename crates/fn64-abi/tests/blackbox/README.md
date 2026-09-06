# Black-box shim observations

Cleanup-plan task 5.6. `AGENTS.md`'s clean-room protocol allows exactly one way
to learn how the GPL reference runtime behaves: a differential experiment
against it as a black box. This directory is the fn64 half of that experiment.

## What is here

- `<scenario>.json` — an input script: a sequence of shim calls with concrete
  register arguments and the registers/rdram words to observe after each.
- `<scenario>.observed.json` — the tuples the **reference runtime** produced for
  that script, with a provenance header naming the runtime commit, the driver
  commit, the date, the platform, and the exact command.

The observation files are facts about a black-box run. They carry no runtime
code and describe no runtime internals, which is what makes them citable here.
Nothing in this directory is derived from reading GPL implementation sources.

`crates/fn64-abi/src/blackbox_replay.rs` replays every script through fn64's own
shims and classifies each tuple. Only `unexplained` fails. It runs in the normal
`cargo nextest run -p fn64-abi`; no GPL code is needed at test time.

| Verdict | Meaning |
|---|---|
| `match` | A value was compared and agreed. |
| `deliberate-divergence` | fn64 differs on purpose, with the public libultra manual citation and fn64's exact pinned value. |
| `not-observed` | The driver could not drive the call as a black box; the measured reason is recorded. |
| `not-compared` | The recording and the script both name nothing, so replaying verified nothing. |
| `unexplained` | Anything else, including a recorded key the script does not observe. **Fails.** |

Current counts, pinned in the test: **18 match, 8 deliberate-divergence,
2 not-observed, 0 not-compared, 0 unexplained.**

`not-compared` and `not-observed` are counted apart from `match` on purpose. A
tuple that compares nothing is a check that cannot fail, and folding it into the
match count overstates coverage — three `osCreateMesgQueue` tuples did exactly
that until they were widened to observe the queue header.

A call the driver could not drive as a black box is recorded `not-observed`
with the measured reason, never invented. `osSetTimer` and `osPiStartDma` are
both in that state: the reference terminates on a signal from a bare context
because they need subsystems the driver does not stand up.

The replay registers its scenario buffer as the process RDRAM allocation before
calling any shim. fn64's executor mirrors the `OSMesgQueue` struct into guest
rdram only once that registration exists; without it the harness would read its
own zeroed buffer and score a comparison against fn64 output that never
happened.

The queue-header words the `osCreateMesgQueue` scenarios peek are `validCount`,
`first`, and `msgCount`. fn64 defines those offsets as `MQ_VALIDCOUNT_OFF`
(0x08), `MQ_FIRST_OFF` (0x0C), and `MQ_MSGCOUNT_OFF` (0x10) in
`crates/fn64-runtime/src/executor/mod.rs`, and
`crates/fn64-runtime/src/executor/tests.rs`'s
`queue_struct_mirrored_into_rdram_on_create_and_send` asserts all three after a
create — so the layout this harness compares against is pinned in-tree, not only
by the public libultra struct it mirrors.

## Regenerating an observation

The driver lives outside this repository, in the GPL aki-recomp checkout at
`tools/shim-probe/` (commit `69b504a`, runtime commit `9f06f81e`). It is GPL by
necessity and is never copied into fn64. Build and run instructions are in that
directory's own README. Recording is:

```sh
./tools/shim-probe/build/shim-probe <fn64>/crates/fn64-abi/tests/blackbox/<scenario>.json 2>/dev/null
```

The runtime writes diagnostics to stderr; the observation JSON is on stdout
alone. Paste the result into `<scenario>.observed.json` under a refreshed
provenance header — a recorded observation without its runtime commit, driver
commit, date and command is not a citable fact, and the replay test asserts the
header is present.

## Adding a scenario

Add the script and its recording as a pair, add the pair to `SCENARIOS` in
`blackbox_replay.rs`, and add any shim the script calls to that file's
`call_fn64_shim` dispatch — it panics on an unrecognized shim rather than
skipping the call. If fn64 intends to differ, add a `DELIBERATE_DIVERGENCES`
entry naming the manual section and the exact fn64 value; a divergence that
later drifts to some third value is reported unexplained rather than silently
tolerated.

Keep the script and the recording in step: every key a recording names must
appear in the script's `observe`/`peek_words`, or the replay reports the tuple
unexplained rather than skipping the key. That is deliberate — a recorded value
the script never asks about is a value nothing verifies. The verdict counts are
pinned in the test, so a new scenario also means updating that pin, the counts
above, and the black-box paragraph in `docs/COMPLETENESS.md` in the same commit.

Scenario addresses are KSEG0 (`0x80xxxxxx`). Seeding `r2` with the sentinel
`0x12345678` is how a shim that never writes `$v0` is told apart from one that
writes `0`; several recorded reference tuples show that sentinel surviving the
call. A scenario may declare host preconditions in a `setup` object (currently
`tv_type`), so both sides are driven from the same file rather than from setup
hidden in the harness.
